# Changelog

All notable changes to ZeroStack are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning targets
[Semantic Versioning](https://semver.org/) once 1.0 is cut. FSZero, GraphZero,
TokenZero, and ZeroStack release in lockstep because they share the
`zero-abi` contract.

## [Unreleased]

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
- `z.asgrep({query, ...})` single-object calling convention.
- `z.snap` on a directory returns guidance pointing at `z.lookup`.
- Calling `z.state()` or other namespaces returns a catchable `TypeError` with
  sub-method guidance.
- `zero-kernel doctor` reports quarantined transaction journals.
- `scripts/refresh-harness-addon.sh`: one-command propagation of kernel
  changes into harness binaries.
