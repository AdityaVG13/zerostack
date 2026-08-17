# Remediation plan -- Phase 12

**Date:** 2026-08-17
**HEAD intent:** `phase12: remediation plan scored`
**Mode:** isomorphic rewrites scored; AUTO-FIX only in-hub mechanical leftovers
**Gate:** keep only proposals with `Impact × Confidence / Effort ≥ 2.0`
**Dashboard:** `effective=0.952282` `strict=0.940573` `gate=red` (honest; do not paint green)

Scoring (same shape as `/extreme-software-optimization` + `RUBRICS.md` perf gate):

| Axis | 1 | 3 | 5 |
|---|---|---|---|
| **Impact** | <5% coverage or no keep-eligible bench | one FeatureUniverse row or one verifier | family-full or keep-eligible primary-score move |
| **Confidence** | guess, no artifact | cited source + existing test seam | live green test / doctor / committed seed |
| **Effort** | <1h, 1 file | 4-16h, 2-5 files | >40h or out-of-repo install |

Universal 6-dimension scores (1-5, bar ≥20/30) sit under each recommended rewrite. Isomorphism proofs use the 5-line template.

**Do not implement:** TokenZero Exact emission, engine ClampEnd lockstep, in-repo conformance CLI, rival-dirty tree, fat-LTO flip, keep-gate perf claims from test-profile or `cv_pct=null`.

---

## Census after this pass

| Bucket | Count | Notes |
|---|---:|---|
| FeatureUniverse | 77 | present=67 partial=7 missing=0 excluded=3 |
| CONFIRMED_GAP remaining | **3** | `SURF-0006`, `PERF-0001`, `PERF-0003` |
| OPEN | 15 | SPEC tags, cass-adjacent measurement, ratchet, quarantine tests |
| NO_EVIDENCE | 1 | `OPEN-0014` (DSR has zero zerostack quality checks) |
| CLOSED this pass | 5 | `SURF-0010`, `SURF-0011`, `IDEA-0012`, `IDEA-0015`, `ADV-0003` |
| Release gate | red | partial + excluded remain |

---

## AUTO-FIXES applied this pass (implement now)

| Gap | Rewrite picked | Score | Evidence |
|---|---|---:|---|
| SURF-0010 / IDEA-0015 | Hub `execute_call_with_cancel` tests in `zero-mcp` | 5×5/1 = **25** | rch: 2 passed |
| ADV-0003 | Same test asserts inflight released (`max_inflight=1` second call) | 3×5/1 = **15** | same |
| SURF-0011 / IDEA-0012 | Empty `SessionResidencyGate` is `unavailable` | 4×5/1 = **20** | rch: 1 passed |
| SURF-0006 docs | Comment in `ci.yml` + rch stanza in `AGENTS-MANDATE.md` | 2×5/1 = **10** | files written; **does not** flip `F-CI-PR-GATES` |
| OPEN-0014 | Probe `dsr quality --tool zerostack` | 2×5/1 = **10** | `No quality checks configured` |

`F-CODEMODE-CANCEL` → **present**. `F-ZSX-Q99-REPORT` stays **partial** (engine residual). Gate stays red.

---

## Remaining CONFIRMED_GAP

### SURF-0006 -- `F-CI-PR-GATES` has no cargo test on PR/push

**Source:** matrix partial; `.github/workflows/ci.yml` is `workflow_dispatch` only; `dsr quality --tool zerostack` has zero checks.
**Pillar:** surface / ci.
**In-repo vs out-of-repo:** GH job is in-repo and forbidden (budget). DSR checks live in `~/.config/dsr/repos.yaml` (out-of-repo).

#### Rewrite A: Document rch as the test runner (done)
- **Sketch:** Comment in `ci.yml` + tracked mandate stanza. No GH `cargo test` job.
- **Isomorphism proof:**
  ```
  Change: Document that targeted tests run via rch, not GH Actions.
  Ordering preserved: yes
  Tie-breaking unchanged: yes
  Floating-point: not-applicable
  RNG seeds: untouched
  Golden outputs: not-applicable (comment + tracked markdown)
  ```
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Universal:** 24/30 (correctness 3, perf 3, blast 5, review 5, maintain 4, parity 4)
- **Runner-up if not picked:** N/A -- applied.

#### Rewrite B: Configure `dsr quality` checks for targeted `cargo test -p`
- **Sketch:** Add `checks:` under `zerostack` in `~/.config/dsr/repos.yaml` (`cargo fmt --check`, `clippy -D warnings`, a small `-p` list). Not a GH job.
- **Impact=4 Conf=4 Effort=2 → 8.0**
- **Universal:** 23/30
- **In-repo?** No. Operator DSR config.
- **Recommended later.** This is the honest close of `F-CI-PR-GATES` without burning GH budget.

#### Rewrite C: Add a GitHub Actions `cargo test` job on push/PR
- **Sketch:** New job `cargo test --workspace` or even `-p zero-ref`.
- **Impact=4 Conf=5 Effort=2 → 10.0** numerically, **rejected by policy** (hammers GH + host). Score is not permission.
- **Retry:** only if GH Actions budget is restored and AGENTS.md drops the DSR-first rule.

#### Recommended: A now (done), B later
- **Bead-summary draft:** `ci: wire dsr quality targeted cargo test -p`
- **Cross-pillar:** docs-only cannot regress perf or conformance. B must stay targeted (`-p`, `--test-threads=1`).

---

### PERF-0001 -- savings-bench `cv_pct` is null

**Source:** `.bench-history/savings-bench.latest.json` `cv_pct=null` `keep_eligible=false`.
**Pillar:** perf.
**In-repo vs out-of-repo:** measurement on rch (OPEN-0015). Not a product patch.

#### Rewrite A: Ten rch `release-perf` repeats (OPEN-0015)
- **Sketch:** Run the savings-bench headline on rch with `RUSTFLAGS=-C force-frame-pointers=yes`. Record numeric `cv_pct`. Keep only if `<=5`.
- **Impact=3 Conf=3 Effort=3 → 3.0**
- **Universal:** 21/30
- **Recommended later.** Measurement, not a keep claim.

#### Rewrite B: Invent `cv_pct` from the seed's 12 rows
- **Sketch:** Fill a number so `keep_eligible` looks true.
- **Impact=1 Conf=1 Effort=1 → 1.0 -- REJECTED (<2.0 and dishonest)**
- **Retry:** never.

#### Rewrite C: Leave the seed as the honest ineligible baseline
- **Sketch:** Status quo. Ratchet already refuses a keep.
- **Impact=2 Conf=5 Effort=1 → 10.0** (honesty, not a win)
- **Recommended now** as the standing position until A lands.

#### Recommended: C now, A later
- **Bead-summary draft:** `perf: rch savings-bench cv_pct n=10`
- **Cross-pillar:** no product code. Do not bless a keep from this card.

---

### PERF-0003 -- `cass` not on PATH

**Source:** `command -v cass` failed; blocker already in `perf-negative-results.md`.
**Pillar:** perf (infra).
**In-repo vs out-of-repo:** install is operator/host.

#### Rewrite A: Install cass and re-run the 60-day mine
- **Sketch:** `jsm install cass` or equivalent; `cass health --robot` green; mine failure terms; replace the blocker entry.
- **Impact=3 Conf=5 Effort=2 → 7.5**
- **Recommended later** (operator).

#### Rewrite B: Keep the recorded blocker + git-log fallback
- **Sketch:** Already done in Phase 8. Mandate forbids silent skip.
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended now** as standing position.

#### Rewrite C: Fabricate cass hits
- **Impact=1 Conf=1 Effort=1 → 1.0 -- REJECTED**

#### Recommended: B now, A later
- **Bead-summary draft:** `infra: install cass and mine 60d`
- **Cross-pillar:** install cannot change product behavior.

---

## Remaining OPEN (do not treat as bugs)

### SURF-0013 -- `F-STORE-QUARANTINE-REAP` tests

**Functions exist** (`quarantine_object`, `reap_stale_temps` from put). Hole is coverage.

#### Rewrite A: Unit test put reaps a stale `.tmp-` and quarantine moves a digest-mismatched body
- **Sketch:** `SharedCas::open(tmp)` + `set_times` older than `CAS_TEMP_REAP_AGE` + put; exclusive lock + `quarantine_object`.
- **Impact=3 Conf=4 Effort=2 → 6.0**
- **Recommended later.** Larger than this pass's <~40-line AUTO-FIX (needs `StoreLock`).

#### Rewrite B: Mark present without a test
- **Impact=1 Conf=2 Effort=1 → 2.0** at the floor, **rejected by honesty** (partial never rounds up).

#### Recommended: A later. Feature stays partial.

---

### CONF-0001 -- `[SPEC-COMP-002]` live multi-mutation rollback

#### Rewrite A: Hub verifier drives two mutations, fails the second, asserts first rolled back
- **Impact=4 Conf=2 Effort=4 → 2.0**
- **In-repo?** Only if the verifier uses public adapter APIs, not rival-dirty `fszero.rs`.

#### Rewrite B: Leave UNVERIFIED until engine-private APIs are public
- **Impact=2 Conf=4 Effort=1 → 8.0**
- **Recommended now.** A later, not in rival-dirty files.

---

### CONF-0002 / IDEA-0024 -- `[SPEC-HON-002]` production spill receipt

#### Rewrite A: Oversize `zsx exec` probe; assert `visibleTokenCount` null + `requires_tokenzero_certification`
- **Impact=4 Conf=2 Effort=3 → 2.67**

#### Rewrite B: Keep UNVERIFIED; fixture-only emission is documented
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended now.** A later. Do not emit Exact from the hub.

---

### CONF-0003 -- `[SPEC-HON-006]` accept TokenZero Exact receipt

#### Rewrite A: Fixture Exact receipt through the hub envelope; assert not rejected
- **Impact=3 Conf=3 Effort=2 → 4.5**
- **In-repo.** Permission tag. Must not *emit* Exact.

#### Rewrite B: Implement hub-side Exact (`billed_tokens`)
- **Impact=1 Conf=1 Effort=5 → 0.2 -- REJECTED** (violates `[SPEC-HON-001]`)

#### Recommended: A later. B never.

---

### CONF-0004 -- `[SPEC-HUB-002]` fail-loud suite on live receipts

#### Rewrite A: Suite feeds silent-success + heuristic-Exact envelopes; expect hard failure
- **Impact=4 Conf=3 Effort=3 → 4.0**

#### Rewrite B: Close from a field-name grep
- **Impact=1 Conf=1 Effort=1 → 1.0 -- REJECTED**

#### Recommended: A later.

---

### CONF-0005 -- `[SPEC-HUB-005]` ABI digest mutation test

#### Rewrite A: Mutate a C-23/24/26 pin constant in a test and fail if digest is unchanged
- **Impact=3 Conf=4 Effort=2 → 6.0**

#### Rewrite B: Static field-name check
- **Impact=1 Conf=2 Effort=1 → 2.0** (floor; weaker than A)

#### Recommended: A later.

---

### OPEN-0015 -- replicated savings-bench `cv_pct`

Child of PERF-0001 Rewrite A. Same score **3.0**. Later. Do not invent numbers.

### OPEN-0016 -- comprehensive-bench 93 on rch

#### Rewrite A: rch `cargo run -p zerostack-harness --bin comprehensive_bench -- --full`
- **Impact=4 Conf=2 Effort=4 → 2.0**

#### Rewrite B: Invent a 93-row matrix on this Mac
- **Impact=1 Conf=1 Effort=2 → 0.5 -- REJECTED** (`comprehensive-bench-93-on-mac` DEFERRED)

#### Recommended: A later, rch only.

### PERF-0007 -- profile-first `SharedCas::put`

#### Rewrite A: samply/flamegraph under `release-perf` store_cas; no source touch
- **Impact=3 Conf=2 Effort=3 → 2.0**

#### Rewrite B: Edit `cas.rs` from a test-profile hunch
- **Impact=1 Conf=1 Effort=2 → 0.5 -- REJECTED** (profile-first)

#### Recommended: A later. Not a keep until `cv_pct<=5` and frame ≥0.1%.

---

### IDEA-0002 -- pin tracked `AGENTS-MANDATE.md`

#### Rewrite A: Point `agents-law` spec source at the tracked mandate hash
- **Impact=3 Conf=3 Effort=2 → 4.5**

#### Rewrite B: Re-pin live gitignored `AGENTS.md`
- **Impact=2 Conf=3 Effort=2 → 3.0** (flake returns)

#### Recommended: A later. CONF-0006 already dropped certifying pin.

### IDEA-0010 -- smoke test requires certifying

#### Rewrite A: Tighten `preflight_does_not_panic_and_is_green` to `aggregate_outcome==green`
- **Impact=3 Conf=4 Effort=1 → 12.0** numerically
- **Blocked** until a certifying pin is blessed (would fail the suite on known advisory yellow).

#### Rewrite B: Rename the test to `preflight_does_not_panic`
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended later** as the honesty rename if A stays blocked.

### IDEA-0017 -- conformal ratchet on `strict=0.940573`

#### Rewrite A: Commit `reports/ratchet_state.json` + `apply-ratchet.sh` floor
- **Impact=3 Conf=3 Effort=2 → 4.5**

#### Rewrite B: Rely on dashboard gate=red without a committed ratchet
- **Impact=2 Conf=5 Effort=1 → 10.0** (status quo; honest)

#### Recommended: B now, A later. Floor must move to **0.940573** (not the Phase-9 0.899590).

### ADV-0001 -- submodular close order

#### Rewrite A: Use the ranked list as a planning artifact only
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended.** Not a product patch. Pass 11 already consumed the high-unit in-hub set.

#### Rewrite B: Re-include engine excluded rows to raise strict
- **Impact=1 Conf=1 Effort=5 → 0.2 -- REJECTED** (paints green)

### ADV-0002 -- e-process on spec-source hash

#### Rewrite A: Second detector on agents-law sha256
- **Impact=2 Conf=3 Effort=3 → 2.0** (redundant with preflight)

#### Rewrite B: Do not build a second detector
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended.** "Not worth retrying as a standalone patch."

---

## Remaining FeatureUniverse partial rows

These are matrix rows, not extra experiment cards.

### `F-REF-ENGINE-ADOPTION-LOCKSTEP` (SURF-0008 CLOSED / out-of-repo)

#### Rewrite A: Hub comment claiming ClampEnd enforcement
- **Impact=1 Conf=1 Effort=1 → 1.0 -- REJECTED**

#### Rewrite B: Engine CI job / live zsx receipt fails on Reject
- **Impact=4 Conf=2 Effort=5 → 1.6 -- below 2.0 in this repo**
- **Out-of-repo.** TokenZero / FSZero / GraphZero.

#### Recommended: stay partial. Do not implement ClampEnd in this hub.

### `F-CONF-HARNESS` (SURF-0005 CLOSED)

#### Rewrite A: Invent an in-repo conformance CLI
- **Impact=1 Conf=1 Effort=4 → 0.25 -- REJECTED** (CONTRACT.md §8)

#### Rewrite B: Keep harness-as-library; CLI stays forbidden
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended.** Honest partial.

### `F-MIRI-NARROW` (SURF-0002 CLOSED / partial)

#### Rewrite A: `rch exec -- cargo +nightly miri test -p zero-ref` green
- **Impact=3 Conf=3 Effort=2 → 4.5**
- **Recommended later.** Script exists; rch previously classified miri as non-compilation.

#### Rewrite B: Treat host toolchain presence as the feature
- **Impact=1 Conf=2 Effort=1 → 2.0** at the floor, **rejected** (matrix notes say host rust-toolchain is not the feature).

### `F-REF-ERROR-TAXONOMY` (SURF-0012 CLOSED / honest partial)

#### Rewrite A: Construct reserved classes on fake store paths in `zero-ref`
- **Impact=2 Conf=2 Effort=3 → 1.33 -- REJECTED** (wrong crate)

#### Rewrite B: Keep reserved classes documented; parser never emits them
- **Impact=2 Conf=5 Effort=1 → 10.0**
- **Recommended.** Retry only when store/resolution production paths construct them.

### `F-ZSX-Q99-REPORT` (SURF-0011 CLOSED / honest partial)

#### Rewrite A: Hub empty-window test (done)
- **Impact=4 Conf=5 Effort=1 → 20.0**

#### Rewrite B: Fake `WorkerTokenAccountingV1` in hub adapters
- **Impact=1 Conf=1 Effort=2 → 0.5 -- REJECTED**

#### Rewrite C: Engine adapters supply measured accounting
- **Impact=5 Conf=2 Effort=5 → 2.0**
- **Out-of-repo.**

#### Recommended: A done. C later in engines. Feature stays partial.

### `F-CI-PR-GATES` -- see SURF-0006 above.

### `F-STORE-QUARANTINE-REAP` -- see SURF-0013 above.

---

## Picked vs later (score ≥ 2.0 only)

| Gap | Recommended rewrite | Score | When | Where |
|---|---|---:|---|---|
| SURF-0010 / IDEA-0015 / ADV-0003 | Hub cancel + inflight tests | 25 / 15 | **now** | in-repo `zero-mcp` |
| SURF-0011 / IDEA-0012 | Empty Q99 unavailable test | 20 | **now** | in-repo `zsx-core` |
| SURF-0006 docs | rch/DSR comment | 10 | **now** | `ci.yml` + mandate |
| OPEN-0014 | DSR probe | 10 | **now** | NO_EVIDENCE |
| PERF-0001 standing | keep seed ineligible | 10 | **now** | no code |
| PERF-0003 standing | recorded cass blocker | 10 | **now** | ledger |
| F-CONF-HARNESS | no CLI | 10 | standing | CONTRACT §8 |
| F-REF-ERROR-TAXONOMY | reserved classes stay reserved | 10 | standing | `zero-ref` |
| ADV-0001 | planning artifact only | 10 | standing | docs |
| ADV-0002 | no second hash detector | 10 | standing | -- |
| SURF-0006 later | `dsr quality` targeted tests | 8.0 | later | out-of-repo DSR |
| PERF-0003 later | install cass | 7.5 | later | operator |
| CONF-0005 | digest mutation test | 6.0 | later | in-repo harness |
| SURF-0013 | quarantine + reap unit test | 6.0 | later | in-repo `zero-store` |
| CONF-0003 | accept fixture Exact (no emit) | 4.5 | later | in-repo |
| IDEA-0002 | pin tracked mandate | 4.5 | later | contract |
| IDEA-0017 | ratchet floor 0.940573 | 4.5 | later | `reports/` |
| F-MIRI-NARROW | miri green on rch | 4.5 | later | DSR/rch |
| CONF-0004 | fail-loud live suite | 4.0 | later | in-repo |
| PERF-0001 / OPEN-0015 | rch cv_pct n=10 | 3.0 | later | rch |
| CONF-0002 / IDEA-0024 | production spill probe | 2.67 | later | rch |
| CONF-0001 | live rollback verifier | 2.0 | later | in-repo, not fszero.rs |
| OPEN-0016 | rch comprehensive-bench | 2.0 | later | rch |
| PERF-0007 | profile-first only | 2.0 | later | rch + samply |
| IDEA-0010 | rename smoke test | 10.0 | later (blocked on pin) | harness |

## Rejected (<2.0 or policy)

| Gap | Rewrite | Why |
|---|---|---|
| SURF-0006 C | GH `cargo test` job | policy: hammers GH + host |
| PERF-0001 B | Invent `cv_pct` | dishonest keep |
| PERF-0003 C | Fabricate cass | dishonest |
| CONF-0003 B | Hub emits Exact | `[SPEC-HON-001]` |
| CONF-0004 B | Field-name grep | not a verifier |
| OPEN-0016 B | Invent 93-row Mac matrix | DEFERRED |
| PERF-0007 B | Edit CAS without profile | profile-first |
| ADV-0001 B | Re-include engine rows | paints gate |
| F-REF-ENGINE-ADOPTION A/B | Comment lockstep / hub ClampEnd | out-of-repo; comment is a lie |
| F-CONF-HARNESS A | Conformance CLI | CONTRACT §8 |
| F-ZSX-Q99 B | Fake worker accounting | dishonest |

---

## Already correct (do not re-open)

1. EngineIdentity Subject≠Oracle (`CLOSED-0001`)
2. Both-error is agreement (`CLOSED-0002`)
3. FailureBundle `/failure/first_divergence` (`CLOSED-0003`)
4. Crash oracle 5/5 (`CLOSED-0004`)
5. `zero-ref` parse/Display proptest (`CLOSED-0005`)
6. Bench-history ratchet exists and is honest (`CLOSED-0006`)
7. FeatureUniverse loader + dashboard; gate red is honest (`CLOSED-0007`, `SURF-0015`)
8. Three-tier goldens + integrity (`CLOSED-0008`)
9. Global `sum(weights)==1.0` waiver (`CLOSED-0009`)
10. Ledger retry lint (`CLOSED-0010`)
11. Test-profile microbench is not a keep (`CLOSED-0011`)
12. CONTRACT §8: no in-repo conformance CLI (`SURF-0005`)
13. Fat-LTO waiver (`PERF-0002`)
14. Engine-only surfaces excluded-as-debt (`SURF-0014`)
15. FromStr + serde, fuzz floor, ABI proptest, ensure_layout, negotiate (`SURF-0003/0001/0007/0009/0004`)
16. `F-CODEMODE-CANCEL` present after Phase-12 hub tests

---

## Verification (this pass)

```
feature-universe ok: features=77 present=67 partial=7 missing=0 excluded=3 weight_sum=1.000000000000
feature-coverage-dashboard ok: effective=0.952282 strict=0.940573 gate=red
golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0
rch cargo test -p zero-mcp: 2 passed (late Ok commit_race; late domain Err)
rch cargo test -p zsx-core empty_window_report_is_unavailable_never_numeric: 1 passed
dsr quality --tool zerostack --dry-run: No quality checks configured for zerostack
command -v cass: missing
```

Gate stays **red**. Partial never rounded up.
