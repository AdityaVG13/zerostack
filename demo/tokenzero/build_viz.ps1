#requires -Version 5.1
<#
.SYNOPSIS
    Render demo\demo_results.json into a self-contained demo\demo_viz.html.

.DESCRIPTION
    Reads the JSON written by run_demo.ps1 and emits a single HTML page
    (inline CSS + inline SVG, zero CDN/script dependencies) that visualises:

      * the totals (raw vs visible vs savings)
      * per-scenario raw-vs-visible bars (log-scaled so 11- and 79,000-token
        rows are both readable)
      * a byte-exact recovery badge (pass/fail derived from the round-trip row)
      * an MCP-dedup callout if the second-read row's savings is materially
        worse than the first-read row's (the gap I observed against v1.0.1)

.PARAMETER ResultsPath
    Path to demo_results.json. Defaults to the sibling file in this script's
    folder.

.PARAMETER OutPath
    Output HTML path. Defaults to demo\demo_viz.html.

.PARAMETER Open
    If set, opens the rendered page in the default browser when done.

.EXAMPLE
    pwsh -File .\demo\build_viz.ps1 -Open
#>

[CmdletBinding()]
param(
    [string] $ResultsPath,
    [string] $GapReportPath,
    [string] $OutPath,
    [switch] $Open
)

$ErrorActionPreference = 'Stop'

$DemoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ResultsPath)   { $ResultsPath   = Join-Path $DemoDir 'demo_results.json' }
if (-not $GapReportPath) { $GapReportPath = Join-Path $DemoDir 'gap_report.json' }
if (-not $OutPath)       { $OutPath       = Join-Path $DemoDir 'demo_viz.html' }

if (-not (Test-Path -LiteralPath $ResultsPath)) {
    throw "demo_results.json not found at $ResultsPath. Run .\demo\run_demo.ps1 first."
}

$data = Get-Content -LiteralPath $ResultsPath -Raw | ConvertFrom-Json
$gap  = $null
if (Test-Path -LiteralPath $GapReportPath) {
    $gap = Get-Content -LiteralPath $GapReportPath -Raw | ConvertFrom-Json
}

function Encode-Html {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Value)
    return ($Value `
        -replace '&','&amp;' `
        -replace '<','&lt;' `
        -replace '>','&gt;' `
        -replace '"','&quot;' `
        -replace "'",'&#39;')
}

function Format-Number {
    param([double]$N)
    if ($N -ge 1) { return ('{0:N0}' -f $N) }
    return '0'
}

# --- Scale (log10, so 11 and 79,424 both render) ---------------------------
$maxRaw = ($data.workloads | Measure-Object -Property raw_tokens -Maximum).Maximum
if (-not $maxRaw -or $maxRaw -le 0) { $maxRaw = 1 }
$logMax = [math]::Log10([math]::Max($maxRaw, 10))

function To-LogPct {
    param([double]$N)
    if ($N -le 0) { return 0.0 }
    $lv = [math]::Log10([math]::Max($N, 1.0))
    return [math]::Max(0.5, ($lv / $logMax) * 100.0)
}

# --- Derive the recovery badge from the round-trip row ---------------------
$recoveryRow = $data.workloads | Where-Object { $_.note -match '(?i)byte-exact' } | Select-Object -First 1
$recoveryOk  = [bool]$recoveryRow

# --- Derive the dedup-gap callout (first vs second read of the same file) --
$dedupCallout = $null
$firstRead  = $data.workloads | Where-Object { $_.workload -match '^large read'   } | Select-Object -First 1
$secondRead = $data.workloads | Where-Object { $_.workload -match '(?i)re-read|dedup' } | Select-Object -First 1
if ($firstRead -and $secondRead -and $secondRead.raw_tokens -gt 0) {
    # README claims ~99.7% on this row. Flag anything within 1pp of the first
    # read's savings (i.e. no meaningful drop on the repeat).
    $delta = [math]::Round($firstRead.savings_pct - $secondRead.savings_pct, 1)
    if ([math]::Abs($delta) -lt 1.0) {
        $dedupCallout = ("Second {0}-token read returned {1} visible tokens (savings {2}%) -- first read was {3} ({4}%). README claims ~99.7%; observed dedup did not fire. See gap finding #9." -f `
            $secondRead.raw_tokens, $secondRead.visible_tokens, $secondRead.savings_pct, $firstRead.visible_tokens, $firstRead.savings_pct)
    }
}

# --- Build the per-row markup ----------------------------------------------
$rowsHtml = New-Object System.Text.StringBuilder
foreach ($w in $data.workloads) {
    $name = Encode-Html $w.workload
    $note = Encode-Html $w.note

    $rawN = [double]$w.raw_tokens
    $visN = [double]$w.visible_tokens
    $rawPct = To-LogPct $rawN
    $visPct = To-LogPct $visN

    $savings = if ($rawN -gt 0) { [math]::Round(100.0 * ($rawN - $visN) / [math]::Max($rawN,1), 1) } else { 0 }
    $isPassthrough = ($rawN -gt 0 -and $rawN -eq $visN)
    $isRecovery    = ($rawN -le 0 -and $visN -le 0)

    $badgeClass = 'savings'
    $badgeText  = ('{0:N1}% saved' -f $savings)
    if ($isPassthrough) { $badgeClass = 'passthrough'; $badgeText = 'pass-through' }
    if ($isRecovery)    { $badgeClass = 'recovery';    $badgeText = $note }

    [void]$rowsHtml.AppendLine('<article class="row">')
    [void]$rowsHtml.AppendLine('  <header>')
    [void]$rowsHtml.AppendLine(('    <h3>{0}</h3>' -f $name))
    [void]$rowsHtml.AppendLine(('    <span class="badge {0}">{1}</span>' -f $badgeClass, (Encode-Html $badgeText)))
    [void]$rowsHtml.AppendLine('  </header>')

    if (-not $isRecovery) {
        [void]$rowsHtml.AppendLine('  <div class="bars">')
        [void]$rowsHtml.AppendLine('    <div class="bar-row">')
        [void]$rowsHtml.AppendLine('      <span class="bar-label">raw</span>')
        [void]$rowsHtml.AppendLine(('      <div class="bar raw" style="width:{0:F2}%"></div>' -f $rawPct))
        [void]$rowsHtml.AppendLine(('      <span class="bar-value">{0}</span>' -f (Format-Number $rawN)))
        [void]$rowsHtml.AppendLine('    </div>')
        [void]$rowsHtml.AppendLine('    <div class="bar-row">')
        [void]$rowsHtml.AppendLine('      <span class="bar-label">visible</span>')
        [void]$rowsHtml.AppendLine(('      <div class="bar visible" style="width:{0:F2}%"></div>' -f $visPct))
        [void]$rowsHtml.AppendLine(('      <span class="bar-value">{0}</span>' -f (Format-Number $visN)))
        [void]$rowsHtml.AppendLine('    </div>')
        [void]$rowsHtml.AppendLine('  </div>')
    }
    if ($note -and -not $isRecovery) {
        [void]$rowsHtml.AppendLine(('  <p class="note">{0}</p>' -f $note))
    }
    [void]$rowsHtml.AppendLine('</article>')
}

# --- Donut SVG for the totals ----------------------------------------------
$totalRaw     = [double]$data.totals.raw_tokens
$totalVisible = [double]$data.totals.visible_tokens
$totalPct     = [double]$data.totals.savings_pct
$donutCircum  = 2 * [math]::PI * 90
$dashSaved    = ($totalPct / 100.0) * $donutCircum
$dashRemain   = $donutCircum - $dashSaved

# Avoid PowerShell substitution headaches in the heredoc by pre-formatting.
$donutSavedDash = ('{0:F2} {1:F2}' -f $dashSaved, $dashRemain)
$totalRawFmt    = Format-Number $totalRaw
$totalVisFmt    = Format-Number $totalVisible
$totalPctFmt    = ('{0:N1}' -f $totalPct)
$generatedAt    = (Get-Date).ToString('yyyy-MM-dd HH:mm K')
$tzVersionHtml  = Encode-Html ([string]$data.tokenzero_version)
$repoHtml       = Encode-Html ([string]$data.repo)

$jumpToBugs = ''
if ($gap) {
    $sevBits = @()
    foreach ($sev in @('critical','high','medium','low')) {
        $n = [int]($gap.summary.by_severity.$sev)
        if ($n -gt 0) { $sevBits += ("{0} {1}" -f $n, $sev) }
    }
    $sevText = $sevBits -join ' / '
    $jumpToBugs = ('<a class="hero-badge fail" href="#gaps" style="text-decoration:none">{0} bugs flagged ({1}) &rarr;</a>' -f $gap.summary.total, $sevText)
}

$recoveryBadgeHtml = if ($recoveryOk) {
    '<span class="hero-badge ok">byte-exact recovery: PASS</span>'
} else {
    '<span class="hero-badge fail">byte-exact recovery: not run</span>'
}
$dedupCalloutHtml = ''
if ($dedupCallout) {
    $dedupCalloutHtml = ('<div class="callout warn"><strong>Observed gap (MCP session dedup).</strong> {0}</div>' -f (Encode-Html $dedupCallout))
}

# --- Gap report section -----------------------------------------------------
$gapHtml = ''
if ($gap) {
    $sb = New-Object System.Text.StringBuilder
    $by = $gap.summary.by_severity
    $srcList = ($gap.summary.sources | ForEach-Object { Encode-Html $_ }) -join ', '

    [void]$sb.AppendLine('<section class="gaps" id="gaps">')
    [void]$sb.AppendLine('  <header>')
    [void]$sb.AppendLine('    <h2>Bugs flagged for the developer</h2>')
    [void]$sb.AppendLine(('    <span class="sub">{0} findings &middot; sources: {1}</span>' -f $gap.summary.total, $srcList))
    [void]$sb.AppendLine('  </header>')

    [void]$sb.AppendLine('  <div class="sev-summary">')
    foreach ($sev in @('critical','high','medium','low')) {
        $n = [int]($by.$sev)
        if ($n -le 0) { continue }
        [void]$sb.AppendLine(('    <span class="sev-count sev-{0}"><span class="n">{1}</span>{0}</span>' -f $sev, $n))
    }
    [void]$sb.AppendLine('  </div>')

    foreach ($f in $gap.findings) {
        $sev   = [string]$f.severity
        $rank  = [int]$f.rank
        $title = Encode-Html ([string]$f.title)
        $impact = Encode-Html ([string]$f.impact)
        $evidence = Encode-Html ([string]$f.evidence)
        $fix = Encode-Html ([string]$f.fix)
        $source = Encode-Html ([string]$f.source)
        $idAttr = Encode-Html ([string]$f.id)

        [void]$sb.AppendLine(('  <details class="finding" data-sev="{0}" id="bug-{1}">' -f $sev, $idAttr))
        [void]$sb.AppendLine('    <summary>')
        [void]$sb.AppendLine(('      <span class="rank">#{0}</span>' -f $rank))
        [void]$sb.AppendLine(('      <span class="sev sev-{0}">{0}</span>' -f $sev))
        [void]$sb.AppendLine(('      <span class="ttl">{0}</span>' -f $title))
        [void]$sb.AppendLine('    </summary>')

        [void]$sb.AppendLine('    <div class="finding-body">')
        [void]$sb.AppendLine('      <div><div class="row-label">Impact</div>' + $impact + '</div>')
        if ($f.claim_contradicted) {
            $cc = Encode-Html ([string]$f.claim_contradicted)
            [void]$sb.AppendLine(('      <div class="contradicts"><strong>Contradicts:</strong> {0}</div>' -f $cc))
        }
        [void]$sb.AppendLine('      <div><div class="row-label">Evidence</div><code>' + $evidence + '</code></div>')
        if ($f.repro) {
            $repro = Encode-Html ([string]$f.repro)
            [void]$sb.AppendLine('      <div><div class="row-label">Repro</div>' + $repro + '</div>')
        }
        [void]$sb.AppendLine('      <div><div class="row-label">Fix sketch</div>' + $fix + '</div>')
        [void]$sb.AppendLine(('      <div><div class="row-label">Source</div>{0}</div>' -f $source))
        [void]$sb.AppendLine('    </div>')
        [void]$sb.AppendLine('  </details>')
    }

    [void]$sb.AppendLine('</section>')
    $gapHtml = $sb.ToString()
}

# --- Assemble the HTML ------------------------------------------------------
$html = @"
<!doctype html>
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
  <p class="sub">$tzVersionHtml &middot; generated $generatedAt &middot; <code>$repoHtml</code></p>
</header>

<section class="hero">
  <div class="donut">
    <svg viewBox="0 0 200 200" width="200" height="200" aria-hidden="true">
      <circle cx="100" cy="100" r="90" stroke="var(--bg-row)" stroke-width="18" fill="none"></circle>
      <circle cx="100" cy="100" r="90" stroke="var(--accent)" stroke-width="18" fill="none"
              stroke-dasharray="$donutSavedDash" stroke-dashoffset="0"
              transform="rotate(-90 100 100)" stroke-linecap="round"></circle>
    </svg>
    <div class="pct">
      <div class="big">$totalPctFmt%</div>
      <div class="small">tokens hidden</div>
    </div>
  </div>
  <div class="stats">
    <div><div class="num">$totalRawFmt</div><div class="lbl">Raw tokens (across runs)</div></div>
    <div><div class="num">$totalVisFmt</div><div class="lbl">Visible to agent</div></div>
    <div><div class="num">$totalPctFmt%</div><div class="lbl">Recovery-aware savings</div></div>
    <div class="hero-badges">
      $recoveryBadgeHtml
      <span class="hero-badge ok">isolated cache &middot; same tokenizer both sides</span>
      $jumpToBugs
    </div>
  </div>
</section>

$dedupCalloutHtml

<section class="scenarios">
$($rowsHtml.ToString())
</section>

$gapHtml

<footer class="page">
  Bars use a log-base-10 scale so 11-token and 79,000-token rows are both
  legible. Source: <code>demo/demo_results.json</code> + <code>demo/gap_report.json</code>.
  Regenerate with <code>pwsh -File demo\run_demo.ps1</code> then
  <code>pwsh -File demo\build_viz.ps1 -Open</code>.
</footer>

</div>
</body>
</html>
"@

Set-Content -LiteralPath $OutPath -Value $html -Encoding UTF8
Write-Host "Wrote: $OutPath"

if ($Open) {
    Start-Process $OutPath
}
