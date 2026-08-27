param(
  [string]$Root = $env:USERPROFILE,
  [string]$TokenZeroExe = "",
  [string]$ReportPath = "results/current/rust_windows_global_rehearsal.json",
  [switch]$SkipBuild,
  [switch]$KeepTemp
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "../.."); Push-Location $RepoRoot

function Write-Report {
  param([hashtable]$Report)

  $parent = Split-Path -Parent $ReportPath
  if ($parent) {
    New-Item -ItemType Directory -Force $parent | Out-Null
  }
  $Report | ConvertTo-Json -Depth 10 | Out-File -Encoding utf8 $ReportPath
}

function Invoke-TokenZeroJson {
  param(
    [string[]]$CommandArgs
  )

  $output = & $script:TokenZeroExe @CommandArgs 2>&1
  $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
  if ($code -ne 0) {
    throw "tokenzero $($CommandArgs -join ' ') failed with exit code $code`n$output"
  }
  return ($output | Out-String | ConvertFrom-Json)
}

function Get-RelativePathUnder {
  param(
    [string]$Path,
    [string]$Base
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path); $fullBase = [System.IO.Path]::GetFullPath($Base).TrimEnd("\", "/")
  if (!$fullPath.StartsWith($fullBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "path is outside root: $fullPath"
  }
  return $fullPath.Substring($fullBase.Length).TrimStart("\", "/")
}

function Test-JsonFile {
  param([string]$Path)

  $text = Get-Content -LiteralPath $Path -Raw -ErrorAction Stop
  if ([string]::IsNullOrWhiteSpace($text)) {
    $text = "{}"
  }
  $null = $text | ConvertFrom-Json -ErrorAction Stop
}

function Quote-CmdArg {
  param([string]$Value)

  return '"' + $Value.Replace('"', '\"') + '"'
}

function Invoke-McpInitialize {
  param(
    [string]$Command,
    [string]$AllowedRoot,
    [string]$CachePath
  )

  # Line-delimited JSON-RPC with the full required initialize params
  # (capabilities and clientInfo are mandatory, not optional).
  $request = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"windows-rehearsal","version":"1.0.0"}}}'
  $requestPath = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-mcp-initialize-" + [Guid]::NewGuid().ToString("N") + ".jsonl")
  [System.IO.File]::WriteAllBytes(
    $requestPath,
    [System.Text.UTF8Encoding]::new($false).GetBytes("$request`n")
  )

  try {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = if ($env:ComSpec) { $env:ComSpec } else { "cmd.exe" }
    $commandLine = "$(Quote-CmdArg $Command) mcp-server --allowed-root $(Quote-CmdArg $AllowedRoot) --cache-path $(Quote-CmdArg $CachePath) < $(Quote-CmdArg $requestPath)"; $psi.Arguments = '/D /C "' + $commandLine + '"'; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false

    $proc = [System.Diagnostics.Process]::Start($psi)
    if (!$proc.WaitForExit(30000)) {
      $proc.Kill()
      return [ordered]@{
        ok = $false; command = $Command; exit_code = $null; timed_out = $true; stdout_preview = ""; stderr_preview = ""
      }
    }

    $stdout = $proc.StandardOutput.ReadToEnd(); $stderr = $proc.StandardError.ReadToEnd(); $ok = ($proc.ExitCode -eq 0) -and $stdout.Replace(" ", "").Contains('"name":"tokenzero"')

    return [ordered]@{
      ok = $ok; command = $Command; exit_code = $proc.ExitCode; timed_out = $false; stdout_preview = $stdout.Substring(0, [Math]::Min(300, $stdout.Length)); stderr_preview = $stderr.Substring(0, [Math]::Min(300, $stderr.Length))
    }
  } finally {
    Remove-Item -LiteralPath $requestPath -Force -ErrorAction SilentlyContinue
  }
}

try {
  if ([string]::IsNullOrWhiteSpace($Root)) {
    throw "Root is required; pass -Root or set USERPROFILE"
  }
  $Root = [System.IO.Path]::GetFullPath($Root)

  if ([string]::IsNullOrWhiteSpace($TokenZeroExe)) {
    $TokenZeroExe = Join-Path $RepoRoot "target/release/tokenzero.exe"
  }
  $script:TokenZeroExe = [System.IO.Path]::GetFullPath($TokenZeroExe)

  if (!(Test-Path -LiteralPath $script:TokenZeroExe) -and !$SkipBuild) {
    cargo build --release -p tokenzero-cli --bin tokenzero --no-default-features --features surface-mcp --locked
    if ($LASTEXITCODE -ne 0) {
      throw "cargo build failed with exit code $LASTEXITCODE"
    }
  }
  if (!(Test-Path -LiteralPath $script:TokenZeroExe)) {
    throw "tokenzero.exe not found at $script:TokenZeroExe"
  }

  $plan = Invoke-TokenZeroJson -CommandArgs @(
    "install",
    "--global",
    "--plan",
    "--mcp",
    "--shell",
    "--cli",
    "--root",
    $Root,
    "--json"
  )

  $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero global rehearsal " + [Guid]::NewGuid().ToString("N")); New-Item -ItemType Directory -Force $tempRoot | Out-Null

  $copied = @(); $jsonParseFailures = @()
  foreach ($write in $plan.writes) {
    if ($write.action -ne "merge") {
      continue
    }
    if (!(Test-Path -LiteralPath $write.path)) {
      continue
    }

    if ([System.IO.Path]::GetExtension($write.path).ToLowerInvariant() -eq ".json") {
      try {
        Test-JsonFile $write.path
      } catch {
        $jsonParseFailures += [ordered]@{
          path = $write.path; error = $_.Exception.Message
        }
      }
    }

    $relative = Get-RelativePathUnder -Path $write.path -Base $Root; $destination = Join-Path $tempRoot $relative; New-Item -ItemType Directory -Force (Split-Path -Parent $destination) | Out-Null; Copy-Item -LiteralPath $write.path -Destination $destination -Force; $copied += $relative
  }

  if ($jsonParseFailures.Count -gt 0) {
    throw "existing JSON merge config is invalid"
  }

  $applied = Invoke-TokenZeroJson -CommandArgs @(
    "install",
    "--global",
    "--apply",
    "--mcp",
    "--shell",
    "--cli",
    "--root",
    $tempRoot,
    "--json"
  )

  $rehearsalPlan = Invoke-TokenZeroJson -CommandArgs @(
    "install",
    "--global",
    "--plan",
    "--mcp",
    "--shell",
    "--cli",
    "--root",
    $tempRoot,
    "--json"
  )

  $postMergeFailures = @(); $tokenzeroEntries = 0
  foreach ($write in $rehearsalPlan.writes) {
    if (($write.action -ne "merge") -or !(Test-Path -LiteralPath $write.path)) {
      continue
    }
    $extension = [System.IO.Path]::GetExtension($write.path).ToLowerInvariant()
    try {
      if ($extension -eq ".json") {
        $parsed = Get-Content -LiteralPath $write.path -Raw | ConvertFrom-Json
        if ($null -ne $parsed.mcpServers.tokenzero) {
          $tokenzeroEntries += 1
        }
      } elseif ($extension -eq ".toml") {
        $text = Get-Content -LiteralPath $write.path -Raw
        if ($text.Contains("[mcp_servers.tokenzero]")) {
          $tokenzeroEntries += 1
        }
      }
    } catch {
      $postMergeFailures += [ordered]@{
        path = $write.path; error = $_.Exception.Message
      }
    }
  }

  $launcher = Join-Path $tempRoot ".tokenzero/bin/tokenzero.cmd"; $cachePath = Join-Path $tempRoot ".tokenzero/recovery-cache.json"; $launcherText = Get-Content -LiteralPath $launcher -Raw
  $runtimeFiles = @(Get-ChildItem -LiteralPath (Join-Path $tempRoot ".tokenzero/bin") -Filter "tokenzero-runtime-*.exe" -ErrorAction SilentlyContinue)
  $launcherUsesInstalledRuntime = $launcherText.Contains("tokenzero-runtime-") -and !$launcherText.ToLowerInvariant().Replace("\", "/").Contains("target/release/tokenzero")
  $mcpCommand = if ($runtimeFiles.Count -gt 0) { $runtimeFiles[0].FullName } else { $launcher }
  $mcp = Invoke-McpInitialize -Command $mcpCommand -AllowedRoot $tempRoot -CachePath $cachePath

  $ok = ($applied.status -eq "ok") -and ($postMergeFailures.Count -eq 0) -and $mcp.ok -and $launcherUsesInstalledRuntime -and ($runtimeFiles.Count -gt 0)
  $report = [ordered]@{
    schema_version = "tokenzero.windows_global_rehearsal.v1"
    status = if ($ok) { "ok" } else { "blocked" }
    ok = $ok; root = $Root; tokenzero_exe = $script:TokenZeroExe; fake_root_contains_space = $tempRoot.Contains(" "); plan_write_count = $plan.writes.Count; copied_existing_merge_configs = $copied.Count; apply_status = $applied.status; applied_written_count = $applied.written.Count
    launcher_uses_installed_runtime = $launcherUsesInstalledRuntime; installed_runtime_count = $runtimeFiles.Count; json_parse_failures = $jsonParseFailures; post_merge_failures = $postMergeFailures; tokenzero_entries_after_merge = $tokenzeroEntries; mcp_launcher = $mcp
    temp_root = if ($KeepTemp) { $tempRoot } else { $null }
  }
  Write-Report $report; $report | ConvertTo-Json -Depth 10

  if (!$KeepTemp) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force
  }

  if (!$ok) {
    exit 1
  }
} catch {
  $report = [ordered]@{
    schema_version = "tokenzero.windows_global_rehearsal.v1"; status = "blocked"; ok = $false; root = $Root; tokenzero_exe = $script:TokenZeroExe; error = $_.Exception.Message; script_stack = $_.ScriptStackTrace
  }
  Write-Report $report; $report | ConvertTo-Json -Depth 10; exit 1
} finally {
  Pop-Location
}
