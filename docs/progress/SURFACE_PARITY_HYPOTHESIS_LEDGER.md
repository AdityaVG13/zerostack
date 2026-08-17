# Surface-parity hypothesis ledger

Index into [GAUNTLET_EXPERIMENT_DESIGNS.md](GAUNTLET_EXPERIMENT_DESIGNS.md). Full template fields live there. Grep the deferral ledger first: [surface-deferrals.md](surface-deferrals.md).

**Open-hypothesis count (this pillar):** 3 (`SURF-0013`, `IDEA-0017`, `ADV-0001`). Combined with perf+conformance OPEN cards the global open-hypothesis count is **15**.
**Pass-11 ranked remediations:** 8 closed. **Phase 12 AUTO-FIX:** `SURF-0010` present; `SURF-0011` closed as honest-partial; `OPEN-0014` `NO_EVIDENCE`.

Matrix after Phase 12: 77 features; present=67 partial=7 missing=0 excluded=3; effective=0.952282; strict=0.940573; gate=red.

## Missing (CONFIRMED_GAP)

None of the pass-11 ranked missing rows remain. Residual missing is only if the dashboard still lists other holes.

## Partial

| ID | Feature | Status | Why |
|---|---|---|---|
| SURF-0005 | `F-CONF-HARNESS` | CLOSED | CONTRACT §8 forbids a product CLI |
| SURF-0006 | `F-CI-PR-GATES` | CONFIRMED_GAP | GH `workflow_dispatch` only; DSR has zero quality checks; rch is documented test runner |
| SURF-0001 | `F-FUZZ` | CLOSED | cargo-fuzz targets `zeroref_parse` + `abi_frame_decode` |
| SURF-0002 | `F-MIRI-NARROW` | CLOSED | script landed; feature stays **partial** until rch miri is green |
| SURF-0003 | `F-REF-SERDE-FROMSTR` | CLOSED | FromStr + Display-form serde |
| SURF-0004 | `F-REF-CAPABILITY-NEGOTIATION` | CLOSED | `negotiate(major, minor)` |
| SURF-0007 | `F-ABI-PROPTEST-ROUNDTRIP` | CLOSED | `crates/zero-abi/tests/abi_proptest.rs` |
| SURF-0008 | `F-REF-ENGINE-ADOPTION-LOCKSTEP` | CLOSED | out-of-repo; hub cannot enforce engines |
| SURF-0009 | `F-STORE-ENSURE-LAYOUT` | CLOSED | `ensure_layout` creates `blobs/` + `gc/` |
| SURF-0010 | `F-CODEMODE-CANCEL` | CLOSED | hub `zero-mcp` tests; feature **present** |
| SURF-0011 | `F-ZSX-Q99-REPORT` | CLOSED | empty-window test landed; feature stays **partial** (engine residual) |
| SURF-0012 | `F-REF-ERROR-TAXONOMY` | CLOSED | reserved classes documented; feature stays **partial** |
| SURF-0013 | `F-STORE-QUARANTINE-REAP` | OPEN | functions exist; tests not inventoried |

## Excluded / gate

| ID | Feature | Status |
|---|---|---|
| SURF-0014 | FSZero/GraphZero/TokenZero private surfaces | CLOSED (excluded-as-debt) |
| SURF-0015 | dashboard gate red at 0.899590 | CLOSED (honest) |

## Other surface cards

| ID | Status | Note |
|---|---|---|
| CLOSED-0007 | CLOSED | FeatureUniverse + dashboard loader |
| CLOSED-0009 | CLOSED | global-sum-1.0 waiver |
| CLOSED-0010 | CLOSED | ledger retry lint |
| OPEN-0014 | NO_EVIDENCE | DSR has zero quality checks for zerostack |
| IDEA-0017 | OPEN | ratchet floor on 0.899590 |
| ADV-0001 | OPEN | submodular close order (ranks pass 11) |

## Pass-11 in-hub rank (copy of designs)

1. SURF-0003 FromStr + serde Display
2. CONF-0006 AGENTS hash re-pin (conformance, unblocks preflight)
3. SURF-0001 fuzz target floor
4. SURF-0007 ABI proptest
5. SURF-0009 ensure_layout blobs/gc or loud error
6. SURF-0004 negotiate(major, minor)
7. SURF-0012 error-class reachability
8. SURF-0002 miri on `zero-ref` via DSR/rch

Out of repo: SURF-0008, SURF-0011 residual, SURF-0014, HON-006 Exact emission.

## Already correct (do not re-open)

- `F-FEATURE-UNIVERSE-INREPO` present (`d141413`)
- `F-ORACLE-ENGINE-IDENTITY` / `F-ORACLE-FAILURE-BUNDLE` present
- `F-STORE-CRASH-ORACLE` / `F-STORE-BENCH-HISTORY` / `F-REF-PROPTEST` present
- `F-MCP-CATALOG-TWO-TOOLS` (exactly `zero_execute` + `zero_wait`)
- Partial never rounds up; excluded is strict debt
