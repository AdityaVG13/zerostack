# TokenZero Pulse

TokenZero Pulse is the recovery-aware observability layer for TokenZero tool calls. It answers the question that a plain savings counter cannot answer: did compression actually help after exact recovery, cache behavior, failed writes, negative-savings events, and latency are counted?

The shipped surface is the Recovery Flight Recorder plus a bounded local report:

- Recovery Flight Recorder: append-only JSONL event ledger for TokenZero tool calls.
- SQLite query cache: rebuilt from JSONL by `tokenzero pulse sync`; never the source of truth.
- Local report: `tokenzero pulse stats` (alias `status`) prints the aggregate summary.

## Commands

The actual clap surface (verified against `tokenzero pulse --help`):

```bash
tokenzero pulse                    # same as `pulse stats`, human-readable report
tokenzero pulse stats [--json]     # aggregate report (alias: status)
tokenzero pulse sync [--json]      # reconcile JSONL ledger into the SQLite cache
tokenzero pulse doctor [--json]    # check markers, PRAGMA integrity_check, hot index
tokenzero pulse export-jsonl <OUTPUT>   # atomic JSONL snapshot from the reconciled cache
tokenzero pulse import-jsonl <INPUT>    # validate snapshot, replace ledger, rebuild cache
```

All subcommands accept the global `--root <ROOT>` (override the ledger root) and `--json` (machine-readable envelope).

### Not in the shipped binary

The following were drafted for Pulse but are not implemented in this CLI; listing them here as design intent, not as runnable commands. They are excluded because the bounded stats envelope above is sufficient for agent self-inspection without becoming its own context bomb, and the richer views were never wired to a clap surface: `--today`, `--session`, `--detail` modes, `--live`, `--tui`, `replay`, `forecast`, `graph`, `compact`, `clear`, `import-stats`, `dashboard`, `perf-budget`, and `pulse expand pulse://...` recovery refs. Do not script against them; this section is the contract that they do not exist yet.

Where hooks or slash commands are supported, `/tz pulse` and `/tz pulse session` can be mapped to the same CLI calls.

## Event Storage

Global events default to:

```text
~/.tokenzero/pulse/events.jsonl
```

Project-local mirror events can be enabled at:

```text
.tokenzero/pulse/events.jsonl
```

The default event schema records token counts, refs, latency, cache hits, recovery cost, and health flags. It does not record raw code, raw shell output, secrets, or daemon artifacts.

### Identity fields and correlatability

Pulse is local, but its event ledger is not anonymous. `PulseEvent::tool_call` leaves `session_id`, `call_id`, and `ref_ids` absent by default. A caller that uses attribution stores all three verbatim in JSONL and the SQLite cache. Session and call ids can contain caller-controlled identifiers. `tz://` refs are stable local join keys and can correlate events with recoverable payloads in the corresponding local store. Review or redact these fields before sharing a Pulse ledger or snapshot.

`PulseEvent::tool_call` stores the first 64 bits of the `source_hint` SHA-256 as 16 lowercase hex characters in `source_hash`, never the hint bytes. This truncated correlation digest is not anonymization or a unique identity: low-entropy paths or identifiers can be guessed and rehashed, and collisions are possible. Direct public struct construction or deserialization can also supply any `source_hash` string, so callers must never place raw source, output, secrets, or other payloads in that field. No Pulse path uploads these values, and `tokenzero pulse stats --json` exposes only bounded aggregates, but export files contain the original local event rows.

### Local Pulse versus shareable telemetry

Pulse is local observability; it is not the default-off shareable usage-telemetry permission. Existing Pulse JSONL/SQLite, ToolMetrics, and response-ledger accounting continue locally regardless of `TOKENZERO_TELEMETRY`. Shareable usage telemetry is off by default and, when explicitly enabled, records only `{execution_path, raw_tokens, spent_tokens}` for MCP and CodeMode into `usage-telemetry.jsonl`. Inspect with `tokenzero session-ledger inspect --json`; `--telemetry` opts in, `--no-telemetry` opts out with precedence, and `TOKENZERO_TELEMETRY` accepts only `1/on/true/yes` case-insensitively. Inspection always reports `exporter=none`: no exporter or upload path exists.

`TOKENZERO_PULSE_DISABLED` is not read by the current Pulse implementation and therefore does **not** disable local Pulse recording. Do not rely on that variable; control whether a caller records Pulse events at that caller's integration surface. This documents the previously implicit name/behavior mismatch rather than implying an unsupported global kill switch.

Pulse uses JSONL as the source of truth. SQLite is a locked, rebuildable query cache at `.tokenzero/pulse/events.sqlite` or `~/.tokenzero/pulse/events.sqlite`. Reconciliation is one-way from JSONL into SQLite and guarded by `.tokenzero/pulse/sync.lock`. Sync, import, and export commands wait briefly for transient lock contention before returning a clear lock-held error. Event appends wait longer, call `sync_data` before returning, and still fail open for normal TokenZero tool responses. Full snapshot exports use temp files, fsync, and atomic persist.

When Pulse sync/import/export/doctor commands are run with `--json`, failures return a machine-readable error body before exiting non-zero. Lock contention uses `schema_version=tokenzero.pulse.error.v1`, `ok=false`, `error_kind=would_block`, `retryable=true`, and an `error` string containing the held lock path.

Version markers are written to both stores: SQLite table `meta` and the sidecar `events.meta.json` contain the source marker, ledger hash, valid event count, skipped-line count, and update time. Snapshot exports also write `<snapshot>.meta.json`; imports refuse marker mismatches, same-second ambiguous snapshots, unmarked overwrites of a different current ledger, and snapshots that would discard unsynced current ledger changes. `tokenzero pulse doctor` compares those markers, runs `PRAGMA integrity_check`, and verifies the hot `tool + timestamp` index through `EXPLAIN QUERY PLAN`.

Use `tokenzero pulse export-jsonl <output>` to write an atomic JSONL snapshot from the reconciled SQLite cache. Use `tokenzero pulse import-jsonl <input>` to validate a snapshot, atomically replace the ledger, and rebuild SQLite. Imports with corrupt JSONL lines fail before replacing the current ledger. A trusted marked snapshot can recover a corrupt current ledger only when its marker is newer than the current ledger marker.

Operational sync and recovery procedures are included below.

Fast-path fields record why TokenZero skipped compression: `output_strategy`, `skip_reason`, `roi_guard_applied`, `raw_passthrough`, `near_raw`, `empty_result`, `tiny_output_passthrough`, `guarded_expansion`, `forced_expansion`, and `compression_would_increase_tokens`. `cache_hit` is separate telemetry. It is never treated as a display strategy.

Batch read/find/tree calls record one parent event with `batch=true`, `item_count`, batch overhead metrics, and capped item rollups. Pulse does not store full item displays, raw file contents, shell output, or debug JSON for batch calls.

## Accounting

Pulse never collapses these into one headline:

- visible-context savings: raw tool tokens minus model-readable capsule tokens.
- recovery-adjusted savings: raw tool tokens minus visible capsule tokens minus recovery expansion tokens.
- exact-cache byte-lossless savings: hidden exact payload tokens kept server-side, not model-readable.
- cache savings: repeated-output/cache wins, separate from first-response compression.
- output/reply savings: optional response budgeting, separate from tool context.
- schema/shell-routing savings: separate module categories when enabled.

Hidden exact refs are useful because they guarantee local recovery. They are not counted as readable context until `tz_expand` returns visible text, and those recovery tokens are charged.

`tokenzero pulse stats --json` returns a bounded aggregate summary. It contains no raw event rows, raw command output, payloads, or debug-only fields, so Pulse cannot become its own context bomb. The JSON shape (schema `tokenzero.pulse.v1`):

```json
{
  "schema_version": "tokenzero.pulse.v1",
  "status": "ok",
  "event_count": 34810,
  "raw_tokens": 836693768,
  "visible_tokens": 670084004,
  "recovery_tokens": 109956688,
  "task_lossless_tokens": 779957692,
  "failures": 606,
  "cache_hits": 0,
  "exact_ref_count": 184009,
  "visible_savings": 0.199,
  "recovery_adjusted_savings": 0.0677,
  "skipped_lines": 0
}
```

`visible_savings` and `recovery_adjusted_savings` are fractions (0.199 = 19.9%). Recovery tokens for `tz_expand` re-expansion are charged into the ledger, so inspecting TokenZero output is counted in recovery-adjusted accounting.

There are no `--detail`/`--max-items`/`--max-events` flags and no `pulse://` recovery refs in the shipped binary; the aggregate above is the entire report. Deeper inspection is done by reading the JSONL ledger or querying the SQLite cache directly.

## Stateless ROI Guard

TokenZero deliberately returns raw or near-raw output when compression would cost more than it saves. This is a hot-path decision and does not require a daemon, watcher, or background index.

Examples:

- tiny shell results such as `echo ok`, `pwd`, `mktemp -d`, and compact `git status --short --branch` use `raw_passthrough`.
- zero-hit searches render a one-line `# <tool> <query> — 0 matches` note (clamped query echo) with refs intact.
- short search hits and tiny files use `near_raw_with_ref`.
- broad expansions use `guarded_expansion` unless force is explicit.
- shallow tree passthrough and rewrite-control events are neutral in Pulse when the only cost is bounded TokenZero routing overhead.

Pulse records these as positive behavior, not failures. The purpose is to avoid inflated visible context while preserving exact refs and honest recovery-adjusted accounting.

Normal MCP/CLI display is also part of the guard: tiny outputs are shown as compact text with lowercase `tz_*` labels, while full metadata remains available only in JSON/debug or explicit structured paths. Pulse records `display_tokens`, `model_visible_tokens`, and `debug_tokens`; `visible_tokens` tracks the model-visible display, not the debug JSON envelope, structured tree rows, or hidden exact payload. Raw payloads are still not logged.

## Configuration surface

There is no `pulse` config-file block in the shipped binary, and `TOKENZERO_PULSE_DISABLED` is not read by the implementation; it does not disable local Pulse recording. There is also no `clear`, `compact`, or `import-stats` subcommand: retention and compaction are not yet automated, so prune the JSONL ledger by hand if it grows beyond what you want to keep. This documents the current behavior exactly; do not rely on variables or commands listed here as absent.

## Fail-Open Behavior

Pulse recording is best-effort. If the event ledger is locked, corrupt, missing, oversized, or unwritable, TokenZero tools still return their normal compressed response. `tokenzero pulse doctor` reports store integrity, marker agreement between JSONL and SQLite, and the hot `tool + timestamp` index plan.

## Sync and recovery

### Source Of Truth

- Primary: JSONL.
- Rationale: Pulse events are append-only telemetry for human inspection, Git backup, and recovery. SQLite is a rebuildable query cache for fast reports, doctor checks, and exports.

### Sync Triggers

- On command: `tokenzero pulse sync`, `tokenzero pulse doctor`, `tokenzero pulse export-jsonl <output>`, and `tokenzero pulse import-jsonl <input>`.
- On normal report: `tokenzero pulse` attempts a best-effort sync before rendering the JSONL report.
- On event write: `record_event` appends one complete JSONL line under the same Pulse lock, calls `sync_data`, and fsyncs the parent directory when it creates the ledger. SQLite catches up on the next sync/report command.
- Timer/throttle: not currently used. Short JSONL to SQLite lag is expected.

### Versioning

- DB marker: SQLite `meta` table stores `schema_version`, `source_of_truth`, `ledger_sha256`, `event_count`, `skipped_lines`, and `updated_unix`.
- JSONL marker: `events.meta.json` stores the same marker for the live ledger.
- Snapshot marker: `export-jsonl` writes `<snapshot>.meta.json`.
- Import rule: marked imports must match their JSONL hash/counts and must be newer than the current marker when hashes differ.

### Concurrency

- Lock file path: sibling `sync.lock` next to the Pulse ledger, for example `.tokenzero/pulse/sync.lock`.
- Busy timeout: SQLite connections use a 5 second busy timeout.
- Sync lock timeout: sync, import, and export wait up to 5 seconds for the Pulse lock.
- Event lock timeout: event appends wait up to 30 seconds for the Pulse lock, then fail open at the caller boundary for normal TokenZero tool responses.
- Ownership: lock files carry a token so an old guard cannot remove a lock reclaimed by a newer process.

### Storage Policy

- SQLite uses WAL mode, `synchronous=NORMAL`, `fullfsync=ON`, `wal_autocheckpoint=1000`, and `foreign_keys=ON`.
- Multi-step SQLite rebuilds run in one transaction.
- Hot indexes cover `tool + timestamp` and `event + timestamp`; `doctor` verifies index usage with `EXPLAIN QUERY PLAN`.
- Append-only JSONL event writes use no-follow/nonblocking opens on Unix and reparse-point opens on Windows, then validate both the ledger and shared lock handles as regular files before mutation. They fsync the ledger before returning. This favors durability over maximum write throughput for Pulse telemetry.
- Full JSONL exports and sidecar writes use temp files, fsync, atomic persist, and parent directory fsync where supported.

### Failure Handling

- DB locked: respect the busy timeout and return a non-zero error if SQLite cannot proceed.
- Sync lock held: retry briefly, then return a clear lock-held error with the `sync.lock` path. In `--json` mode the command prints `tokenzero.pulse.error.v1` with `error_kind=would_block`, `retryable=true`, and exits non-zero.
- JSONL parse error: sync skips corrupt lines and reports `skipped_lines`; imports reject corrupt input before replacing the ledger.
- Stale import: reject missing, stale, mismatched, or ambiguous markers when they would overwrite a different current ledger.
- Unsynced current ledger: reject importing a different snapshot until `tokenzero pulse sync` refreshes markers.
- Corrupt current ledger: allow a trusted marked snapshot to replace it only when the snapshot marker is newer than the current marker.
- DB corruption: remove the SQLite cache files and rebuild from JSONL.

## Recovery runbook

### Symptoms

- `tokenzero pulse doctor` reports `ok: false`.
- `sqlite_integrity` is not `ok`.
- `marker_match` is false.
- `skipped_lines` is non-zero.
- `import-jsonl` refuses a stale, corrupt, or marker-mismatched snapshot.
- Sync, import, or export reports that `.tokenzero/pulse/sync.lock` is held.

### Commands

```bash
tokenzero pulse doctor --json
tokenzero pulse sync --json
tokenzero pulse export-jsonl /tmp/tokenzero-pulse-snapshot.jsonl --json
tokenzero pulse import-jsonl /tmp/tokenzero-pulse-snapshot.jsonl --json
```

### Rebuild SQLite From JSONL

1. Run `tokenzero pulse doctor --json`.
2. If the DB is corrupt, rerun `tokenzero pulse sync --json`.
3. Confirm `sqlite_integrity` is `ok`, `marker_match` is true, and `skipped_lines` is `0`.
4. Keep the JSONL ledger as the recovery source. SQLite is disposable cache.
5. If recovery fails because `sync.lock` is held, JSON mode returns `error_kind=would_block`, `retryable=true`, and the lock path. Wait for the owning process to finish and rerun the command. Do not delete the lock anchor while a process may still hold it.

### Export A Clean Snapshot

1. Run `tokenzero pulse sync --json`.
2. Run `tokenzero pulse export-jsonl <snapshot.jsonl> --json`.
3. Keep `<snapshot>.meta.json` with the snapshot.
4. Verify the exported snapshot with `tokenzero pulse import-jsonl <snapshot.jsonl> --json` in a temporary root before using it for recovery.

### Import A Snapshot

1. Keep the snapshot JSONL and sidecar meta together.
2. Run `tokenzero pulse import-jsonl <snapshot.jsonl> --json`.
3. If import fails with a stale marker, run `tokenzero pulse sync --json` and inspect whether the current ledger has newer events.
4. If import fails with a marker mismatch, regenerate the snapshot sidecar from a trusted export.
5. If import fails on corrupt JSONL, restore the snapshot from Git or another backup and retry.
6. If the current ledger is corrupt, import a trusted marked snapshot whose marker is newer than the current `events.meta.json` marker.

### Safety Rules

- Do not overwrite a different current ledger with an unmarked snapshot.
- Do not import a marked snapshot whose hash/counts differ from its sidecar.
- Do not use a same-age or older snapshot to replace a corrupt current ledger.
- Do not manually delete `sync.lock` unless the owning process is dead and the lock is stale.
- Do not edit `events.sqlite` directly. Delete it only when rebuilding from JSONL.
- Preserve `events.jsonl`, `events.meta.json`, and exported `<snapshot>.meta.json` as a set.
