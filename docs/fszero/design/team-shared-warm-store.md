# Team-shared warm store: design (fszero-e9e)

- **Status:** Proposed
- **Depends on (design input):** `docs/design/zeroref-v1-annex.md` (same-store
  limitation, trust model, storage precedence)
- **Acceptance target:** fszero-qhz (teammate-two benchmark)
- **Prerequisite:** fszero-qzt (pack sidecar GC) -- artifact size
- **Epic:** fszero-c6q (ZeroRef v1, shared CAS, embeddable store)

## Problem

A fresh `git clone` pays a cold index: `benchmarks/index-scaling.md` measures
29.6s at 100k files, with `ast_persist` (23.6% share) and
`merge_and_asgrep_upserts` (56.8% share) dominating. Every teammate who
clones the repo re-derives the same AST rows, call edges, and content blobs
that a teammate who already indexed has sitting in their local store. A
teammate cloning the repo should be able to import that work instead of
re-paying it.

## Ground truth: what's actually in a warm store

Four files make up an indexed FSZero workspace today (`src/core/recovery.rs`,
`src/core/ast_store.rs`, `src/core/ast/index.rs`):

| File | Owner | Contents | Measured (this repo) |
|---|---|---|---|
| `store.sqlite3` | `RecoveryStore` (fsqlite) | `payloads` (content-addressed blobs + small packed-locator rows), `mutation_log`, `facts`, `access_log`, `meta` (incl. `ast/index_manifest`) | 8.8M |
| `store.sqlite3.pack` | `PackFile` sidecar | Append-only bytes for payloads >= `PACK_MIN_BYTES` (4096) | 6.4M |
| `store.sqlite3.ast` | `AstStore` (real sqlite via rusqlite) | `ast_nodes`, `call_edges` -- rebuildable, versioned by `ast_generation` | 29K |
| `.asgrep/index.db` | `ast-sgrep-core::IndexStore` | Literal/regex line index consumed by `Searcher` | 65M (dominant artifact size) |

`.asgrep/index.db` sits at the **workspace root** (`asgrep_root =
root.canonicalize()`), not under `.fszero/`; the other three sit next to
`store.sqlite3` under `.fszero/` (or `.zerostack/fszero/`). Any packaging
plan must gather all four paths, not just the `.fszero/` directory.

### Shareable vs per-user vs machine-local

- **Shareable (content-addressed, commit-independent):**
  `payloads` rows keyed `fz://blob/<sha256>` and their pack-sidecar bytes.
  Identity is the hash (`try_put_content_ref`, `zeroref-v1-annex.md` §2) --
  byte-identical regardless of which machine or commit produced them. This
  is the actual "avoid re-reading N files" payoff.
- **Shareable (derived, but keyed to file *content* not file *identity*):**
  `ast_nodes` / `call_edges` rows and `.asgrep/index.db` rows. These are
  keyed by `file_key` (repo-relative path) + `version`/`ast_generation`, and
  their correctness depends only on file *contents* at that key, not on
  local mtimes. Structurally shareable, but see the re-keying problem below.
- **Per-user, not shareable:** the ref-index (`~/.fszero/ref-index/*.ndjson`,
  `ref_index_root()`) is a *pointer layer* mapping blob refs to
  `store_path` -- a local filesystem path on the machine that minted it.
  Shipping these files verbatim to a teammate produces dangling pointers to
  a store that doesn't exist on their disk (`expand_from_ref_index` degrades
  to a miss, which is fail-open but useless). The ref-index must be rebuilt
  post-import, not shipped.
- **Machine-local, must not ship:** `access_log` (per-agent read/search/edit
  telemetry, 14-day retention per `maintain_wal_cadence`), `mutation_log`
  (`fs.history`/`fs.undo` basis -- tied to a specific working tree's edit
  history, not reusable across machines and privacy-sensitive), `.indexlock`
  (advisory flock, PID-specific), `-wal`/`-shm` (must be checkpointed out
  before packaging, never shipped raw).
- **Ambiguous, exclude by default:** `facts` (provenance table) -- currently
  low-volume and edit-cert-linked; treat as machine-local until a concrete
  cross-team use case demands it (non-goal below).

**Conclusion: the artifact is `store.sqlite3` (payloads only, WAL-checkpointed,
`mutation_log`/`access_log`/`facts` stripped) + `store.sqlite3.pack` +
`store.sqlite3.ast` + `.asgrep/index.db`. The ref-index is never packaged.**

## The re-keying problem (the design has to solve this or it's worthless)

`build_index` (`src/core/ast/index.rs`) decides incremental vs. cold purely
from `INDEX_MANIFEST_KEY` (`ast/index_manifest`), which stores per-file
`(mtime_ns, len)` signatures (`FileSig`, `sig_of`). The diff loop:

```rust
for (_, key, sig) in &current {
    if prev_sigs.get(key) != Some(sig) {
        dirty.insert(key.as_str());
    }
}
```

`git clone` sets every file's mtime to clone time. A naive "copy the four
files into a fresh clone" import ships a manifest whose `(mtime, len)` pairs
match *nobody's* checkout -- every file signature-mismatches, every file
lands in `dirty`, and `build_index` reruns the full cold path (walk + parse
+ persist) despite the AST rows and blobs already sitting in the imported
store. The import would cost artifact-download time *in addition to* the
unchanged cold-index time. That is strictly worse than not importing.

**Fix: re-key the manifest on content, not on wall-clock mtime, at import
time.** Two options, and the recommendation:

1. **Post-import signature refresh pass (recommended for v1).** After
   unpacking the artifact into place, before the first `build_index` call,
   walk the checkout once, stat each file (existing `walk_rs_files` +
   `sig_of` machinery, already O(files) and only 1.8% of total cold-index
   time per `benchmarks/index-scaling.md`), and rewrite
   `ast/index_manifest` with *this checkout's* real `(mtime, len)` pairs for
   every file whose current bytes still content-hash to a blob already
   present in the imported `payloads` table. This requires one content hash
   per file (SHA-256, same primitive as `try_put_content_ref`) to confirm
   the import's AST rows are still valid for that file's current bytes --
   this is unavoidable and is exactly the delta-scan cost quantified below.
   Files whose content hash doesn't match anything in the store (created
   after the artifact was built) fall through to normal cold-index handling
   for that file only.
2. **Content-hash-keyed manifest (structural fix, larger blast radius).**
   Replace `FileSig = (mtime_ns, len)` with a manifest keyed on
   `(content_hash, len)` so import never needs a rewrite pass -- the
   manifest is portable by construction. Rejected for v1: `sig_of` is a
   free `fs::metadata()` stat; a content-hash-keyed manifest means every
   `build_index` call (not just post-import) reads and hashes every file to
   detect staleness, which reintroduces the exact O(files) read cost the
   mtime signature was designed to avoid. This regresses the *common* case
   (no import, just local edits) to speed up the *rare* case (fresh
   import). Revisit only if profiling shows the refresh pass itself becomes
   a bottleneck.

v1 ships option 1: a one-time `fszero store import` refresh pass, not a
schema change to the hot manifest format.

## Staleness: importing store@X into checkout@Y

The imported artifact is built at some commit X; the teammate's checkout may
be at a different commit Y (later commits, local uncommitted changes, or a
different branch). This is unavoidable -- the artifact is necessarily built
before it's downloaded. The import is still a large win because the
signature-refresh pass (above) already produces exactly the right dirty set:

- Files unchanged between X and Y: content hash matches an existing
  `payloads` blob and an `ast_nodes` row at the current `file_key` -- kept
  verbatim, zero re-parse.
- Files changed between X and Y (`git diff X Y --name-only`, or simply
  content-hash mismatch during the refresh walk): fall into `dirty`,
  re-parsed by the normal incremental path (`clear_ast_for_file` +
  `insert_indexed_file`), same as any local edit today.
- Files added/removed between X and Y: handled by the existing
  `current_keys` vs `prev_sigs` diff in `build_index` -- new files parse
  cold, removed files' rows get pruned via `removed`.

This degrades gracefully to the exact incremental-build cost model already
in production (`manifest_diff`, `dirty`/`removed` sets) -- which
`benchmarks/index-scaling.md` shows costs near-zero (`manifest_diff` is
<1% of total at every corpus size measured). The quantified claim: import
cost = O(files changed between X and Y), not O(files in repo). For a repo
with N files and a typical PR-sized delta of d files between clone-time and
import-time (d << N in the common case of "import right after clone"), the
win is `cold_cost(N) - refresh_pass(N) - incremental_cost(d)`, which is
strictly positive whenever `refresh_pass(N) + incremental_cost(d) <
cold_cost(N)` -- true by construction since refresh is a stat+hash walk
(no parse, no AST insert) and `benchmarks/index-scaling.md` shows the parse
+ AST-persist phases (`parallel_ingest` + `ast_persist` +
`merge_and_asgrep_upserts`) are >90% of cold-index wall time at every size
measured. fszero-qhz is the benchmark that measures this end-to-end and
must publish real d/N numbers, not an assumed best case.

## Integrity on import

Two layers, reusing existing primitives rather than inventing new ones:

1. **Per-blob (already exists, reused as-is).** `verified_blob` in
   `src/core/recovery.rs` SHA-256-verifies every `fz://blob/<hash>` payload
   against its key before serving it. This runs on every read regardless of
   provenance -- an imported store gets the same protection a locally-built
   store gets, for free, with zero import-time change. A poisoned blob (key
   says one hash, bytes hash to another) is caught the first time anything
   reads it and reported via `integrity_report()` (fszero-ku8), never served.
2. **Whole-artifact digest (new, needed because per-blob verification is
   lazy).** Per-blob verification only fires on read -- an artifact could
   ship thousands of *unread* poisoned or truncated blobs and nothing would
   notice until each one happens to be touched. Import must additionally
   compute and check a manifest-level digest over the artifact as a unit
   before any of its bytes are trusted:
   - The published artifact carries a `MANIFEST.json` (outside the
     store files) listing: artifact format version, source commit X,
     per-file SHA-256 of each of the four packaged files
     (`store.sqlite3`, `.pack`, `.ast`, `.asgrep/index.db`), and a
     SHA-256-of-the-manifest-JSON as the top-level digest.
   - `fszero store import` recomputes each file's SHA-256 and rejects the
     import outright (no files placed into the checkout) on any mismatch --
     `digest_mismatch`, matching the ZeroRef v1 error taxonomy
     (`zeroref-v1-annex.md` §5) rather than inventing a parallel one.
   - This is a transport/tamper check (did the bytes I downloaded match what
     was published), independent from and in addition to the per-blob
     content-address check (does this specific blob's claimed hash match
     its claimed content) -- the latter also catches internal inconsistency
     (a correctly-transported artifact whose *producer* wrote a bad row).

## Trust model: who signs, and the poisoned-store risk

A store serves bytes claimed to be file contents, verified only against a
hash that the *same artifact* also supplies (`payloads.key` and
`payloads.value` both come from the untrusted artifact). Content-addressing
proves internal consistency (the bytes match the claimed hash) -- it does
**not** prove the bytes are *correct file contents* for that repo-relative
path at that commit. This is the same limitation the ZeroRef v1 annex
already states plainly: "a ref names content but proves nothing" beyond its
own hash (`zeroref-v1-annex.md` §7).

Concretely, a malicious or compromised artifact producer can:
- Ship a blob whose hash is internally consistent but whose *content* is
  wrong for the `file_key` the AST rows associate it with (e.g. inject a
  backdoored `auth.rs` at the correct symbol table position). Per-blob
  SHA-256 does not catch this -- it only proves the bytes weren't corrupted
  in transit/storage, not that they're the *actual* current or historical
  content of that path.
- Ship AST rows / call-edge rows that don't correspond to any real parse of
  the shipped blobs (these tables have no content-address tie-back --
  `ast_nodes.file_key` is a plain string, not verified against the blob at
  that key). A poisoned AST index could misdirect `fn_span`/`query_callers`
  results silently.

Mitigations for v1, ranked by cost:
1. **Provenance, not cryptographic trust, for v1.** The artifact is
   published by an authenticated CI job from a known repo (see transport
   options below) using the org's existing CI identity -- the same trust
   boundary the team already extends to "code that lands on `main`." No new
   signing infrastructure. This is deliberately the same trust level as
   trusting a `git clone` from the canonical remote; it does not attempt to
   exceed it.
2. **Reproducibility as a check, not a gate (v1, cheap).** Ship the source
   commit X in `MANIFEST.json`. A suspicious teammate can locally rebuild
   the AST/asgrep index for a sample of files at commit X and diff against
   the imported rows -- this is exactly the fallback path (cold index) the
   import is meant to avoid, used only as an audit tool, not the default
   flow.
3. **Cryptographic artifact signing (explicit non-goal for v1, noted for
   later).** GPG/sigstore signing of `MANIFEST.json` by a specific
   maintainer key, verified before import. Deferred: no current requirement
   states an adversarial-teammate threat model (the CI-provenance boundary
   already listed is the team's existing trust boundary for code); revisit
   if the artifact starts crossing organizational boundaries (e.g. an
   external contributor importing a store built by someone outside the
   core team).
4. **AST/asgrep rows stay advisory, never authoritative for security
   decisions.** This is already true today (`ast_store.rs`: "Rebuildable
   ... losing it costs one cold rebuild, never data") and remains true for
   imported rows -- a poisoned AST row produces a wrong `fn_span` answer, not
   a wrong file write. File *reads* and *writes* always go through
   `verified_blob`-checked payloads or the live filesystem, never through
   AST rows directly.

## Transport options compared

| Option | Integration cost | Size fit (65M+ `.asgrep`, growing per fszero-qzt) | Freshness / push model | Verdict |
|---|---|---|---|---|
| **Git LFS** | Low -- `git lfs track`, artifact versioned alongside the commit that produced it | Fine at current sizes; LFS storage billing scales with churn (every rebuild = new LFS object unless dedup) | Tied to git history; naturally versioned per-commit but bloats `.git` metadata over time even with LFS | Good default for "one canonical import per notable commit" but couples artifact lifecycle to git history churn |
| **CI/release artifact (GitHub Releases, GitLab package registry, etc.)** | Low -- most CI systems already upload artifacts; auth reuses existing CI credentials | Fine; most providers cap release assets in the GB range | Push-on-CI-build, pull-on-demand; naturally time-stamped and versioned by release tag | **Recommended for v1** -- lowest new infrastructure, matches "authenticated CI job" trust model above |
| **Object storage (S3/GCS + signed URLs)** | Medium -- needs a bucket, IAM/signing setup, lifecycle policy for old artifacts | Best fit long-term (no per-provider size caps, cheap for large/growing `.asgrep` index) | Most flexible (any cadence, easy to prune old artifacts, easy to add a manifest index of available snapshots) | Best for scale-out (many repos, many teams) but is new infra the project doesn't have today |
| **rsync to a peer's live store** | High -- needs a running peer to be reachable (network, auth, port), and is a distributed-store protocol, not a synced-artifact model | N/A (streams, not a fixed artifact) | Always current (pulls from a live index) but requires someone's machine to be up and reachable | Rejected for v1: solves staleness by paying an availability/ops cost this project does not want to take on; see below |

**The bead's framing is "(a) synced pack artifact vs (b) distributed store
protocol." This design picks (a).** A distributed protocol (rsync-to-peer,
or a always-on shared index server) trades the staleness problem for an
availability and ops problem: it requires a reachable, trusted peer process,
network access control, and a live protocol surface -- none of which exist
in FSZero today, and all of which cross into the same "no shared-CAS I/O
yet" territory the ZeroRef v1 annex explicitly scopes out as future work.
A synced artifact is a plain file teammates already know how to fetch (`git
lfs pull`, `curl` a release asset), verify (§ integrity above), and place
locally -- no new server, no new open port, no new persistent process.

## Recommendation

1. **Artifact = synced pack, not a distributed store.** Package
   `store.sqlite3` (WAL-checkpointed, `mutation_log`/`access_log`/`facts`
   rows stripped), `store.sqlite3.pack`, `store.sqlite3.ast`, and
   `.asgrep/index.db`, plus a `MANIFEST.json` (format version, source
   commit, per-file SHA-256, manifest digest).
2. **Transport = CI/release artifact for v1**, with object storage as the
   natural v2 once multiple repos/teams need this (revisit trigger: more
   than a handful of repos wanting shared stores, or artifact size growth
   from fszero-qzt-classed pack bloat outpacing release-asset size limits).
   Git LFS is a viable alternative if the team prefers versioning tied to
   git history over release tags; not recommended as primary because it
   couples artifact lifecycle to git churn.
3. **Import = `fszero store import <artifact>`** (new CLI surface, out of
   scope for this design doc to fully spec, but its steps are pinned here):
   verify `MANIFEST.json` digest -> verify per-file SHA-256 -> place the four
   files -> run the signature-refresh pass (content-hash every current file,
   rewrite `ast/index_manifest` with this checkout's real `(mtime, len)` for
   every file whose content still matches the store) -> rebuild the local
   ref-index from the imported `payloads` keys (never ship the source
   machine's ref-index shards) -> hand off to the existing incremental
   `build_index` path, which now sees mostly-clean signatures.
4. **Trust = CI provenance, not artifact signing, for v1.** Revisit signing
   only if the trust boundary needs to cross outside the team's existing CI
   identity.
5. **Prerequisite: fszero-qzt (pack GC) lands first.** The pack sidecar is
   append-only and never reclaims space (`.pack` in this repo is 6.4M and
   "growing" per fszero-qzt's own description) -- shipping an ever-growing
   append log as a recurring team artifact is the wrong shape without
   compaction. Publishing artifacts before GC exists means every published
   snapshot carries forward all previously-superseded dead byte ranges.
6. **Acceptance = fszero-qhz.** The teammate-two benchmark (import + verify
   + delta-index on the 23k corpus, target under 1s end-to-end) is the
   concrete pass/fail gate for this design; it must publish real
   import/verify/delta-index numbers against a real artifact, per the
   bead's binding integrity policy (no cherry-picked corpora, losses
   published alongside wins).

## Migration and versioning

- `MANIFEST.json` carries an explicit `artifact_format_version` (start at
  1). `fszero store import` refuses artifacts with a major version it
  doesn't understand (`incompatible_version`, mirroring the ZeroRef v1
  precedent of `ZEROREF_MAJOR`/`ZEROREF_MINOR` in `src/core/zeroref.rs`)
  rather than attempting a best-effort partial import.
- The four packaged files already carry their own internal versioning
  independent of the artifact format: `ast_generation` /
  `ast/index_manifest`'s `gen=` line (bumped on cold rebuild, checked by
  `build_index`), and the `ast_nodes.version`/`call_edges.version` columns.
  Import does not need to invent new schema-migration machinery -- it needs
  to place files whose *internal* generation the existing `build_index`
  incremental/cold decision already understands. If a future schema change
  alters `init_tables`' table shapes, the existing legacy-migration
  discipline in `recovery.rs` (probe-before-ALTER, e.g. the `pre_mtime_ns`/
  `pre_mode`/`pre_xattrs` migrations) already handles opening an
  older-schema imported store safely.
- Artifact producers should retain at least the last N published snapshots
  (exact N is an ops decision, not a design constraint here) so a teammate
  importing slightly behind HEAD still gets a reasonably fresh base; this is
  a publishing-cadence policy, not a code change.

## Non-goals

- **A distributed store protocol** (peer-to-peer sync, always-on index
  server, rsync-to-live-peer). Explicitly rejected in favor of a synced
  artifact for v1; see transport comparison above.
- **Cryptographic artifact signing** (GPG/sigstore). CI provenance is the
  v1 trust boundary; signing is future work if the trust boundary needs to
  widen.
- **Content-hash-keyed manifest as the hot-path signature format.** The
  mtime-based `FileSig` stays; re-keying happens only as a one-time
  post-import pass, not a schema change to `build_index`'s common path.
- **Shared-CAS I/O across engines** (FSZero reading GraphZero/TokenZero
  blobs live, or vice versa). Out of scope; this design is single-engine,
  same as the ZeroRef v1 annex's current same-store limitation.
- **Sharing `mutation_log`, `access_log`, or `facts`.** These are
  machine-local / privacy-sensitive by construction (per-agent edit
  history, read telemetry) and are stripped before packaging, not made
  configurable-to-share.
- **Sharing the ref-index.** It is a pointer layer to a local filesystem
  path (`store_path`) and is meaningless off the machine that wrote it;
  every importer rebuilds their own from the imported `payloads` keys.
- **Automatic/continuous import** (e.g. import-on-every-pull). v1 is an
  explicit, one-shot `fszero store import` command; a background-sync
  daemon is a distributed-store-shaped feature explicitly out of scope
  above.
- **Pack compaction/GC itself.** That is fszero-qzt's scope; this design
  only states it as a size-hygiene prerequisite for recurring publication.
