# Engine MCP compatibility policy

- **Policy version:** 2.0
- **Date:** 2026-08-12
- **Owner:** ZeroStack owner

## No-release boundary

ZeroStack is a public shared-base repository because FSZero, GraphZero, and
TokenZero depend on it. ZeroStack does not publish releases, tags, signed
bundles, registry packages, or coordinated reinstall artifacts. `pi-zsx` is an
internal adapter and must not be published unless the owner gives new explicit
approval in the future.

Release cadence, elapsed time, telemetry, or usage data can never authorize
publication or source removal. Any future publication or removal proposal needs
a new owner decision and a replacement policy. This document schedules neither.

## Execution policy

CodeMode is the canonical and default execution mode. Engine MCP is an explicit
compatibility alternative, never a layer beneath CodeMode. A deployment must
select exactly one mode and must not register both.

- New local development configuration selects CodeMode by default.
- Existing explicit MCP configuration remains MCP until its operator migrates.
- Standard MCP registers only the selected engine catalogs.
- CodeMode does not register, discover, proxy, or call engine MCP catalogs.
- A generic MCP carrier may expose only `zero_execute` and `zero_wait`; engine
  calls still cross planner-free raw-worker boundaries.

## Compatibility maintenance

Supported compatibility adapters receive security and correctness fixes for
their documented behavior. This includes isolation, validation, secret
exposure, protocol framing, corruption, crashes, hangs, and regressions.
Maintenance does not promise CodeMode feature parity, aggregate planning,
performance parity, new catalog entries, or new platform integrations.

Compatibility warnings may use stderr, telemetry, or another out-of-band
control channel. They must never contaminate protocol stdout. When no safe
warning channel exists, suppress the warning.

## Local migration and rollback

1. Inventory the engine tools used by the client.
2. Validate CodeMode locally, including refs, effects, cancellation, and the
   execution-path predicate below.
3. Stop the client and unregister every engine MCP catalog.
4. Enable only CodeMode, then verify catalog exclusivity before resuming work.

Rollback reverses those steps without a dual-mode interval: stop work,
unregister CodeMode, restore the prior explicit MCP configuration, then register
engine MCP only after CodeMode is absent. Source and configuration fixtures stay
in the repositories; no release package is created.

## Aggregate execution-path predicate

Every checked aggregate execution, including a correlated `zero_wait`, must
satisfy:

~~~text
mode == "codemode"
and ingress in {"zero_execute", "zero_wait"}
and engine_transport == "raw_worker"
and engine_mcp_hop_count == 0
and engine_mcp_catalog_registered == false
and trace_id is non-empty
and request_id is non-empty
and sidecar_revision is non-empty
and every participating worker_revision is non-empty
~~~

Ingress, sidecar, and worker records must correlate to the same trace and
request. Missing, unknown, empty, or uncorrelated fields fail the check.
Telemetry proves only the checked execution path. It grants no publication or
removal authority.

## Source-removal rule

Keep compatibility source. Do not schedule removal from release counts or time
gates because ZeroStack has no releases. Source removal requires a future,
explicit owner decision with a supported-client migration matrix, rollback
proof, defect review, and a new policy. Until then, security and correctness
maintenance continues.
