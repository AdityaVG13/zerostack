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
- Distribution scaffolds now live under `packaging/package/` for Homebrew,
  npm, and Pi, with the Node binding remaining authoritative in `bindings/node/`.
- The repository now has one demo, one xtask crate, one fuzz workspace, and one
  flat contracts surface. Archived metadata, obsolete engine-local scripts,
  duplicate documentation, and the retired root conformance workspace were
  removed.

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

### Added
- `TokenEngine::certify`: re-measure bytes against a claimed accounting so the
  response boundary can prove reported numbers equal reality.
- `z.find({query, ...})` single-object calling convention.
- `z.read` on a directory returns a deterministic listing; use `z.find` for structural search.
- Calling `z.state()` or other namespaces returns a catchable `TypeError` with
  sub-method guidance.
- `zero-kernel doctor` reports quarantined transaction journals.
