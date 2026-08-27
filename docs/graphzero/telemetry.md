# Shareable usage telemetry (default off)

GraphZero keeps **local operational counters** separate from **shareable usage telemetry**.

## Local operational counters (always local)

These stay on the machine and are not the shareable usage-telemetry permission:

- CodeMode `telemetry_ref` / `CodeModeTelemetry` (logical/physical ops, wall time, refs for that execution)
- Query response `accounting` (`visible_tokens`, `full_tokens`, `savings_tokens`)
- Local aggregate file `.graphzero/telemetry/local_counters.json` (`graphzero.local_counters.v1`: `raw_tokens`, `saved_tokens` only)

Local counters never include source content, paths, graph facts, queries, refs, commands, or user/machine identifiers in the shareable path.

## Shareable usage telemetry (default off)

Usage telemetry is **off by default**. There is **no exporter**: opting in or inspecting never uploads or opens a network path. Inspection always reports `exporter=none`.

When explicitly opted in, GraphZero may append **only** closed JSONL records under `.graphzero/telemetry/usage-telemetry.jsonl`:

```json
{"execution_path":"mcp"|"codemode","raw_tokens":N,"spent_tokens":M}
```

with `spent_tokens <= raw_tokens`. No prompts, paths, refs, tool names, errors, durations, timestamps, or other identifiers.

This matches TokenZero family parity (`tokenzero-f409`).

### Opt-in surfaces

| Surface | How |
| --- | --- |
| CLI | `graphzero telemetry inspect --telemetry` |
| CLI opt-out | `graphzero telemetry inspect --no-telemetry` (wins over opt-in) |
| Config | `.graphzero/config.json` → `"telemetry": true` or `false` |
| Environment | `GRAPHZERO_TELEMETRY=1` / `on` / `true` / `yes` (case-insensitive) |

### Precedence

1. `--no-telemetry` → off
2. `--telemetry` → on
3. config `telemetry` boolean (if present)
4. `GRAPHZERO_TELEMETRY` (only `1`/`on`/`true`/`yes` enable; anything else is off)
5. default → off

### Inspect

```bash
graphzero telemetry inspect --json --repo .
graphzero telemetry inspect --telemetry --repo .
```

The inspect envelope contains `enabled`, `exporter` (always `"none"`), and `records` (allowlisted usage rows only when enabled).

### Exporter

`export_shareable_telemetry` always returns no outbound payload. Enabling permission does not create an upload path.
