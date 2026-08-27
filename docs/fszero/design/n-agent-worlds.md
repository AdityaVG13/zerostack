# N-Agent Worlds: Concurrent Speculative Edit Worlds Over a Shared Base

Status: design (Northstar section 2 -- "Worlds: N-agent parallel editing, git for
agent edits at ms granularity"). Describes the current single-track
implementation, the target N-world model, and the delta between them.

## 0. Scope

This doc answers: how do N concurrent worlds share a base snapshot, how does
each world track its journal independently, what does base advancement mean
while worlds are open, and what is the fork/edit/preview/commit/drop
lifecycle under concurrency. It defines invariants the implementation must
hold once N > 1 worlds can be open at once, and lists the sibling beads that
carry out the delta.

---

## 1. Current state (single-track worlds, as implemented today)

Grounded in `src/core/world.rs`, `src/core/subsystems.rs`,
`src/core/session.rs`, `src/core/access_world_ops.rs`.

### 1.1 Data model

`WorldRegistry` (`src/core/subsystems.rs:41-59`) is one `HashMap<String,
WorldEdit>` (`active`) plus a `next_id: u32` counter, owned directly by
`FSZeroSession::worlds` (`src/core/session.rs:81`). There is exactly one
registry per session process -- no separate per-agent namespace, no
generation counter on the underlying tree.

`WorldEdit` (`src/core/world.rs:6-10`) is:

```rust
struct WorldEdit {
    edits: Vec<WorldFileEdit>,
    cert_ref: String,
}
struct WorldFileEdit {
    path: PathBuf,
    pre: String,   // preimage captured at fork time
    post: String,  // postimage computed at fork time
    cert_ref: String,
}
```

A world is a **precomputed diff**, not a live overlay: `pre`/`post` for every
touched file are computed once, in memory, when the world is created
(`prepare_world_file_edit`, `src/core/world.rs:93-110`), by reading the real
file off disk (`fs::read_to_string`) and applying the edit spec
(`apply_unique_replace`). Both strings are also persisted content-addressed
via `self.recovery.put_content_ref` so they survive as `fz://` refs even if
the in-memory `WorldEdit` is dropped later.

### 1.2 Lifecycle today

- **Fork+edit (`new:` / `newbatch:`)** -- `do_world` (`src/core/world.rs:236-259`)
  dispatches to `create_world_from_edit` / `create_world_from_batch` ->
  `create_world_from_edits` (`src/core/world.rs:66-91`). This is a combined
  fork-and-stage: there is no separate "fork with no edits yet" state. Each
  edit spec is resolved against the *current real tree* (not a frozen base
  ref), so `pre` is whatever is on disk at the moment of world creation. A
  new world id `W<n>` is allocated from `next_id` and the `WorldEdit` is
  inserted into `worlds.active`. A world-scoped manifest key
  (`world_{wid}/manifest`) is written to the recovery store.
- **Preview** -- not implemented as a distinct op. The only way to inspect a
  pending world today is `verify_cert` (`src/core/world.rs:21-52`), which
  re-expands the cert's `pre`/`post` refs and checks their hashes match the
  stored content. There is no diff-rendering or read-through-world API yet
  (tracked as fszero-otm, fszero-1wm).
- **Commit (`commit:<wid>[:git]`)** -- `commit_world`
  (`src/core/world.rs:112-200`):
  1. Removes the world from `active` (so a world cannot be committed twice
     concurrently within one process -- the `HashMap::remove` is the
     concurrency boundary).
  2. **Preimage guard**: re-reads every touched file off disk and compares
     against the captured `pre`. Any mismatch fails the whole commit with
     `"world preimage changed"` and puts the (unmodified) world back into
     `active` so the caller can retry or drop it. This is the only
     conflict-detection FSZero has today, and it is coarse: whole-file
     equality, not hunk-level, and only checked at commit time, not live.
  3. Path-guards every target under `self.root` again (`ensure_path_under_root`).
  4. Captures pre-mtime/mode/xattrs per file (`pre_meta`) so `fs.undo` can
     restore them exactly (fszero-md6/7be/l4g).
  5. Writes `post` to each file with `fs::write`. On a write failure
     mid-batch, it rolls back every already-applied file to its `pre` and
     re-inserts the world into `active` (best-effort; rollback failures are
     reported but not retried).
  6. On success: invalidates the ls/content caches for each path, reindexes
     the path, and calls `record_mutation("world", ...)` per file
     (`src/core/world.rs:187-196`) so the commit shows up in the *same*
     `mutation_log` table (`src/core/recovery.rs:1176-1218`) as ordinary
     `fs.edit`/`fs.write` mutations -- `fs.history` / `fs.undo` do not
     distinguish a world-commit from a direct edit.
  7. Optional `:git` suffix additionally runs `git_commit_world`
     (`src/core/world.rs:206-234`): `git add -- <paths>` then
     `git commit -m "fszero: world <wid> commit (...)" -- <paths>`, pathspec
     scoped so it never touches other staged work. This step is fail-open:
     a git failure is reported in the response string but does not roll
     back the filesystem write from step 5.
- **Drop (`drop:<wid>`)** -- `worlds.active.remove(wid)`
  (`src/core/world.rs:285-289`). No file was ever touched, so this is a pure
  in-memory deallocation; nothing to undo.

### 1.3 What's absent today (the gap this doc's later sections address)

- No frozen "base" concept distinct from "whatever is on disk right now" --
  a world's preimage is read live at fork time, and the preimage guard at
  commit time is the *only* place staleness is detected.
- No copy-on-write journal per world; edits are materialized as full
  pre/post string pairs in memory, one entry per file, not an append-only
  op log.
- No live conflict detection; two worlds can both stage edits to the same
  file and neither knows about the other until one commits and the other's
  next commit attempt fails the preimage guard.
- No three-way merge; a preimage mismatch is a hard failure, not a merge
  attempt. The agent must drop and refork.
- `WorldRegistry` has one flat namespace; nothing associates a world with
  "the agent that owns it" beyond the `FSZERO_AGENT_ID` env var stamped onto
  `mutation_log.agent` at commit time (`record_mutation`,
  `src/core/fs_ops.rs:300-333`).
- No fork/commit latency budget is enforced structurally; fork cost today is
  O(files touched) reads, not O(1).

Everything below is design for closing this gap under true N-agent
concurrency, cross-referencing the sibling beads that implement each piece.

---

## 2. Shared base snapshot model

**Base** = the content-addressed state of the tracked tree at a specific
point, expressed as the recovery store's `fz://blob/<sha256>` refs for every
tracked file plus the on-disk tree those refs were read from. FSZero already
has the content-addressing primitive (`RecoveryStore::put_content_ref`,
`src/core/recovery.rs:525-533`, and `expand`, `src/core/recovery.rs:832`);
what's missing is a first-class "this is generation G's tree manifest" ref
that N worlds can fork from concurrently instead of each computing its own
ad hoc preimage from a live `fs::read_to_string`.

- **Base generation counter.** A single monotonically increasing `u64`
  (`base_generation`) lives on the session, analogous to the existing
  `version: u64` field already on `FSZeroSession` (`src/core/session.rs:93`,
  currently used for AST/index staleness). It increments exactly once per
  event that changes the real tree outside of the process's own knowledge of
  it having already accounted for the change: a world commit, or a detected
  external write (mtime/hash drift on a tracked path). It does **not**
  increment on world fork, edit staging, preview, or drop -- those are pure
  reads/no-ops against base.
- **One base per generation, N worlds per base.** All worlds forked while
  `base_generation == G` share the same base manifest. The manifest is
  conceptually `{ generation: G, files: { rel_path -> content_ref } }` for
  the tracked subtree, materialized lazily: FSZero does not eagerly hash the
  whole tree on every generation bump (that would defeat the sub-10ms fork
  goal, fszero-ap9); instead each world's fork step reads-and-hashes only
  the files *that world touches*, and the base generation number is the
  cheap, shared handle. This mirrors how `commit_world` already works file-
  by-file rather than tree-wide.
- **Copy-on-write journal per world.** A world does not copy the tree. It
  holds `(base_generation, Vec<JournalEntry>)` where each entry is
  computed against base content read on first touch of that path within the
  world (copy-on-write at the *entry* granularity, not the *tree*
  granularity). This is a direct generalization of today's
  `WorldFileEdit { pre, post, cert_ref }` (`src/core/world.rs:12-18`) --
  same shape, but tagged with the generation it was read against instead of
  assuming "pre is always current HEAD".

---

## 3. Per-world journal independence

- **Format.** One journal per world id, append-only, entries in edit order.
  Each entry captures: `path`, `pre_ref` (content ref of the base-generation
  content the edit assumes), `post_ref` (content ref of the edit's result),
  `cert_ref` (existing edit-cert mechanism, `store_edit_cert`, referenced at
  `src/core/world.rs:103`), and the `base_generation` the entry was created
  against. This is the `WorldFileEdit` struct today, minus the in-memory
  `pre`/`post` `String`s (which become on-demand `recovery.expand(pre_ref)`
  lookups instead of held strings -- keeps a world's memory footprint
  bounded by edit count, not edit size, which matters once N worlds are
  live simultaneously).
- **Why worlds never see each other's uncommitted entries.** Each world's
  journal is keyed by its own `wid` in `WorldRegistry.active` (already true
  today, `src/core/subsystems.rs:41-44`) and no read path in the current
  codebase ever iterates `active` values across ids to answer a read for a
  *different* world -- `fs.read`/`fs.ls`/`fs.search` all read the real
  on-disk tree or the shared index, never another world's `WorldEdit`. That
  isolation is structural (separate hash-map entries, no cross-referencing
  code path) rather than enforced by a lock, and it is preserved by
  construction as long as every future "read through a world" API
  (fszero-1wm) resolves strictly `journal[wid] -> else base`, never
  `journal[other_wid]`.
- **What a world's journal is *not***: it is not a branch of the
  `mutation_log` table. `mutation_log` (`src/core/recovery.rs:1176-1218`) is
  the durable, cross-agent-visible record of things that actually happened
  to the real tree -- a world's journal entries are only promoted into
  `mutation_log` at commit time (`record_mutation("world", ...)`,
  `src/core/world.rs:187-196`). This is deliberate: `fs.history`/`fs.undo`
  must never expose a live agent's in-flight, uncommitted edits to another
  agent inspecting history (this is Invariant I1 below, applied to the
  journal-vs-mutation_log boundary specifically).

---

## 4. Base advancement while worlds are open

Base moves are **explicit**, never implicit. Concretely:

- An open world's `base_generation` is fixed at fork time and never mutated
  in place. If the real tree advances (another world commits, or an
  external process writes a tracked file), the session's
  `base_generation` counter increments, but every already-open world keeps
  the generation number it forked from. The world's `pre_ref`s continue to
  refer to the content at *its* fork point, not the new tree state.
- **How an open world observes advancement**: only two ways, both explicit.
  1. **Commit-time three-way merge** (fszero-glg): when the world commits,
     the commit path compares `base-at-fork` (this world's `pre_ref`s),
     `world-edits` (this world's `post_ref`s), and `base-now` (current
     content on disk / current generation's refs) for every touched path.
     If `base-at-fork == base-now` for a path, it degenerates to today's
     preimage-guard-passes case: apply `post` directly (Invariant I3). If
     they differ, attempt a structural merge; on merge failure, return a
     structured conflict report and do **not** write anything (never a
     silent clobber -- extends today's "world preimage changed" hard-fail
     into "diagnosed conflict with a report", rather than removing the
     hard-fail).
  2. **Explicit rebase operation** (new op, not yet named/beaded beyond this
     doc): an agent can ask its open world to re-baseline against the
     current generation before committing, e.g. to pick up an unrelated
     collaborator's already-committed change to a *different* file in the
     same world's touch set. Rebase re-reads `pre` for each touched path
     against `base-now`, re-derives `pre_ref`, and bumps the world's
     recorded `base_generation` -- but only when the agent asks for it. A
     world that never rebases and never hits a conflicting commit behaves
     exactly like today's single-track world.
  3. There is deliberately **no third way**: a world does not implicitly
     rebase on every read, and a world's `fs.read` through the world overlay
     (fszero-1wm) never silently blends in another world's or the
     tree's newer state without one of the two paths above.
- **Two simultaneous commits.** Commits serialize on the recovery store's
  existing single-writer discipline: `RecoveryStore` mutation methods
  already assume single-threaded access
  (begin/end batch txn helpers at `src/core/recovery.rs:1064-1145`; parallel
  CodeMode workers no longer share the store — `fs.search` stays on the main
  session thread per fszero-tucs / R-003). Under
  N concurrent worlds, `commit_world` must serialize mutations for the
  duration of its preimage-check + write + journal-append sequence (it
  already does this implicitly today for one world at a time; N-world
  concurrency requires an explicit lock to gate cross-world commit
  interleaving, not just cross-thread store access). The second committer
  in a race sees the generation the first committer just produced as
  `base-now` and goes through the three-way merge path from 4.1 -- it does
  not get an inconsistent read.

---

## 5. Lifecycle under concurrency

State machine per world (each world independently in one of these states;
N worlds occupy independent instances of this machine over a shared base):

```
            fork
  (none) ----------> Open (editable)
                        |  ^
                 edit   |  | edit (more)
                        v  |
                      Open (staged)
                     /            \
              preview              commit
             (read-only,           |
              no state           three-way merge
              transition)         vs base-now
                                    |
                        +-----------+-----------+
                        |                       |
                    no conflict             conflict
                        |                       |
                    Committed              Conflict-reported
                (journal -> mutation_log,   (world stays Open;
                 base_generation advances)   agent resolves via
                        |                     fszero-e8s, then
                        v                     re-commits or drops)
                    (terminal)

  Open (any substate) --drop--> Dropped (terminal, zero trace)
```

Allowed transitions and per-transition concurrency semantics:

| Transition | Cost | Concurrency semantics |
|---|---|---|
| **fork** | O(1) target: allocate `wid`, record `base_generation`, empty journal. Today's implementation is O(files in first batch) because fork and first-edit are fused (`create_world_from_edits`); fszero-ap9 splits fork into a true O(1) step. | Any number of worlds can fork from the same generation concurrently; forking never blocks on other worlds' state, only on `next_id` allocation (currently a plain `u32` field mutation, `src/core/subsystems.rs:43,50` -- needs the same store-lock discipline as commit once fork is reachable from concurrent request handlers). |
| **edit** (append to journal) | O(1) amortized per entry (content hash + store put). | Two worlds independently staging edits to the *same* path never observe each other -- per Section 3, journals are namespaced by `wid` and no cross-world read exists. Live conflict *detection* (not prevention) at edit time is fszero-4wp's interval index: it flags overlapping byte ranges across open worlds' journals as an advisory signal, but does not block either edit -- blocking happens only at commit. |
| **preview** | O(edits in world): materialize world-relative content by applying journal on top of base, without touching disk. | Read-only, no state transition, no lock needed beyond the recovery store's normal read path (`expand`). Concurrent previews of the same or different worlds never conflict with each other or with concurrent edits elsewhere (previews always see a consistent snapshot of *this world's own* journal at call time). fszero-otm defines the API surface. |
| **commit** | O(edits in world) for the preimage/merge check + O(edits) writes, serialized on the store lock (Section 4). | Two simultaneous commits against the same base generation: first to acquire the store lock wins outright (base-at-fork == base-now for it, so it's a fast-path apply per I3). The second, now facing a `base-now` that has moved, runs the three-way merge; if the two worlds touched disjoint files, the merge is trivially clean and it also succeeds. If they touched overlapping files with conflicting edits, it returns a structured conflict (Section 4.2) and the world stays Open for resolution. |
| **drop** | O(1): remove `wid` from `active` (today: `src/core/world.rs:285-289`); no disk I/O since nothing was ever written to the real tree. | Dropping never contends with other worlds -- it only removes this world's own map entry and (once journals are store-persisted rather than purely in-memory) deletes this world's journal keys. Concurrent drop + commit of the *same* world race on the same map entry; whichever operation's `remove`/lookup wins is authoritative (today: `commit_world`'s `self.worlds.active.remove(wid)` at the top of the function already makes commit atomic against a concurrent drop of the same id -- only one of the two finds the entry). |

---

## 6. Invariants

Numbered, each stated to be testable against the implementation (unit or
property test) rather than just aspirational prose.

- **I1 -- No world observes another world's uncommitted edits.** No API
  reachable from world `A` can return content, refs, or metadata derived
  from world `B`'s journal while `B` is uncommitted. Testable: for any two
  concurrently open worlds touching the same path, `fs.read`/preview/stat
  issued "through" `A` must be independent of what `B` has staged.
  Currently holds by construction (Section 3) because no code path indexes
  `worlds.active` by anything other than its own `wid`.
- **I2 -- Base moves are explicit.** An open world's `base_generation`
  field is immutable except via the explicit rebase operation (Section
  4.2). Testable: fork a world, commit a *different* world that changes the
  same base generation, then assert the first world's recorded
  `base_generation` and `pre_ref`s are byte-identical to what they were at
  fork time, until either commit or explicit rebase runs.
- **I3 -- Commit of a world forked from an unchanged base is byte-identical
  to single-track commit.** When `base-at-fork == base-now` for every
  touched path, the three-way merge path must degenerate to exactly what
  `commit_world` does today (`src/core/world.rs:112-200`): straight
  preimage check + `fs::write(post)`, same journal/mutation_log shape, same
  git-export behavior. Testable: run the existing single-world commit test
  suite unmodified against the N-world commit path with N=1; outputs
  (file bytes, mutation_log rows, cert refs) must match.
- **I4 -- Drop leaves zero trace.** After dropping world `W`, no on-disk
  file differs from before `W` was forked, and no `mutation_log` row exists
  for `W`. Testable: hash every tracked file and snapshot `mutation_log`
  before fork, fork+edit+drop, hash/snapshot again -- must be identical.
  Already true today since edits are never written until commit
  (`src/core/world.rs:285-289` is a pure map removal).
- **I5 -- Conflicts are detected, never silently clobbered.** Any commit
  where `base-at-fork != base-now` for a touched path either (a) produces a
  clean merge whose result is a well-defined function of base-at-fork,
  world-edits, and base-now (no data loss for either side's non-overlapping
  changes), or (b) returns a structured conflict report and writes nothing
  for that world's commit. There is no third outcome where one side's edit
  is dropped without being surfaced. Testable: construct base-at-fork,
  world-edits, base-now triples with a known overlapping hunk; assert the
  commit either merges deterministically or reports, never applies only one
  side's bytes unreported.
- **I6 -- Base generation is monotonic and commit-attributed.** The
  `base_generation` counter only increases, and every increment is
  attributable to exactly one committed world or one detected external
  write -- never to a fork, preview, edit-stage, or drop. Testable: fork N
  worlds, preview and stage edits on all of them, assert
  `base_generation` unchanged; commit one, assert exactly +1.
- **I7 -- Journal entries are content-addressed and re-verifiable.** Every
  journal entry's `pre_ref`/`post_ref` must satisfy
  `content_ref_matches(ref, expand(ref))` (existing check pattern,
  `src/core/world.rs:301-308`, already used by `verify_cert`). Testable:
  for any committed or open world, re-derive the hash from the expanded
  payload and compare to the ref suffix.
- **I8 -- Fork cost is independent of tree size and of the number of other
  open worlds.** (Target invariant for fszero-ap9; not yet true today,
  since fork is fused with first-edit and reads whichever files the first
  batch touches.) Testable via the existing scaling harness
  (`benchmarks/index-scaling.json`/`.md` pattern) once fork is decoupled
  from first-edit: fork latency at N=1 open world vs N=1000 open worlds
  must be within noise.

---

## 7. Failure and durability

- **Crash mid-commit.** `commit_world` today writes files one at a time
  (`src/core/world.rs:147-167`) and only calls `record_mutation` per file
  *after* the write succeeds (`src/core/world.rs:170-197`). A crash between
  writing file *k* and journaling it leaves the on-disk file changed but no
  `mutation_log` row for it -- `fs.undo` cannot find it by path/seq. Under
  N-world concurrency this risk is unchanged in kind but the blast radius
  question changes: a half-committed world with some files written and some
  not is observationally indistinguishable from "someone hand-edited a
  subset of these files outside FSZero", which is exactly what the
  preimage-guard-on-retry already handles when the agent retries the
  commit (files that changed fail the guard; files that didn't get
  re-attempted). No new mechanism is required beyond making the per-file
  write-then-journal pairing atomic-per-file (already true) and documenting
  that partial commits are recoverable via `fs.history` on the files that
  *did* get journaled, plus a manual preimage check on the ones that
  didn't.
- **Torn journal lines.** World journals, once persisted (they are
  currently in-memory `Vec<WorldFileEdit>`, not durable across process
  restart -- an open world does not survive a crash today), must be
  appended the same way `mutation_log` rows are: one `INSERT` per entry
  (`append_mutation`, `src/core/recovery.rs:1176-1218`), relying on SQLite's
  atomic-row-insert guarantee rather than a hand-rolled line-oriented log
  format. This sidesteps "torn line" failure modes entirely as long as
  world journals reuse the SQLite-backed recovery store rather than a flat
  file. A world journal table's crash-recovery story is then identical to
  `mutation_log`'s: on restart, any world whose last-known state has no
  matching `commit` marker row is presumed still-open-uncommitted and
  resumable (or the caller re-forks and discards it -- today's behavior,
  since worlds don't survive restart at all).
- **Store degradation.** `FSZeroSession.durable_degraded`
  (`src/core/session.rs:77`, set at `src/core/session.rs:149` when
  `with_repo_store` falls back to in-memory) already gates
  `record_mutation` and `record_access` to no-ops
  (`src/core/fs_ops.rs:311-313`, `src/core/access_world_ops.rs:35-37`).
  World commits should follow the same rule: in degraded mode, world
  commits may still apply file writes (the tree mutation is what the agent
  actually wants) but journaling/history/undo for that commit is
  unavailable, same as for direct edits today. This must be surfaced in the
  commit's response string (a `degraded:1` flag) so a caller relying on
  undo-ability for conflict recovery knows it isn't there. Under N-world
  concurrency, degraded mode also means the three-way merge's `base-now`
  read has no durable generation history to consult beyond the live
  filesystem -- merges still work (they only need current content, not
  history), but rebase-to-a-specific-past-generation (Section 4.2) is not
  possible in degraded mode since past generations aren't retained.

---

## 8. Open questions and sibling beads

- **fszero-ap9** -- sub-10ms world fork via copy-on-write journal, no tree
  scan. Decouples fork from first-edit (Section 1.2/1.3 gap) and delivers
  Invariant I8.
- **fszero-4wp** -- live hunk-level conflict detection via an interval index
  at edit time. Upgrades Section 5's "edit" row from advisory-only to an
  actual live signal agents can react to before committing, without
  changing the commit-time merge as the enforcement point.
- **fszero-glg** -- three-way merge on commit: base-at-fork vs world-edits
  vs base-now, structured conflict report, never silent clobber. This is
  the mechanism behind Section 4.1's commit-time observation path and
  Invariant I5; today's `"world preimage changed"` hard-fail
  (`src/core/world.rs:121-124,136-139`) is the degenerate one-sided version
  this bead generalizes.
- **fszero-1wm** -- virtual overlay reads served from journal+base with zero
  disk writes. Needed before "preview" (Section 5) or any "read through a
  world" API can exist; must preserve Invariant I1 by construction (read
  path resolves `journal[wid] -> base`, never another world's journal).
- **fszero-otm** -- world preview API. Defines the concrete surface for the
  preview state in Section 5's lifecycle diagram.
- **fszero-e8s** -- conflict resolution API: accept-mine / accept-theirs /
  supply-merged-hunk / abort. This is the agent-facing response to a
  Section 4.1(b) conflict report; without it, a conflicted world has no
  path forward except drop-and-refork.
- **fszero-cxq** -- N-agent swarm acceptance test. Should assert I1-I8
  directly, plus an end-to-end scenario: K agents forking from one base,
  overlapping and non-overlapping edits, interleaved commits, at least one
  induced conflict resolved via fszero-e8s.
- **fszero-cbt** -- stable world-ref format for graphzero speculative
  blast. Needs the base-generation-tagged journal entry shape from Section
  3 to be a stable, externally-referenceable format (`fz://world/<wid>/...`
  or similar), not just an internal struct -- open question: whether world
  refs are minted eagerly per-entry (like content refs today) or
  lazily/only-on-request.
- **Not yet beaded**: the explicit rebase operation described in Section
  4.2. It's necessary for the lifecycle to be complete (an agent needs a
  way to pull in unrelated upstream progress without committing or
  dropping) but has no owning bead yet -- should be filed once fszero-glg's
  merge machinery exists to reuse, since rebase is essentially "run the
  three-way merge logic without the final write, then adopt the result as
  the new base-at-fork."
- **Open question**: whether `base_generation` is global to the session (one
  counter for the whole tracked tree) or scoped per top-level subtree/repo
  root. A global counter is simpler and matches how `FSZeroSession` is
  already single-root today (`root: Option<PathBuf>`,
  `src/core/session.rs:72`), but forces unrelated worlds (touching disjoint
  files) to see generation bumps they don't care about, which weakens the
  "cheap generation check before falling back to per-file merge" fast path
  in Section 5's commit row. Recommend starting global (matches current
  single-root session model) and revisiting only if profiling under
  fszero-cxq shows generation-churn false-positives dominating merge cost.
