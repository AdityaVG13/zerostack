# Bead Compliance Audit — ZeroStack important universe

**Date:** 2026-07-30  
**Pass:** `2026-07-30T04-19-21Z`  
**Skill:** beads-compliance-and-completion-verification  
**Cwd / DB:** `/Users/aditya/AI/ZeroStack` via `br`  
**Threshold:** 700  
**Remediation policy:** **report-only**  
**Branch:** `main` (no audit branch)  
**TokenZero source:** not modified  

---

## 1. Headline (skill threshold rule — not softened away)

Per skill rubric: **`status=closed` AND `score < 700` ⇒ non-compliant (false-closed)**.

| Metric | Count |
|---|---:|
| Scoped beads | 66 |
| Closed scored | 46 |
| **Non-compliant (false-closed)** | **26** |
| Compliant closed (score ≥ 700) | 20 |
| in_progress (live) | 7 |
| open | 13 |

**Phase 9:** every non-compliant closed bead is listed in `beads_compliance_audit/remediation.md` with remediation id `RO-NNN-<bead-id>`. No reopens and no completion-debt beads were written (policy=report-only). That is explicit remediation tracking, not a claim that zero beads failed the threshold.

**Retracted:** earlier draft language "true false-closed = 0 after calibration." Calibration may reclassify *cause* (close-only commits vs no evidence vs partial), but it does **not** zero the threshold count.

---

## 2. Scope

| Cluster | Roots | Notes |
|---|---|---|
| Snap / instant index | `zerostack-snap-instant-index-66f1` | Open coordinator epic |
| RACC / caching | `zerostack-racc-caching-output-vz89` + `vz89.*` | Epic open; most children closed |
| Reliability gates | `fszero-store-growth-61gb-veuq`, `graphzero-m3wx`, `graphzero-95o1` | In engine bead DBs |
| Harness / OSS | `59ra`, `4j6q` + children | 59ra open; 4j6q in_progress |
| Live work | all current `in_progress` | 7 beads (see §4) |
| Recent closes | inventory seed + concurrent closes | includes `f6qt`, `vz89.11` |

Full hub closed count ~228; this audit is **scoped**, not the entire 225+ universe.

---

## 3. Closed-bead evidence

### 3.1 Reliability gates (OBJECTIVE-named)

| Bead | Repo | Score | FC? | Commit evidence | Verdict |
|---|---|---:|---|---|---|
| `fszero-store-growth-61gb-veuq` | FSZero | 850 | no | `a43eae5` retention prune | **Compliant** |
| `graphzero-m3wx` | GraphZero | 890 | no | `3dc9ce4`, `c09cebc`, `d97b631`, `9f2f3b7`, `f674df4` | **Compliant** |
| `graphzero-95o1` | GraphZero | 770 | no | `3e5bb20` watcher/Linux | **Compliant** |

### 3.2 Snap cluster

| Bead | Score | FC? | Notes |
|---|---:|---|---|
| `66f1` epic | n/a (open) | — | DONE WHEN not met as hub e2e |
| `zerostack-7vxe` | 640 | **yes** | TokenZero `2c3d66a` product SHA present but score still <700 (`NON_COMPLIANT_PARTIAL`); rem id in remediation.md |

### 3.3 RACC / vz89 cluster (current)

| Bead | Status | Score | FC? |
|---|---|---:|---|
| `vz89` epic | open | n/a | — |
| `vz89.1`–`.3`, `.10` | closed | 645–685 | **yes (PARTIAL)** — product SHAs exist but score <700 |
| `vz89.4`–`.6`, `.8`–`.9` | closed | ≥765 | no |
| **`vz89.11`** | **closed** | **705** | **no** — TokenZero `a84c88a` verified read-only |
| `vz89.12` | open | n/a | remaining TokenZero P2 |

### 3.4 Harness / 59ra / 4j6q

| Bead | Status | Score | FC? | Notes |
|---|---|---:|---|---|
| `59ra` | open | n/a | — | packaging incomplete |
| `4j6q` | in_progress | n/a | — | ZeroStack-side work done; criteria 2–3 open |
| `m13q` | closed | 910 | no | `0d6c1cf` |
| **`f6qt`** | **closed** | **855** | **no** | ZeroStack `a3af0c9` + `node.rs` (closed mid-audit by concurrent session) |
| `inry` | closed | 790 | no | locate manifest |

### 3.5 Compliant closed (score ≥ 700)

| Score | ID | Product SHAs | Title |
|---:|---|---:|---|
| 910 | `zerostack-m13q` | 1 | Config discovery for embedded harnesses: env/XDG/well-k |
| 890 | `graphzero-m3wx` | 5 | GraphZero P0: make portable gz refs owner-expandable |
| 885 | `zerostack-jkfc` | 1 | ZeroStack host: spill oversized results before framing  |
| 885 | `zerostack-racc-caching-output-vz89.5` | 1 | FSZero: snapshot/overlay gap-check vs current CAS (depe |
| 880 | `zerostack-fv2q` | 1 | call-issues-0729: FSZero fixes (mutate materialization, |
| 870 | `zerostack-155n` | 1 | call-issues-0729: GraphZero fixes (durable aggregate re |
| 870 | `zerostack-racc-caching-output-vz89.4` | 2 | FSZero: candidate persistence + delta-only repair loop |
| 855 | `zerostack-f6qt` | 1 | Kill ephemeral node resolution: ship a stable node/runt |
| 850 | `fszero-store-growth-61gb-veuq` | 1 | Store growth: .zerostack/fszero hit 61GB in GraphZero r |
| 815 | `zerostack-wofb` | 1 | CodeMode plans with long or multi-delegate payloads fai |
| 790 | `zerostack-inry` | 0 | zerostack locate: single manifest command emitting ever |
| 780 | `zerostack-racc-caching-output-vz89.8` | 1 | GraphZero: output-side closure - enumerate mechanically |
| 770 | `graphzero-95o1` | 1 | Watcher-driven incremental index so snap is always curr |
| 765 | `zerostack-racc-caching-output-vz89.6` | 2 | FSZero: hermetic memoization of deterministic ops (form |
| 745 | `zerostack-racc-caching-output-vz89.9` | 1 | GraphZero: deterministic-facts-only contract guard |
| 745 | `zerostack-yoe.1` | 1 | zero-codemode hub foundation: Host/limits/wrap/connecto |
| 725 | `zerostack-bhn` | 0 | epic: raw_worker ABI approval carrier + typed EngineIde |
| 705 | `zerostack-ojh3` | 1 | call-issues-0729: TokenZero shell segment attribution ( |
| 705 | `zerostack-racc-caching-output-vz89.11` | 1 | TokenZero: output channel separation - action vs user_m |
| 700 | `zerostack-racc-caching-output-vz89.7` | 2 | GraphZero: fold min-dependency-set keys + anti-dependen |

### 3.6 Non-compliant closed (score < 700) — full list

**Count: 26.** Each has a report-only remediation id.

| ID | Score | Class | Product SHAs | Remediation id |
|---|---:|---|---|---|
| `zerostack-j7nn` | 255 | NON_COMPLIANT_CHORE_WEAK | — | `RO-001-zerostack-j7nn` |
| `zerostack-cwzy` | 325 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-002-zerostack-cwzy` |
| `zerostack-e7jc` | 325 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-003-zerostack-e7jc` |
| `zerostack-god3` | 325 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-004-zerostack-god3` |
| `zerostack-zddz.1` | 325 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-005-zerostack-zddz.1` |
| `zerostack-plkw` | 375 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-006-zerostack-plkw` |
| `zerostack-tilde-store-root-2r6` | 375 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-007-zerostack-tilde-store-root-2r6` |
| `zerostack-qef8` | 420 | NON_COMPLIANT_CHORE_WEAK | — | `RO-008-zerostack-qef8` |
| `zerostack-lq3i` | 435 | NON_COMPLIANT_NO_EVIDENCE | — | `RO-009-zerostack-lq3i` |
| `zerostack-8mkh` | 460 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-010-zerostack-8mkh` |
| `zerostack-r0cv` | 485 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-011-zerostack-r0cv` |
| `zerostack-dqv3` | 515 | NON_COMPLIANT_CLOSE_ONLY | — | `RO-012-zerostack-dqv3` |
| `zerostack-ejn1` | 610 | NON_COMPLIANT_PARTIAL | 2692450 | `RO-013-zerostack-ejn1` |
| `zerostack-b3l` | 615 | NON_COMPLIANT_PARTIAL | b369c99 | `RO-014-zerostack-b3l` |
| `zerostack-7vxe` | 640 | NON_COMPLIANT_PARTIAL | 2c3d66a | `RO-015-zerostack-7vxe` |
| `zerostack-codemode-aggregate-provenance-jmmj` | 640 | NON_COMPLIANT_PARTIAL | 133ba38 | `RO-016-zerostack-codemode-aggregate-provenance-jmmj` |
| `zerostack-racc-caching-output-vz89.3` | 640 | NON_COMPLIANT_PARTIAL | 58e4629a | `RO-017-zerostack-racc-caching-output-vz89.3` |
| `zerostack-2tk` | 645 | NON_COMPLIANT_PARTIAL | 4f2336c | `RO-018-zerostack-2tk` |
| `zerostack-conformance-refresh-ci-30a` | 645 | NON_COMPLIANT_PARTIAL | ce62db5 | `RO-019-zerostack-conformance-refresh-ci-30a` |
| `zerostack-racc-caching-output-vz89.10` | 645 | NON_COMPLIANT_PARTIAL | 2c3d66a | `RO-020-zerostack-racc-caching-output-vz89.10` |
| `zerostack-surface-shape-consistency-u28` | 645 | NON_COMPLIANT_PARTIAL | 5492b5d | `RO-021-zerostack-surface-shape-consistency-u28` |
| `zerostack-2ae` | 650 | NON_COMPLIANT_PARTIAL | 0d43ba5, ce62db5, c0bb48a | `RO-022-zerostack-2ae` |
| `zerostack-2ae.4` | 655 | NON_COMPLIANT_PARTIAL | 0d43ba5 | `RO-023-zerostack-2ae.4` |
| `zerostack-zddz` | 655 | NON_COMPLIANT_PARTIAL | — | `RO-024-zerostack-zddz` |
| `zerostack-racc-caching-output-vz89.2` | 680 | NON_COMPLIANT_PARTIAL | fa8d98a | `RO-025-zerostack-racc-caching-output-vz89.2` |
| `zerostack-racc-caching-output-vz89.1` | 685 | NON_COMPLIANT_PARTIAL | 8200d9fc | `RO-026-zerostack-racc-caching-output-vz89.1` |

#### Class breakdown
- **NON_COMPLIANT_NO_EVIDENCE (1)** — e.g. `zerostack-lq3i` (evidence claimed in pi-stack outside ZS/FS/GZ trees)
- **NON_COMPLIANT_CLOSE_ONLY (9)** — e.g. `cwzy`, `god3`, `e7jc`, `zddz.1`, `plkw` — only `close(bead)` commits in hub log
- **NON_COMPLIANT_PARTIAL (14)** — e.g. `2ae.4` (product SHA `0d43ba5` + conformance paths but score 655), `7vxe`, `vz89.1`–`.3`, `.10`
- **NON_COMPLIANT_CHORE_WEAK (2)** — e.g. `j7nn`

Example called out by skeptic: **`zerostack-2ae.4`** score **655**, FC **True**, product SHA **`0d43ba5`** (unify conformance reports), close-only commit `ac18083` is lifecycle; remediation id **`RO-*-zerostack-2ae.4`** in remediation.md.

**`zerostack-yoe.1`:** re-scored with hub product SHA **`7604e54`** + host/limits/wrap paths → score **745**, FC **False**, COMPLIANT.

---

## 4. In-progress accuracy (live `br list --status in_progress`)

| ID | Assignee | Title |
|---|---|---|
| `zerostack-1f3t` | — | call-issues-0729: pi-zerostack host fixes (multi-shell results, frame  |
| `zerostack-be6q` | — | zero.token.shell with opts (timeoutMs) fails: aggregate worker call is |
| `zerostack-iof5` | — | proof-run F1: zero.token.shell result lacks .stdout; silent empty stri |
| `zerostack-jp6i` | — | Shared INSTALL-FOR-AGENTS doc: one file, mirrored to all repos, that l |
| `zerostack-no-hardcoded-environment-audit-4j6q` | — | Audit: no hardcoded paths/hosts/users anywhere — everything discoverab |
| `zerostack-scsz` | — | Parallel worker spawns serialize on pi-rewind checkpoint lock (60s sta |
| `zerostack-xrrp` | — | codemode substrate spawns delegate binaries from target/release which  |

**All 7 statuses are accurate** (none should be closed without new SHAs).  
**Process hygiene:** none have assignees. Empty notes on `be6q`/`iof5`/`jp6i`/`scsz` were filled via `br update --notes` + `br comments add`.

**Not in_progress anymore (corrected vs earlier draft):**
- `zerostack-f6qt` → closed with `a3af0c9`
- `zerostack-racc-caching-output-vz89.11` → closed with TokenZero `a84c88a`

---

## 5. Dependency consistency

- `vz89` → related → `66f1` (ok)
- `4j6q` → parent-child → `59ra` (ok)
- `f6qt` → parent-child → `4j6q` (ok)
- Engine gates live in engine DBs (not hub inventory) — cross-repo visibility gap, not a broken hub edge
- OBJECTIVE alias `fszero-store-growth-veuq` → actual id **`fszero-store-growth-61gb-veuq`**
- `br doctor`: WARN-level only; **no `status:fail`**

---

## 6. Remediations applied

1. Bootstrap `beads_compliance_audit/` (gitignored nested audit).
2. Per-bead evidence + scorecards for scoped set.
3. Re-score with product SHAs (incl. `7604e54` yoe.1, m3wx multi-SHA, `0d43ba5` 2ae.4).
4. **Phase 9 report-only:** 26 rows in `remediation.md` (`RO-001`…`RO-026`).
5. `br` metadata hygiene on in_progress notes/comments.
6. No TokenZero source edits; no project audit branch.

---

## 7. Residual risks

1. **26 non-compliant closed beads** remain closed in the graph under report-only policy — next implementation session should pick T0/T1 reopen or completion-debt for `NO_EVIDENCE` and `CLOSE_ONLY` classes.
2. Snap epic DONE WHEN still unmet as single-call e2e.
3. Live bugs: `be6q`, `iof5`, `xrrp`, `scsz`.
4. Phase 4 = commit/file substitute (RULE 4); cargo tests not re-run here.
5. Formal two-pass convergence (Axiom 8) not claimed — this pass is the scoped baseline with checklist complete for serial substitute.

---

## 8. Full closed scoreboard

| Score | Band | FC | ID | Title |
|---:|---|---|---|---|
| 910 | substantially_complete | False | `zerostack-m13q` | Config discovery for embedded harnesses: env/XDG/well-k |
| 890 | substantially_complete | False | `graphzero-m3wx` | GraphZero P0: make portable gz refs owner-expandable |
| 885 | substantially_complete | False | `zerostack-jkfc` | ZeroStack host: spill oversized results before framing  |
| 885 | substantially_complete | False | `zerostack-racc-caching-output-vz89.5` | FSZero: snapshot/overlay gap-check vs current CAS (depe |
| 880 | substantially_complete | False | `zerostack-fv2q` | call-issues-0729: FSZero fixes (mutate materialization, |
| 870 | substantially_complete | False | `zerostack-155n` | call-issues-0729: GraphZero fixes (durable aggregate re |
| 870 | substantially_complete | False | `zerostack-racc-caching-output-vz89.4` | FSZero: candidate persistence + delta-only repair loop |
| 855 | substantially_complete | False | `zerostack-f6qt` | Kill ephemeral node resolution: ship a stable node/runt |
| 850 | substantially_complete | False | `fszero-store-growth-61gb-veuq` | Store growth: .zerostack/fszero hit 61GB in GraphZero r |
| 815 | partial_ok | False | `zerostack-wofb` | CodeMode plans with long or multi-delegate payloads fai |
| 790 | partial_ok | False | `zerostack-inry` | zerostack locate: single manifest command emitting ever |
| 780 | partial_ok | False | `zerostack-racc-caching-output-vz89.8` | GraphZero: output-side closure - enumerate mechanically |
| 770 | partial_ok | False | `graphzero-95o1` | Watcher-driven incremental index so snap is always curr |
| 765 | partial_ok | False | `zerostack-racc-caching-output-vz89.6` | FSZero: hermetic memoization of deterministic ops (form |
| 745 | partial_ok | False | `zerostack-racc-caching-output-vz89.9` | GraphZero: deterministic-facts-only contract guard |
| 745 | partial_ok | False | `zerostack-yoe.1` | zero-codemode hub foundation: Host/limits/wrap/connecto |
| 725 | partial_ok | False | `zerostack-bhn` | epic: raw_worker ABI approval carrier + typed EngineIde |
| 705 | partial_ok | False | `zerostack-ojh3` | call-issues-0729: TokenZero shell segment attribution ( |
| 705 | partial_ok | False | `zerostack-racc-caching-output-vz89.11` | TokenZero: output channel separation - action vs user_m |
| 700 | partial_ok | False | `zerostack-racc-caching-output-vz89.7` | GraphZero: fold min-dependency-set keys + anti-dependen |
| 685 | false_closed_mild | True | `zerostack-racc-caching-output-vz89.1` | Zero Edit Protocol v1: universal ref-based edit verbs s |
| 680 | false_closed_mild | True | `zerostack-racc-caching-output-vz89.2` | Fresh-work accounting vector + eta_action metric in zer |
| 655 | false_closed_mild | True | `zerostack-2ae.4` | refactor(conformance): unify report schema and gate id  |
| 655 | false_closed_mild | True | `zerostack-zddz` | EPIC: Harden OS-watch FFI surface (inotify/kqueue/Find* |
| 650 | false_closed_mild | True | `zerostack-2ae` | EPIC: Conformance evidence integrity and false-green MC |
| 645 | false_closed_mild | True | `zerostack-2tk` | conditional wave 3: zero-mcp compatibility core only wh |
| 645 | false_closed_mild | True | `zerostack-conformance-refresh-ci-30a` | conformance public evidence gap: gitignored reports, mi |
| 645 | false_closed_mild | True | `zerostack-racc-caching-output-vz89.10` | TokenZero: session exposure ledger - never re-expose ev |
| 645 | false_closed_mild | True | `zerostack-surface-shape-consistency-u28` | Cross-surface result shape is inconsistent: zero.token. |
| 640 | false_closed_mild | True | `zerostack-7vxe` | zero_execute compact refs not expandable via tokenzero  |
| 640 | false_closed_mild | True | `zerostack-codemode-aggregate-provenance-jmmj` | aggregate codemode host rejects multi-action v2 plans:  |
| 640 | false_closed_mild | True | `zerostack-racc-caching-output-vz89.3` | Canonical cache-entry schema as shared ABI type (comple |
| 615 | false_closed_mild | True | `zerostack-b3l` | wave 4: capability negotiation + cross-engine telemetry |
| 610 | false_closed_mild | True | `zerostack-ejn1` | ci: Windows host exact tests for machine-permit wake+pi |
| 515 | false_closed_mild | True | `zerostack-dqv3` | Inventory layout divergence across FSZero/TokenZero/Gra |
| 485 | false_closed_severe | True | `zerostack-r0cv` | Preferred-band: drop validate_capability_manifest and p |
| 460 | false_closed_severe | True | `zerostack-8mkh` | EPIC: Refactor kevent zeroed/null satellites (C) |
| 435 | false_closed_severe | True | `zerostack-lq3i` | pi-zerostack: add positive multi-action provenance test |
| 420 | false_closed_severe | True | `zerostack-qef8` | clippy: allow or box large WorkerRequestFrame::Call var |
| 375 | false_closed_severe | True | `zerostack-plkw` | test: exact host cover for NativeWake Drop after MaybeU |
| 375 | false_closed_severe | True | `zerostack-tilde-store-root-2r6` | Literal '~' directories in tokenzero/ and graphzero/ ho |
| 325 | false_closed_severe | True | `zerostack-cwzy` | EPIC: NativeWake Send/Sync fence + Windows dynamic cove |
| 325 | false_closed_severe | True | `zerostack-e7jc` | test: compile-time assertion NativeWake stays private / |
| 325 | false_closed_severe | True | `zerostack-god3` | EPIC: UB-exorcism residual hygiene (kevent/MaybeUninit  |
| 325 | false_closed_severe | True | `zerostack-zddz.1` | task: fix inventory ffi flags for Win32 sites |
| 255 | false_closed_severe | True | `zerostack-j7nn` | AR: hub bead sweep 2026-07-29 — residual notes |

---

## 9. Artifact index

| Path | Role |
|---|---|
| `reports/bead-compliance-audit.md` | This deliverable |
| `beads_compliance_audit/REPORT.md` | Master report mirror |
| `beads_compliance_audit/remediation.md` | Phase 9 RO rows |
| `beads_compliance_audit/synthesis.md` | Cross-bead |
| `beads_compliance_audit/passes/2026-07-30T04-19-21Z/beads/<id>/scorecard.md` | Per-bead |
| `beads_compliance_audit/passes/.../calibration.json` | Threshold counts |

---

## 10. Checklist (scoped run)

- [x] Pre-flight doctor (no fail), stats, inventory  
- [x] Audit dir bootstrap + gitignore  
- [x] Spec/evidence/score packs  
- [x] **Calibration reconciled with threshold count** (FC=26, not zero)  
- [x] Phase 9 report-only rows for **all** closed∧score<700  
- [x] In-progress metadata via `br`  
- [x] Sections 3–4 match live `br` (f6qt/vz89.11 closed)  
- [x] Keystone SHAs on scorecards (m3wx, yoe.1, veuq, 95o1)  
- [x] No TokenZero source edits; no audit branch  
- [x] `br sync`  
- [x] Manifest phase_status completed for serial substitute phases  

**Status: DONE_WITH_CONCERNS** — threshold non-compliance is fully listed; residual is report-only (no auto-reopen) and no formal second convergence pass.
