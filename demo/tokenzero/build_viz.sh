#!/usr/bin/env bash
# Render demo/demo_results.json into a self-contained demo/demo_viz.html.
# Bash port of build_viz.ps1: same derivations (log10 scale, savings rounding,
# byte-exact recovery badge, MCP-dedup callout, gap-report section).
#
# Usage:
#   ./demo/build_viz.sh [--results PATH] [--gap-report PATH] [--out PATH] [--open]
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_PATH=""
GAP_REPORT_PATH=""
OUT_PATH=""
OPEN=0

usage() {
    cat <<'USAGE'
Usage: build_viz.sh [options]

  -r, --results PATH     Path to demo_results.json (default: alongside this script)
  -g, --gap-report PATH  Path to gap_report.json   (default: alongside this script)
  -o, --out PATH         Output HTML path          (default: demo/demo_viz.html)
      --open             Open the rendered page in the default browser
  -h, --help             Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--results)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            RESULTS_PATH="$2"; shift 2 ;;
        -g|--gap-report)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            GAP_REPORT_PATH="$2"; shift 2 ;;
        -o|--out)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            OUT_PATH="$2"; shift 2 ;;
        --open)
            OPEN=1; shift ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ -z "$RESULTS_PATH" ]]    && RESULTS_PATH="$DEMO_DIR/demo_results.json"
[[ -z "$GAP_REPORT_PATH" ]] && GAP_REPORT_PATH="$DEMO_DIR/gap_report.json"
[[ -z "$OUT_PATH" ]]        && OUT_PATH="$DEMO_DIR/demo_viz.html"

if [[ ! -f "$RESULTS_PATH" ]]; then
    echo "demo_results.json not found at $RESULTS_PATH. Run ./demo/run_demo.ps1 first." >&2
    exit 1
fi

python3 - "$RESULTS_PATH" "$GAP_REPORT_PATH" "$OUT_PATH" <<'PYEOF'
import datetime
import json
import math
import os
import re
import sys

results_path, gap_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]


def enc(value):
    """Mirror Encode-Html: & < > " ' replaced in that order."""
    if value is None:
        value = ""
    s = str(value)
    return (s.replace("&", "&amp;")
             .replace("<", "&lt;")
             .replace(">", "&gt;")
             .replace('"', "&quot;")
             .replace("'", "&#39;"))


def fmt_number(n):
    # Mirror Format-Number: '{0:N0}' when N >= 1, else '0'.
    n = float(n)
    if n >= 1:
        return f"{n:,.0f}"
    return "0"


def num(v, default=0.0):
    if v is None:
        return default
    return float(v)


with open(results_path, encoding="utf-8-sig") as fh:
    data = json.load(fh)

gap = None
if os.path.isfile(gap_path):
    with open(gap_path, encoding="utf-8-sig") as fh:
        gap = json.load(fh)

workloads = data.get("workloads") or []

# --- Scale (log10, so 11 and 79,424 both render) ---------------------------
max_raw = max((num(w.get("raw_tokens")) for w in workloads), default=None)
if not max_raw or max_raw <= 0:
    max_raw = 1
log_max = math.log10(max(max_raw, 10))


def to_log_pct(n):
    n = float(n)
    if n <= 0:
        return 0.0
    lv = math.log10(max(n, 1.0))
    return max(0.5, (lv / log_max) * 100.0)


def first_row(pred):
    for w in workloads:
        if pred(w):
            return w
    return None


# --- Derive the recovery badge from the round-trip row ---------------------
# (ps1 -match is case-insensitive by default; the ps1 also adds (?i).)
recovery_row = first_row(lambda w: re.search(r"byte-exact", str(w.get("note") or ""), re.I))
recovery_ok = recovery_row is not None

# --- Derive the dedup-gap callout (first vs second read of the same file) --
dedup_callout = None
first_read = first_row(lambda w: re.match(r"large read", str(w.get("workload") or ""), re.I))
second_read = first_row(lambda w: re.search(r"re-read|dedup", str(w.get("workload") or ""), re.I))
if first_read is not None and second_read is not None and num(second_read.get("raw_tokens")) > 0:
    # README claims ~99.7% on this row. Flag anything within 1pp of the first
    # read's savings (i.e. no meaningful drop on the repeat).
    # [math]::Round(x, 1) defaults to banker's rounding, matching round().
    delta = round(num(first_read.get("savings_pct")) - num(second_read.get("savings_pct")), 1)
    if abs(delta) < 1.0:
        dedup_callout = (
            "Second {0}-token read returned {1} visible tokens (savings {2}%) -- "
            "first read was {3} ({4}%). README claims ~99.7%; observed dedup did "
            "not fire. See gap finding #9."
        ).format(
            second_read.get("raw_tokens"),
            second_read.get("visible_tokens"),
            second_read.get("savings_pct"),
            first_read.get("visible_tokens"),
            first_read.get("savings_pct"),
        )

# --- Build the per-row markup ----------------------------------------------
rows = []
for w in workloads:
    name = enc(w.get("workload"))
    note = enc(w.get("note"))

    raw_n = num(w.get("raw_tokens"))
    vis_n = num(w.get("visible_tokens"))
    raw_pct = to_log_pct(raw_n)
    vis_pct = to_log_pct(vis_n)

    savings = round(100.0 * (raw_n - vis_n) / max(raw_n, 1), 1) if raw_n > 0 else 0
    is_passthrough = raw_n > 0 and raw_n == vis_n
    is_recovery = raw_n <= 0 and vis_n <= 0

    badge_class = "savings"
    badge_text = f"{savings:,.1f}% saved"
    if is_passthrough:
        badge_class = "passthrough"
        badge_text = "pass-through"
    if is_recovery:
        badge_class = "recovery"
        badge_text = note  # ps1 quirk: badge is encoded again below (double-encode)

    rows.append('<article class="row">')
    rows.append('  <header>')
    rows.append(f'    <h3>{name}</h3>')
    rows.append(f'    <span class="badge {badge_class}">{enc(badge_text)}</span>')
    rows.append('  </header>')

    if not is_recovery:
        rows.append('  <div class="bars">')
        rows.append('    <div class="bar-row">')
        rows.append('      <span class="bar-label">raw</span>')
        rows.append(f'      <div class="bar raw" style="width:{raw_pct:.2f}%"></div>')
        rows.append(f'      <span class="bar-value">{fmt_number(raw_n)}</span>')
        rows.append('    </div>')
        rows.append('    <div class="bar-row">')
        rows.append('      <span class="bar-label">visible</span>')
        rows.append(f'      <div class="bar visible" style="width:{vis_pct:.2f}%"></div>')
        rows.append(f'      <span class="bar-value">{fmt_number(vis_n)}</span>')
        rows.append('    </div>')
        rows.append('  </div>')
    if note and not is_recovery:
        rows.append(f'  <p class="note">{note}</p>')
    rows.append('</article>')
rows_html = "\n".join(rows) + "\n"

# --- Donut SVG for the totals ----------------------------------------------
totals = data.get("totals") or {}
total_raw = num(totals.get("raw_tokens"))
total_visible = num(totals.get("visible_tokens"))
total_pct = num(totals.get("savings_pct"))
donut_circum = 2 * math.pi * 90
dash_saved = (total_pct / 100.0) * donut_circum
dash_remain = donut_circum - dash_saved

donut_saved_dash = f"{dash_saved:.2f} {dash_remain:.2f}"
total_raw_fmt = fmt_number(total_raw)
total_vis_fmt = fmt_number(total_visible)
total_pct_fmt = f"{total_pct:,.1f}"
now = datetime.datetime.now().astimezone()
z = now.strftime("%z")
generated_at = now.strftime("%Y-%m-%d %H:%M ") + (f"{z[:3]}:{z[3:]}" if z else "+00:00")
tz_version_html = enc(data.get("tokenzero_version"))
repo_html = enc(data.get("repo"))

jump_to_bugs = ""
if gap is not None:
    summary = gap.get("summary") or {}
    by_sev = summary.get("by_severity") or {}
    sev_bits = []
    for sev in ("critical", "high", "medium", "low"):
        n = int(num(by_sev.get(sev)))
        if n > 0:
            sev_bits.append(f"{n} {sev}")
    sev_text = " / ".join(sev_bits)
    jump_to_bugs = ('<a class="hero-badge fail" href="#gaps" style="text-decoration:none">'
                    f'{summary.get("total")} bugs flagged ({sev_text}) &rarr;</a>')

if recovery_ok:
    recovery_badge_html = '<span class="hero-badge ok">byte-exact recovery: PASS</span>'
else:
    recovery_badge_html = '<span class="hero-badge fail">byte-exact recovery: not run</span>'
dedup_callout_html = ""
if dedup_callout:
    dedup_callout_html = ('<div class="callout warn"><strong>Observed gap (MCP session dedup).</strong> '
                          f'{enc(dedup_callout)}</div>')

# --- Gap report section -----------------------------------------------------
gap_html = ""
if gap is not None:
    sb = []
    summary = gap.get("summary") or {}
    by = summary.get("by_severity") or {}
    src_list = ", ".join(enc(s) for s in (summary.get("sources") or []))

    sb.append('<section class="gaps" id="gaps">')
    sb.append('  <header>')
    sb.append('    <h2>Bugs flagged for the developer</h2>')
    sb.append(f'    <span class="sub">{summary.get("total")} findings &middot; sources: {src_list}</span>')
    sb.append('  </header>')

    sb.append('  <div class="sev-summary">')
    for sev in ("critical", "high", "medium", "low"):
        n = int(num(by.get(sev)))
        if n <= 0:
            continue
        sb.append(f'    <span class="sev-count sev-{sev}"><span class="n">{n}</span>{sev}</span>')
    sb.append('  </div>')

    for fnd in (gap.get("findings") or []):
        sev = str(fnd.get("severity") or "")
        rank = int(num(fnd.get("rank")))
        title = enc(fnd.get("title"))
        impact = enc(fnd.get("impact"))
        evidence = enc(fnd.get("evidence"))
        fix = enc(fnd.get("fix"))
        source = enc(fnd.get("source"))
        id_attr = enc(fnd.get("id"))

        sb.append(f'  <details class="finding" data-sev="{sev}" id="bug-{id_attr}">')
        sb.append('    <summary>')
        sb.append(f'      <span class="rank">#{rank}</span>')
        sb.append(f'      <span class="sev sev-{sev}">{sev}</span>')
        sb.append(f'      <span class="ttl">{title}</span>')
        sb.append('    </summary>')

        sb.append('    <div class="finding-body">')
        sb.append('      <div><div class="row-label">Impact</div>' + impact + '</div>')
        if fnd.get("claim_contradicted"):
            cc = enc(fnd.get("claim_contradicted"))
            sb.append(f'      <div class="contradicts"><strong>Contradicts:</strong> {cc}</div>')
        sb.append('      <div><div class="row-label">Evidence</div><code>' + evidence + '</code></div>')
        if fnd.get("repro"):
            repro = enc(fnd.get("repro"))
            sb.append('      <div><div class="row-label">Repro</div>' + repro + '</div>')
        sb.append('      <div><div class="row-label">Fix sketch</div>' + fix + '</div>')
        sb.append(f'      <div><div class="row-label">Source</div>{source}</div>')
        sb.append('    </div>')
        sb.append('  </details>')

    sb.append('</section>')
    gap_html = "\n".join(sb) + "\n"

# --- Assemble the HTML ------------------------------------------------------
template = r"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>TokenZero demo &mdash; visualisation</title>
<style>
  :root {
    --bg: #0d1117;
    --bg-elev: #161b22;
    --bg-row: #1c222b;
    --fg: #e6edf3;
    --fg-dim: #8b949e;
    --accent: #3fb950;
    --accent-dim: #265d36;
    --raw: #f97583;
    --visible: #3fb950;
    --warn: #d29922;
    --warn-bg: #2a210a;
    --border: #30363d;
    --link: #58a6ff;
  }
  @media (prefers-color-scheme: light) {
    :root {
      --bg: #ffffff;
      --bg-elev: #f6f8fa;
      --bg-row: #f0f3f6;
      --fg: #1f2328;
      --fg-dim: #57606a;
      --accent: #1f883d;
      --accent-dim: #cfe7d4;
      --raw: #cf222e;
      --visible: #1f883d;
      --warn: #9a6700;
      --warn-bg: #fff8c5;
      --border: #d0d7de;
      --link: #0969da;
    }
  }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    padding: 32px 24px 64px;
    background: var(--bg);
    color: var(--fg);
    font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
  }
  .wrap { max-width: 1040px; margin: 0 auto; }
  header.page h1 {
    margin: 0 0 4px;
    font-size: 28px;
    letter-spacing: -0.01em;
  }
  header.page p.sub {
    margin: 0;
    color: var(--fg-dim);
    font-size: 13px;
  }
  .hero {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: 32px;
    align-items: center;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 24px 28px;
    margin: 24px 0 16px;
  }
  .hero .donut { position: relative; width: 200px; height: 200px; }
  .hero .donut .pct {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    flex-direction: column;
    font-weight: 600;
  }
  .hero .donut .pct .big   { font-size: 36px; letter-spacing: -0.02em; }
  .hero .donut .pct .small { font-size: 11px; color: var(--fg-dim); text-transform: uppercase; letter-spacing: 0.08em; }
  .hero .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; }
  .hero .stats div { background: var(--bg-row); border-radius: 8px; padding: 14px; }
  .hero .stats div .num { font-size: 20px; font-weight: 600; letter-spacing: -0.01em; }
  .hero .stats div .lbl { color: var(--fg-dim); font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; margin-top: 2px; }
  .hero-badges { grid-column: 1 / -1; display: flex; gap: 8px; flex-wrap: wrap; margin-top: 8px; }
  .hero-badge { font-size: 12px; padding: 4px 10px; border-radius: 999px; font-weight: 600; }
  .hero-badge.ok   { background: var(--accent-dim); color: var(--accent); }
  .hero-badge.fail { background: var(--warn-bg);    color: var(--warn); }
  .callout {
    border-radius: 10px;
    padding: 12px 16px;
    margin: 12px 0;
    font-size: 13px;
    line-height: 1.55;
  }
  .callout.warn { background: var(--warn-bg); border: 1px solid var(--warn); color: var(--fg); }
  section.scenarios { display: grid; gap: 12px; margin-top: 24px; }
  article.row {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 18px;
  }
  article.row header {
    display: flex; align-items: center; justify-content: space-between; gap: 12px;
    margin-bottom: 10px;
  }
  article.row h3 { margin: 0; font-size: 14px; font-weight: 600; }
  .badge { font-size: 11px; padding: 3px 9px; border-radius: 999px; font-weight: 600; white-space: nowrap; }
  .badge.savings    { background: var(--accent-dim); color: var(--accent); }
  .badge.passthrough{ background: var(--bg-row);     color: var(--fg-dim); }
  .badge.recovery   { background: var(--accent-dim); color: var(--accent); }
  .bars { display: grid; gap: 6px; margin-top: 6px; }
  .bar-row {
    display: grid;
    grid-template-columns: 56px 1fr 90px;
    align-items: center; gap: 10px;
  }
  .bar-label { font-size: 11px; color: var(--fg-dim); text-transform: uppercase; letter-spacing: 0.05em; }
  .bar { height: 14px; border-radius: 4px; min-width: 2px; }
  .bar.raw     { background: var(--raw); }
  .bar.visible { background: var(--visible); }
  .bar-value   { font-size: 12px; color: var(--fg-dim); text-align: right; font-variant-numeric: tabular-nums; }
  .note { margin: 8px 0 0; font-size: 12px; color: var(--fg-dim); }
  footer.page { margin-top: 32px; font-size: 11px; color: var(--fg-dim); }
  footer.page code { font-size: 11px; }
  a { color: var(--link); }

  /* --- Gap report section ------------------------------------------------ */
  section.gaps { margin-top: 40px; }
  section.gaps > header { display: flex; align-items: baseline; gap: 12px; margin-bottom: 16px; }
  section.gaps > header h2 { margin: 0; font-size: 22px; letter-spacing: -0.01em; }
  section.gaps > header .sub { color: var(--fg-dim); font-size: 13px; }
  .sev-summary { display: flex; gap: 8px; flex-wrap: wrap; margin: 0 0 18px; }
  .sev-count {
    display: inline-flex; align-items: baseline; gap: 6px;
    padding: 4px 10px; border-radius: 999px; font-size: 12px; font-weight: 600;
    border: 1px solid var(--border);
  }
  .sev-count .n { font-size: 13px; font-weight: 700; }
  .sev-critical { background: rgba(248,81,73,0.12);  color: #f85149; border-color: rgba(248,81,73,0.4); }
  .sev-high     { background: rgba(210,153,34,0.12); color: var(--warn); border-color: rgba(210,153,34,0.4); }
  .sev-medium   { background: rgba(88,166,255,0.10); color: var(--link); border-color: rgba(88,166,255,0.35); }
  .sev-low      { background: rgba(139,148,158,0.10); color: var(--fg-dim); border-color: var(--border); }
  details.finding {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-left: 4px solid var(--border);
    border-radius: 8px;
    padding: 10px 14px;
    margin-bottom: 8px;
  }
  details.finding[data-sev="critical"] { border-left-color: #f85149; }
  details.finding[data-sev="high"]     { border-left-color: var(--warn); }
  details.finding[data-sev="medium"]   { border-left-color: var(--link); }
  details.finding[data-sev="low"]      { border-left-color: var(--fg-dim); }
  details.finding > summary {
    cursor: pointer; list-style: none;
    display: grid;
    grid-template-columns: 36px 90px 1fr;
    gap: 10px; align-items: baseline;
  }
  details.finding > summary::-webkit-details-marker { display: none; }
  details.finding > summary .rank { color: var(--fg-dim); font-size: 12px; font-variant-numeric: tabular-nums; }
  details.finding > summary .sev  { font-size: 10px; padding: 2px 8px; border-radius: 999px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.06em; text-align: center; }
  details.finding > summary .ttl  { font-size: 14px; font-weight: 600; }
  details.finding[open] > summary .ttl { color: var(--fg); }
  .finding-body { margin-top: 12px; padding-top: 12px; border-top: 1px dashed var(--border); display: grid; gap: 10px; font-size: 13px; }
  .finding-body .row-label { color: var(--fg-dim); font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 2px; }
  .finding-body code,
  .finding-body pre { background: var(--bg-row); padding: 1px 6px; border-radius: 4px; font-size: 12px; }
  .finding-body pre { padding: 10px 12px; overflow-x: auto; white-space: pre-wrap; }
  .finding-body .contradicts {
    background: var(--warn-bg); color: var(--warn);
    border-left: 3px solid var(--warn);
    padding: 6px 10px; font-size: 12px; border-radius: 0 4px 4px 0;
  }
</style>
</head>
<body>
<div class="wrap">

<header class="page">
  <h1>TokenZero demo &mdash; visualisation</h1>
  <p class="sub">__TZ_VERSION__ &middot; generated __GENERATED_AT__ &middot; <code>__REPO__</code></p>
</header>

<section class="hero">
  <div class="donut">
    <svg viewBox="0 0 200 200" width="200" height="200" aria-hidden="true">
      <circle cx="100" cy="100" r="90" stroke="var(--bg-row)" stroke-width="18" fill="none"></circle>
      <circle cx="100" cy="100" r="90" stroke="var(--accent)" stroke-width="18" fill="none"
              stroke-dasharray="__DONUT_DASH__" stroke-dashoffset="0"
              transform="rotate(-90 100 100)" stroke-linecap="round"></circle>
    </svg>
    <div class="pct">
      <div class="big">__TOTAL_PCT_FMT__%</div>
      <div class="small">tokens hidden</div>
    </div>
  </div>
  <div class="stats">
    <div><div class="num">__TOTAL_RAW_FMT__</div><div class="lbl">Raw tokens (across runs)</div></div>
    <div><div class="num">__TOTAL_VIS_FMT__</div><div class="lbl">Visible to agent</div></div>
    <div><div class="num">__TOTAL_PCT_FMT__%</div><div class="lbl">Recovery-aware savings</div></div>
    <div class="hero-badges">
      __RECOVERY_BADGE__
      <span class="hero-badge ok">isolated cache &middot; same tokenizer both sides</span>
      __JUMP_TO_BUGS__
    </div>
  </div>
</section>

__DEDUP_CALLOUT__

<section class="scenarios">
__ROWS__
</section>

__GAP_HTML__

<footer class="page">
  Bars use a log-base-10 scale so 11-token and 79,000-token rows are both
  legible. Source: <code>demo/demo_results.json</code> + <code>demo/gap_report.json</code>.
  Regenerate with <code>pwsh -File demo\run_demo.ps1</code> then
  <code>pwsh -File demo\build_viz.ps1 -Open</code>.
</footer>

</div>
</body>
</html>
"""

html_out = (template
            .replace("__TZ_VERSION__", tz_version_html)
            .replace("__GENERATED_AT__", generated_at)
            .replace("__REPO__", repo_html)
            .replace("__DONUT_DASH__", donut_saved_dash)
            .replace("__TOTAL_PCT_FMT__", total_pct_fmt)
            .replace("__TOTAL_RAW_FMT__", total_raw_fmt)
            .replace("__TOTAL_VIS_FMT__", total_vis_fmt)
            .replace("__RECOVERY_BADGE__", recovery_badge_html)
            .replace("__JUMP_TO_BUGS__", jump_to_bugs)
            .replace("__DEDUP_CALLOUT__", dedup_callout_html)
            .replace("__ROWS__", rows_html)
            .replace("__GAP_HTML__", gap_html))

with open(out_path, "w", encoding="utf-8", newline="\n") as fh:
    fh.write(html_out)
PYEOF

echo "Wrote: $OUT_PATH"

if [[ "$OPEN" -eq 1 ]]; then
    case "$(uname -s)" in
        Darwin)
            open "$OUT_PATH" ;;
        *)
            xdg-open "$OUT_PATH" >/dev/null 2>&1 & ;;
    esac
fi
