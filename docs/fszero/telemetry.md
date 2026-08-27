# Telemetry (shareable vs local)

FSZero keeps two separate telemetry surfaces.

## Local operational counters (always local)

CodeMode responses include a `telemetry` object (and `codemode/telemetry` /
execution refs) with operational fields such as logical/physical ops, cache
hits, wall time, and optional local root diagnosis. That envelope is
**response-local protocol metadata** for the current session. It is not
uploaded and is not the shareable telemetry path.

Every CodeMode response reports per-execution measurement evidence:

- `bytes_materialized`: the larger observed UTF-8 byte count of operation/expand
  payloads or the returned result (avoiding inline double counting), reset for
  each execution.
- `raw_token_estimate` and `visible_token_estimate`: estimates labeled by
  `token_estimator: "estimator:utf8-bytes-div-4"`; they never claim to be a
  model tokenizer.
- `measurement_coverage`: measured/unmeasured status, execution/wire stage,
  covered and total operation counts, operation kinds, and whether bytes are
  observed or tokens estimated.
- `visible_bytes`: the final wire acknowledgement size when emitted through
  the CodeMode tool envelope.

The v2 structured result repeats this telemetry object so an aggregator can
compute measured visible-vs-materialized savings without a hypothetical
baseline. Zero bytes remain a measured zero; unavailable evidence must be
reported as unmeasured, never coerced to 0% savings.

Doctor / store-health reports are likewise local diagnostics.

## Shareable telemetry (default off)

Shareable telemetry is an explicit opt-in permission for inspecting an
allowlisted aggregate payload. FSZero has **no exporter**: enabling permission
or running inspect never uploads, phones home, or writes an outbound network
payload (`exporter` is always `"none"`).

### Opt-in surfaces (opt-out wins)

| Source | How |
| :-- | :-- |
| CLI | `fszero telemetry inspect --telemetry` |
| CLI opt-out | `fszero telemetry inspect --no-telemetry` (wins over all other sources) |
| Config | `{ "telemetry": true }` in `<store>/config.json` (`.zerostack/config.json` or `.fszero/config.json`) |
| Env | `FSZERO_TELEMETRY=1` / `on` / `true` / `yes` |

Precedence: CLI `--no-telemetry` > CLI `--telemetry` > config boolean >
environment > **default off**. Missing, invalid, or non-boolean config values
defer to env (then off).

### Exact allowlisted payload

```json
{
  "enabled": true,
  "exporter": "none",
  "payload": {
    "schema": "fszero.telemetry",
    "version": "<CARGO_PKG_VERSION>",
    "raw_tokens": 0,
    "saved_tokens": 0
  }
}
```

Only those four payload fields are allowed. Never included: content, paths,
queries, refs, commands, project/user/machine identifiers, IP-derived
identity, or hidden tracking fields.

Local aggregates that feed the dry-run live under
`<store>/telemetry/local_counters.json` with schema
`fszero.local_counters` and are **not** the shareable envelope.

### Inspect / dry-run

```bash
fszero telemetry inspect [--root PATH]
fszero telemetry dry-run --telemetry [--root PATH]
```

Prints the exact JSON above. `inspect` and `dry-run` are aliases.


## Opt-in usage telemetry (token accounting only)

Separate from the aggregate dry-run payload above, FSZero can append closed
JSONL usage records when explicitly opted in via the same permission surfaces
(`FSZERO_TELEMETRY`, config `telemetry`, CLI `--telemetry`). **Default off** —
no `usage-telemetry.jsonl` file is created unless opted in.

When enabled, MCP and CodeMode may append only:

```json
{"execution_path":"mcp"|"codemode","raw_tokens":N,"spent_tokens":M}
```

with `spent_tokens <= raw_tokens`. Never prompts, paths, refs, tool names,
errors, durations, timestamps, or identifiers. Records live beside the store
db as `usage-telemetry.jsonl`. Exporter remains `none` (no upload).

In-session CodeMode ack fields (`codemode/telemetry` logical/physical ops,
etc.) stay response-local protocol metadata and are not this durable usage
path.


## KPI pin: accounting vs quality

`raw_tokens` and `saved_tokens` (local counters and the allowlisted inspect
payload) are **token accounting estimates**, not product quality scores.

- Do not optimize `saved_tokens` as a success KPI, bakeoff winner, or agent
  self-score. Inflating counters must not change dispatch, ranking, admission,
  or gates.
- Decision quality uses task outcomes and (when claims are made) receipt-backed
  metrics with **labeled denominators** -- never unlabeled savings percentages.
- Normative write-up: `docs/design/objective-hygiene.md` (bead `fszero-w2g.52` /
  `fszero-w2g.30`).
