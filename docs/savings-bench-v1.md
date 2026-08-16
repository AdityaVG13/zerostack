# Savings bench v1

Measured 2026-08-16 on live `zsx mcp` (`pid 26663`, harness-stdio) against `/tmp/zsx-savings-20260816`.
Machine-readable twin: [`savings-bench-v1.json`](savings-bench-v1.json).

This is the citation set for the next ZeroStack bench iteration. Three layers. Do not mix them.

## The three layers

| Layer | What it is | When it is real |
|---|---|---|
| **Exact tokens** | TokenZero `billed_tokens` / `raw_tokens` / `visible_tokens` | `token.read` / `token.expand` / `token.shell` / `token.compact` |
| **Envelope bytes** | `result_finalization_receipt.v1` `savingsBytes` | Any spilled `zero_execute` |
| **Call fusion** | 1 MCP call vs N | Every fused surface |

FS adapters emit `input_token_cost: 0` and no `WorkerTokenAccountingV1`. That is not "free." It is **uncertified**. Spill receipts say `visibleTokenCountStatus: requires_tokenzero_certification`.

Do not call `savingsBytes` a token saving. Do not call `recovery_tokens` billed.

## Headline (Exact)

`token.read("compact_50k.txt", { max_visible_tokens: 200 })` on a **36,645-byte** unique file (`sha256 d7bd8d96…`):

| | tokens |
|---|---:|
| raw | **4500** |
| billed / visible | **198** |
| recovery (expand-all, not billed) | 10679 |
| exact_ref | 21 |

**4302 Exact tokens not billed** -- billed is 4.4% of raw.

Same file, ranged `token.read` lines 500-510: **billed 55 / raw 55**. Versus dumping the file (raw 4500) that is **55 / 4500** -- 1.2% of the naive full-file token load.

That is the number to quote: snap or budgeted read vs dumping the file.

## Envelope bytes (not tokens)

| Row | raw JSON B | visible B | savingsBytes |
|---|---:|---:|---:|
| `token.read` full file (unbudgeted, spilled) | 38744 | 1198 | 37546 |
| `fs.search ROW_500` (HIT in 36k file) | 20053 | 1200 | 18853 |
| 5 `fs.read` mashed in one plan | 12743 | 1202 | 11541 |

Small HIT (`SNAP_FILE_NEEDLE` in a 79-byte file) did not spill. No byte savings there; the win is one call.

## Call fusion

| Product path | Calls | Naive | Ratio |
|---|---:|---|---:|
| Shape A `multi_edit` writes (6 files) | 1 | 6 `fs.write` | 1/6 |
| Shape B three `await fs.write` | 1 | 3 | 1/3 |
| snap-to-file | 1 | search + read | 1/2 |
| snap-to-effect | 1 | search + edit | 1/2 |
| `multi_read` 5 files | 1 | 5 `fs.read` | 1/5 |
| `multi_edit` 3 files | 1 | 3 `fs.edit` | 1/3 |

`multi_read` kernel time on this fixture: **506 µs**, one physical pass.

## How to rerun

1. Temp root only. Do not measure `~/AI/ZeroStack`.
2. One `zero_execute` per row. Do not hide N searches inside a plan and call that one snap.
3. Record the full MCP envelope, including `receipt` when `spilled: true`.
4. Exact column stays empty unless TokenZero `accounting` is present.
5. Bump this file to `v2` when FS grows Exact certification. Do not backfill estimates into v1.

## Residual

FS Exact is still omitted. Next iteration of the bench should either certify HIT windows through TokenZero or keep the two-step (search HIT + `token.read` range) and quote the Exact window number, as v1 does.
