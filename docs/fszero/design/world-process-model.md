# World process model (single-writer honesty)

Status: normative companion to `n-agent-worlds.md` and filesystem-contract v1.
Bead: fszero-w2g.46 / AC3.

## Ownership

- **One process, one session** owns durable world registry mutations and
  world commits for a repository store.
- Concurrent **in-process** N-worlds (fork / stage / resolve / rebase) are
  supported; they share the session’s SQLite metadata and CAS.
- Concurrent **multi-process** world commits on the same tree are **not**
  claimed. Cross-process agents should serialize externally (lock, queue, or
  single FSZero server).

## Durability

| Event | Durable? |
| --- | --- |
| `new:` / `newbatch:` | Yes — `worlds` + `world_edits` upsert |
| `fork` | Yes — empty active row (edits deferred) |
| `edit:` stage | Yes — full upsert of staged certs |
| `resolve:` / `rebase:` | Yes — upsert after in-memory update |
| `drop:` / `resolve:…:abort` | Yes — state `dropped`, edits cleared |
| `commit:` | Per-file `atomic_write` + mutation journal; registry → `committed` |

Rehydrate on session open rebuilds active worlds from certs. Empty forks
remain active until staged or dropped.

## Encoding: world edit rehydrate is text-only (UTF-8)

Finding **F-ROT-18-WORLD-UTF8-LOSSY** (`fszero-rotation-i1-gqgt.13`).

World file edits are stored as content-addressed **byte** pre/post payloads in
the recovery store. On session open, `Session::world_file_edit_from_cert`
rebuilds in-memory `WorldFileEdit` fields (`pre`, `post`, `old`, `new`) for
hunk / three-way merge logic.

**Policy (current):**

1. Cert text and pre/post payloads are decoded with `String::from_utf8_lossy`
   (`src/core/session.rs` rehydrate path). Invalid UTF-8 sequences become U+FFFD
   replacement characters.
2. Worlds are a **text edit** surface (line hunks, find/replace, three-way
   merge on strings). They are **not** an exact-bytes overlay for arbitrary
   binary files.
3. Exact bytes for any path still live behind `fz://` content refs / CAS; those
   paths do not go through lossy rehydrate. Do not use worlds to stage binary
   assets if you need bit-identical restore of invalid-UTF-8 regions.

**Not fail-closed today:** rehydrate does not reject non-UTF-8 payloads; lossy
decode is intentional for text-world liveness. A future fail-closed mode
(reject world rehydrate when pre/post are not valid UTF-8) would be a separate
product decision and must not silently green binary-world claims.

**Operator rule:** treat world `pre`/`post` strings as UTF-8 text views. For
binary fidelity use CAS refs + ordinary write/edit tools, not world string
hunks.

## Crash and multi-file honesty

- Multi-file commit is **plan → sequential per-file publish** with
  **compensating rollback** on observed write/journal/preimage failure.
- It is **not** power-loss atomic across files. A kill mid-loop can leave a
  subset of files updated; recovery is operator/journal guided.
- Each successful file write is journaled (`op=world`) with the actual
  pre-write bytes so `fs.undo` restores the live base, not the staging
  preimage alone.
- Journal INSERT failures **fail the op** (fail-closed); callers restore
  published bytes when possible. Silent journal holes are a bug.

## Legal set L — store barrier vs workspace multi-file

Parent AC: `fszero-ai-filesystem-excellence-jqf.5` (workspace durability
column). FAULT-R02-003 / epic `fszero-k4ur`.

After any kill/restart, a multi-file world commit must land in a member of
legal set **L** — never an undocumented hybrid. Today the **store** half of a
world (registry + CAS/journal metadata) sits in the absolute-durable barrier
class (`docs/durability.md`); the **workspace tree** half does not yet have a
multi-file crash barrier. Score the workspace column against this table, not
against pack/SQLite alone.

| Dimension | Store barrier (SQLite + pack) | Workspace multi-file (`commit_world`) |
| --- | --- | --- |
| Unit of atomicity | One SQLite txn / one pack locator after `sync_all` | One file via `atomic_write`; N files are sequential |
| Crash mid-unit | AllPre or AllPost for that txn/locator | Per file: AllPre or AllPost for that path |
| Crash across units | N/A (single barrier) | Partial is transient only: reopen recovery rolls the unacked publish back to AllPre |
| Legal set L (target) | `{AllPre, AllPost}` only | `{AllPre, AllPost}` **or** documented Partial→recover to `{AllPre\|AllPost}` |
| Observed failure path | Fail closed; integrity report | Compensating rollback when the process still runs; kill mid-loop is undone by reopen recovery from the durable commit-intent record (`fszero-k4ur.3`) |
| What jqf.5 scores here | Pack/SQLite column (already gated) | **Workspace column** — must not claim store-class atomicity for multi-file publish |
| Evidence today | `tests/crash_injection.rs`, durability barrier tests | `tests/world_durability.rs` T-WMC-01..05 (SIGKILL windows + post-reopen classification) |

**L (normative vocabulary):**

- **AllPre** — none of the commit’s workspace paths reflect the new bytes;
  world registry not `committed` (or rolled back).
- **AllPost** — every planned path matches post-commit bytes; registry
  `committed`; journal rows cover each publish.
- **Partial** — some paths AllPost and some AllPre. Legal **only** as a
  transient observed state that recovery must collapse to AllPre or AllPost
  before the next successful ack (not a stable advertised outcome).

### Reopen recovery (fszero-k4ur.3)

`commit_world` writes a durable commit-intent record (`world/{wid}/commit_intent`:
one `rel<TAB>pre_ref<TAB>post_ref` line per planned path) and moves the world to
state `committing` BEFORE the first workspace byte lands. The record is retired
on ack (`committed`) or on compensating rollback. Every durable open therefore
sees any world killed mid-publish and, before rehydrating the registry, restores
each published path to its pre bytes and returns the world to `active` for retry.
A path whose live bytes match neither pre nor post was touched by someone else
after the crash and is left untouched rather than clobbered.

Multi-file world commit is still a **sequential publish**, not a store-class
single barrier: Partial is observable by another process during the window
between the first and last `atomic_write`. What is guaranteed is that no
*stable* Partial survives a reopen.

## Preimage

- Plan phase merges against current disk.
- Write phase **re-reads** each file immediately before `atomic_write` and
  aborts with compensating rollback if bytes ≠ planned current (TOCTOU /
  external writer).
- This matches edit’s `final_verify` spirit; it does not provide multi-process
  serializability.

## World refs

World IDs and overlay refs are store-scoped identity for speculative edits.
They are not a cross-process capability token (`world-ref.md`).

## Out of scope here

- `base_generation` shared base manifests (design in `n-agent-worlds.md`).
- Multi-process serializable world commit.
- Cross-host world handles.
