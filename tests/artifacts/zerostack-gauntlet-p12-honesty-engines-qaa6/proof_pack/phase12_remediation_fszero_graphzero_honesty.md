# Phase 12 — FSZero fail-closed + GraphZero typed refs

**Bead:** `zerostack-gauntlet-p12-honesty-engines-qaa6`  
**Mode:** design pack. Implementation lands on the listed finding beads, one lever per commit.

## FSZero (A)

Fail closed on durable-store open failure unless `OpenMode::AllowInMemory { reason }`. Do not leave an unread `degraded` bool. Finding: `zerostack-gauntlet-p11i2-fszero-silent-degrade-y8b8`.

## GraphZero (A)

GC pins from typed `DomainResult.refs` only. Unparseable `gz://blob/` string scans are `Err`, not skip. Finding: `zerostack-gauntlet-p11i2-gz-heuristic-refs-v60n`.

## Constraints

- Do not touch `crates/zero-store/src/cas.rs`.
- Do not delete `scripts/zs`.
- Do not touch `zerostack-demonolith-*`.
