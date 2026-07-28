# ADR 0003: Do not extract a shared engine MCP compatibility core

- **Date:** 2026-07-28
- **Status:** Accepted (no-build)
- **Decision owners:** ZeroStack engine maintainers

## Context

ZeroStack supports two mutually exclusive deployment surfaces: CodeMode and the legacy MCP compatibility adapters. The question is whether FSZero, TokenZero, and GraphZero should extract their remaining MCP framing, catalog, and transport code into a fourth shared compatibility crate.

This decision is based on source and contract inspection, not production telemetry. No runtime hop traces, traffic volumes, defect rates, or maintenance-time measurements were available or inferred.

## Evidence

### Adapter inventory and overlap

The current source snapshot contains 28 top-level adapter module files and 9,054 physical lines across the three implementations:

| Engine | Repository-relative inventory | Module files | Physical lines |
| --- | --- | ---: | ---: |
| FSZero | `FSZero/src/mcp_protocol/*.rs` | 10 | 1,460 |
| TokenZero | `TokenZero/crates/tokenzero-mcp-compat/src/*.rs` | 13 | 5,413 |
| GraphZero | `GraphZero/crates/graphzero-cli/src/{mcp.rs,mcp_catalog.rs,mcp_protocol.rs,fastmcp_adapter.rs,fastmcp_mode.rs}` | 5 | 2,181 |
| **Total** |  | **28** | **9,054** |

These are inventory counts, not a claim that 9,054 lines are duplicate. File-level inspection shows overlap only in compatibility concerns such as JSON-RPC envelopes, initialization, tool catalog publication, stdio lifecycle, FastMCP mode selection, and error conversion. The engine adapters also contain materially different policy and transport code: FSZero has HTTP and request-guard modules (`FSZero/src/mcp_protocol/http.rs`, `request_guard.rs`); TokenZero owns supervision, resources, and its compatibility catalog (`TokenZero/crates/tokenzero-mcp-compat/src/supervisor.rs`, `resources.rs`, `catalog.rs`); GraphZero integrates MCP into its CLI dispatch and graph-specific catalog (`GraphZero/crates/graphzero-cli/src/mcp.rs`, `mcp_catalog.rs`). Module names therefore overstate extractable duplication.

The small FSZero executable wrapper, `FSZero/crates/fszero-mcp/src/main.rs` (17 physical lines), already delegates to the engine library. It is packaging, not another protocol implementation.

### What is already shared

Canonical operation identity and typed dispatch contracts are already shared through the hub's `zero-abi` crate (`crates/zero-abi/src/lib.rs`) and exercised by the shared ABI/conformance contracts under `conformance/`. Engine catalogs adapt that canonical registry/dispatch boundary to their surface instead of defining a second cross-engine operation ABI. TokenZero explicitly checks this boundary in `TokenZero/crates/tokenzero-mcp-compat/src/operation_abi_parity.rs`; FSZero's surface split is in `FSZero/src/mcp_protocol/surface.rs` and `FSZero/src/surfaces/{mcp.rs,codemode.rs}`; GraphZero's catalog-to-dispatch seam is in `GraphZero/crates/graphzero-cli/src/{mcp_catalog.rs,dispatch.rs}`.

Thus the high-value shared layer already exists. A new crate would share compatibility transport mechanics, not canonical operation semantics.

### Aggregate raw-worker path

The aggregate contract and current source establish a direct worker boundary:

1. The hub contract in `docs/adr/0001-codemode-execution-boundary.md` defines CodeMode as a deployment mode distinct from MCP.
2. FSZero's aggregate entry is `FSZero/src/mcp_protocol/raw_worker.rs`; the raw-worker protocol is an internal typed request/response channel, not an MCP server session.
3. The current aggregate host dispatch contract calls engine raw-worker-v2 processes directly. Tool discovery is supplied by the host's typed surface, while canonical operation validation/dispatch remains at the zero-abi boundary.

Source/contract result: an aggregate operation performs **zero engine MCP hops and zero engine `tools/list` hops**. This is a static path proof. It is not measured runtime telemetry and does not prove a latency number.

## Decision

Do not build a shared `zero-mcp` compatibility core.

The extraction has no demonstrated positive ROI for the remaining compatibility-only lifetime:

- It would add a fourth publish/version/dependency and security-review surface, then migrate three adapters whose transport, lifecycle, and policy requirements differ.
- The maximum apparent extraction pool is bounded by 28 files/9,054 lines, but inspection shows only a subset is generic framing. The canonical registry and dispatch semantics are already shared by zero-abi.
- Centralizing legacy parsing would increase blast radius: one parser or dependency vulnerability could affect all three adapters simultaneously.
- Current policy sends new aggregate functionality through typed raw workers and CodeMode, so a compatibility transport abstraction would not serve the primary execution path.

This is an explicit accepted no-build decision, not deferred implementation work.

## Invariants

1. Aggregate workers must never traverse MCP, initialize an MCP session, or call engine `tools/list`.
2. Aggregate host-to-engine calls must remain direct typed raw-worker calls validated against the canonical zero-abi registry/dispatch contract.
3. MCP adapters remain compatibility surfaces and must not become an internal dependency of CodeMode or aggregate dispatch.

A violation is architectural severity **high** because it reintroduces protocol hops, duplicate discovery, and a broader parser/security boundary into the aggregate path.

## Reopen triggers

Reconsider extraction only when evidence satisfies at least one trigger:

1. **Measured duplicate security maintenance:** the same protocol/security defect requires substantively equivalent fixes in at least two engine adapters on two separate incidents. Link the fixes and identify duplicated modules/lines.
2. **Aggregate generic transport reuse:** an approved aggregate design needs a generic transport used by multiple engines outside compatibility MCP, while preserving the invariant that aggregate workers never traverse MCP or `tools/list`.

A reopen proposal must provide a source diff inventory, expected removed versus added LOC, dependency changes, threat-boundary changes, migration cost, and compatibility sunset assumptions. Runtime claims require captured measurements; source inspection alone must stay labeled as static proof.

## Consequences

- No new crate, dependency edge, release artifact, or shared parser attack surface is added.
- Each engine continues to own its adapter-specific transport and compatibility policy.
- Cross-engine consistency remains enforced at zero-abi and conformance boundaries rather than by sharing MCP server machinery.
- Small framing fixes may still be repeated. That cost is accepted until a reopen trigger supplies measured evidence.
- The raw-worker no-MCP invariant becomes a review requirement for aggregate changes.
