# Preimage guards: isolation honesty (fszero-ip16.5)

Status: normative honesty note. Does **not** claim multi-process SI.

## Claim (what is true)

Within **one** FSZero session/process:

- Verified edit and world commit re-read the live file immediately before
  publish and refuse with `stale preimage` / equivalent when bytes differ
  from the planned base (`src/core/fs_ops.rs`, `src/core/world.rs` write phase).
- `fs.undo` similarly requires current content to match the journaled
  post-state, else `undo:0 (stale…)`.
- Failed preimage is a **typed, non-silent** error (fail closed for that op).

Evidence: `tests/filesystem_contract.rs` (stale preimage refuses commit),
`tests/world_durability.rs` (write-phase stale preimage), classifier
`stale_preimage` in `operation_abi.rs`.

## Non-claim (what is false)

Preimage guards are **not** multi-process snapshot isolation (SI) and are
**not** a distributed lock.

| Anomaly class | Status under multi-process concurrent writers |
| --- | --- |
| Lost update | Possible if two processes plan against the same base and both pass their own process-local preimage race window before either renames. |
| Write skew | Possible across multi-file plans without an external coordinator. |
| Silent last-writer-wins | **Not** the intended single-process path: loser should see stale preimage **if** the winner’s bytes land before the loser’s re-read. Timing races remain a control-surface gap. |

Cross-process agents must serialize externally (OS lock, single FSZero
server, queue). See also `world-process-model.md` (multi-process world
commits not claimed) and `n-agent-worlds.md`.

## Kill / red criteria

A concurrent external rewrite of the same path during another process’s
plan→write window must **not** be documented as SI. Acceptable outcomes:

1. Loser observes `stale preimage` / fail-closed error, or
2. Documented known anomaly (this file) when both processes race without
   external serialization.

No product surface may advertise multi-process SI solely on the basis of
preimage checks.
