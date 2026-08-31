# Changelog

All notable changes to ZeroStack are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning targets
[Semantic Versioning](https://semver.org/) once 1.0 is cut. ZeroStack releases
as one product. FSZero, GraphZero, and TokenZero are its three domain
subprojects and share the `zero-abi` contract.

## [Unreleased]

### Changed
- Retired engine-local product surfaces. ZeroStack exposes one model-facing
  API: `z.read`, `z.find`, `z.edit`, `z.apply`, `z.run`, and `z.state`. MCP is
  a one-tool carrier for clients that cannot embed ZeroKernel. It does not
  expose engine catalogs, aliases, or a second planner.
- FSZero, GraphZero, and TokenZero now provide file, structure, and token
  domain logic behind ZeroStack. They remain separate crate domains with no
  engine-to-engine imports and are not separately installable products.
- All workspace packages inherit one release identity and dependency policy
  from the root Cargo workspace.
- The supported download is the source tree. The canonical executable is
  `zero-kernel`; the asynchronous Node binding remains under `bindings/node/`.
- The public README leads with ZeroKernel and recovery-aware context
  compression. RACC documentation is one page at `docs/racc/RACC.md`.
- The repository has one demo, one xtask crate, one fuzz workspace, and one
  flat machine-readable contract surface.
- Operator diagnostics live on `zero-kernel doctor`.
- Node `executeCell` abort is a structured `Cancelled` outcome with
  `liveTasks == 0`, not a thrown engine string.

### Fixed
- Guest-frame JavaScript string literals now decode every escape sequence in a
  single pass; `\r`, `\uXXXX`, and `\u{...}` can no longer survive as literal
  text, and template-literal escape sequences decode correctly.
- A removal request through `z.edit` on a missing path returns a typed
  `NotFound` instead of poisoning the cell transaction.
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
