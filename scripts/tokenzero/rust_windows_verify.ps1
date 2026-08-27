$ErrorActionPreference = "Continue"

$env:CARGO_TARGET_DIR = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target\windows-verify" }

New-Item -ItemType Directory -Force results/current | Out-Null

$script:Steps = @()
$script:Artifacts = @(
  "results/current/rust_windows_cargo_test.log",
  "results/current/rust_windows_cargo_build.log",
  "results/current/rust_adaptive_scorecard.json",
  "results/current/rust_adaptive_scorecard.log",
  "results/current/rust_shell_matrix_windows.json",
  "results/current/rust_shell_matrix_windows.md",
  "results/current/rust_shell_matrix_windows.log",
  "results/current/rust_package_audit_windows.json",
  "results/current/rust_package_audit_windows.log",
  "results/current/rust_windows_global_rehearsal.json",
  "results/current/rust_windows_global_rehearsal.log",
  "results/current/rust_windows_migration_plan.json",
  "results/current/rust_windows_migration_plan.log",
  "results/current/rust_windows_verify.json"
)

function Add-Step {
  param(
    [string]$Name,
    [bool]$Ok,
    [int]$ExitCode,
    [string]$Log
  )
  $script:Steps += [ordered]@{
    name = $Name; ok = $Ok; exit_code = $ExitCode; log = $Log
  }
}

function Write-Report {
  param(
    [string]$Status,
    [bool]$Ok
  )
  $report = [ordered]@{
    schema_version = "tokenzero.windows_verify.v1"; status = $Status; ok = $Ok; host = $env:COMPUTERNAME; os = [System.Environment]::OSVersion.VersionString; cargo = (& cargo --version); rustc = (& rustc --version); target_dir = $env:CARGO_TARGET_DIR; steps = $script:Steps
    artifacts = $script:Artifacts
  }
  $report | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 results/current/rust_windows_verify.json
}

function Write-DiagnosticArtifact {
  param(
    [string]$Label,
    [string]$Path
  )
  if ([string]::IsNullOrWhiteSpace($Path) -or !(Test-Path -LiteralPath $Path)) {
    return
  }

  Write-Host "::group::$Label ($Path)"; Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue | Write-Host; Write-Host "::endgroup::"
}

function Run-Step {
  param(
    [string]$Name,
    [string]$Log,
    [scriptblock]$Block
  ); $global:LASTEXITCODE = 0; $output = & $Block 2>&1
  $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
  $output | Out-File -Encoding utf8 $Log; Add-Step -Name $Name -Ok ($code -eq 0) -ExitCode $code -Log $Log
  if ($code -ne 0) {
    Write-Report -Status "blocked" -Ok $false; Write-DiagnosticArtifact -Label "$Name log" -Path $Log; Write-DiagnosticArtifact -Label "windows verifier report" -Path "results/current/rust_windows_verify.json"
    throw "$Name failed with exit code $code"
  }
}

function Run-Json-Step {
  param(
    [string]$Name,
    [string]$JsonPath,
    [string]$Log,
    [scriptblock]$Block
  ); $global:LASTEXITCODE = 0; $stdout = & $Block
  $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
  $stdout | Out-File -Encoding utf8 $JsonPath; $stdout | Out-File -Encoding utf8 $Log; Add-Step -Name $Name -Ok ($code -eq 0) -ExitCode $code -Log $Log
  if ($code -ne 0) {
    Write-Report -Status "blocked" -Ok $false; Write-DiagnosticArtifact -Label "$Name output" -Path $JsonPath; Write-DiagnosticArtifact -Label "$Name log" -Path $Log; Write-DiagnosticArtifact -Label "windows verifier report" -Path "results/current/rust_windows_verify.json"
    throw "$Name failed with exit code $code"
  }
}

Run-Step "cargo test workspace" "results/current/rust_windows_cargo_test.log" {
  cargo test --workspace --locked
}

Run-Step "cargo build canonical release artifacts" "results/current/rust_windows_cargo_build.log" {
  cargo build -p tokenzero-cli --bin tokenzero --no-default-features --locked --release
  if ($LASTEXITCODE -eq 0) {
    cargo build -p tokenzero-worker --bin tokenzero-codemode --no-default-features --locked --release
  }
  if ($LASTEXITCODE -eq 0) {
    cargo build -p tokenzero-cli --bin tokenzero --no-default-features --features surface-mcp --locked --release --target-dir "$env:CARGO_TARGET_DIR\mcp-compat"
  }
}

$tokenzero = Join-Path (Get-Location) "$env:CARGO_TARGET_DIR\release\tokenzero.exe"
$worker = Join-Path (Get-Location) "$env:CARGO_TARGET_DIR\release\tokenzero-codemode.exe"
$mcpCompat = Join-Path (Get-Location) "$env:CARGO_TARGET_DIR\mcp-compat\release\tokenzero.exe"
foreach ($artifact in @($tokenzero, $worker, $mcpCompat)) {
  if (!(Test-Path $artifact)) {
    Add-Step -Name "resolve release artifact" -Ok $false -ExitCode 1 -Log ""; Write-Report -Status "blocked" -Ok $false
    throw "release artifact not found at $artifact"
  }
}

Run-Json-Step "adaptive scorecard" `
  "results/current/rust_adaptive_scorecard.json" `
  "results/current/rust_adaptive_scorecard.log" {
  powershell -NoProfile -ExecutionPolicy Bypass `
    -File scripts/tokenzero/rust_adaptive_scorecard.ps1 `
    -TokenZeroExe $tokenzero `
    -ReportPath results/current/rust_adaptive_scorecard.json
}

Run-Step "shell matrix" "results/current/rust_shell_matrix_windows.log" {
  & $tokenzero shell-matrix `
    --output-json results/current/rust_shell_matrix_windows.json `
    --output-md results/current/rust_shell_matrix_windows.md `
    --json
}

Run-Json-Step "package audit" `
  "results/current/rust_package_audit_windows.json" `
  "results/current/rust_package_audit_windows.log" {
  & $tokenzero package-audit `
    --dist "$env:CARGO_TARGET_DIR\release" `
    --json
}

Run-Json-Step "global install rehearsal" `
  "results/current/rust_windows_global_rehearsal.json" `
  "results/current/rust_windows_global_rehearsal.log" {
  powershell -NoProfile -ExecutionPolicy Bypass `
    -File scripts/tokenzero/rust_windows_global_rehearsal.ps1 `
    -TokenZeroExe $mcpCompat `
    -ReportPath results/current/rust_windows_global_rehearsal.json `
    -SkipBuild
}

Run-Json-Step "migration dry run" `
  "results/current/rust_windows_migration_plan.json" `
  "results/current/rust_windows_migration_plan.log" {
  powershell -NoProfile -ExecutionPolicy Bypass `
    -File scripts/tokenzero/rust_windows_migrate.ps1 `
    -ReportPath results/current/rust_windows_migration_plan.json
}

Write-Report -Status "ok" -Ok $true
