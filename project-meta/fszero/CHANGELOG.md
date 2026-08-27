# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Restored the real ast-sgrep search engine (pinned rev) after vendor path stubs shipped a no-op engine; adapter/index forward-fixed for the pinned API.
- HTTP MCP sessions funnel through one owner thread: the per-root session cache is reused (no reload storms) without sharing !Send state across connection threads (the previous cache could not compile).
- Built-in explore recipe stays within max parallel width; the defs probe runs as a named sequential step.
- Analysis/index/heavy permits scope per repo via zerostack-machine-permit::scoped_permit_base, ending cross-repo serialization through one machine-global slot (2026-07-16 incident).

### Added

- Exact expand: `fs.expand` accepts `ref#L<start>-<end>` line windows (always delivered, seen-set bypassed) and inlines expand payloads up to 64KB instead of re-capsuling the exactness escape hatch.
- `fs.expand` registered as a first-class CodeMode kernel method (JSON plans, discovery, describe).### Added

- Durable speculative worlds: active worlds persist in SQLite and rehydrate on session open (no daemon).
- Memory volume MVP: `compound mem:put|get|ls` stores path-keyed agent memory under `mem://` in the recovery store.
- First-class memory on both surfaces: MCP `fszero.memory_put|get|ls|delete|rename`, CodeMode `fs.memory.*` + recipe `memory:…` (see `docs/memory.md`).
- Memory opcode `M` with strict ref-first get (body only via expand) and `memory_paths` index for O(prefix) list.
- Session seen-set on expand: re-expand of the same content-addressed payload answers `unchanged since <ref>`.
- Pack durability barrier: pack sidecar `sync_all` (fsync) before SQLite locators commit; recovery store `PRAGMA synchronous=FULL`; open-time repair of torn locators; `pack_torn:` expand errors instead of silent misses.
- Fail-closed durable store open: servers use `try_with_repo_store`; in-memory fallback requires `FSZERO_ALLOW_EPHEMERAL=1`.


## [0.1.0] - 2026-07-05

### Fixed

- Cross-root wire expand now returns byte-exact payloads: ok acknowledgments can no longer ship with empty bodies, and the wire contract test rejects that failure class.

### Added

- Initial public FSZero release with executable filesystem operations and repository RAG over local project state.
- Speculative worlds with history and undo support for safe preview, commit, and rollback flows.
- Durable mutation journal for auditable file changes and recovery-aware editing.
- FastMCP dual-mode server support, including per-operation MCP tools and CodeMode plan execution.
- Envelope v2 responses with ref-first payloads to keep large results addressable without dumping raw bodies.
- Per-user reference index for cross-session recovery of stored refs.
- Corrective-hint errors that explain the supported shape when a request is malformed or unsupported.
- String-literal-safe sandbox handling for CodeMode execution.
- README command audit coverage for documented install, build, and usage commands.
