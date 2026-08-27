# Verification purpose gate + failure-first authoring

Bead: `fszero-88yr`
Date: 2026-08-07

Policy for every new test, oracle, or harness in this repo. Skills that encode
the same ideas (`failure-first-test-authoring` / the-setup, `verification-purpose-gate`
/ proof-gate) are **adopted here as law**; running those skills is optional.

## Purpose classes

Before shipping a test, classify it:

| Class | Meaning | Keep? |
| --- | --- | --- |
| **(b) Program contract** | Catches a named shipped bug class: golden vectors, conformance corpora, equality gates, crash-safety, concurrency, path jail, ABI/catalog, durability kill points | **Yes** -- durable regression asset |
| **(a) Session scaffolding** | Asserts incidental structure, restates the implementation, or exists only so an agent could green its own loop | **No** -- delete or fold into a (b) suite |

Default for new work: **minimal (b) only**. Prefer extending an existing contract file over adding a new top-level `tests/*.rs`.

## Failure-first (RED → green from production)

A new contract test only counts if:

1. It fails **RED** on today's code for the **named** contract reason (or is a pure characterization of an already-pinned golden with a frozen vector).
2. It goes green from a **production** change alone -- never by editing the test to match a broken implementation.
3. The test name or module docs name the **bug class** it catches (one sentence).

Skip full RED proof only for pure no-claim / inventory docs, not for behavior pins.

## Shared contracts live once (hub)

Cross-engine identity/ref/store/gate contracts belong in **ZeroStack** contract crates once. FSZero keeps **engine-owned policy** tests only (memory volume, world process model, FSZero surface catalogs, FSZero journal WA). Do not duplicate hub golden vectors here -- open a hub bead and depend.

Child work: `fszero-88yr.2` (hub move candidates).

## Suite inventory (on-disk, 2026-08-24)

The 2026-08-07 count (~83 integration files; ~631 `#[test]` in `tests/`; ~313 in `src/**`; ~944 total) is **stale**. Those files were pruned as fluff; Cargo.toml no longer declares them. Do not restore vanished suites.

Live counts (files that exist; `[[test]]` `path =` matches disk):

| Bucket | Files | `#[test]` |
| --- | --- | ---: |
| `fs-zero` integration | 8 | 143 |
| `tests/unit/fszero-store/durable_integrity_tests.rs` (`#[path]`, wired) | 1 | 28 |
| `tests/unit/fszero-core/raw_worker_protocol_tests.rs` (on disk, **unwired**) | 1 | 2 |
| `src/**` crate-inline | 2 files in `fszero-engine` | 4 |
| **Total on-disk `#[test]`** | | **177** |

`fs-zero` live `[[test]]` targets: `capability`, `crash_injection`, `confinement_hardening`, `path_confinement`, `racc_durability_matrix`, `smoke`, `structural_contract`, `wire_contract`. `fszero-kernel` has no `[[test]]`.

### (b) Contract-bearing -- keep (files that exist)

| Bucket | Files |
| --- | --- |
| Contract / ABI / wire | `tests/core/capability.rs` (12), `tests/core/wire_contract.rs` (6), `tests/engine/structural_contract.rs` (2) |
| Durability / crash | `tests/engine/crash_injection.rs` (22), `tests/engine/racc_durability_matrix.rs` (7), `tests/unit/fszero-store/durable_integrity_tests.rs` (28, `#[path]`) |
| Path jail | `tests/store/path_confinement.rs` (8), `tests/store/confinement_hardening.rs` (7) |
| Entry smoke | `tests/engine/smoke.rs` (79) — keep, do not grow unbounded; new cases belong in domain files |

### Mixed / review

| File | Note |
| --- | --- |
| `smoke.rs` | High-value entry smoke. Do not grow unbounded. |
| `raw_worker_protocol_tests.rs` | On disk under AGENTS `tests/unit/<crate>/` but not `#[path]`-included. Leave unwired this pass. |
| Vanished `[[test]]` paths (84 files) | **Not restored.** Declarations were removed from Cargo.toml. Re-add only with RED-from-production for a named bug class. |

### (a) Scaffolding

Do not recreate pruned integration files as class (a). Inline `src/**` unit tests: **4** remaining in `fszero-engine` (was ~318 on 2026-08-07).

Further hub-dedupe of shared golden vectors: `fszero-4xx6` (blocked on ZeroStack ownership).

## Authoring checklist (copy into PR / commit body when adding tests)

```
purpose: (b) program contract | (a) scaffolding (reject)
bug_class: <one sentence>
red_proof: <command that failed on OLD> | characterization-of-frozen-vector
green_from: production change only
shared?: if hub-owned, link ZeroStack bead instead of duplicating
```

## RCH / AGENTS constraints

- Never full-workspace `cargo test` on this Mac.
- Targeted: `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo test --test <file> <filter> -- --test-threads=1`
- Pre-existing failures: stash-baseline, file bead, move on.

## Children

| Bead | Work |
| --- | --- |
| `fszero-88yr.1` | Unit-test scaffolding sweep under `src/**` -- classify, delete/fold pure (a), prove suite size down |
| `fszero-88yr.2` | List hub-duplicated contract tests; move or delete FSZero copies after ZeroStack owns them |

## Done for this bead

Policy adopted in-repo. 2026-08-24 honesty pass: Cargo.toml `[[test]]` graph matches on-disk files (8 live `fs-zero` targets; kernel none). Vanished suites were not restored.
