# Conformance Negative Results Ledger

> This ledger records conformance hypotheses that were investigated and rejected. Check it before opening a new conformance bead. Add an entry whenever a suspected divergence is shown to be a known false-positive class, a duplicate of an existing root cause, or a deferred-by-spec case.

ZeroStack-specific failure terms in this pillar: `mcp-late-ok-salvage`,
`commit-race-mislabel`, `clamp-end-vs-reject`, `fszero-fail-closed`,
`estimate-labeled-exact`, `engine-import-cycle`, `host-path-leak`.

## Mandatory Fields per Entry

Same schema as `perf-negative-results.md`. `retry_condition_predicate` is load-bearing.

## Retry-Condition Predicate Vocabulary

Same eight forms as `perf-negative-results.md`. Lint with
`python3 scripts/check_ledger_retry.py`.

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

See `perf-negative-results.md` § Cass mine. cass is **MISSING**. This file
does not silently skip the mine: the live blocker lives in the perf ledger
as `cass-unavailable-phase8`. Conformance-relevant `git log --since='60 days ago'`
hits: `commit_race` 6, `mcp-late-ok` 1 (`96361ad` / `zerostack-mcp-late-ok-salvage-tmh9`),
`fszero-fail-closed` 1 (`ba7c07e`). Exact slugs `mcp-late-ok-salvage`,
`commit-race-mislabel`, `clamp-end-vs-reject`, `fszero-fail-closed` as
in-tree identifiers: **none** (they are failure-term vocabulary, not file names).

---

## Entries

### 2026-08-17 -- invent-second-conformance-catalog -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Stand up an in-repo conformance CLI / second MCP catalog (`{ns}_execute_code`) so FeatureUniverse can mark `F-CONF-HARNESS` present.
- **result:** REJECTED
- **evidence:** `conformance/CONTRACT.md` §8 forbids an in-repo conformance CLI and forbids resurrecting `{ns}_execute_code`. Pass 6 wired `crates/zerostack-harness` (FailureBundle, crash oracle, e-process) and **left** `F-CONF-HARNESS` partial on purpose. `[SPEC-NEG-001]`, `[SPEC-NEG-002]`.
- **retry_condition_predicate:** "Reconsider only inside the broader CONTRACT.md §8 redesign (track as F-CONF-HARNESS)."

### 2026-08-17 -- f-conf-harness-stays-partial -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Promote `F-CONF-HARNESS` from partial to present after Phase 6 harness modules landed.
- **result:** DEFERRED
- **evidence:** Pass 6 + pass 7: harness implements CONTRACT; §8 still forbids a product CLI catalog. Matrix row remains `partial` with retry on a §8 amendment. Dashboard family `conformance` verdict is `partial` (weighted 0.687499).
- **retry_condition_predicate:** "Worth reconsidering when conformance/CONTRACT.md §8 is amended to allow an in-repo conformance CLI and F-CONF-HARNESS status is rewritten to present."

### 2026-08-17 -- self-compare-oracle-identity -- CLOSED

- **date:** 2026-08-17
- **hypothesis:** Certify spec-oracle comparisons without an EngineIdentity discriminator (Subject can compare against itself).
- **result:** CLOSED
- **evidence:** Pass 3 landed `crates/zerostack-harness/src/engine_identity.rs` + asserted-distinct check at the comparator. `F-ORACLE-ENGINE-IDENTITY` is **present**. Subject=`zerostack`, Oracle in `{spec-v1, property-suite-v1, prior-commit-<sha>, round-trip, miri, clippy}`. Distinct from `raw_worker::EngineIdentity` (fz/gz/tz).
- **retry_condition_predicate:** "Retry condition not applicable -- the gain is structural, not numerical."

### 2026-08-17 -- spec-comp-002-unverified -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat `[SPEC-COMP-002]` (FSZero journaled mutation; a subsequent failed step MUST roll back earlier mutation in the same execute) as verified from journal+undo types alone.
- **result:** DEFERRED
- **evidence:** Pass 2 extracted the tag. Pass 3 left it `UNVERIFIED`: journal+undo metadata exists; live subsequent-failed-step rollback needs a multi-mutation execute. `docs/spec/SPEC-TAGS.md`.
- **retry_condition_predicate:** "Blocked until a hub verifier drives a multi-mutation execute whose subsequent step fails and proves earlier mutation rolled back; track as SPEC-COMP-002."

### 2026-08-17 -- spec-hon-002-unverified -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat `[SPEC-HON-002]` (spill receipts that cannot certify tokens MUST null `visibleTokenCount` and set `requires_tokenzero_certification`) as verified from bench fixtures.
- **result:** DEFERRED
- **evidence:** Pass 2/3: hub crates do not yet emit `result_finalization_receipt.v1`; only bench fixtures do. `docs/spec/SPEC-TAGS.md`.
- **retry_condition_predicate:** "Blocked until a hub production path emits result_finalization_receipt.v1 with null visibleTokenCount and status requires_tokenzero_certification on an uncertified spill; track as SPEC-HON-002."

### 2026-08-17 -- spec-hon-006-unverified -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Implement hub-side Exact `billed_tokens` / `raw_tokens` / `visible_tokens` to "satisfy" `[SPEC-HON-006]`.
- **result:** DEFERRED
- **evidence:** `[SPEC-HON-006]` is a permission, not a hub requirement. Exact certification is TokenZero-owned. Pass 3 left the tag `UNVERIFIED` for that reason. Hub FeatureUniverse excludes engine-only Exact as `F-TOKENZERO-PRIVATE-ENGINE-SURFACE`.
- **retry_condition_predicate:** "Blocked until a TokenZero certification receipt is accepted by the hub without rejection and a verifier asserts that permission; track as SPEC-HON-006."

### 2026-08-17 -- spec-hub-002-unverified -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Close `[SPEC-HUB-002]` (fail loud; no silent success; no heuristic labeled exact) with a single static field-name check.
- **result:** DEFERRED
- **evidence:** Pass 3: global law; no single static surface is a complete fail-loud proof. Related honesty: `[SPEC-HON-004]`, `[SPEC-HON-005]`.
- **retry_condition_predicate:** "Blocked until a fail-loud suite covers silent-success and heuristic-labeled-exact on live zsx receipts (not comments); track as SPEC-HUB-002."

### 2026-08-17 -- spec-hub-005-unverified -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Treat `[SPEC-HUB-005]` (ABI digest bumps on Wire/version pins C-23/24/26) as verified by reading field names.
- **result:** DEFERRED
- **evidence:** Pass 3: digest bump on pin change is a mutation test, not a static field-name check. C-25 semantic-mutation bumps stay untagged (Ambiguous).
- **retry_condition_predicate:** "Blocked until a mutation test changes a C-23/24/26 pin and fails if the ABI digest does not bump; track as SPEC-HUB-005."

### 2026-08-17 -- mcp-late-ok-salvage -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Salvage a cancelled / timed-out MCP call as a success (`Ok` / retryable) when a late Ok arrives, or leave the kind unlabeled.
- **result:** DEFERRED
- **evidence:** `conformance/CONTRACT.md` §6: reported kind MUST be `commit_race`, `retryable` false; committed payload stays attached. Commit `96361ad` named late MCP Ok after cancel as `commit_race` (`zerostack-mcp-late-ok-salvage-tmh9`). `F-CODEMODE-CANCEL` is **partial**: CancellationSignal + CommitRace + MCP late-Ok salvage exist; the matrix retry requires a hub test that does **not** live only in rival-dirty `crates/zsx-core/src/fszero.rs`.
- **retry_condition_predicate:** "Retry only if this workload class exhibits measurable commit_race coverage below 1.0 on a hub test outside rival-dirty fszero.rs."
- **failure_term:** `mcp-late-ok-salvage`

### 2026-08-17 -- commit-race-mislabel -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Report a late Ok after cancel as `cancelled`, `timeout`, `ok`, or any kind other than `commit_race`.
- **result:** REJECTED
- **evidence:** CONTRACT §6; commits `140a1d7`, `132c6f1`, `e167e38`, `53fdfc0`, `422f095`, `96361ad`. A late domain Err MUST stay that Err (`[SPEC-SETL-002]`). Mislabeling hides a committed payload behind a cancel.
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `commit-race-mislabel`

### 2026-08-17 -- clamp-end-vs-reject -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Enforce TokenZero `Reject` fragment policy as the hub canonical, or claim lockstep because `CANONICAL_LINE_END_POLICY = ClampEnd` exists as a const.
- **result:** DEFERRED
- **evidence:** Matrix `F-REF-SELECT-STRICT` / engine-adoption notes: TokenZero sibling gauntlet found EmbeddedStore reject vs RecoveryStore clamp TrueDivergence. Hub defines ClampEnd but cannot enforce engines. `git log` hits `ClampEnd` once in 60d.
- **retry_condition_predicate:** "Worth reconsidering when a hub gate fails a sibling engine that uses Reject instead of ClampEnd, evidenced by a live zsx receipt or an engine CI job."
- **failure_term:** `clamp-end-vs-reject`

### 2026-08-17 -- fszero-fail-closed -- DEFERRED

- **date:** 2026-08-17
- **hypothesis:** Continue after FSZero cannot open its durable root, or treat type-level journal presence as proof of fail-closed rollback.
- **result:** DEFERRED
- **evidence:** Commit `ba7c07e` (`zerostack-fszero-fail-closed-d8vv`) fails closed when the durable root cannot be opened. Crash-oracle notes: MidJournalRecover is fail-closed or a consistent prefix. Live `[SPEC-COMP-002]` multi-mutation rollback remains UNVERIFIED. Rival-dirty `crates/zsx-core/src/fszero.rs` is out of scope for this pass.
- **retry_condition_predicate:** "Blocked until SPEC-COMP-002 live rollback and a hub test outside rival-dirty fszero.rs both land; track as fszero-fail-closed."
- **failure_term:** `fszero-fail-closed`

### 2026-08-17 -- engine-import-cycle -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Let FSZero / GraphZero / TokenZero import each other (or register engine MCP servers next to `zsx mcp`) to "simplify" composition.
- **result:** REJECTED
- **evidence:** CONTRACT §1: engines MUST NOT import each other. `[SPEC-COMP-001]`. E-process invariant `EnginesDoNotImportEachOther` (software `p0=1e-6, λ=0.9, α=0.001`). `F-HUB-NO-ENGINE-SOURCE` present. Engine MCP next to `zsx mcp` is also forbidden (`[SPEC-SURF-005]`).
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `engine-import-cycle`

### 2026-08-17 -- host-path-leak -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Commit absolute host paths (`/Users/...`, `/home/...`) in tracked evidence, beads exports, or docs because they are "just metadata."
- **result:** REJECTED
- **evidence:** `scripts/check_no_host_paths.py` + `scripts/check-portability.sh`. Commits `e59ae11`, `5b89857`, `062bec8`, `0361be1`. CI privacy job is manual `workflow_dispatch` only -- the gate exists; leaking is still a reject.
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."
- **failure_term:** `host-path-leak`

### 2026-08-17 -- both-error-as-failure -- REJECTED

- **date:** 2026-08-17
- **hypothesis:** Treat `(Err(_), Err(_))` oracle pairs as a conformance failure when error strings differ.
- **result:** REJECTED
- **evidence:** Pass 3/6: `crates/zerostack-harness/src/oracle.rs` -- both-error is agreement regardless of message; one-error-one-OK is a hard failure. `INV-BOTH-ERROR` in `conformance/contracts/invariant_catalog.toml`.
- **retry_condition_predicate:** "Not worth retrying as a standalone patch."

---

## Open Candidates

- `SPEC-COMP-002` live multi-mutation rollback verifier
- `SPEC-HON-002` production spill-receipt emission
- `F-CODEMODE-CANCEL` hub test outside rival-dirty `fszero.rs`

## Retired Candidates Worth Flagging

- `self-compare-oracle-identity` -- CLOSED in pass 3
- `invent-second-conformance-catalog` -- permanent reject unless CONTRACT §8 changes

---

*Phase 8 durable ledger. Lint with `python3 scripts/check_ledger_retry.py`.*
