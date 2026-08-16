# Benchmarks

Local development measurements. Do not ratchet a number from a single run.

## Savings (v1)

Live `zsx mcp` against a 36,645-byte unique file. Full table: [savings-bench-v1.md](savings-bench-v1.md).

Three layers. Do not mix them.

| Layer | What |
| --- | --- |
| Exact tokens | TokenZero `billed_tokens` / `raw_tokens` |
| Envelope bytes | spill `savingsBytes` (not tokens) |
| Call fusion | 1 MCP call vs N |

Headline Exact: `token.read` with `max_visible_tokens: 200` billed **198** against raw **4500**. Ranged read of the hit window billed **55**.

FS surfaces do not certify Exact. Their spill receipts say `requires_tokenzero_certification`.

## Call fusion (same fixture)

| Path | Calls | Naive |
| --- | ---: | --- |
| `multi_edit` writes (6 files) | 1 | 6 |
| snap-to-file | 1 | search + read |
| snap-to-effect | 1 | search + edit |
| `multi_read` 5 files | 1 | 5 |
| `multi_edit` 3 files | 1 | 3 |

`multi_read` kernel time: 506 µs, one physical pass.

## How to add a number

One `zero_execute` per row. Record the full MCP envelope, including `receipt` when spilled. Exact column stays empty unless TokenZero `accounting` is present. Bump to v2 when the method changes; do not backfill estimates into v1.
