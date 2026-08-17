# ZeroStack contract

Normative. This is the product contract for the hub and the three engines.
It is not a test runner. A later harness, if written, must implement this
file. It must not invent a second catalog.

RFC 2119 keywords apply.

## 1. Composition

ZeroStack is the hub. FSZero, GraphZero, and TokenZero are independent
engines. They MUST NOT import each other. The hub composes them in one
process.

| Engine | `ns` | Mutation |
| --- | --- | --- |
| FSZero | `fz` | `allowed` -- file mutation is journaled; a later failed step MUST roll back earlier mutation in the same execute |
| TokenZero | `tz` | `denied` -- no workspace file mutation |
| GraphZero | `gz` | `store_only` -- engine-store writes MAY occur; workspace file mutation MUST be rejected |

## 2. Surfaces

CodeMode and MCP are mutually exclusive catalogs. A process MUST serve one.

| Surface | Entry | Tools |
| --- | --- | --- |
| CodeMode | `zsx exec -C ROOT` | none -- the plan calls `zero.fs.*`, `zero.graph.*`, `zero.token.*` |
| MCP | `zsx mcp` | exactly `zero_execute` and `zero_wait` |

`zsx mcp` MUST be harness-owned stdio. It MUST die with the parent. It MUST
NOT detach, wrap in Python, or register engine MCP servers.

`zero_execute` takes a JavaScript plan and an optional `timeout_ms`.
`zero_wait` reports process identity and image freshness. It MUST NOT spawn
a child.

Engine binaries MUST NOT be registered as MCP servers next to `zsx mcp`.

## 3. Result shape

Every public `zero.*` call returns `zero-result`:

- `ack` -- short status
- `content` -- either inline value or a typed ref

An oversize result MUST spill to a content-addressed ref. The visible
envelope MAY carry `result_finalization_receipt.v1` (`rawResultJsonBytes`,
`finalizedValueJsonBytes`, `savingsBytes`). `savingsBytes` is not a token
count.

## 4. Refs

| Scheme | Producer |
| --- | --- |
| `fz://` | FSZero |
| `gz://` | GraphZero |
| `tz://` | TokenZero |

A blob ref is `{ns}://blob/<64 lowercase hex>` plus an optional `#Bstart-end`
or `#Lstart-end` fragment. Consumers MUST preserve the scheme. A missing or
stale ref MUST fail loudly.

## 5. Honesty

TokenZero MAY emit Exact `billed_tokens` / `raw_tokens` / `visible_tokens`.
FSZero and GraphZero MUST NOT pretend to. Spill receipts that cannot certify
tokens MUST set `visibleTokenCount` to null and
`visibleTokenCountStatus` to `requires_tokenzero_certification`.

`recovery_tokens` is the cost of expanding a ref. It is not billed.

Estimates MUST NOT be labeled Exact. A skipped measurement is not a pass.

## 6. Settlement

If a call is already cancelled or timed out and a late Ok arrives, the
reported kind MUST be `commit_race`, `retryable` false. The committed
payload stays attached. A late domain Err MUST stay that Err.

## 7. What this folder holds

- `engine-topology.json` -- crate roles and dependency direction
- `contracts/` -- cache entry, fresh-work vector, edit protocol
- `schemas/` -- JSON Schema for the wire types above
- `models/` -- example instances
- `authority/` -- claim ledger. Entries start unproven. A hash is not a pass.

## 8. What is not claimed

There is no in-repo conformance CLI. There is no `{ns}_execute_code` catalog.
Those were a previous surface. Do not resurrect them as synonyms for
`zero_execute`.

Proof that the product compiles: `cargo build --workspace`.
Proof that a behavior holds: a live `zsx` receipt, not a paragraph here.
