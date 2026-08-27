#requires -Version 5.1
<#
.SYNOPSIS
    Emit a self-contained, auto-refreshing live viewer for agent runs.
.DESCRIPTION
    Reads demo\agent_results.json on a timer (in the browser) and renders
    a live table + summary cards. Single static HTML page; the only JS is
    a tiny fetch+render loop. No CDN, no frameworks.
#>
[CmdletBinding()]
param(
    [string] $OutPath,
    [string] $DataPath = 'agent_results.json',
    [int]    $RefreshMs = 1500,
    [int]    $Port = 8765,
    [switch] $Open
)
$ErrorActionPreference = 'Stop'
$DemoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $OutPath) { $OutPath = Join-Path $DemoDir 'agent_viz.html' }

$html = @"
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>TokenZero &mdash; live agent runs</title>
<style>
:root {
  --bg:#0d1117; --bg-elev:#161b22; --bg-row:#1c222b; --fg:#e6edf3; --fg-dim:#8b949e;
  --accent:#3fb950; --accent-dim:#265d36; --raw:#f97583; --visible:#3fb950;
  --warn:#d29922; --warn-bg:#2a210a; --border:#30363d; --link:#58a6ff;
  --danger:#f85149;
}
@media (prefers-color-scheme: light) {
  :root { --bg:#fff; --bg-elev:#f6f8fa; --bg-row:#f0f3f6; --fg:#1f2328; --fg-dim:#57606a;
          --accent:#1f883d; --accent-dim:#cfe7d4; --raw:#cf222e; --visible:#1f883d;
          --warn:#9a6700; --warn-bg:#fff8c5; --border:#d0d7de; --link:#0969da; --danger:#cf222e; }
}
* { box-sizing:border-box; }
body { margin:0; padding:24px 24px 64px; background:var(--bg); color:var(--fg);
       font:15px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif; }
.wrap { max-width:1180px; margin:0 auto; }
header.page h1 { margin:0 0 4px; font-size:24px; letter-spacing:-0.01em; }
header.page p.sub { margin:0; color:var(--fg-dim); font-size:13px; }
.live-dot { display:inline-block; width:9px; height:9px; border-radius:50%; background:var(--accent); margin-right:6px;
            animation:pulse 1.6s ease-in-out infinite; vertical-align:middle; }
@keyframes pulse { 0%,100%{opacity:1; transform:scale(1)} 50%{opacity:.35; transform:scale(.85)} }
.cards { display:grid; grid-template-columns:repeat(auto-fit, minmax(190px,1fr)); gap:12px; margin:18px 0; }
.card { background:var(--bg-elev); border:1px solid var(--border); border-radius:10px; padding:14px 16px; }
.card .lbl { color:var(--fg-dim); font-size:11px; text-transform:uppercase; letter-spacing:.06em; }
.card .num { font-size:22px; font-weight:600; letter-spacing:-0.01em; margin-top:4px; }
.card .sub { color:var(--fg-dim); font-size:11px; margin-top:2px; }

.split { display:grid; grid-template-columns:1fr 1fr; gap:16px; margin:18px 0 24px; }
.split .panel { background:var(--bg-elev); border:1px solid var(--border); border-radius:10px; padding:18px; }
.split .panel h3 { margin:0 0 10px; font-size:14px; font-weight:600; }
.split .panel.baseline  { border-left:4px solid var(--raw); }
.split .panel.tokenzero { border-left:4px solid var(--accent); }
.panel .big { font-size:28px; font-weight:600; letter-spacing:-0.01em; }
.panel .meta { color:var(--fg-dim); font-size:12px; margin-top:4px; }
.panel .bar-mini { height:8px; background:var(--bg-row); border-radius:99px; margin-top:10px; overflow:hidden; }
.panel .bar-mini > span { display:block; height:100%; transition:width .5s ease; }
.panel.baseline  .bar-mini > span { background:var(--raw); }
.panel.tokenzero .bar-mini > span { background:var(--accent); }

table { width:100%; border-collapse:collapse; font-size:13px; }
th, td { padding:8px 10px; text-align:left; border-bottom:1px solid var(--border); font-variant-numeric:tabular-nums; }
th { color:var(--fg-dim); font-weight:500; font-size:11px; text-transform:uppercase; letter-spacing:.05em; }
tr.running { background:rgba(210,153,34,.10); }
tr.done    { }
tr.failed  { background:rgba(248,81,73,.10); }
tr.pending { color:var(--fg-dim); }
.pill { font-size:10px; font-weight:700; padding:2px 8px; border-radius:99px; text-transform:uppercase; letter-spacing:.05em; }
.pill.baseline  { background:rgba(248,81,73,.15);  color:var(--raw); }
.pill.tokenzero { background:rgba(63,185,80,.15);  color:var(--accent); }
.pill.running   { background:rgba(210,153,34,.20); color:var(--warn); }
.pill.done      { background:rgba(63,185,80,.18);  color:var(--accent); }
.pill.failed    { background:rgba(248,81,73,.18);  color:var(--danger); }
.pill.pending   { background:var(--bg-row);        color:var(--fg-dim); }
.spinner { display:inline-block; width:10px; height:10px; border:2px solid var(--warn); border-top-color:transparent;
           border-radius:50%; animation:spin 0.9s linear infinite; vertical-align:middle; margin-right:4px; }
@keyframes spin { to { transform:rotate(360deg); } }
.empty { color:var(--fg-dim); padding:30px; text-align:center; font-size:13px; }
.disconnected { color:var(--danger); }
.muted { color:var(--fg-dim); }
footer.page { margin-top:24px; color:var(--fg-dim); font-size:11px; }
footer.page code { font-size:11px; background:var(--bg-row); padding:1px 5px; border-radius:3px; }
</style>
</head>
<body>
<div class="wrap">

<header class="page">
  <h1><span class="live-dot" id="dot"></span>TokenZero &mdash; live agent runs</h1>
  <p class="sub">Real Copilot CLI sessions on the JSON-RPC-errors task &middot;
     baseline vs TokenZero-only &middot; updates every $RefreshMs ms &middot;
     data: <code id="data-path">$DataPath</code> &middot;
     <span id="status">connecting&hellip;</span></p>
</header>

<section class="cards" id="cards"></section>
<section class="split" id="split"></section>

<section class="panel" style="background:var(--bg-elev); border:1px solid var(--border); border-radius:10px; padding:0; overflow:hidden;">
  <table id="runs">
    <thead>
      <tr>
        <th>#</th><th>Condition</th><th>Status</th><th>Wall</th><th>Input tok</th>
        <th>Output tok</th><th>Tool calls</th><th>Tool-out tok</th>
        <th>API ms</th><th>Notes</th>
      </tr>
    </thead>
    <tbody id="rows"><tr><td colspan="10" class="empty">waiting for runs&hellip;</td></tr></tbody>
  </table>
</section>

<footer class="page">
  Generated by <code>demo\build_agent_viz.ps1</code>. Re-render any time with
  <code>pwsh -File demo\build_agent_viz.ps1 -Open</code>.
  Combined report (perf + bugs + live runs) at <a href="demo_viz.html">demo_viz.html</a>.
</footer>

</div>
"@ + @'
<script>
const DATA_URL = "__DATA_URL__";
const REFRESH_MS = __REFRESH_MS__;
const fmt = n => (n == null || n === '') ? '\u2014'
                : (typeof n === 'number' ? n.toLocaleString() : String(n));
const esc = v => String(v == null ? '' : v)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;')
  .replace(/>/g, '&gt;').replace(/"/g, '&quot;')
  .replace(/'/g, '&#39;');
const css = v => String(v || '').replace(/[^a-z0-9_-]/gi, '');
const fmtTime = ms => {
  if (ms == null) return '\u2014';
  if (ms < 1000) return ms + ' ms';
  return (ms/1000).toFixed(1) + ' s';
};
const byId = id => document.getElementById(id);

async function tick() {
  try {
    const res = await fetch(DATA_URL + '?_=' + Date.now(), {cache:'no-store'});
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const j = await res.json();
    render(j);
    byId('status').textContent = 'live (updated ' + new Date().toLocaleTimeString() + ')';
    byId('status').className = '';
    byId('dot').style.background = 'var(--accent)';
  } catch (e) {
    byId('status').textContent = 'no data yet (' + e.message + ')';
    byId('status').className = 'disconnected';
    byId('dot').style.background = 'var(--fg-dim)';
  }
}

function render(j) {
  const runs = j.runs || [];
  const meta = j.meta || {};
  const totals = j.totals || {};

  const cards = [
    ['Task', meta.task || '\u2014', meta.model || ''],
    ['Replicates / condition', meta.replicates != null ? meta.replicates : '\u2014', ''],
    ['Runs done', (totals.done || 0) + ' / ' + (runs.length || 0),
       (totals.running ? totals.running + ' running' : '')],
    ['Failures', totals.failed || 0, ''],
    ['Elapsed', fmtTime(totals.elapsed_ms || 0), meta.started_at || '']
  ];
  byId('cards').innerHTML = cards.map(c =>
    '<div class="card"><div class="lbl">' + esc(c[0]) + '</div>' +
    '<div class="num">' + esc(c[1]) + '</div>' +
    (c[2] ? '<div class="sub">' + esc(c[2]) + '</div>' : '') + '</div>'
  ).join('');

  const summary = j.summary || {baseline:{}, tokenzero:{}};
  const splitHtml = ['baseline','tokenzero'].map(k => {
    const s = summary[k] || {};
    const meanTok = s.mean_tool_output_tokens || 0;
    const maxTok = Math.max(
      (summary.baseline||{}).mean_tool_output_tokens || 0,
      (summary.tokenzero||{}).mean_tool_output_tokens || 0, 1);
    const pct = Math.round((meanTok / maxTok) * 100);
    const title = k === 'baseline' ? 'Run A \u2014 native tools' : 'Run B \u2014 TokenZero MCP only';
    const sd = s.stddev_tool_output_tokens != null ? ', \u03c3=' + Math.round(s.stddev_tool_output_tokens) : '';
    const subN = s.n ? ' (n=' + s.n + sd + ')' : '';
    const mc = (s.mean_tool_calls != null) ? Number(s.mean_tool_calls).toFixed(1) : '\u2014';
    return '<div class="panel ' + k + '"><h3>' + title + '</h3>' +
      '<div class="big">' + fmt(Math.round(meanTok)) + '</div>' +
      '<div class="meta">mean tool-output tokens per run' + subN + '</div>' +
      '<div class="bar-mini"><span style="width:' + pct + '%"></span></div>' +
      '<div class="meta" style="margin-top:10px">' +
        'tool calls: ' + mc +
        ' \u00b7 wall: ' + fmtTime(s.mean_wall_ms) +
        ' \u00b7 in tok: ' + fmt(Math.round(s.mean_input_tokens || 0)) +
        ' \u00b7 out tok: ' + fmt(Math.round(s.mean_output_tokens || 0)) +
      '</div></div>';
  }).join('');
  byId('split').innerHTML = splitHtml;

  if (runs.length === 0) {
    byId('rows').innerHTML = '<tr><td colspan="10" class="empty">waiting for runs\u2026</td></tr>';
    return;
  }
  byId('rows').innerHTML = runs.map(r => {
    const cls = css(r.status || 'pending');
    const condition = css(r.condition || '');
    const statusBadge = r.status === 'running'
      ? '<span class="spinner"></span><span class="pill running">running</span>'
      : '<span class="pill ' + cls + '">' + esc(r.status || 'pending') + '</span>';
    return '<tr class="' + cls + '">' +
      '<td>' + esc(r.index != null ? r.index : '') + '</td>' +
      '<td><span class="pill ' + condition + '">' + esc(r.condition || '') + '</span></td>' +
      '<td>' + statusBadge + '</td>' +
      '<td>' + esc(fmtTime(r.wall_ms)) + '</td>' +
      '<td>' + esc(fmt(r.input_tokens)) + '</td>' +
      '<td>' + esc(fmt(r.output_tokens)) + '</td>' +
      '<td>' + esc(fmt(r.tool_calls)) + '</td>' +
      '<td>' + esc(fmt(r.tool_output_tokens)) + '</td>' +
      '<td>' + esc(fmt(r.api_ms)) + '</td>' +
      '<td class="muted">' + esc(r.note || '') + '</td>' +
    '</tr>';
  }).join('');
}

tick();
setInterval(tick, REFRESH_MS);
</script>
</body>
</html>
'@

# Inject the dynamic values into the single-quoted JS block.
$html = $html.Replace('__DATA_URL__', $DataPath).Replace('__REFRESH_MS__', "$RefreshMs")

Set-Content -LiteralPath $OutPath -Value $html -Encoding UTF8
Write-Host "Wrote: $OutPath"
if ($Open) {
    $py = Get-Command python3 -ErrorAction SilentlyContinue
    if (-not $py) { $py = Get-Command python -ErrorAction SilentlyContinue }
    if ($py) {
        $null = Start-Process -FilePath $py.Source -ArgumentList @('-m','http.server',"$Port",'--bind','127.0.0.1') -WorkingDirectory $DemoDir -PassThru
        Start-Sleep -Milliseconds 400
        Start-Process ("http://127.0.0.1:$Port/" + [System.IO.Path]::GetFileName($OutPath))
    } else {
        Write-Warning 'python3/python not found; open the viewer through a local HTTP server so fetch(agent_results.json) works.'
    }
}
