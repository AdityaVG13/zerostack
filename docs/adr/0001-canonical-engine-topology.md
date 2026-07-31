# ADR 0001: Canonical engine and binary topology

- Status: Accepted for migration
- Date: 2026-07-31
- Bead: zerostack-hsyv

## Decision

ZeroStack is the shared contract hub for TokenZero, FSZero, and GraphZero. The
engines stay separate and retain domain code, but all three adopt one workspace
shape and one planner-free raw-worker v2 boundary. The aggregate
zerostack-codemode-host is the harness-neutral execution boundary. Pi is one
adapter, not the architecture.

The machine-readable source of truth is conformance/engine-topology-v1.json,
validated by conformance/schemas/engine-topology-v1.schema.json. Dependent
migration, worker-adapter, artifact, and conformance beads consume that
manifest instead of inventing repository-local layouts.

## Canonical workspace

The hub owns zero-abi, zero-ref, zero-store, zero-ledger, zero-gate, zero-cert,
zero-codemode, zero-testkit, zero-gauge, and zerostack-machine-permit. The hub
owns the versioned host/client contract, raw-worker adapter, reference bridge,
artifact manifest, and parameterized conformance. zero-codemode owns the
zerostack-codemode-host aggregate host and may enable QuickJS only there.

Each engine has these roles:

| Role | Canonical package | Canonical binary | Boundary |
| --- | --- | --- | --- |
| Domain/core | existing domain crates or <engine>-core | none | Product logic only |
| Raw worker | <engine>-worker | <engine>-codemode | Domain dispatch through raw-worker v2 |
| CLI | <engine>-cli | <engine> | Thin public host/client consumer |
| Test support | <engine>-test-support | none | Engine-only fixtures and hooks |
| Optional tool | <engine>-xtask or verification package | tool-specific | Non-shipping developer work |

The worker has an empty default feature set. Transport and host features belong
to the hub or thin adapter. Domain features may remain in domain crates only if
they do not alter the raw-worker wire contract. The CLI does not copy worker
lifecycle, protocol, store-root, or accounting code.

## Build and dependency rules

The manifest stores Cargo argv arrays plus runner, program, working directory, and
an isolated CARGO_TARGET_DIR for every command. Hub commands use
.rch-target/zerostack; each engine uses .rch-target/<engine-id>. The directory
is relative to the selected repository and therefore portable while remaining
isolated. The hub commands are workspace validation and aggregate-host release;
engine commands are workspace validation, CLI release, worker release with no
default features, and support compilation.

1. cargo check --locked --workspace --all-targets;
2. cargo build --locked --release -p <engine>-cli --bin <engine>;
3. cargo build --locked --release -p <engine>-worker --bin <engine>-codemode --no-default-features;
4. cargo test --locked -p <engine>-test-support --no-run.

Every Cargo command is marked runner=rch and program=cargo. This ADR authorizes
no local Rust compilation. All repository paths and command working directories
are portable relative POSIX paths: absolute paths, parent traversal, drive-letter
paths, UNC paths, and backslashes are invalid.

Allowed dependency direction is hub host/protocol to hub foundation and worker
adapter; engine domain to hub contracts, refs, store, and accounting; engine
worker to engine domain and hub worker adapter; CLI and adapters to the public
hub host/client and manifest; conformance to public worker interfaces. Forbidden
edges are engine-private cross-dependencies, domain to adapter, worker to
host-runtime or QuickJS, worker to MCP catalog, nested CodeMode, planner, and
adapter to Pi internals outside the Pi adapter.

## Raw-worker v2

A worker is a bounded raw-worker v2 process. It propagates engine identity,
contract digest, request identity, operation, deadline, cancellation, store root,
bounded I/O, durable refs, receipts, and typed errors. Unknown fields are
versioned and unsafe skew fails closed.

Workers contain no planner, JavaScript/QuickJS runtime, MCP catalog or transport,
harness discovery, nested CodeMode, Pi routing, unbounded child process, private
protocol, hidden asynchronous tail, or bypass of journaling, cancellation, RACC,
and typed failures.
Canonical prohibition tokens: planner, QuickJS-runtime, MCP-catalog, harness-discovery, nested-CodeMode, Pi-routing, unbounded-child-process, hidden-async-tail, journal-bypass, cancellation-bypass, RACC-bypass, typed-failure-bypass, private-protocol
Only
zerostack-codemode-host owns aggregate JavaScript.

## Harness-neutral adapters

Plain CLI, MCP/Claude Code, Pi, and third-party adapters all use the same
public-versioned zerostack-host-client-v1 contract, host binary, reference
bridge, artifact manifest, and conformance. A plain CLI owns argument parsing
and terminal rendering. MCP/Claude Code owns MCP transport and negotiation. Pi
owns Pi presentation/routing only. Third-party adapters own configuration and
host lifecycle. None copies engine-private runtime, worker protocol, or build
flags; Pi is not imported by other adapters.

Adapters resolve binaries by explicit config, environment, platform/XDG config,
manifest discovery, then PATH. Atomic publication and stable names prevent
sibling-repository and transient-ENOENT assumptions.

## Current-to-target map

The companion manifest maps every audited current engine Cargo package and
binary, including fuzzers, fixture generators, benchmark workers, xtasks, and
compatibility surfaces. When sibling repositories are available, conformance
independently enumerates their Cargo.toml package metadata and declared/default
binary source paths and compares the exact mapping; a frozen snapshot is used
only when a sibling is absent. move, merge, and retire-from-release are migration
instructions, not deletion approval. Cleanup still requires explicit path
approval. TokenZero splits its broad package into CLI and worker; FSZero moves
its shim and CodeMode package into canonical CLI and worker roles; GraphZero
extracts its raw worker from graphzero-query while retaining graph domain code.

## Consequences

The target lets zerostack-uf1u publish one discovery/install descriptor and lets
zerostack-tau2 extract one worker implementation. Dependent migration beads
must preserve domain behavior, use this manifest, and land in dependency order.
No engine repository is changed by this ADR.
