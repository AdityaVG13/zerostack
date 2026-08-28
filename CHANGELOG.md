# Changelog

All notable changes to ZeroStack are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning targets
[Semantic Versioning](https://semver.org/) once 1.0 is cut. FSZero, GraphZero,
TokenZero, and ZeroStack release in lockstep because they share the
`zero-abi` contract.

## [Unreleased]

### Changed
- ZeroStack, FSZero, GraphZero, and TokenZero now build from one Cargo
  workspace, with separate crate domains and no engine-to-engine imports.
- There is no public package. Homebrew, npm, and Pi distribution scaffolds
  were removed from the tracked tree. The Node binding remains source-only
  under `bindings/node/`.
- The public README leads with ZeroKernel and recovery-aware context
  compression. RACC documentation is one page at `docs/racc/RACC.md`.
- The repository now has one demo, one xtask crate, one fuzz workspace, and one
  flat contracts surface. Archived metadata, obsolete engine-local scripts,
  duplicate documentation, and the retired root conformance workspace were
  removed.
- Engine-domain manifests no longer declare missing test or crate paths.
  V6 `zero.fs.*` / `zero.graph.*` / `zero.token.*` CodeMode bindings are
  retired from shipped catalogs; operator CLIs remain installer/re-exec
  shims, not a second model API.
- Node `executeCell` abort is a structured `Cancelled` outcome with
  `liveTasks == 0`, not a thrown engine string.

### Fixed
- CodeMode string literals now decode every JavaScript escape sequence in a
  single pass; `\r`, `\uXXXX`, and `\u{...}` can no longer survive as literal
  text, and template-literal escape sequences decode correctly.
- `z.remove` on a missing path returns a typed `NotFound` instead of poisoning
  the cell transaction.
- Large-file reads return an unmistakable `[ZeroStack READ OUTLINE]` header
  with the exact-content handle; outline projections can no longer be mistaken
  for file bytes.
- Path-form `z.edit` refuses bare replacement strings that would silently
  replace a whole file; substitution and deliberate `replace_file` remain.
- Shell child supervision on macOS no longer blocks `spawn` for the lifetime
  of the child (watcher file-descriptor leak) and no longer misses exit events
  registered after process exit (`waitid` fast path).
- A timed-out or cancelled `z.find` on a cold isolated store reports the
  root deadline or cancel error. Quiescence drain no longer replaces that
  error with "frame did not quiesce". Failed-frame drain is capped at 30s
  independently of a large wall budget; successful cells keep the short
  settle grace.

### Added
- `TokenEngine::certify`: re-measure bytes against a claimed accounting so the
  response boundary can prove reported numbers equal reality.
- `z.find({query, ...})` single-object calling convention.
- `z.read` on a directory returns a deterministic listing; use `z.find` for structural search.
- Calling `z.state()` or other namespaces returns a catchable `TypeError` with
  sub-method guidance.
- `zero-kernel doctor` reports quarantined transaction journals.
