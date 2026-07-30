# AGENTS.md — ZeroStack (hub)

> Operating contract for AI coding agents in this repository. Read completely before your first edit.

## RULE 0 — OPERATOR OVERRIDE
If the operator (Aditya) tells you to do something, even against this file, you listen. He is in charge, not you.

## RULE 1 — NO FILE DELETION
Never delete a file or directory without express written permission — even files you created. Ask first, every time.

## RULE 2 — GIT DISCIPLINE
- Branch: ONLY `main`. No feature branches, no worktrees, no `master`.
- NEVER: force-push, rebase published history, `reset --hard` shared state, `clean -fd`, amend pushed commits.
- Commit subjects: generic, conventional (`fix:`, `feat:`, `test:`, `chore:`). NEVER put bead ids, agent names, or session ids in commit messages.
- Pull before you start. Push when a logical unit is done and verified.

## RULE 3 — ONE WRITER PER REPO
Multiple agents work this codebase concurrently. Before editing:
1. `git status --porcelain` — if the tree is dirty with paths you did not touch, STOP. Another agent is mid-flight. Do not commit over them, do not stash their work. Back out and report.
2. Check `br show <bead>` — if assignee is not you and status is in_progress, it is NOT yours.
Watched mtimes advancing on files you did not edit = live rival writer. Halt.

## RULE 4 — TEST POLICY (HARD)
- NEVER run full `cargo test` / `cargo build` for the whole workspace on this machine.
- Targeted tests only, and only when the change genuinely needs them (most one-line changes do not).
- All compilation/tests go through RCH (remote compilation helper, DGX Spark):
  `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_<reposlug> cargo test -p <crate> <filter> -- --test-threads=1`
- A pre-existing failure is not yours to absorb: verify via `git stash` baseline, then file a bead for it.

## RULE 5 — BEADS ARE THE MEMORY
- `br show <id>` before starting; read description, notes, AND acceptance criteria — acceptance is the contract.
- Close only with evidence (commit sha + verification output). Never close unverified.
- Any defect you find en route — even trivial — gets its own bead with reproduction evidence. Never fix-and-forget silently, never leave it untracked.
- Blocked? Set status blocked with a note naming the exact blocker. Someone sweeps blocked beads to reopen them.

## RULE 6 — TOKEN EFFICIENCY (RACC)
This project's reason to exist is minimizing agent round-trips. Practice it:
- Refs first: results return durable, expandable refs; read them, expand only when needed, FIRST try.
- One-call discovery: search/query hits carry snap-to-file targets — `HIT <path>#L<start>-L<end> kind=<k> sym=<enclosing>` with content inlined (sub-4KB never preview-only). Grammar: FSZero docs/design/target-ref-grammar.md. Do not invent a second grammar.
- Batch shell work into single calls; write reports to files, keep chat output tight.
- If the substrate itself wastes your round-trips (missing ref, preview-only small result, silent undefined, exit-0 error JSON), that is a BUG: file a bead in the owning repo.

## THE RACC CONTRACT (what every change must preserve)
1. Honesty: billed/visible token accounting is a receipt, never an estimate presented as fact.
2. Determinism: same op => same bytes across every surface/adapter (CLI, MCP, CodeMode, raw-worker).
3. Durability: a ref handed to an agent survives process restart and expands from any session.
4. Loud failure: errors are typed and expandable; never silent undefined, never exit 0 with error JSON on stdout.
5. Certificates over vibes: lossy/compressed output must carry certification; uncertified lossy presents as expandable, never as a committed result.

## THIS REPO — ZeroStack (the hub)
ZeroStack is the shared trust authority and contract hub for the three engines (FSZero, GraphZero, TokenZero). Engines pin hub crates by git rev against https://github.com/AdityaVG13/zerostack.git — a hub commit is only usable by engines once PUSHED to public origin/main. If you land contract changes, push, or engine adoption is impossible.

### Crate atlas (crates/)
- zero-abi — operation ABI, contract digests, raw-worker v2 wire rules + frame codec, typed EngineIdentity, approval grants. Changing ABI semantics REQUIRES bumping the manifest digest; engines pin it.
- zero-ref — ZeroRef v1: (fz|gz|tz)://blob/<sha256> + strict #B fragment algebra (never clamps). Live-file path#B ranges clamp and are NOT ZeroRefs.
- zero-store — extracted CAS/store; NOT yet consumed by any engine (bead ee2). Do not assume adoption.
- zero-ledger / zero-gate — RACC accounting core + decision gates (passthrough/compact/lossy). Integer arithmetic only, no floats, no double-tokenization.
- zero-codemode — shared QuickJS host foundation (rev 7604e54): deadline/fuel/memory/stack bounds, null-prototype registration, JSON-string connector ABI.
- zero-testkit — shared engine-parameterized behavioral suites (packaging, README claims, durability matrix).

### Key areas
- conformance/ — cross-engine conformance harness + CONTRACT.md (evidence model: local-only, Option B). Feeds RACC gates (RACC-RECEIPT, RACC-BUDGET, RACC-INLINE, RACC-CERT, T13).
- docs/adr/ — ADRs govern execution boundary and dispatch authorization. Read before touching codemode/raw-worker surfaces.
- Raw-worker v2 invariant: a raw worker contains NO planner, NO JS runtime, NO MCP catalog, NO nested CodeMode. Unknown fields are versioned; unsafe skew fails closed.

### Cross-repo rules
- The aggregate pi-zerostack is the SOLE JS host over planner-free raw workers. Do not add per-engine JS hosts.
- Store-root resolution is a known defect area (literal '~' directories, project-key divergence). Never construct store paths with unexpanded '~'; never invent a new store layout — see beads sce/ljx/ee2/2r6.
- TokenZero repo may be owned by another session. Check before editing; prefer hub-side beads with links.
