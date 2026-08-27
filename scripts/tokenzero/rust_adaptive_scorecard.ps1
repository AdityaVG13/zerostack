param(
  [string]$TokenZeroExe = "",
  [string]$ReportPath = "results/current/rust_adaptive_scorecard.json"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "../..")
if ([string]::IsNullOrWhiteSpace($TokenZeroExe)) {
  $TokenZeroExe = Join-Path $RepoRoot "target/debug/tokenzero.exe"
}

function Quote-ProcessArg {
  param([string]$Value)

  return '"' + $Value.Replace('"', '\"') + '"'
}

function Invoke-TokenZeroJson {
  param(
    [string]$Name,
    [string[]]$Arguments,
    [hashtable]$Environment = @{}
  )

  $psi = [System.Diagnostics.ProcessStartInfo]::new(); $psi.FileName = $TokenZeroExe; $psi.WorkingDirectory = $RepoRoot; $psi.Arguments = ($Arguments | ForEach-Object { Quote-ProcessArg $_ }) -join " "; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  foreach ($key in $Environment.Keys) {
    $psi.EnvironmentVariables[$key] = $Environment[$key]
  }

  $timer = [System.Diagnostics.Stopwatch]::StartNew(); $process = [System.Diagnostics.Process]::Start($psi); $stdout = $process.StandardOutput.ReadToEnd(); $stderr = $process.StandardError.ReadToEnd(); $process.WaitForExit(); $timer.Stop()

  $json = $null; $parseError = $null
  try {
    $json = $stdout | ConvertFrom-Json
  } catch {
    $parseError = $_.Exception.Message
  }

  return [ordered]@{
    name = $Name; args = $Arguments; exit_code = $process.ExitCode; wall_ms = $timer.ElapsedMilliseconds; stdout_preview = $stdout.Substring(0, [Math]::Min(500, $stdout.Length)); stderr_preview = $stderr.Substring(0, [Math]::Min(500, $stderr.Length)); parse_error = $parseError; json = $json
  }
}

function Accounting-Row {
  param(
    [hashtable]$Case,
    [string]$Expectation
  )

  $json = $Case.json; $accounting = $json.accounting; $telemetry = $json.telemetry
  return [ordered]@{
    name = $Case.name; expectation = $Expectation; ok = $false; status = $json.status; command_success = $telemetry.command_success; output_strategy = $telemetry.output_strategy; wall_ms = $Case.wall_ms; latency_ms = $telemetry.latency_ms; raw_tokens = $accounting.raw_tokens
    visible_tokens = $accounting.visible_tokens; recovery_tokens = $accounting.recovery_tokens; exact_ref_tokens = $accounting.exact_ref_tokens; parse_error = $Case.parse_error; exit_code = $Case.exit_code
  }
}

$failures = @(); $cases = @()

$null = Invoke-TokenZeroJson `
  -Name "warmup" `
  -Arguments @("run", "--json", "--", "cmd", "/D", "/C", "echo warmup")

$tiny = Invoke-TokenZeroJson `
  -Name "tiny success" `
  -Arguments @("run", "--json", "--", "cmd", "/D", "/C", "echo ok")
$tinyRow = Accounting-Row -Case $tiny -Expectation "compact view, no token overhead, fast route"
$tinyRow.ok = (
  $tiny.exit_code -eq 0 -and; $null -eq $tiny.parse_error -and; $tiny.json.status -eq "ok" -and; $tiny.json.telemetry.command_success -eq $true -and; $tiny.json.telemetry.output_strategy -eq "compact_adaptive_shell" -and
  $tiny.json.accounting.visible_tokens -le $tiny.json.accounting.raw_tokens -and; $tiny.json.telemetry.latency_ms -le 100 -and; $tiny.wall_ms -le 3500
)
if (!$tinyRow.ok) {
  $failures += "tiny success violated adaptive overhead or latency guard"
}
$cases += $tinyRow

$tinyReadDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-long-label-" + [guid]::NewGuid().ToString("N")); $tinyReadFile = Join-Path $tinyReadDir "tiny.md"
try {
  New-Item -ItemType Directory -Force $tinyReadDir | Out-Null; "ok" | Out-File -Encoding utf8 $tinyReadFile
  $tinyRead = Invoke-TokenZeroJson `
    -Name "tiny read long label" `
    -Arguments @("read", $tinyReadFile, "--allowed-root", $tinyReadDir, "--max-visible-tokens", "20", "--json")
  $tinyReadRow = Accounting-Row -Case $tinyRead -Expectation "long path labels do not hide tiny payloads"
  $tinyReadRow.ok = (
    $tinyRead.exit_code -eq 0 -and; $null -eq $tinyRead.parse_error -and; $tinyRead.json.status -eq "ok" -and; $tinyRead.json.visible.text.Contains("ok") -and; !$tinyRead.json.visible.text.Contains("omitted") -and; $tinyRead.json.accounting.visible_tokens -le 20
  )
  if (!$tinyReadRow.ok) {
    $failures += "tiny read payload was hidden by long label budget"
  }
  $cases += $tinyReadRow
} finally {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tinyReadDir
}

$powerShellScript = '$tzTmp = Join-Path $env:TEMP "tz-quote"; [Console]::Out.Write($tzTmp)'
$powerShellRoute = Invoke-TokenZeroJson `
  -Name "powershell variable route" `
  -Arguments @("run", "--json", "--", $powerShellScript)
$powerShellRouteRow = Accounting-Row -Case $powerShellRoute -Expectation "raw PowerShell variables route to PowerShell without cmd quoting loss"
$powerShellRouteRow.ok = (
  $powerShellRoute.exit_code -eq 0 -and; $null -eq $powerShellRoute.parse_error -and; $powerShellRoute.json.status -eq "ok" -and; $powerShellRoute.json.telemetry.command_success -eq $true -and; $powerShellRoute.json.telemetry.execution_mode -eq "shell" -and
  $powerShellRoute.json.telemetry.argv[0] -eq "powershell" -and; $powerShellRoute.json.telemetry.stdout_preview.EndsWith("tz-quote") -and; $powerShellRoute.wall_ms -le 3500
)
if (!$powerShellRouteRow.ok) {
  $failures += "PowerShell variable route failed or exceeded latency guard"
}
$cases += $powerShellRouteRow

$timeoutRoute = Invoke-TokenZeroJson `
  -Name "timeout override" `
  -Arguments @("run", "--json", "--timeout-seconds", "1", "--", "powershell", "-NoProfile", "-Command", "Start-Sleep -Seconds 3; Write-Output late")
$timeoutRouteRow = Accounting-Row -Case $timeoutRoute -Expectation "explicit shell timeout is honored"
$timeoutRouteRow.ok = (
  $timeoutRoute.exit_code -eq 0 -and; $null -eq $timeoutRoute.parse_error -and; $timeoutRoute.json.status -eq "ok" -and; $timeoutRoute.json.telemetry.command_success -eq $false -and; $timeoutRoute.json.telemetry.timeout -eq $true -and; $timeoutRoute.json.telemetry.latency_ms -le 1500 -and
  $timeoutRoute.wall_ms -le 5000
)
if (!$timeoutRouteRow.ok) {
  $failures += "timeout override was not honored"
}
$cases += $timeoutRouteRow

$searchDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-search-" + [guid]::NewGuid().ToString("N"))
try {
  New-Item -ItemType Directory -Force $searchDir | Out-Null
  0..30 | ForEach-Object {
    "hay" | Out-File -Encoding utf8 (Join-Path $searchDir ("a{0:D3}.txt" -f $_))
  }
  "needle" | Out-File -Encoding utf8 (Join-Path $searchDir "zmatch.txt")
  # The internal backend reports traversal counts; rg only reports matched
  # files, which would make the visited_files floor meaningless.
  $deepSearch = Invoke-TokenZeroJson `
    -Name "deep search traversal" `
    -Arguments @("find", "needle", $searchDir, "--allowed-root", $searchDir, "--json") `
    -Environment @{ TOKENZERO_SEARCH_BACKEND = "internal" }
  $deepSearchRow = Accounting-Row -Case $deepSearch -Expectation "search traverses beyond the visible result limit before declaring sparse output"
  $deepSearchRow.ok = (
    $deepSearch.exit_code -eq 0 -and; $null -eq $deepSearch.parse_error -and; $deepSearch.json.status -eq "ok" -and; $deepSearch.json.telemetry.matches -ge 1 -and; $deepSearch.json.telemetry.visited_files -gt 20 -and; $deepSearch.json.telemetry.truncated_by_visit -eq $false -and
    $deepSearch.json.accounting.exact_ref_tokens -gt 2
  )
  if (!$deepSearchRow.ok) {
    $failures += "search did not traverse beyond the old sparse 20-file limit"
  }
  $cases += $deepSearchRow
} finally {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $searchDir
}

$cacheDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-cache-dir-" + [guid]::NewGuid().ToString("N")); $cacheFile = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-cache-read-" + [guid]::NewGuid().ToString("N") + ".txt")
try {
  New-Item -ItemType Directory -Force $cacheDir | Out-Null; "alpha" | Out-File -Encoding utf8 $cacheFile
  # Split-Path keeps the root free of a trailing backslash, which would
  # otherwise escape the closing quote when the argv line is built.
  $cacheDegrade = Invoke-TokenZeroJson `
    -Name "cache write degrade" `
    -Arguments @("read", $cacheFile, "--allowed-root", (Split-Path -Parent $cacheFile), "--cache-path", $cacheDir, "--json")
  $cacheDegradeRow = Accounting-Row -Case $cacheDegrade -Expectation "read still returns compressed output when recovery cache persistence fails"
  $cacheDegradeRow.ok = (
    $cacheDegrade.exit_code -eq 0 -and; $null -eq $cacheDegrade.parse_error -and; $cacheDegrade.json.status -eq "ok" -and; $cacheDegrade.json.diagnostic.code -eq "cache_write_failed" -and; $cacheDegrade.json.visible.text.Contains("alpha")
  )
  if (!$cacheDegradeRow.ok) {
    $failures += "read did not degrade cleanly when recovery cache persistence failed"
  }
  $cases += $cacheDegradeRow
} finally {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $cacheDir; Remove-Item -Force -ErrorAction SilentlyContinue $cacheFile
}

if (![string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
  $homeReadFile = Join-Path $env:USERPROFILE ".tokenzero-scorecard-home-read.tmp"; $homeMarkdownFile = Join-Path $env:USERPROFILE ".tokenzero-scorecard-home-read.md"
  try {
    $homeLines = @("home-ok " + ((0..40 | ForEach-Object { "intro$_" }) -join " "))
    0..40 | ForEach-Object {
      $homeLines += ("line $_ " + ((0..30 | ForEach-Object { "token${_}" }) -join " "))
    }
    $homeLines | Out-File -Encoding utf8 $homeReadFile
    $homeRead = Invoke-TokenZeroJson `
      -Name "home allowed root read" `
      -Arguments @("read", $homeReadFile, "--allowed-root", $env:USERPROFILE, "--max-visible-tokens", "120", "--json")
    $homeReadRow = Accounting-Row -Case $homeRead -Expectation "explicit home allowed root grants reads and budgets are honored"
    $homeReadRow.ok = (
      $homeRead.exit_code -eq 0 -and; $null -eq $homeRead.parse_error -and; $homeRead.json.status -eq "ok" -and; $homeRead.json.visible.text.Contains("home-ok") -and; $homeRead.json.accounting.raw_tokens -gt 500 -and; $homeRead.json.accounting.visible_tokens -le 120
    )
    if (!$homeReadRow.ok) {
      $failures += "home directory read failed despite an explicit home allowed root"
    }
    $cases += $homeReadRow

    "# home-ok`n`nmarkdown body" | Out-File -Encoding utf8 $homeMarkdownFile
    $markdownRead = Invoke-TokenZeroJson `
      -Name "markdown content type" `
      -Arguments @("read", $homeMarkdownFile, "--allowed-root", $env:USERPROFILE, "--json")
    $markdownReadRow = Accounting-Row -Case $markdownRead -Expectation "read reports detected Markdown content type"
    $markdownReadRow.ok = (
      $markdownRead.exit_code -eq 0 -and; $null -eq $markdownRead.parse_error -and; $markdownRead.json.status -eq "ok" -and; $markdownRead.json.content_type -eq "markdown"
    )
    if (!$markdownReadRow.ok) {
      $failures += "Markdown read did not report markdown content type"
    }
    $cases += $markdownReadRow
  } finally {
    Remove-Item -Force -ErrorAction SilentlyContinue $homeReadFile; Remove-Item -Force -ErrorAction SilentlyContinue $homeMarkdownFile
  }
}

$noisyScript = "1..200 | ForEach-Object { Write-Output ('line ' + `$_) }"
$noisy = Invoke-TokenZeroJson `
  -Name "noisy stdout" `
  -Arguments @("run", "--json", "--", "powershell", "-NoProfile", "-Command", $noisyScript)
$noisyRow = Accounting-Row -Case $noisy -Expectation "compressed visible output for noisy logs"
$noisyRow.ok = (
  $noisy.exit_code -eq 0 -and; $null -eq $noisy.parse_error -and; $noisy.json.status -eq "ok" -and; $noisy.json.telemetry.command_success -eq $true -and; $noisy.json.telemetry.output_strategy -ne "compact_adaptive_shell" -and
  ($noisy.json.accounting.visible_tokens * 2) -lt $noisy.json.accounting.raw_tokens
)
if (!$noisyRow.ok) {
  $failures += "noisy stdout did not compress below half of raw tokens"
}
$cases += $noisyRow

$read = Invoke-TokenZeroJson `
  -Name "source read" `
  -Arguments @("read", "crates/tokenzero-core/src/lib.rs", "--json")
$readRow = Accounting-Row -Case $read -Expectation "source reads compress aggressively"
$readRow.ok = (
  $read.exit_code -eq 0 -and; $null -eq $read.parse_error -and; $read.json.status -eq "ok" -and; ($read.json.accounting.visible_tokens * 4) -lt $read.json.accounting.raw_tokens
)
if (!$readRow.ok) {
  $failures += "source read did not compress below one quarter of raw tokens"
}
$cases += $readRow

$report = [ordered]@{
  schema_version = "tokenzero.adaptive_scorecard.v1"
  status = if ($failures.Count -eq 0) { "ok" } else { "blocked" }
  ok = $failures.Count -eq 0; tokenzero_exe = $TokenZeroExe
  thresholds = [ordered]@{
    tiny_latency_ms_max = 100; tiny_wall_ms_max = 3500; powershell_variable_wall_ms_max = 3500; timeout_override_latency_ms_max = 1500; timeout_override_wall_ms_max = 5000; tiny_visible_tokens_lte_raw = $true; noisy_visible_tokens_lt_half_raw = $true; read_visible_tokens_lt_quarter_raw = $true
  }
  cases = $cases; failures = $failures
}

$parent = Split-Path -Parent $ReportPath
if ($parent) {
  New-Item -ItemType Directory -Force $parent | Out-Null
}
$report | ConvertTo-Json -Depth 10 | Out-File -Encoding utf8 $ReportPath; $report | ConvertTo-Json -Depth 10

if (!$report.ok) {
  exit 1
}
