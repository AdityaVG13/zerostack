#requires -Version 5.1
[CmdletBinding()]
param(
    [int] $Replicates = 3,
    [string] $Model = 'gpt-5-mini',
    [string[]] $Conditions = @('baseline','tokenzero'),
    [int] $Port = 8765,
    [switch] $NoServe, [switch] $NoOpen,
    [int] $PerRunTimeoutSec = 300,
    [string] $BinaryPath, [string] $CopilotPath
)

$ErrorActionPreference = 'Stop'; $DemoDir = Split-Path -Parent $MyInvocation.MyCommand.Path; $RepoDir = Split-Path -Parent $DemoDir
$Paths = [ordered]@{
    Runs = Join-Path $DemoDir 'agent_runs'; Cache = Join-Path $DemoDir '.cache'; Results = Join-Path $DemoDir 'agent_results.json'; Mcp = Join-Path $DemoDir 'tokenzero-mcp.json'
}
New-Item -ItemType Directory -Force -Path $Paths.Runs, $Paths.Cache | Out-Null

function Resolve-Executable {
    param([string] $Path, [string] $Command, [string] $Missing, [switch] $MustExist)
    if (-not $Path) {
        $found = Get-Command $Command -ErrorAction SilentlyContinue
        if ($found) { $Path = $found.Source }
    }
    if (-not $Path -or ($MustExist -and -not (Test-Path $Path))) { throw $Missing }
    $Path
}

function ConvertTo-Win32CommandLineArg {
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Value)
    if ($Value -eq '') { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }
    $builder = [System.Text.StringBuilder]::new(); [void] $builder.Append('"'); $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        if ($character -eq '"') {
            [void] $builder.Append('\' * (2 * $slashes + 1)); [void] $builder.Append('"'); $slashes = 0
            continue
        }
        if ($slashes) { [void] $builder.Append('\' * $slashes); $slashes = 0 }
        [void] $builder.Append($character)
    }
    if ($slashes) { [void] $builder.Append('\' * (2 * $slashes)) }
    [void] $builder.Append('"'); $builder.ToString()
}

function Set-ProcessArguments {
    param([System.Diagnostics.ProcessStartInfo] $Info, [string[]] $Arguments)
    if ($Info.PSObject.Properties.Name -contains 'ArgumentList') {
        foreach ($argument in $Arguments) { [void] $Info.ArgumentList.Add($argument) }
    } else {
        $Info.Arguments = ($Arguments | ForEach-Object { ConvertTo-Win32CommandLineArg $_ }) -join ' '
    }
}

function Read-FileOrEmpty {
    param([string] $Path)
    if (Test-Path $Path) { Get-Content -LiteralPath $Path -Raw } else { '' }
}

function Add-Text {
    param([System.Text.StringBuilder] $Builder, $Value)
    if ($null -ne $Value) {
        [void] $Builder.Append([string] $Value); [void] $Builder.Append([Environment]::NewLine)
    }
}

$BinaryPath = Resolve-Executable $BinaryPath tokenzero 'tokenzero binary not found. Pass -BinaryPath.' -MustExist; $CopilotPath = Resolve-Executable $CopilotPath copilot 'copilot CLI not found on PATH. Pass -CopilotPath.'
foreach ($message in @("tokenzero: $BinaryPath", "copilot:   $CopilotPath", "repo:      $RepoDir", "runs dir:  $($Paths.Runs)")) {
    Write-Host $message
}

# Template shape lives in docs/install.md ("Manual MCP config"); embedded inline here.
$template = '{"mcpServers":{"tokenzero":{"type":"local","command":"__TOKENZERO_BIN__","args":["mcp-server","--allowed-root","__REPO__","--cache-path","__CACHE__"],"tools":["*"]}}}'
$jsonValues = @($BinaryPath, $RepoDir, (Join-Path $Paths.Cache 'agent-tokenzero.json')) | ForEach-Object {
    $_ -replace '\\','\\' -replace '"','\"'
}
$config = $template -replace '__TOKENZERO_BIN__', $jsonValues[0] -replace '__REPO__', $jsonValues[1] -replace '__CACHE__', $jsonValues[2]; Set-Content -LiteralPath $Paths.Mcp -Value $config -Encoding UTF8; Write-Host "wrote MCP config: $($Paths.Mcp)"

$NativeDeny = @('view','bash','powershell','read_powershell','str_replace_editor','create','edit','grep','glob','find','read','write','run') -join ','; $Prompt = @'
TASK: Find every place a JSON-RPC error response is constructed in the
tokenzero-mcp crate (crates/tokenzero-mcp/src/). For each, report file:line
and a short note about when it fires.

RULES (follow exactly):
- Start with a tool call IMMEDIATELY. Do not write a plan first.
- Use at most 6 tool calls.
- Final reply must be ONLY a markdown table with columns:
  | File:Line | Code | When |
- No prose. No reasoning. No "intent". Table only.
'@

$plan = @(); $index = 0
foreach ($replicate in 1..$Replicates) {
    foreach ($condition in $Conditions) {
        $plan += [ordered]@{
            index = ++$index; condition = $condition; replicate = $replicate; status = 'pending'; wall_ms = $null; api_ms = $null; input_tokens = $null; output_tokens = $null
            tool_calls = $null; tool_output_tokens = $null; exit_code = $null; note = ''; jsonl_path = ''
        }
    }
}

function Save-Results {
    param($Meta, $Runs, $StartTime)
    $all = @($Runs)
    function Count-Status($status) { @($all | Where-Object { $_.status -eq $status }).Count }
    function Stats($condition) {
        $rows = @($all | Where-Object { $_.condition -eq $condition -and $_.status -eq 'done' })
        if (-not $rows.Count) { return [ordered]@{ n = 0 } }
        function Values($property) { @($rows | ForEach-Object { $_.$property } | Where-Object { $null -ne $_ }) }
        function Mean($property) {
            $values = Values $property
            if ($values.Count) { ($values | Measure-Object -Average).Average } else { $null }
        }
        function Std($property) {
            $values = Values $property
            if ($values.Count -lt 2) { return $null }
            $mean = ($values | Measure-Object -Average).Average; $sum = 0.0
            foreach ($value in $values) { $sum += ($value - $mean) * ($value - $mean) }
            [Math]::Sqrt($sum / ($values.Count - 1))
        }
        [ordered]@{
            n = $rows.Count; mean_tool_output_tokens = Mean 'tool_output_tokens'; stddev_tool_output_tokens = Std 'tool_output_tokens'; mean_tool_calls = Mean 'tool_calls'
            mean_wall_ms = Mean 'wall_ms'; mean_api_ms = Mean 'api_ms'; mean_input_tokens = Mean 'input_tokens'; mean_output_tokens = Mean 'output_tokens'
        }
    }
    $payload = [ordered]@{
        meta = $Meta
        totals = [ordered]@{
            done = Count-Status 'done'; failed = Count-Status 'failed'; running = Count-Status 'running'; total = $Runs.Count; elapsed_ms = [int]([DateTime]::UtcNow - $StartTime).TotalMilliseconds
        }
        summary = [ordered]@{ baseline = Stats 'baseline'; tokenzero = Stats 'tokenzero' }; runs = $Runs
    }
    $temporary = $Paths.Results + '.tmp'; $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temporary -Encoding UTF8; Move-Item -LiteralPath $temporary -Destination $Paths.Results -Force
}

$Meta = [ordered]@{
    task = 'jsonrpc_errors'; model = $Model; replicates = $Replicates; conditions = $Conditions; repo = $RepoDir; started_at = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
}
$StartUtc = [DateTime]::UtcNow; Save-Results $Meta $plan $StartUtc; Write-Host "wrote initial: $($Paths.Results)"; $VizPath = Join-Path $DemoDir 'agent_viz.html'
if (-not (Test-Path $VizPath)) { & (Join-Path $DemoDir 'build_agent_viz.ps1') | Out-Null }

$serverProc = $null
if (-not $NoServe) {
    Write-Host "starting HTTP server on port $Port (serving $DemoDir)..."; $python = (Get-Command python -ErrorAction SilentlyContinue).Source
    if (-not $python) { $python = (Get-Command py -ErrorAction SilentlyContinue).Source }
    if (-not $python) { throw "python not found; pass -NoServe and serve $DemoDir yourself." }
    $serverProc = Start-Process -FilePath $python -ArgumentList '-u','-m','http.server',"$Port",'--bind','127.0.0.1' -WorkingDirectory $DemoDir -WindowStyle Hidden -PassThru; Start-Sleep -Milliseconds 600
    if ($serverProc.HasExited) { throw "HTTP server exited immediately (port $Port in use?)." }
    Write-Host "server PID: $($serverProc.Id)"
    if (-not $NoOpen) {
        $url = "http://127.0.0.1:$Port/agent_viz.html"; Write-Host "opening $url"; Start-Process $url
    }
}

function Invoke-CopilotRun {
    param([string] $Condition, [string] $JsonlPath, [scriptblock] $OnTick)
    $arguments = @('-p',$Prompt,'--output-format','json','--model',$Model,'--no-ask-user','--allow-all-paths','-C',$RepoDir,'--log-level','error')
    if ($Condition -eq 'baseline') {
        $arguments += '--allow-all-tools'
    } else {
        $arguments += @('--additional-mcp-config', "@$($Paths.Mcp)", '--allow-all-tools', '--excluded-tools', $NativeDeny)
    }
    $arguments += '--disable-builtin-mcps'
    foreach ($server in 'Azure','icm-mcp-prod','github') { $arguments += @('--disable-mcp-server', $server) }
    $stderrPath = "$JsonlPath.err"
    foreach ($path in $JsonlPath, $stderrPath) {
        if (Test-Path $path) { Remove-Item -LiteralPath $path -Force }
    }
    $quoted = ($arguments | ForEach-Object { ConvertTo-Win32CommandLineArg $_ }) -join ' '
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew(); $process = Start-Process -FilePath $CopilotPath -ArgumentList $quoted -WorkingDirectory $RepoDir -RedirectStandardOutput $JsonlPath -RedirectStandardError $stderrPath -NoNewWindow -PassThru
    while (-not $process.WaitForExit(2000)) {
        if ($OnTick) { try { & $OnTick $stopwatch.ElapsedMilliseconds $JsonlPath } catch {} }
        if ($stopwatch.Elapsed.TotalSeconds -le $PerRunTimeoutSec) { continue }
        try {
            if ($process.PSObject.Methods.Name -contains 'Kill') {
                try { $process.Kill($true) } catch { $process.Kill() }
            }
        } catch {
            try { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue } catch {}
        }
        if (-not $process.WaitForExit(5000)) { throw "timed out waiting for process $($process.Id) to exit after kill" }
        $stopwatch.Stop()
        return @{ exit_code = -1; wall_ms = $stopwatch.ElapsedMilliseconds; timed_out = $true; stderr = Read-FileOrEmpty $stderrPath }
    }
    $stopwatch.Stop()
    @{ exit_code = $process.ExitCode; wall_ms = $stopwatch.ElapsedMilliseconds; timed_out = $false; stderr = Read-FileOrEmpty $stderrPath }
}

function Get-MidRunProgress {
    param([string] $JsonlPath)
    if (-not (Test-Path $JsonlPath)) { return $null }
    $counts = @{ lines = 0; tool_calls = 0; messages = 0 }
    try {
        $reader = [System.IO.StreamReader]::new([System.IO.File]::Open($JsonlPath,'Open','Read','ReadWrite'), [System.Text.Encoding]::UTF8)
        try {
            while (-not $reader.EndOfStream) {
                $line = $reader.ReadLine()
                if (-not $line) { continue }
                $counts.lines++
                if ($line -match '"type"\s*:\s*"tool\.execution_start"') { $counts.tool_calls++ }
                elseif ($line -match '"type"\s*:\s*"assistant\.message"') { $counts.messages++ }
            }
        } finally { $reader.Dispose() }
    } catch { return $null }
    $counts
}

function Measure-ToolTokens {
    param([string] $Text)
    if (-not $Text.Length) { return 0 }
    $cachePath = Join-Path $Paths.Cache ('ingest-' + [Guid]::NewGuid().ToString('N') + '.json')
    try {
        $info = New-Object System.Diagnostics.ProcessStartInfo; $info.FileName = $BinaryPath
        Set-ProcessArguments $info @('ingest','--stdin','--json','--cache-path',$cachePath)
        $info.RedirectStandardInput = $true; $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true; $info.UseShellExecute = $false
        $info.CreateNoWindow = $true; $process = [System.Diagnostics.Process]::Start($info); $process.StandardInput.Write($Text); $process.StandardInput.Close()
        $output = $process.StandardOutput.ReadToEnd(); $process.WaitForExit()
        if ($output) {
            try {
                $json = $output | ConvertFrom-Json
                foreach ($value in $json.accounting.raw_tokens, $json.tokens, $json.token_count) {
                    if ($null -ne $value) { return [int] $value }
                }
            } catch { return [int]($Text.Length / 4) }
        }
    } catch { return [int]($Text.Length / 4) }
    0
}

function Parse-RunMetrics {
    param([string] $JsonlPath)
    $events = foreach ($line in Get-Content -LiteralPath $JsonlPath) {
        if ($line) { try { $line | ConvertFrom-Json } catch {} }
    }
    $messages = @($events | Where-Object { $_.type -eq 'assistant.message' })
    $outputTokens = ($messages | ForEach-Object { if ($_.data.outputTokens) { [int] $_.data.outputTokens } else { 0 } } | Measure-Object -Sum).Sum
    if (-not $outputTokens) { $outputTokens = 0 }
    $toolEvents = @($events | Where-Object { $_.type -eq 'tool.execution_complete' })
    $builder = New-Object System.Text.StringBuilder
    foreach ($event in $toolEvents) {
        $result = $event.data.result
        if ($null -eq $result) { continue }
        if ($result -is [string]) { Add-Text $builder $result; continue }
        if ($result.content -is [string]) { Add-Text $builder $result.content }
        elseif ($result.content) {
            foreach ($content in $result.content) {
                if ($content.text) { Add-Text $builder $content.text }
                elseif ($content -is [string]) { Add-Text $builder $content }
            }
        }
        if ($result.detailedContent -is [string]) { Add-Text $builder $result.detailedContent }
        elseif ($result.detailedContent) { Add-Text $builder ($result.detailedContent | ConvertTo-Json -Depth 6 -Compress) }
        if ($result.output) { Add-Text $builder $result.output }
    }
    $last = $events | Where-Object { $_.type -eq 'result' } | Select-Object -Last 1
    $usage = if ($last.usage) { $last.usage } elseif ($last.data) { $last.data.usage }
    [ordered]@{
        output_tokens = $outputTokens; input_tokens = $null; tool_calls = $toolEvents.Count; tool_output_tokens = Measure-ToolTokens $builder.ToString()
        api_ms = if ($null -ne $usage.totalApiDurationMs) { [int] $usage.totalApiDurationMs } else { $null }
        session_ms = if ($null -ne $usage.sessionDurationMs) { [int] $usage.sessionDurationMs } else { $null }
        premium_requests = if ($null -ne $usage.premiumRequests) { [int] $usage.premiumRequests } else { $null }
    }
}

function Copy-Metrics {
    param($Run, $Metrics, [switch] $NonNull)
    foreach ($key in $Metrics.Keys) {
        if (-not $NonNull -or $null -ne $Metrics[$key]) { $Run[$key] = $Metrics[$key] }
    }
}

Write-Host ""; Write-Host "starting $($plan.Count) runs..."
foreach ($run in $plan) {
    $tag = "{0}-r{1}" -f $run.condition, $run.replicate
    $jsonl = Join-Path $Paths.Runs "$tag.jsonl"; $run.jsonl_path = $jsonl; $run.status = 'running'
    Save-Results $Meta $plan $StartUtc; Write-Host "  [$($run.index)/$($plan.Count)] $tag ... " -NoNewline
    $result = Invoke-CopilotRun $run.condition $jsonl {
        param($elapsed, $path)
        $progress = Get-MidRunProgress $path
        if ($progress) {
            $run.note = "live: $($progress.lines) events, $($progress.tool_calls) tool calls, $($progress.messages) msgs ($([Math]::Round($elapsed / 1000))s)"; $run.wall_ms = [int] $elapsed; $run.tool_calls = $progress.tool_calls
            Save-Results $Meta $plan $StartUtc
        }
    }
    $run.wall_ms = [int] $result.wall_ms; $run.exit_code = $result.exit_code; $wallSec = [Math]::Round($result.wall_ms / 1000, 1)
    if ($result.timed_out) {
        $run.status = 'failed'; $run.note = "timeout @ $PerRunTimeoutSec s"; Write-Host "TIMEOUT ($wallSec s)"
        try {
            Copy-Metrics $run (Parse-RunMetrics $jsonl) -NonNull; $run.note += " (partial: $($run.tool_calls) tool calls, $($run.output_tokens) out tok)"
        } catch {}
    } elseif ($result.exit_code -ne 0) {
        $run.status = 'failed'
        $errorLine = $result.stderr -split [char]10 | Where-Object { $_ } | Select-Object -First 1
        if (-not $errorLine) { $errorLine = '(no stderr)' }
        $run.note = "exit=$($result.exit_code) $errorLine"; Write-Host "FAILED exit=$($result.exit_code) ($wallSec s)"
    } else {
        try {
            Copy-Metrics $run (Parse-RunMetrics $jsonl); $run.status = 'done'
            $apiSec = if ($run.api_ms) { [Math]::Round($run.api_ms / 1000, 1) } else { 'n/a' }
            Write-Host "OK  wall=$($wallSec)s api=$($apiSec)s tools=$($run.tool_calls) toolTok=$($run.tool_output_tokens) outTok=$($run.output_tokens)"
        } catch {
            $run.status = 'failed'; $run.note = "parse error: $($_.Exception.Message)"; Write-Host "PARSE-FAIL: $($_.Exception.Message)"
        }
    }
    Save-Results $Meta $plan $StartUtc
}

Write-Host ""; Write-Host "all runs complete. results: $($Paths.Results)"
if (-not $NoServe -and $serverProc -and -not $serverProc.HasExited) {
    Write-Host "HTTP server still running on port $Port (PID $($serverProc.Id))."; Write-Host "Stop with: Stop-Process -Id $($serverProc.Id)"
}
