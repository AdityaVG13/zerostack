# Large-file LOC budget (advisory ratchet)

Scan date: **2026-08-15**. Method: `scripts/loc_budget_offenders.py` (line
count over `*.rs`, excluding `target/` and `.git/`).

## Advisory budget

- **Soft cap:** new Rust modules should stay **≤ 500 LOC** unless the change
  includes a short exemption note (PR description or nearby module docs)
  naming why a split would be worse.
- This is **not** a CI job and does **not** run `cargo`. Re-run the script
  before citing counts; the tree can move under concurrent writers.
- Existing offenders are listed below as extraction targets, not as instant
  rewrite mandates.

## Current files > 500 LOC

| LOC | Owner (crate) | Path | Extraction target (advisory) |
| ---: | --- | --- | --- |
| 6331 | graphzero-store | `crates/graphzero-store/src/store/indexer.rs` | Split walk / shard write / publish / tests |
| 2027 | graphzero-store | `crates/graphzero-store/src/store/daemon.rs` | Session vs spawn vs auth paths |
| 1856 | graphzero-engine | `crates/graphzero-engine/src/witness_cache.rs` | Cache core vs policy vs tests |
| 1736 | graphzero-engine | `crates/graphzero-engine/src/surface_handshake.rs` | Handshake steps / codecs |
| 1563 | graphzero-store | `crates/graphzero-store/src/store/query/snapshot.rs` | Open/load vs query helpers |
| 1540 | graphzero-store | `crates/graphzero-store/src/store/entity.rs` | Publish hydrate vs query API |
| (split) | graphzero-engine | `crates/graphzero-engine/src/blast.rs` facade + `blast/{types,parse,traverse,render}/` | Done this pass — re-run `scripts/loc_budget_offenders.py` before citing |
| 1419 | graphzero-engine | `crates/graphzero-engine/src/dispatcher/execute.rs` | Op handlers by family |
| 1265 | graphzero-store | `crates/graphzero-store/src/store/expand.rs` | Expand strategies |
| 1192 | graphzero-core | `crates/graphzero-core/src/invalidation.rs` | Rules vs evaluation |
| 1149 | graphzero-store | `crates/graphzero-store/src/store/durability_receipt.rs` | Receipt IO vs validation |
| (split) | graphzero-engine | `crates/graphzero-engine/src/query_surface/surfaces.rs` facade + `surfaces/*.rs` | Done this pass — re-run `scripts/loc_budget_offenders.py` before citing |
| 1036 | graphzero | `crates/graphzero-cli/src/dispatch.rs` | CLI dispatch tables |
| 1008 | graphzero | `crates/graphzero-cli/src/packaging.rs` | Pack vs verify |
| 990 | graphzero-core | `crates/graphzero-core/src/grades.rs` | Grade tables vs scoring |
| 969 | graphzero-engine | `crates/graphzero-engine/src/surface_bench.rs` | Bench harness only |
| 889 | graphzero-engine | `crates/graphzero-engine/src/codemode/executor.rs` | Host vs interrupt/fuel |
| 856 | graphzero-store | `crates/graphzero-store/src/store/provenance.rs` | Record write vs query |
| 855 | graphzero-test-support | `crates/graphzero-test-support/src/gates/snap_export_perf_gate.rs` | Gate fixtures |
| 845 | graphzero-store | `crates/graphzero-store/src/store/session.rs` | Session state vs IO |
| 836 | graphzero-engine | `crates/graphzero-engine/src/codemode/steps.rs` | Step kinds |
| 818 | tests | `tests/types/child_identity.rs` | Test-only; split cases OK |
| 789 | graphzero-store | `crates/graphzero-store/src/store/query/name_bigram.rs` | Build vs published sidecar |
| 781 | graphzero-store | `crates/graphzero-store/src/store/query/lexical.rs` | Same |
| 759 | graphzero-engine | `crates/graphzero-engine/src/task_lens.rs` | Lens builders |
| 749 | graphzero-store | `crates/graphzero-store/src/store/compaction.rs` | Fold vs publish |
| 737 | graphzero-engine | `crates/graphzero-engine/src/release_gates.rs` | Gate registry |
| 733 | graphzero-engine | `crates/graphzero-engine/src/conformance.rs` | Suites by adapter |
| 722 | graphzero-engine | `crates/graphzero-engine/src/operation_abi/registry.rs` | Op tables |
| 714 | graphzero-store | `crates/graphzero-store/src/store/gc_roots.rs` | Root enum vs scan |
| 712 | graphzero | `crates/graphzero-cli/src/cli_args.rs` | Arg groups |
| 693 | graphzero-extract | `crates/graphzero-extract/src/rust_analyzer_lsp.rs` | LSP client vs mapping |
| 677 | graphzero-store | `crates/graphzero-store/src/store/ordinals.rs` | Sidecar IO vs bind |
| 667 | graphzero-engine | `crates/graphzero-engine/src/codemode/response.rs` | Response shaping |
| 651 | graphzero | `crates/graphzero-cli/src/mcp.rs` | Tool registration |
| 618 | graphzero-store | `crates/graphzero-store/src/store/blob_store.rs` | CAS write vs read |
| 614 | graphzero-store | `crates/graphzero-store/src/store/publish.rs` | Validate vs WAL append |
| 609 | graphzero-engine | `crates/graphzero-engine/src/query_surface/helpers.rs` | Shared formatters |
| 580 | graphzero-reserve | `crates/graphzero-reserve/src/service.rs` | Declare/check/release |
| 573 | graphzero-core | `crates/graphzero-core/src/refinement.rs` | Passes |
| 544 | graphzero-scip | `crates/graphzero-scip/src/ingest.rs` | Decode vs edge emit |
| 542 | tests | `tests/engine/query_surface_contract.rs` | Test-only |
| 540 | graphzero-why | `crates/graphzero-why/src/store.rs` | Why store API |
| 537 | graphzero-store | `crates/graphzero-store/src/store/claim/verify.rs` | Claim checks |
| 524 | graphzero-store | `crates/graphzero-store/src/store/delta_log.rs` | Segment IO |
| 507 | graphzero | `crates/graphzero-cli/src/agent_cli.rs` | Agent surface |
| 506 | tests | `tests/engine/impact_before_edit.rs` | Test-only |
| 505 | graphzero-store | `crates/graphzero-store/src/store/query/snap.rs` | Snap export |

**Count:** 48 files > 500 LOC on this scan.

## Local report script

```bash
python3 scripts/loc_budget_offenders.py
python3 scripts/loc_budget_offenders.py --threshold 500
```

No Cargo invocation; read-only line counts only.
