# NCIB release gate waivers (fszero-ncib.10)

Waivers are **fail-closed by default**: a gate may stay open only when this file
records **owner**, **expiry**, **scope**, and **evidence-backed rationale**.
Expired waivers block release mode (`scripts/ncib_release_gates.sh release`).

## Active waivers

### W1 — Absolute latency thresholds (microseconds / JS empty plan)

| Field | Value |
| --- | --- |
| **id** | `ncib-w1-absolute-latency` |
| **owner** | `aditya` |
| **expiry** | `2026-08-20` |
| **scope** | Absolute targets from epic AC: warm recipe/JSON orchestration overhead ≤ max(250µs, 15% raw engine p50); warm empty JavaScript plan p50 ≤ 1ms and p99 ≤ 5ms |
| **gates_affected** | release mode absolute latency assertions (not PR relative ratchet) |
| **rationale** | Absolute microsecond bounds are hardware- and profile-class dependent. Debug CI and mixed ARM hosts cannot honestly enforce 250µs/1ms without like-for-like release artifacts and recorded machine class. |
| **evidence** | Relative ratchet **is** enforced: CodeMode N≥3 p50/p95 must not exceed N sequential MCP calls (see `codemode_not_slower_than_n_mcp`, `tests/surface_bench.rs`). Bench harness records raw trials + provenance (`fszero.surface_bench`) for class ratchets once release samples exist. |
| **replacement** | After expiry: land release-profile samples under `tests/artifacts/perf/` with **machine class tags** from `fszero.surface_bench` provenance (`host_class`, `cpu_model`, `cargo_profile` — fszero-bkeu) and hard-assert absolute bounds only when run host_class matches baseline; or file a new waiver with fresh owner/expiry. See also `docs/benchmark-integrity.md` host_class policy. |

### W2 — Full transport matrix (stdio + HTTP × surfaces)

| Field | Value |
| --- | --- |
| **id** | `ncib-w2-full-transport-matrix` |
| **owner** | `aditya` |
| **expiry** | `2026-08-20` |
| **scope** | Full platform/transport matrix “before release” wording in .10 AC |
| **gates_affected** | HTTP MCP + dual-OS packaging matrix |
| **rationale** | In-process domain parity (MCP dispatch / CodeMode method / raw worker) is enforced by `tests/ncib_conformance.rs`. Full process-level stdio/HTTP matrix is owned by packaging e2e + install smoke and is environment-heavy on this host. |
| **evidence** | `ncib_conformance`, `packaging_lifecycle`, `packaging_e2e`, `scripts/ncib_release_gates.sh pr` |
| **replacement** | Expand release mode to invoke packaging e2e under both feature artifacts when CI runners are available. |

## Policy

1. No silent skips: any non-asserted AC number must appear here.
2. Owner must be a human id; expiry is ISO date `YYYY-MM-DD`.
3. `scripts/ncib_release_gates.sh release` parses this file and fails if any
   active waiver is past expiry.
4. Relative performance ratchet (CodeMode vs MCP N≥3) is **not** waived.
