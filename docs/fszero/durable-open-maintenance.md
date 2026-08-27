# Durable-store open integrity and maintenance

Every existing durable store, including the light ref-index open path, passes a fail-closed gate before fsqlite opens the database, creates tables, or repairs pack locators. The gate uses stock SQLite through rusqlite and reads all rows from PRAGMA main.integrity_check. Exactly one row equal to ok is the only successful result. Immutable mode is not used, so committed WAL content participates.

## Snapshot and contention model

The oracle sets a five-second busy timeout and obtains BEGIN IMMEDIATE. This excludes writers while SQLite reads the database and WAL and while FSZero copies the database, existing -wal/-shm files, and every sibling pack generation. Lock, open, query, or integrity failures stop startup. FSZero never runs REINDEX or VACUUM against the source and never auto-promotes salvage. If the writer lock cannot be acquired, no coherent copy is possible, so startup fails without pretending that an unlocked copy is forensic evidence.

## Attestation trust model

A successful full check writes a versioned sidecar containing the gate version, fsqlite version, and DB/WAL/SHM/pack size and mtime fingerprint. It is considered only while holding the writer-excluding lock. Any file change, gate version change, fsqlite version change, malformed or missing sidecar, or `FSZERO_FORCE_INTEGRITY_CHECK` forces a full check. Attestations from the known-vulnerable fsqlite 0.1.15 writer are never trusted. The current 0.1.18 dependency contains the relevant B-tree fixes, so unchanged stores use attested steady-state opens after one independent check. This sidecar is an accidental-corruption migration cache, not a security boundary: an attacker able to rewrite the store, timestamps, and attestation can bypass it.

## Forensics and salvage

Under the held lock, failures allocate unique non-overwriting sibling `.forensic-*` and `.salvage-*` directories. The forensic directory contains byte-for-byte DB/WAL/SHM/pack evidence plus a coherent stock-SQLite backup used for salvage. It starts with `INCOMPLETE`; copied files and `SHA256SUMS` are fsynced before the completion marker is removed and the directory is fsynced again. Salvage reads only the coherent forensic backup, maps shared columns by name into the current schema, inserts rows without conflict suppression, rebuilds current indexes, reruns stock integrity checking, and reports imported, unreadable, and failed rows per table. Every pack generation is preserved unchanged. The report includes source and destination payload counts and notes that locator/hash guarantees are limited to values stock SQLite can materialize. Startup always returns forensic, salvage, and report paths plus data-loss caveats.

Existing bounded pack-validation and memory-backfill maintenance runs only after this gate succeeds. The large-store benchmark reports cold forced-integrity time separately from trusted-attestation open time.
