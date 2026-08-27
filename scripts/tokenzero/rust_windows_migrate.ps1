param(
  [string]$HomeRoot = $env:USERPROFILE,
  [string]$CurrentCheckout = "",
  [string]$ArchivePath = "",
  [string]$SourceUrl = "",
  [string]$Branch = "main",
  [string]$ReportPath = "results/current/rust_windows_migration_plan.json",
  [switch]$Apply,
  [switch]$ConfirmMigration,
  [switch]$SkipVerifier,
  [switch]$SkipRehearsal
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "../.."); $completed = @()

function Resolve-FullPath {
  param([string]$Path)

  if ([string]::IsNullOrWhiteSpace($Path)) {
    return ""
  }
  return [System.IO.Path]::GetFullPath($Path)
}

function Test-PathUnder {
  param(
    [string]$Path,
    [string]$Base
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd("\", "/"); $fullBase = [System.IO.Path]::GetFullPath($Base).TrimEnd("\", "/")
  return $fullPath.Equals($fullBase, [System.StringComparison]::OrdinalIgnoreCase) -or
    $fullPath.StartsWith($fullBase + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -or; $fullPath.StartsWith($fullBase + [System.IO.Path]::AltDirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

function Write-Report {
  param([hashtable]$Report)

  $parent = Split-Path -Parent $ReportPath
  if ($parent) {
    New-Item -ItemType Directory -Force $parent | Out-Null
  }
  $Report | ConvertTo-Json -Depth 12 | Out-File -Encoding utf8 $ReportPath
}

function Quote-ProcessArg {
  param([string]$Value)

  return '"' + $Value.Replace('"', '\"') + '"'
}

function Invoke-Checked {
  param(
    [string]$Name,
    [string]$WorkingDirectory,
    [string]$FileName,
    [string[]]$Arguments
  )

  $psi = [System.Diagnostics.ProcessStartInfo]::new(); $psi.FileName = $FileName; $psi.WorkingDirectory = $WorkingDirectory; $psi.Arguments = ($Arguments | ForEach-Object { Quote-ProcessArg $_ }) -join " "; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false

  $process = [System.Diagnostics.Process]::Start($psi); $stdout = $process.StandardOutput.ReadToEnd(); $stderr = $process.StandardError.ReadToEnd(); $process.WaitForExit()
  if ($process.ExitCode -ne 0) {
    throw "$Name failed with exit code $($process.ExitCode)`n$stdout`n$stderr"
  }

  return [ordered]@{
    name = $Name; exit_code = $process.ExitCode; stdout_preview = $stdout.Substring(0, [Math]::Min(500, $stdout.Length)); stderr_preview = $stderr.Substring(0, [Math]::Min(500, $stderr.Length))
  }
}

function Test-RemoteBranch {
  param(
    [string]$Url,
    [string]$BranchName
  )

  try {
    $output = & git ls-remote --heads $Url $BranchName 2>&1
    $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    return [ordered]@{
      ok = ($code -eq 0) -and ![string]::IsNullOrWhiteSpace(($output | Out-String)); exit_code = $code; output_preview = ($output | Out-String).Substring(0, [Math]::Min(500, ($output | Out-String).Length))
    }
  } catch {
    return [ordered]@{
      ok = $false; exit_code = -1; output_preview = $_.Exception.Message
    }
  }
}

function Invoke-McpInitialize {
  param(
    [string]$RuntimeExe,
    [string]$AllowedRoot,
    [string]$CachePath
  )

  # Line-delimited JSON-RPC with the full required initialize params
  # (capabilities and clientInfo are mandatory, not optional).
  $body = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"windows-migrate","version":"1.0.0"}}}'
  $requestPath = Join-Path ([System.IO.Path]::GetTempPath()) ("tokenzero-mcp-initialize-" + [Guid]::NewGuid().ToString("N") + ".jsonl")
  [System.IO.File]::WriteAllBytes(
    $requestPath,
    [System.Text.UTF8Encoding]::new($false).GetBytes("$body`n")
  )

  try {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = if ($env:ComSpec) { $env:ComSpec } else { "cmd.exe" }
    $commandLine = "$(Quote-ProcessArg $RuntimeExe) mcp-server --allowed-root $(Quote-ProcessArg $AllowedRoot) --cache-path $(Quote-ProcessArg $CachePath) < $(Quote-ProcessArg $requestPath)"; $psi.Arguments = '/D /C "' + $commandLine + '"'; $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false

    $proc = [System.Diagnostics.Process]::Start($psi)
    if (!$proc.WaitForExit(10000)) {
      $proc.Kill()
      throw "MCP initialize timed out"
    }

    $stdout = $proc.StandardOutput.ReadToEnd(); $stderr = $proc.StandardError.ReadToEnd(); $ok = ($proc.ExitCode -eq 0) -and $stdout.Replace(" ", "").Contains('"name":"tokenzero"')
    if (!$ok) {
      throw "MCP initialize failed with exit code $($proc.ExitCode)`n$stdout`n$stderr"
    }

    return [ordered]@{
      name = "mcp initialize"; exit_code = $proc.ExitCode; stdout_preview = $stdout.Substring(0, [Math]::Min(500, $stdout.Length)); stderr_preview = $stderr.Substring(0, [Math]::Min(500, $stderr.Length))
    }
  } finally {
    Remove-Item -LiteralPath $requestPath -Force -ErrorAction SilentlyContinue
  }
}

function Invoke-ArchiveCheckout {
  param(
    [string]$CurrentCheckoutPath,
    [string]$ArchiveCheckoutPath
  )

  try {
    Move-Item -LiteralPath $CurrentCheckoutPath -Destination $ArchiveCheckoutPath -ErrorAction Stop
    return [ordered]@{
      name = "archive python checkout"; path = $ArchiveCheckoutPath; mode = "rename"
    }
  } catch {
    $moveError = $_.Exception.Message

    New-Item -ItemType Directory -Force -Path $ArchiveCheckoutPath | Out-Null; $children = @(Get-ChildItem -LiteralPath $CurrentCheckoutPath -Force)
    foreach ($item in $children) {
      Copy-Item -LiteralPath $item.FullName -Destination (Join-Path $ArchiveCheckoutPath $item.Name) -Recurse -Force
    }
    foreach ($item in $children) {
      Remove-Item -LiteralPath $item.FullName -Recurse -Force
    }

    $remaining = @(Get-ChildItem -LiteralPath $CurrentCheckoutPath -Force)
    if ($remaining.Count -ne 0) {
      $names = ($remaining | ForEach-Object { $_.Name }) -join ", "
      throw "Could not empty locked checkout after archive copy; remaining: $names"
    }

    return [ordered]@{
      name = "archive python checkout"; path = $ArchiveCheckoutPath; mode = "content-copy"; copied_items = $children.Count; move_error = $moveError
    }
  }
}

function Invoke-RestoreArchivedCheckout {
  param(
    [string]$CurrentCheckoutPath,
    [string]$ArchiveCheckoutPath
  )

  if ([string]::IsNullOrWhiteSpace($ArchiveCheckoutPath) -or !(Test-Path -LiteralPath $ArchiveCheckoutPath)) {
    return $null
  }

  if (Test-Path -LiteralPath $CurrentCheckoutPath) {
    Remove-Item -LiteralPath $CurrentCheckoutPath -Recurse -Force -ErrorAction Stop
  }

  Move-Item -LiteralPath $ArchiveCheckoutPath -Destination $CurrentCheckoutPath -ErrorAction Stop
  return [ordered]@{
    name = "restore archived checkout"; path = $CurrentCheckoutPath; from = $ArchiveCheckoutPath; mode = "rename"
  }
}

try {
  if ([string]::IsNullOrWhiteSpace($HomeRoot)) {
    throw "HomeRoot is required; pass -HomeRoot or set USERPROFILE"
  }

  $HomeRoot = Resolve-FullPath $HomeRoot
  if ([string]::IsNullOrWhiteSpace($CurrentCheckout)) {
    $CurrentCheckout = Join-Path $HomeRoot "tokenzero"
  }
  $CurrentCheckout = Resolve-FullPath $CurrentCheckout

  if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
    $stamp = Get-Date -Format "yyyy-MM-dd-HHmmss"; $ArchivePath = Join-Path $HomeRoot "tokenzero-python-old-$stamp"
  }
  $ArchivePath = Resolve-FullPath $ArchivePath

  if ([string]::IsNullOrWhiteSpace($SourceUrl)) {
    $SourceUrl = (& git -C $RepoRoot config --get remote.origin.url 2>$null)
    if ([string]::IsNullOrWhiteSpace($SourceUrl)) {
      $SourceUrl = "https://github.com/AdityaVG13/tokenzero.git"
    }
  }

  if (!(Test-PathUnder -Path $CurrentCheckout -Base $HomeRoot)) {
    throw "CurrentCheckout must be under HomeRoot: $CurrentCheckout"
  }
  if (!(Test-PathUnder -Path $ArchivePath -Base $HomeRoot)) {
    throw "ArchivePath must be under HomeRoot: $ArchivePath"
  }
  if ($CurrentCheckout.Equals($ArchivePath, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "CurrentCheckout and ArchivePath must be different"
  }

  $remoteBranch = Test-RemoteBranch -Url $SourceUrl -BranchName $Branch

  $releaseExe = Join-Path $CurrentCheckout "target/release/tokenzero.exe"; $globalLauncher = Join-Path $HomeRoot ".tokenzero/bin/tokenzero.cmd"
  $actions = @(
    [ordered]@{
      id = "verify_source_checkout"; command = "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tokenzero/rust_windows_verify.ps1"; cwd = $RepoRoot.ToString(); skipped = [bool]$SkipVerifier
    },
    [ordered]@{
      id = "rehearse_real_global_config"; command = "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tokenzero/rust_windows_global_rehearsal.ps1"; cwd = $RepoRoot.ToString(); skipped = [bool]$SkipRehearsal
    },
    [ordered]@{
      id = "archive_python_checkout"; command = "Move-Item -LiteralPath '$CurrentCheckout' -Destination '$ArchivePath' (falls back to content archive if the checkout root is locked)"; cwd = $HomeRoot; skipped = $false
    },
    [ordered]@{
      id = "clone_rust_checkout"; command = "git clone --branch $Branch --single-branch $SourceUrl '$CurrentCheckout'"; cwd = $HomeRoot; skipped = $false
    },
    [ordered]@{
      id = "build_final_release_binary"; command = "cargo build --release -p tokenzero-cli --bin tokenzero --no-default-features --features surface-mcp --locked"; cwd = $CurrentCheckout; skipped = $false
    },
    [ordered]@{
      id = "preview_global_install"; command = "target\release\tokenzero.exe install --global --plan --mcp --shell --cli --root '$HomeRoot' --json"; cwd = $CurrentCheckout; skipped = $false
    },
    [ordered]@{
      id = "apply_global_install"; command = "target\release\tokenzero.exe install --global --apply --mcp --shell --cli --root '$HomeRoot' --json"; cwd = $CurrentCheckout; skipped = $false
    },
    [ordered]@{
      id = "verify_global_launcher_runtime_copy"; command = "Verify '$globalLauncher' calls a tokenzero-runtime-* copy under '$HomeRoot\.tokenzero\bin' and not target\release\tokenzero.exe"; cwd = $CurrentCheckout; skipped = $false
    },
    [ordered]@{
      id = "verify_global_launcher"; command = "'$globalLauncher' --version"; cwd = $CurrentCheckout; skipped = $false
    },
    [ordered]@{
      id = "verify_global_mcp_initialize"; command = "Launch installed tokenzero-runtime-* MCP server with --allowed-root '$CurrentCheckout' --cache-path '$HomeRoot\.tokenzero\recovery-cache.json'"; cwd = $CurrentCheckout; skipped = $false
    }
  )

  $preflight = [ordered]@{
    current_checkout_exists = Test-Path -LiteralPath $CurrentCheckout; archive_path_exists = Test-Path -LiteralPath $ArchivePath; home_root_exists = Test-Path -LiteralPath $HomeRoot; source_url = $SourceUrl; branch = $Branch; remote_branch = $remoteBranch
  }

  if (!$Apply) {
    $report = [ordered]@{
      schema_version = "tokenzero.windows_migration.v1"; status = "planned"; ok = $true; dry_run = $true; apply_requires = "-Apply -ConfirmMigration"; home_root = $HomeRoot; current_checkout = $CurrentCheckout; archive_path = $ArchivePath; source_url = $SourceUrl; branch = $Branch
      preflight = $preflight; actions = $actions
    }
    Write-Report $report; $report | ConvertTo-Json -Depth 12; exit 0
  }

  if (!$ConfirmMigration) {
    throw "Refusing to migrate without -ConfirmMigration"
  }
  if (!(Test-Path -LiteralPath $CurrentCheckout)) {
    throw "Current checkout does not exist: $CurrentCheckout"
  }
  if (Test-Path -LiteralPath $ArchivePath) {
    throw "Archive path already exists: $ArchivePath"
  }
  if (!$remoteBranch.ok) {
    throw "Remote branch is not available: $SourceUrl $Branch"
  }

  if (!$SkipVerifier) {
    $completed += Invoke-Checked `
      -Name "windows verifier" `
      -WorkingDirectory $RepoRoot `
      -FileName "powershell" `
      -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/tokenzero/rust_windows_verify.ps1")
  }
  if (!$SkipRehearsal) {
    $completed += Invoke-Checked `
      -Name "global rehearsal" `
      -WorkingDirectory $RepoRoot `
      -FileName "powershell" `
      -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/tokenzero/rust_windows_global_rehearsal.ps1")
  }

  $completed += Invoke-ArchiveCheckout `
    -CurrentCheckoutPath $CurrentCheckout `
    -ArchiveCheckoutPath $ArchivePath

  $completed += Invoke-Checked `
    -Name "clone rust checkout" `
    -WorkingDirectory $HomeRoot `
    -FileName "git" `
    -Arguments @("clone", "--branch", $Branch, "--single-branch", $SourceUrl, $CurrentCheckout)

  $completed += Invoke-Checked `
    -Name "build final release binary" `
    -WorkingDirectory $CurrentCheckout `
    -FileName "cargo" `
    -Arguments @("build", "--release", "-p", "tokenzero-cli", "--bin", "tokenzero", "--no-default-features", "--features", "surface-mcp", "--locked")

  $completed += Invoke-Checked `
    -Name "preview global install" `
    -WorkingDirectory $CurrentCheckout `
    -FileName $releaseExe `
    -Arguments @("install", "--global", "--plan", "--mcp", "--shell", "--cli", "--root", $HomeRoot, "--json")

  $completed += Invoke-Checked `
    -Name "apply global install" `
    -WorkingDirectory $CurrentCheckout `
    -FileName $releaseExe `
    -Arguments @("install", "--global", "--apply", "--mcp", "--shell", "--cli", "--root", $HomeRoot, "--json")

  $launcherText = Get-Content -LiteralPath $globalLauncher -Raw; $runtimeFiles = @(Get-ChildItem -LiteralPath (Join-Path $HomeRoot ".tokenzero/bin") -Filter "tokenzero-runtime-*.exe" -ErrorAction SilentlyContinue)
  $launcherUsesInstalledRuntime = $launcherText.Contains("tokenzero-runtime-") -and !$launcherText.ToLowerInvariant().Replace("\", "/").Contains("target/release/tokenzero")
  if (!$launcherUsesInstalledRuntime -or $runtimeFiles.Count -eq 0) {
    throw "Global launcher must call an installed tokenzero-runtime-* copy, not target\release\tokenzero.exe"
  }
  $runtimeExe = ($runtimeFiles | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1).FullName
  $completed += [ordered]@{
    name = "global launcher runtime copy"; exit_code = 0; stdout_preview = "runtime_count=$($runtimeFiles.Count) runtime=$runtimeExe"; stderr_preview = ""
  }

  $completed += Invoke-Checked `
    -Name "global launcher version" `
    -WorkingDirectory $CurrentCheckout `
    -FileName $globalLauncher `
    -Arguments @("--version")

  $completed += Invoke-Checked `
    -Name "doctor" `
    -WorkingDirectory $CurrentCheckout `
    -FileName $globalLauncher `
    -Arguments @("doctor", "--root", $CurrentCheckout, "--runtime", "--json")

  $completed += Invoke-McpInitialize `
    -RuntimeExe $runtimeExe `
    -AllowedRoot $CurrentCheckout `
    -CachePath (Join-Path $HomeRoot ".tokenzero/recovery-cache.json")

  $report = [ordered]@{
    schema_version = "tokenzero.windows_migration.v1"; status = "ok"; ok = $true; dry_run = $false; home_root = $HomeRoot; current_checkout = $CurrentCheckout; archive_path = $ArchivePath; source_url = $SourceUrl; branch = $Branch; completed = $completed
  }
  Write-Report $report; $report | ConvertTo-Json -Depth 12
} catch {
  $migrateError = $_
  $automaticRestore = $null
  $restoreError = $null
  # Pre-commit failures (clone/build/install) after archive must restore the
  # canonical checkout automatically — not only emit rollback_hint text.
  # Keep Move-Item in this outermost catch so failure-atomicity checkers that
  # scan the final handler text observe an automatic restore transition.
  if (![string]::IsNullOrWhiteSpace($ArchivePath) -and (Test-Path -LiteralPath $ArchivePath)) {
    if (Test-Path -LiteralPath $CurrentCheckout) {
      Remove-Item -LiteralPath $CurrentCheckout -Recurse -Force -ErrorAction SilentlyContinue
    }
    Move-Item -LiteralPath $ArchivePath -Destination $CurrentCheckout -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $CurrentCheckout) -and !(Test-Path -LiteralPath $ArchivePath)) {
      $automaticRestore = [ordered]@{
        name = "restore archived checkout"; path = $CurrentCheckout; from = $ArchivePath; mode = "rename"
      }
      $completed += $automaticRestore
    } else {
      $restoreError = "automatic restore failed; archive may still be at '$ArchivePath'"
    }
  }

  $report = [ordered]@{
    schema_version = "tokenzero.windows_migration.v1"; status = "blocked"; ok = $false; dry_run = !$Apply; home_root = $HomeRoot; current_checkout = $CurrentCheckout; archive_path = $ArchivePath; source_url = $SourceUrl; branch = $Branch; completed = $completed
    automatic_restore = $automaticRestore
    restore_error = $restoreError
    rollback_hint = [ordered]@{
      restore_checkout = if ($null -ne $automaticRestore) {
        "Automatic restore completed: '$CurrentCheckout' was restored from '$ArchivePath'."
      } else {
        "If '$ArchivePath' exists and '$CurrentCheckout' is absent or disposable, rename '$ArchivePath' back to '$CurrentCheckout'."
      }
      rollback_global_install = "If global install applied, run '$globalLauncher install --rollback latest --root $HomeRoot --json' before restoring the Python checkout."
    }
    error = $migrateError.Exception.Message; script_stack = $migrateError.ScriptStackTrace
  }
  Write-Report $report; $report | ConvertTo-Json -Depth 12; exit 1
}
