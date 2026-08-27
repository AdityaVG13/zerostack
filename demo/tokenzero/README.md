# TokenZero demo

A self-contained, byte-honest demo that walks an AI agent's "day in the life"
through TokenZero and reports how many tokens it hid versus how many it
actually fed back to the agent.

Every file in this directory is explained below. There is no npm, no Remotion,
and no recorded result JSON here: the scripts PRODUCE the numbers and the
HTML; generated artifacts are gitignored (see `.gitignore`).

## Files

| File | What it is |
| :-- | :-- |
| `run_demo.sh` / `run_demo.ps1` | Main demo driver. Seven scenarios against this repo's own source tree, writes `demo_results.json`, renders `demo_viz.html` (via `build_viz`). macOS/Linux run the `.sh`, Windows the `.ps1`; both produce the same table and the same JSON schema. |
| `run_agent_demo.sh` / `run_agent_demo.ps1` | Agent A/B demo: drives the GitHub `copilot` CLI with and without TokenZero across replicates, writes `agent_results.json`, renders `agent_viz.html` (via `build_agent_viz`). Requires `copilot` on PATH (or `--copilot-path`). |
| `build_viz.sh` / `build_viz.ps1` | Re-render `demo_viz.html` from an existing `demo_results.json` without re-running the demo. |
| `build_agent_viz.sh` / `build_agent_viz.ps1` | Re-render `agent_viz.html` from an existing `agent_results.json`. |
| `.gitignore` | Ignores everything the scripts generate: `.tokenzero-bin/`, `.cache/`, `*_results.json`, `gap_report.json`, `composition_benchmark.json`, `*_viz.html`. |

The MCP server config that `run_agent_demo` writes is generated inline; its
canonical shape is documented in `docs/install.md` ("Manual MCP config").

## What it shows

Seven real scenarios run against this repository's own source tree:

| # | Scenario | What you should see |
| -: | :-- | :-- |
| 1 | small read (`crates/tokenzero-cli/Cargo.toml`)        | pass-through; capsule never costs more than raw |
| 2 | large read (`crates/tokenzero-mcp/src/lib.rs`)    | heavy savings + a `tz://blob/...` ref |
| 3 | re-read same large file                            | seen-set dedup: visible tokens drop sharply against the same cache |
| 4 | grep `fn ` across `crates\`                       | recoverable hit set; raw is the full `rg` dump |
| 5 | `expand <ref>` of the large-read blob              | **byte-exact** round-trip check — the script fails if a single byte differs |
| 6 | `recall 'fn main'`                                | re-find content already in cache, no filesystem rescan |
| 7 | `run -- git --version` (or `cmd /c ver`)          | shell stream captured behind a ref |

The driver counts raw tokens by piping the raw output through
`tokenzero ingest --stdin` (TokenZero's own tokenizer) and reads
`accounting.visible_tokens` from each call's JSON. Same tokenizer on both
sides → the savings number is fair.

## Visualization

The driver writes `demo_results.json` and then renders a fully self-contained
`demo_viz.html` (inline CSS + inline SVG, no CDN, no JS). Pass `--open-viz`
(`-OpenViz` on Windows) to have it pop in your default browser at the end:

```bash
# macOS / Linux
./demo/run_demo.sh --open-viz
```

```powershell
# Windows (PowerShell 7+ recommended; Windows PowerShell 5.1 also works)
pwsh -File .\demo\run_demo.ps1 -OpenViz
```

The page has two sections:

**Performance** — donut for the recovery-aware total savings + raw / visible / savings
stats, plus per-scenario panels with side-by-side raw vs visible bars
(log-scaled so the 11-token shell row and the 79,000-token grep row are both
legible). A `byte-exact recovery: PASS` badge is derived from scenario 5,
and a warning callout fires if the dedup row's savings does not improve on
the first-read row (the empirical gap I observed against v1.0.1).

**Bugs flagged for the developer** — ranked CRITICAL → HIGH → MEDIUM → LOW,
from `demo/gap_report.json`. Each finding is an expandable card with impact,
evidence (file:line citations), repro (where applicable), fix sketch, and
the review pass that surfaced it. The header includes a `N bugs flagged
(critical/high/medium/low)` button that jumps to the section.

Re-render without re-running the demo:

```bash
# macOS / Linux
./demo/build_viz.sh --open
# Windows
pwsh -File .\demo\build_viz.ps1 -Open
```

Re-render against a custom gap report:

```bash
# macOS / Linux
./demo/build_viz.sh --gap-report ./my_gaps.json --open
# Windows
pwsh -File .\demo\build_viz.ps1 -GapReportPath .\my_gaps.json -Open
```

Skip the viz entirely (just write `demo_results.json`):

```bash
# macOS / Linux
./demo/run_demo.sh --no-viz
# Windows
pwsh -File .\demo\run_demo.ps1 -NoViz
```

## Run it

Pick the script for your OS: `run_demo.sh` (macOS/Linux, needs `bash`, `curl`,
and `jq` or `python3`) or `run_demo.ps1` (Windows, PowerShell 5.1+).

```bash
# macOS / Linux, from the repo root
./demo/run_demo.sh
```

```powershell
# Windows, from the repo root
pwsh -File .\demo\run_demo.ps1
```

The script will:

1. Use `tokenzero` from `PATH` if present;
2. else reuse `demo/.tokenzero-bin/tokenzero` (or `tokenzero.exe` on Windows)
   if it's already there;
3. else download the `v1.0.1` GitHub Release asset for the current OS/CPU
   (`x86_64-pc-windows-msvc.zip`, `x86_64-unknown-linux-gnu.tar.gz`,
   `aarch64-apple-darwin.tar.gz`, or `x86_64-apple-darwin.tar.gz`), verify
   the published SHA256, and extract it into `demo/.tokenzero-bin/`.

All runtime state lives under `demo/.cache/` (deleted at the top of every
run) so the demo never touches your real TokenZero cache or telemetry.

### Options

```text
--binary-path <path>  (-BinaryPath)   Use a specific tokenzero binary (skip PATH/download)
--release-tag <vX.Y.Z> (-ReleaseTag)  Release to download if no binary is found (default: v1.0.1)
--skip-download       (-SkipDownload) Fail instead of downloading when no binary is found
```

The `.sh` also honors `TOKENZERO_BINARY_PATH` and `TOKENZERO_RELEASE_TAG`.

## What the output looks like

You'll get a Markdown-friendly table on stdout, plus a machine-readable
`demo/demo_results.json` (`demo\demo_results.json` on Windows) you can diff
between runs or post-process:

```json
{
  "tokenzero_version": "tokenzero 1.0.1",
  "workloads": [
    { "workload": "large read (...)", "raw_tokens": 16929, "visible_tokens": 150, "savings_pct": 99.1 },
    ...
  ],
  "totals": { "raw_tokens": ..., "visible_tokens": ..., "savings_pct": ... }
}
```

## Reading the numbers honestly

TokenZero's claim is **Recovery-Aware Context Compression**: tokens hidden
behind a `tz://` ref that the agent later has to `expand` *do not count* as
savings. That's exactly why scenario 5 round-trips the large-read ref and
fails the demo if recovery isn't byte-exact — a "saving" you can't actually
recover wouldn't be a saving.

## Extending the demo

Each scenario is one block in `run_demo.sh` / `run_demo.ps1` that ends in an
`add_row` / `Add-Row` call. Copy one, point it at another path / command /
query, and it will show up in the summary and the JSON automatically.
