# Clone / allocation baseline (source scan; unmeasured perf)

Scan date: **2026-08-15**. Method: count literal `.clone()` occurrences in
`*.rs` under the repo, excluding `target/` and `.git/`.

This is a **static hygiene baseline**, not an allocation or latency
benchmark. Allocation/perf numbers require a future measured pass on Spark
with `CARGO_TARGET_DIR=/tmp/rch_target_graphzero` (RCH). All timings and
byte deltas below are **unmeasured / advisory**.

## `.clone()` counts by crate (crates/ only)

| Crate | `.clone()` count |
| --- | ---: |
| `graphzero-store` | 379 |
| `graphzero-engine` | 367 |
| `graphzero-reserve` | 35 |
| `graphzero-core` | 34 |
| `graphzero` | 33 |
| `graphzero-why` | 30 |
| `graphzero-extract` | 21 |
| `graphzero-coverage` | 12 |
| `graphzero-scip` | 12 |
| `graphzero-test-support` | 9 |
| `graphzero-pack` | 6 |
| `graphzero-semantic` | 5 |
| `graphzero-mcp-compat` | 4 |
| `graphzero-types` | 1 |
| **crates total** | **948** |

Including `tests/` and other top-level `.rs` raises the workspace total to
**1135** (tests alone: 186). Prefer crate totals for hotspot triage.

### Naming note (store vs query)

There is no `graphzero-query` crate in the current workspace. Query /
dispatch / blast / codemode live primarily in **`graphzero-engine`** (plus
store-side query modules under `graphzero-store/src/store/query/`). Treat
"query" in older beads as **engine + store/query**.

## Likely hot-path candidates (not measured)

Static concentration only -- candidates for a future measured allocation
pass:

| Area | Why candidate | Static signal (2026-08-15) |
| --- | --- | --- |
| **Index / publish** | Indexer builds large owned graphs then publishes shards | `graphzero-store/.../indexer.rs` ~90 `.clone()`; also entity/daemon/memory |
| **Blast** | Blast-radius walks clone node/edge labels into results | `graphzero-engine/src/blast.rs` ~28 `.clone()` |
| **Codemode** | Executor/steps/response build owned JS/host payloads | `graphzero-engine/src/codemode/` area ~81 `.clone()` total |
| **Query surfaces / dispatch** | Surface JSON and dispatcher context ownership | `query_surface/` ~43; `dispatcher/` ~21; `graphzero/src/dispatch.rs` ~10 |

Top store files by `.clone()` count: `indexer.rs` (90),
`durability_receipt.rs` (45), `daemon.rs` (29), `entity.rs` (26),
`memory.rs` (22).

## Future measured pass (required before claiming wins)

Envelope for a real baseline (do not invent numbers here):

1. Host: Spark via RCH; `env CARGO_TARGET_DIR=/tmp/rch_target_graphzero`.
2. Workloads: index a fixed corpus; blast-radius on a fixed symbol; one
   codemode script that exercises bindings.
3. Metrics: allocator stats or `dhat`/`heaptrack`-class profiles (bytes
   allocated, peak RSS), plus p50/p95 wall time -- all labeled with corpus
   commit and binary build id.
4. Only after that pass may clone-removal PRs cite allocation deltas.

Until then: **allocation/perf numbers = unmeasured / advisory**.
