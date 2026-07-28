# Release-N engine MCP compatibility policy

- **Policy version:** 1.0
- **Date:** 2026-07-28
- **Applies from:** Release N
- **Decision basis:** [ADR 0001: CodeMode execution boundary](adr/0001-codemode-execution-boundary.md) (Accepted)
- **Owner:** ZeroStack owner

## Policy

CodeMode is the canonical and default ZeroStack execution mode. Standard engine
MCP is a compatibility alternative, not a layer beneath CodeMode. Every
deployment MUST select exactly one mode and MUST NOT register both.

From Release N:

- TokenZero remains public. Its engine MCP adapter is an explicit opt-in,
  maintenance-only compatibility surface.
- FSZero and GraphZero remain private and in development. Their engine MCP
  adapters are unmarketed, compatibility-only surfaces while that publication
  status remains unchanged.
- Fresh installs and newly generated configuration select CodeMode by default.
- Standard MCP mode registers only the selected engines' ordinary MCP catalogs
  and does not register CodeMode.
- CodeMode does not register, discover, or call any engine MCP catalog.

A generic MCP transport MAY carry aggregate CodeMode for harness compatibility.
That carrier MAY expose only `zero_execute` and `zero_wait`. It MUST NOT
register, proxy, discover, or advertise TokenZero, FSZero, or GraphZero MCP
catalogs. The carrier does not change the deployment's mode: it is CodeMode
transport, with aggregate engine calls dispatched only through planner-free
raw-worker v2 workers.

## Compatibility maintenance commitment

While an engine MCP compatibility package is supported, its owner maintains:

- security fixes, including authorization, isolation, input validation, secret
  exposure, dependency vulnerability, and protocol-injection fixes;
- correctness fixes for documented compatibility behavior, including data
  corruption or loss, invalid protocol framing, crashes, hangs, and regressions;
- packaging or build fixes required to keep the documented opt-in path usable on
  its stated supported platforms; and
- narrowly scoped client-interoperability fixes that preserve the existing
  compatibility contract.

Maintenance-only does not promise feature parity with CodeMode. Engine MCP is
not promised new CodeMode operations, aggregate planning, raw-worker behavior,
performance parity, identical telemetry, every new platform integration, or
catalog expansion. A request outside the supported fix classes requires a new
owner decision; it is not implied by compatibility status.

## Deprecation warnings and protocol safety

Compatibility and deprecation warnings MAY be emitted only through stderr, an
out-of-band control channel, or telemetry. They MUST NOT be written to protocol
stdout. Protocol stdout is reserved for framed protocol messages. When no safe
warning channel exists, the implementation MUST suppress the warning rather
than contaminate stdout. Tests at each release gate MUST cover clean startup,
request, error, shutdown, and warning paths.

## Existing upgrades, migration, and rollback

Release N MUST preserve an existing standard engine MCP deployment during an
upgrade. A detected existing MCP selection remains in standard MCP mode through
preserved or generated explicit compatibility configuration; the upgrade MUST
NOT silently switch modes or enable CodeMode beside it. Fresh installs default
to CodeMode.

An operator migration follows this order:

1. Inventory the client's used engine tools and confirm CodeMode replacements.
2. Install and validate CodeMode in a non-production or maintenance context,
   including refs, effects, cancellation, and the telemetry predicate below.
3. Stop the client and unregister all per-engine MCP catalogs.
4. Enable only the standalone or aggregate CodeMode ingress, then verify the
   exclusive catalog and execution path before resuming traffic.

Rollback MUST remain available while compatibility is supported. Stop traffic,
unregister CodeMode (including `zero_execute` and `zero_wait`), restore the
pinned compatibility package and prior explicit MCP configuration, and register
engine MCP catalogs only after CodeMode is absent. Each staged release MUST
retain a tested rollback artifact and configuration fixture. Rollback MUST NOT
create a dual-mode interval.

## Staged timeline

| Stage | Earliest timing | Required result |
| --- | --- | --- |
| Release N | Policy effective date for the stable release | CodeMode is the fresh-install default; existing MCP upgrades are preserved as explicit compatibility selections. |
| Release N+1 | At least 90 days after Release N becomes stable | Engine MCP adapters may move to explicit compatibility packages only after the N+1 gates pass. |
| Removal decision | At least 180 days and at least two stable releases after the N+1 compatibility-package split | Source or package removal may be considered only for a major release after every removal gate passes and the ZeroStack owner explicitly approves it. |

These are minimum intervals, not scheduled removal dates. A failed gate extends
the stage without weakening maintenance commitments.

## Aggregate telemetry acceptance

For every aggregate CodeMode execution in the gate's acceptance evidence,
including a `zero_wait` continuation correlated to its originating execution, the
following predicate MUST be true:

~~~text
mode == "codemode"
and ingress in {"zero_execute", "zero_wait"}
and engine_transport == "raw_worker_v2"
and engine_mcp_hop_count == 0
and engine_mcp_catalog_registered == false
and trace_id is non-empty
and request_id is non-empty
and sidecar_revision is non-empty
and every participating worker_revision is non-empty
and ingress, sidecar, and worker records correlate to the same trace_id/request_id
~~~

The required fields are `mode`, `ingress`, `engine_transport`,
`engine_mcp_hop_count`, `engine_mcp_catalog_registered`, `trace_id`,
`request_id`, `sidecar_revision`, and the revision for every participating
worker. For `zero_wait`, these fields MUST correlate to and preserve the values
of the originating execution. Any missing, empty, unknown, uncorrelated, or
unexpected value fails the gate. Every execution in the acceptance set must
pass; aggregate counts or averages cannot substitute for per-execution proof.

Telemetry is necessary evidence for the aggregate path, but telemetry alone
MUST NOT authorize engine MCP removal. This policy specifies evidence that must
be collected; it does not claim that a current runtime already emits or passes
that evidence.

## Release gates

### Release N

- Default-install and upgrade fixtures prove CodeMode-by-default for new installs
  and preserved explicit MCP mode for existing upgrades, without dual exposure.
- Catalog tests prove standard MCP and CodeMode are exclusive; an aggregate
  generic carrier exposes only `zero_execute` and `zero_wait`.
- Path tests prove aggregate calls use raw-worker v2 and never an engine MCP hop.
- Warning-path tests prove protocol stdout remains clean.
- Owners and support channels for security and correctness fixes are recorded.

### Release N+1 compatibility-package split

- At least 90 days have elapsed since Release N became stable.
- Explicit compatibility packages install, upgrade, and roll back on every
  stated supported platform; no fresh install selects them by default.
- Supported-client migration fixtures and the exclusive-mode tests pass.
- Release notes identify maintenance-only scope without marketing private
  FSZero or GraphZero adapters.

### Removal evaluation

Removal remains blocked unless all of these conditions are met:

- at least 180 days and two stable releases have elapsed after the N+1 split;
- the supported-client migration matrix is complete and passes;
- no P0 or P1 migration defect has remained open or newly occurred for 60
  continuous days;
- low compatibility usage and low support demand are corroborated, and any
  opt-in telemetry used is sufficiently representative and documented;
- the per-execution aggregate telemetry predicate passes with no missing or
  unknown fields;
- a pinned rollback package and configuration are tested successfully;
- a major-release removal notice is published; and
- the ZeroStack owner gives explicit approval for that major-version removal.

Security or correctness risk, incomplete evidence, an unrepresentative data set,
or a failed rollback test blocks removal regardless of elapsed time.
