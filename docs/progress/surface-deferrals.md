# Surface Deferrals Ledger

> This ledger records surface items that were proposed for inclusion and excluded, with rationale. Check it before adding a new feature to the SurfaceMatrix. Add an entry whenever a feature is moved from `supported` or `partial` to `excluded`, with the architectural reason and the retry-condition predicate.

ZeroStack-specific failure terms in this pillar: `rival-dirty-tree`,
`host-path-leak`, `engine-import-cycle`, `daemon-install`.

## Mandatory Fields per Entry

Same schema as `perf-negative-results.md`. Every `excluded` / `missing` /
`partial` row that stays deferred MUST have a retry-condition predicate.

## Retry-Condition Predicate Vocabulary

Same eight forms. Lint with `python3 scripts/check_ledger_retry.py`.

### Forbidden phrases (listed so the linter can skip this section)

- later
- in the future
- down the road
- if it seems important
- we should revisit
- tracked elsewhere
- TBD
- TODO
- FIXME
- maybe
- eventually
- when we have time
- if circumstances change
- future work
- might be worth trying
- someone should look at this
- interesting direction
- worth exploring

---

## Cass mine (Phase 8)

See `perf-negative-results.md` § Cass mine. cass is **MISSING** (blocker,
not a skip). Surface-relevant `git log --since='60 days ago'` hits:
`rejected` 2, `abandoned` 1, `host path` 5. Exact slug `rival-dirty-tree`:
**none** in `rg` or git subject lines (the four dirty paths are live
untracked/modified in the working tree).

---

## Entries

### 2026-08-17 -- ten-crates-absent-from-feature-universe -- CLOSED

- **date:** 2026-08-17
- **hypothesis:** Leave zero-cert, zero-codemode, zero-gate, zero-gauge, zero-ledger, zero-mcp, zero-process, zsx-core, zsx-node, and zsx with zero FeatureUniverse rows and still claim a complete surface.
- **result:** CLOSED
- **evidence:** Pass 1 finding. Pass 2 grew the matrix 38 → 71 rows (then 77 after passes 5–7) and classified those crates. Weight script requires `retry_condition` on every partial/missing/excluded row. Current dashboard: present=61 partial=9 missing=4 excluded=3.
- **retry_condition_predicate:** "Retry condition not applicable -- the gain is structural, not numerical."

### 2026-08-17 -- global-sum-weight-waiver -- CLOSED

- **date:** 2026-08-17
- **hypothesis:** Treat the in-repo global `sum(weights) == 1.0` (no per-family 1.0) as a FeatureUniverse bug and rebalance to per-category 1.0.
- **result:** CLOSED
- **evidence:** Pass 2 documented this as a **waiver, not a bug**. Hub FeatureUniverse is one product universe. Per-family 1.0 would give a 1-row crate the same category mass as CONTRACT-core `zsx`. Enforced by `scripts/check_feature_universe_weights.py` (`weight_policy = "global-sum-1.0"`). `[SPEC-FU-003]`.
- **retry_condition_predicate:** "Reconsider only inside the broader parity-score category redesign (track as SPEC-FU-003)."

### 2026-08-17 -- F-FSZERO-PRIVATE-ENGINE-SURFACE -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Absorb FSZero engine-only surfaces into the hub FeatureUniverse so excluded-as-debt goes away.
- **result:** DEFERRED
- **evidence:** Matrix excluded row with non-zero weight. CONTRACT §1: FSZero is a sibling engine. Strict coverage (0.899590) < effective (0.910788) because excluded still counts as debt.
- **retry_condition_predicate:** "Worth reconsidering when FSZero is a public Cargo workspace member of this hub with its own CONTRACT.md."

### 2026-08-17 -- F-GRAPHZERO-PRIVATE-ENGINE-SURFACE -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Absorb GraphZero engine-only surfaces into the hub FeatureUniverse.
- **result:** DEFERRED
- **evidence:** Same excluded-as-debt policy as FSZero. Pass 2 added the row so a strict-100% claim stays blocked on purpose.
- **retry_condition_predicate:** "Worth reconsidering when GraphZero is a public Cargo workspace member of this hub with its own CONTRACT.md."

### 2026-08-17 -- F-TOKENZERO-PRIVATE-ENGINE-SURFACE -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Claim hub Exact-token surface by folding TokenZero Exact certification into the hub FeatureUniverse.
- **result:** DEFERRED
- **evidence:** CONTRACT §5 permits Exact on TokenZero. Hub correctly excludes engine-only Exact. Related: `estimate-labeled-exact` and `[SPEC-HON-006]` in the conformance ledger.
- **retry_condition_predicate:** "Worth reconsidering when TokenZero is a public Cargo workspace member of this hub with its own CONTRACT.md."

### 2026-08-17 -- F-FUZZ -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Count the checked-in untrusted-bytes corpus file as a cargo-fuzz campaign and mark `F-FUZZ` present.
- **result:** DEFERRED
- **evidence:** Pass 7: still missing. Matrix rationale: no `fuzz/fuzz_targets/` campaign; only `tests/unit/zero-abi/fuzz_corpus_untrusted_bytes_20260815.rs`. Family `quality` verdict is `none`.
- **retry_condition_predicate:** "Blocked until fuzz/fuzz_targets/ contains at least one cargo-fuzz target for ZeroRef parse, CAS put, or raw_worker frame decode and cargo fuzz list names it; track as F-FUZZ."

### 2026-08-17 -- F-MIRI-NARROW -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat host rust-toolchain miri presence as `F-MIRI-NARROW` present.
- **result:** DEFERRED
- **evidence:** Pass 7: still missing. `spec_version_contract.toml` miri status: host toolchain matches; no CI job. No rch job runs `cargo +nightly miri test -p zero-ref`.
- **retry_condition_predicate:** "Blocked until CI or rch runs cargo +nightly miri test -p zero-ref (or zero-store hot paths) and a failing UB report fails the gate; track as F-MIRI-NARROW."

### 2026-08-17 -- F-REF-SERDE-FROMSTR -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat `ZeroRefV1::parse` + Display as FromStr/serde parity.
- **result:** DEFERRED
- **evidence:** Pass 7: still missing. `ZeroRefV1` has Display and parse(); no `FromStr` impl and no serde of the Display form. Bead note ZS-REF-008 is a note, not evidence.
- **retry_condition_predicate:** "Blocked until ZeroRefV1 implements FromStr and serde of the Display form with a unit test that parse-via-FromStr equals ZeroRefV1::parse; track as F-REF-SERDE-FROMSTR."

### 2026-08-17 -- F-REF-CAPABILITY-NEGOTIATION -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat `ZEROREF_VERSION` / MAJOR / MINOR consts as a negotiate API.
- **result:** DEFERRED
- **evidence:** Pass 7: still missing. No public `negotiate(major, minor) -> IncompatibleVersion`. Bead note ZS-REF-009.
- **retry_condition_predicate:** "Blocked until a public function accepts a peer (major, minor) and returns IncompatibleVersion for a different major before any payload work; track as F-REF-CAPABILITY-NEGOTIATION."

### 2026-08-17 -- dashboard-gate-red -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Round partial/missing/excluded families up and call the release gate green (strict-100% certifiable).
- **result:** DEFERRED
- **evidence:** Pass 7: `conformance/contracts/feature_coverage_dashboard.json` -- global effective=0.910788, strict=0.899590, `strict_100_certifiable: false`, gate **red**. Partial never rounds up. Four missing + three excluded + nine partial rows remain.
- **retry_condition_predicate:** "Worth reconsidering when scripts/check_feature_coverage_dashboard.py reports gate=green and strict_100_certifiable=true."

### 2026-08-17 -- detector-no-greenfield-branch -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Trust `scripts/detect-project-class.sh` auto-detect (`UNKNOWN`) as the project class without an operator override.
- **result:** DEFERRED
- **evidence:** Pass 1: detector has no Greenfield branch. Operator confirmed `Greenfield-Rust-class`. `phase0_project_class.json` + `phase0_workspace_init.md` yellow waiver.
- **retry_condition_predicate:** "Blocked until detect-project-class.sh grows a Greenfield branch that emits Greenfield-Rust-class without an operator override; track as detect-project-class-greenfield."

### 2026-08-17 -- rival-dirty-tree -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Edit or "clean up" the four rival-dirty files as part of a hub gauntlet pass (`crates/zsx-core/src/fszero.rs`, `docs/codemode.md`, `tests/unit/zsx-core/fszero_tests.rs`, `.zsx_patch.diff`).
- **result:** DEFERRED
- **evidence:** Working tree at Phase 8 HEAD `d141413`: those four paths are modified/untracked. Gauntlet law: do not touch them. They are not this pass's surface to classify or rewrite.
- **retry_condition_predicate:** "Reconsider only inside the broader one-repo FSZero adapter cutover (track as rival-dirty-tree)."
- **failure_term:** `rival-dirty-tree`

### 2026-08-17 -- F-CI-PR-GATES-no-tests -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat manual `workflow_dispatch` fmt/clippy/build as full PR CI and mark `F-CI-PR-GATES` present.
- **result:** DEFERRED
- **evidence:** Pass 1: CI has no test job. `.github/workflows/ci.yml` is `workflow_dispatch` only (no push/PR). Jobs: feature-universe, lint, build, privacy. Matrix: `F-CI-PR-GATES` partial. Notes: still no `cargo test` job.
- **retry_condition_predicate:** "Worth reconsidering when .github/workflows/ci.yml or DSR runs cargo test on every PR/push, not only workflow_dispatch fmt/clippy/build."

### 2026-08-17 -- agents-md-as-feature-evidence -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Use gitignored `AGENTS.md` as FeatureUniverse evidence so CI path-exists checks pass on a fresh checkout.
- **result:** REJECTED
- **evidence:** Pass 2: AGENTS.md is a spec source (on-disk hash) but must not appear as FeatureUniverse evidence. CI checkout would fail a path-exists gate. Tracked mandate pointer: `docs/progress/AGENTS-MANDATE.md`.
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."

---

## Open Candidates (matrix-missed / still missing)

- `F-CI-PR-GATES`, `F-ZSX-Q99-REPORT` (hub empty-window tested; engine accounting residual), `F-STORE-QUARANTINE-REAP`, `F-MIRI-NARROW` (partial until rch miri is green), `F-REF-ERROR-TAXONOMY` (honest partial)
- Dashboard families still not `full`: ci, conformance, quality, zero-ref, zero-store, zsx-core (zero-codemode is now full)

## Retired Candidates Worth Flagging

- `ten-crates-absent-from-feature-universe` -- CLOSED in pass 2
- `global-sum-weight-waiver` -- not a bug
- seed `example-candidate-do-not-keep` -- replaced by real entries

---

*Phase 8 durable ledger. Lint with `python3 scripts/check_ledger_retry.py`.*
