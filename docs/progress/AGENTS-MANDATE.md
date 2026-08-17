# AGENTS.md Negative-Evidence Mandate (tracked pointer)

`AGENTS.md` is gitignored (agent law; live on disk, never remote). The
operating copy on this machine must still carry the mandate block below.
This file is the **tracked** copy so a fresh clone can see the discipline
without opening the gitignored file.

In-repo ledgers (this tree):

- `docs/progress/perf-negative-results.md`
- `docs/progress/conformance-negative-results.md`
- `docs/progress/surface-deferrals.md`

Workspace ledgers (sibling gauntlet working memory, not this git tree):

- `ZeroStack__gauntlet_workspace/docs/progress/perf-negative-results.md`
- `ZeroStack__gauntlet_workspace/docs/progress/conformance-negative-results.md`
- `ZeroStack__gauntlet_workspace/docs/progress/surface-deferrals.md`

Lint: `python3 scripts/check_ledger_retry.py`

---

## Negative-Evidence Discipline

This project maintains three durable negative-evidence ledgers in
`docs/progress/`:

- `perf-negative-results.md` -- performance ideas that were measured and rejected.
- `conformance-negative-results.md` -- conformance hypotheses that were tested and refuted.
- `surface-deferrals.md` -- surface features explicitly Excluded with rationale and retry-condition predicate.

> **Verbatim from the gauntlet methodology (CC.md lines 479–482):** "This ledger records performance ideas that were measured and rejected. Check it before starting a new optimization pass, and add an entry whenever a candidate is abandoned, reverted, or kept out of the tree because the benchmark matrix did not move in the intended direction."

Before any agent starts a perf-affecting change, a conformance-affecting change, or a surface-affecting change, the agent MUST:

1. **Grep the relevant ledger** for the proposed hotspot, behavior, or feature. If the ledger already names this candidate, READ the rejection rationale + the load-bearing **retry-condition predicate** before proceeding. If the current evidence does not satisfy the predicate, do not proceed.

2. **Mine 60 days of `cass` session history** for the failure terms:
   - **Universal terms:** `rejected`, `reverted`, `abandoned`, `slower`, `regressed`, `didn't help`, `within noise`, `no improvement`, `failed to improve`, `rolled back`, `backed out`, `not a keep`, `keep gate`.
   - **Project-class-specific terms:** `mcp-late-ok-salvage`, `raw-worker-planner-creep`, `daemon-install`, `host-path-leak`, `savingsbytes-as-tokens`, `engine-import-cycle`, `cargo-target-dir-suffix`, `rival-dirty-tree`, `clamp-end-vs-reject`, `commit-race-mislabel`, `estimate-labeled-exact`, `fszero-fail-closed`.

   ```bash
   for term in rejected reverted abandoned slower regressed "within noise" "keep gate"; do
     timeout 30s cass search "$term" --robot --days 60 --limit 50 --mode lexical --timeout 30000 \
       | jq '.matches[]? // .hits[]? // .results[]? | {file, line, snippet}'
   done
   ```

   If `cass` is not on PATH, record a blocker entry and mine `git log --since='60 days ago'` plus the gauntlet workspace instead.

3. **Check recent commits** (`git log --since='60 days ago' --grep -iE 'perf|optimiz|hot.path|bench|ratchet'`) for prior closure on this candidate.

4. **If `cass` is unavailable or the ledger is reserved** (per MCP Agent Mail reservations), the agent MUST record a *blocker* entry in the ledger ("Cannot proceed -- cass unavailable; recheck before next attempt") rather than silently skipping the step.

> **Verbatim from CODEX.md §10.2 lines 1464–1472:** "For major perf campaigns, agents must also mine: last 60 days of CASS session history, recent commits, perf artifacts, failed/rejected/slower/regressed terms. If CASS or the ledger is unavailable or reserved, the agent must record a blocker or patch-ready entry rather than silently skipping the step."

When closing or rejecting a candidate, the ledger entry MUST include the load-bearing **retry-condition predicate** -- never a non-predicate deferral. The predicate is a concrete, falsifiable condition under which the candidate becomes worth reconsidering. The 8 acceptable predicate forms are documented in the running-the-gauntlet-on-your-rust-port skill (`references/methodology/RETRY-CONDITION-VOCABULARY.md`).

Examples of load-bearing predicates:

- "Retry only if a profiler attributes a clearly-above-noise share to `<specific counter>` on `<wider workload shape>`."
- "Reconsider only inside the broader `<X>` redesign (track as `<beads_id>`)."
- "Worth reconsidering when `<specific gate>` crosses `<threshold>`."
- "Not worth retrying as a standalone patch."
- "Do not retry from a cold read; use comprehensive-bench attribution instead."

A ledger entry without one of these load-bearing predicates fails `scripts/check_ledger_retry.py` and blocks the parent bead from closing.

## Ledger-grep before perf work

Before opening any performance-related bead or starting any optimization pass, run `python3 scripts/check_ledger_retry.py` and grep the three ledgers for the hotspot. If cass is on PATH, also run the workspace helpers `scripts/mine-ledger.sh` and `scripts/mine-cass-cross-machine.sh`. The first checks the three negative-evidence ledgers for prior rejected attempts on the same hotspot. The second mines 60 days of cass session history across local + css + csd + ts1 + ts2 for failure terms (`rejected, reverted, abandoned, slower, regressed, didn't help, within noise, no improvement, failed to improve, rolled back, backed out, not a keep, keep gate`). Skipping these checks is the most common form of rejection-by-omission. If the candidate hotspot appears in a prior negative-ledger entry, you MUST cite the entry and state how your retry-condition predicate is satisfied before proceeding.
