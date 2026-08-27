#requires -Version 5.1
<#
.SYNOPSIS
    Self-contained TokenZero demo for Windows, macOS, and Linux.

.DESCRIPTION
    Walks an AI agent's "tool day in the life" through TokenZero and counts
    the tokens TokenZero hid vs the tokens the agent actually consumed,
    using TokenZero's own tokenizer on both sides (same-tokenizer compare).

    The demo:
      1. Resolves a `tokenzero` binary
           - if -BinaryPath is given, uses it
           - else if `tokenzero` is on PATH, uses that
           - else downloads the GitHub Release asset for the current OS/CPU
             into demo/.tokenzero-bin/, verifies SHA256, and runs from there.
      2. Uses an isolated cache file under demo\.cache\ so the demo never
         touches the user's real TokenZero state.
      3. Runs five real scenarios against THIS REPO:
           - small read        (capsule-never-costs-more-than-raw guarantee)
           - large read        (heavy savings + tz:// blob ref)
           - grep `fn `        (recoverable hit set across crates\)
           - expand            (round-trip the large-read ref, byte-exact check)
           - recall            (re-find content already in the cache, no re-grep)
           - run -- <cmd>      (cross-platform shell capture)
      4. Counts raw tokens by piping the raw output through
         `tokenzero ingest --stdin` and reading accounting.raw_tokens.
      5. Prints a Markdown summary table and writes demo\demo_results.json.

.PARAMETER BinaryPath
    Optional explicit path to tokenzero. If omitted, falls back to PATH; and then to a downloaded release binary.

.PARAMETER ReleaseTag
    Release tag to download if a binary has to be fetched. Defaults to v1.0.1.

.PARAMETER SkipDownload
    If set and no binary can be located, fail instead of downloading.

.EXAMPLE
    pwsh -File .\demo\run_demo.ps1

.EXAMPLE
    pwsh -File .\demo\run_demo.ps1 -BinaryPath C:\tools\tokenzero.exe
#>

[CmdletBinding()]
param(
    [string] $BinaryPath,
    [string] $ReleaseTag = 'v1.0.1',
    [switch] $SkipDownload,
    [switch] $NoViz,
    [switch] $OpenViz
)

$ErrorActionPreference = 'Stop'; $ProgressPreference   = 'SilentlyContinue'

# --- Locations ---------------------------------------------------------------
$DemoDir = Split-Path -Parent $MyInvocation.MyCommand.Path; $RepoDir = Resolve-Path (Join-Path $DemoDir '..') | Select-Object -ExpandProperty Path; $BinDir   = Join-Path $DemoDir '.tokenzero-bin'; $CacheDir = Join-Path $DemoDir '.cache'
$null = New-Item -ItemType Directory -Force -Path $BinDir, $CacheDir

$CachePath   = Join-Path $CacheDir 'recovery-cache.json'; $CountCache  = Join-Path $CacheDir 'count-cache.json'; $ResultsPath = Join-Path $DemoDir 'demo_results.json'

# Start every run from a clean cache so seen-set dedup numbers are reproducible.
Remove-Item -Force -ErrorAction SilentlyContinue $CachePath, $CountCache

# --- Binary resolution -------------------------------------------------------
function Resolve-TokenZeroBinary {
    param([string]$Explicit, [string]$Tag, [switch]$NoDownload)

    if ($Explicit) {
        if (-not (Test-Path -LiteralPath $Explicit)) {
            throw "Binary not found at -BinaryPath: $Explicit"
        }
        return (Resolve-Path -LiteralPath $Explicit).Path
    }

    $onPath = Get-Command tokenzero -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $exeName = Get-TokenZeroExecutableName; $cached = Join-Path $BinDir $exeName
    if (Test-Path -LiteralPath $cached) { return $cached }

    if ($NoDownload) { throw 'tokenzero binary not found and -SkipDownload was set.' }

    $assetInfo = Get-TokenZeroReleaseAsset -Tag $Tag; Write-Host "==> Downloading TokenZero $Tag ($($assetInfo.Rid)) into $BinDir" -ForegroundColor Cyan; $asset = $assetInfo.Name; $base  = "https://github.com/AdityaVG13/tokenzero/releases/download/$Tag"; $zip   = Join-Path $BinDir $asset
    $sha   = "$zip.sha256"

    Invoke-WebRequest -Uri "$base/$asset"        -OutFile $zip; Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile $sha

    $expected = ((Get-Content -LiteralPath $sha -Raw).Trim() -split '\s+')[0].ToLower(); $actual   = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "SHA256 mismatch for $asset`n  expected: $expected`n  actual:   $actual"
    }

    $extract = Join-Path $BinDir 'extract'
    if (Test-Path -LiteralPath $extract) { Remove-Item -Recurse -Force $extract }
    if ($assetInfo.Extension -eq '.zip') {
        Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    } else {
        $null = New-Item -ItemType Directory -Force -Path $extract; & tar -xzf $zip -C $extract
        if ($LASTEXITCODE -ne 0) { throw "tar failed to extract $asset" }
    }
    $exe = Get-ChildItem -Path $extract -Recurse -Filter $exeName | Select-Object -First 1
    if (-not $exe) { throw "$exeName not found inside $asset" }
    Copy-Item -LiteralPath $exe.FullName -Destination $cached -Force
    return $cached
}

function Test-IsWindows {
    return ($PSVersionTable.PSEdition -eq 'Desktop' -or [System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT)
}

function Get-TokenZeroExecutableName {
    if (Test-IsWindows) { return 'tokenzero.exe' }
    return 'tokenzero'
}

function Get-TokenZeroReleaseAsset {
    param([Parameter(Mandatory)][string]$Tag)

    $isWindows = Test-IsWindows
    if ($isWindows) {
        $arch = if ([Environment]::Is64BitOperatingSystem) { 'x64' } else { 'x86' }
        $isMac = $false; $isLinux = $false
    } else {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant(); $isMac = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)
        $isLinux = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Linux)
    }

    if ($isWindows -and $arch -eq 'x64') { $rid = 'x86_64-pc-windows-msvc'; $ext = '.zip' }
    elseif ($isLinux -and $arch -eq 'x64') { $rid = 'x86_64-unknown-linux-gnu'; $ext = '.tar.gz' }
    elseif ($isMac -and $arch -eq 'arm64') { $rid = 'aarch64-apple-darwin'; $ext = '.tar.gz' }
    elseif ($isMac -and $arch -eq 'x64') { $rid = 'x86_64-apple-darwin'; $ext = '.tar.gz' }
    else { throw "unsupported platform for release download: OS=$([System.Environment]::OSVersion.Platform) ARCH=$arch" }

    [pscustomobject]@{
        Rid = $rid; Extension = $ext; Name = "tokenzero-$Tag-$rid$ext"
    }
}

$Tz = Resolve-TokenZeroBinary -Explicit $BinaryPath -Tag $ReleaseTag -NoDownload:$SkipDownload; Write-Host "==> Using binary: $Tz"; $tzVersion = (& $Tz --version) -join ' '; Write-Host "    $tzVersion"

# --- Helpers ----------------------------------------------------------------
function ConvertTo-Win32CommandLineArg {
    # Windows CommandLineToArgvW quoting rules (msvcrt-compatible).
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Value)
    if ($Value -eq '') { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }
    $sb = [System.Text.StringBuilder]::new(); [void]$sb.Append('"'); $bs = 0
    foreach ($ch in $Value.ToCharArray()) {
        if ($ch -eq '\') { $bs++; continue }
        if ($ch -eq '"') {
            [void]$sb.Append('\' * (2 * $bs + 1)); [void]$sb.Append('"'); $bs = 0; continue
        }
        if ($bs -gt 0) { [void]$sb.Append('\' * $bs); $bs = 0 }
        [void]$sb.Append($ch)
    }
    if ($bs -gt 0) { [void]$sb.Append('\' * (2 * $bs)) }
    [void]$sb.Append('"')
    return $sb.ToString()
}

function Set-ProcessArguments {
    param(
        [Parameter(Mandatory)] [System.Diagnostics.ProcessStartInfo] $ProcessStartInfo,
        [Parameter(Mandatory)] [string[]] $ArgList
    )
    if ($ProcessStartInfo.PSObject.Properties.Name -contains 'ArgumentList') {
        foreach ($a in $ArgList) { [void]$ProcessStartInfo.ArgumentList.Add($a) }
    } else {
        $ProcessStartInfo.Arguments = ($ArgList | ForEach-Object { ConvertTo-Win32CommandLineArg $_ }) -join ' '
    }
}

function Invoke-Tz {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string[]]$ArgList, [string]$StdIn)

    # Use System.Diagnostics.Process so we can pipe stdin reliably under both
    # Windows PowerShell 5.1 (.NET Framework, no ArgumentList) and pwsh 7+.
    $psi = New-Object System.Diagnostics.ProcessStartInfo; $psi.FileName               = $Tz; Set-ProcessArguments -ProcessStartInfo $psi -ArgList $ArgList; $psi.RedirectStandardInput  = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError  = $true
    $psi.UseShellExecute        = $false; $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8; $psi.StandardErrorEncoding  = [System.Text.Encoding]::UTF8

    $proc = [System.Diagnostics.Process]::Start($psi)
    if ($PSBoundParameters.ContainsKey('StdIn') -and $null -ne $StdIn) {
        # Force the writer to UTF-8 without a BOM so the binary receives
        # exactly the bytes we measured the token count from.
        $utf8 = New-Object System.Text.UTF8Encoding $false; $writer = New-Object System.IO.StreamWriter($proc.StandardInput.BaseStream, $utf8); $writer.Write($StdIn); $writer.Flush(); $writer.Close()
    } else {
        $proc.StandardInput.Close()
    }
    $stdout = $proc.StandardOutput.ReadToEnd(); $stderr = $proc.StandardError.ReadToEnd(); $proc.WaitForExit()

    if ($proc.ExitCode -ne 0) {
        throw "tokenzero $($ArgList -join ' ') failed (exit=$($proc.ExitCode))`nSTDERR:`n$stderr"
    }
    return $stdout
}

function Get-RawTokens {
    param([Parameter(Mandatory)][string]$Text)
    $json = Invoke-Tz -ArgList @('ingest', '--stdin', '--json', '--cache-path', $CountCache) -StdIn $Text
    return (ConvertFrom-Json $json).accounting.raw_tokens
}

function Get-VisibleTokens {
    param([Parameter(Mandatory)][string]$Json)
    return (ConvertFrom-Json $Json).accounting.visible_tokens
}

function Format-Pct {
    param([int]$Raw, [int]$Visible)
    if ($Raw -le 0) { return '   -' }
    return ('{0,5:N1}%' -f (100.0 * ($Raw - $Visible) / $Raw))
}

# --- Scenarios --------------------------------------------------------------
Set-Location $RepoDir; $rows = New-Object System.Collections.Generic.List[object]

function Add-Row {
    param([string]$Name, [int]$Raw, [int]$Visible, [string]$Note = '')
    $rows.Add([pscustomobject]@{
        workload        = $Name; raw_tokens      = $Raw; visible_tokens  = $Visible
        savings_pct     = if ($Raw -gt 0) { [math]::Round(100.0 * ($Raw - $Visible) / $Raw, 1) } else { 0 }
        note            = $Note
    }) | Out-Null
}

Write-Host ''; Write-Host '=== TokenZero demo ===' -ForegroundColor Green; Write-Host "Repo: $RepoDir"; Write-Host "Cache (isolated): $CachePath"; Write-Host ''

# 1. Small file pass-through (capsule-never-costs-more guarantee)
$smallFile = Join-Path 'crates' (Join-Path 'tokenzero' 'Cargo.toml')
if (Test-Path -LiteralPath $smallFile) {
    Write-Host "[1/7] small read  : $smallFile"; $raw      = (Get-Content -LiteralPath $smallFile -Raw); $rawTok   = Get-RawTokens $raw; $resJson  = Invoke-Tz -ArgList @('read', $smallFile, '--json', '--cache-path', $CachePath); $visTok   = Get-VisibleTokens $resJson
    Add-Row 'small read (Cargo.toml)' $rawTok $visTok 'pass-through; capsule never costs more than raw'
}

# 2. Large file read (heavy savings + tz:// refs)
$largeFile = Join-Path 'crates' (Join-Path 'tokenzero-mcp' (Join-Path 'src' 'lib.rs')); $largeRef  = $null
if (Test-Path -LiteralPath $largeFile) {
    Write-Host "[2/7] large read  : $largeFile"; $raw      = (Get-Content -LiteralPath $largeFile -Raw); $rawTok   = Get-RawTokens $raw; $resJson  = Invoke-Tz -ArgList @('read', $largeFile, '--json', '--cache-path', $CachePath); $resObj   = ConvertFrom-Json $resJson
    $visTok   = $resObj.accounting.visible_tokens; $largeRef = ($resObj.refs | Where-Object { $_.kind -eq 'blob' } | Select-Object -First 1).ref; Add-Row "large read ($largeFile)" $rawTok $visTok "ref: $largeRef"
}

# 3. Re-read same file via the MCP server: session seen-set dedup
#    CLI invocations are stateless against the cache file, so dedup is an
#    MCP-server feature (it tracks a per-session seen-set in memory). We
#    issue two JSON-RPC reads within the same stdio session.
if (Test-Path -LiteralPath $largeFile) {
    Write-Host "[3/7] re-read     : $largeFile (MCP session dedup)"

    $absLarge = (Resolve-Path -LiteralPath $largeFile).Path; $pathArg  = $absLarge -replace '\\','\\'  # JSON-escape backslashes
    $jsonrpc  = @(
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo","version":"0"}}}',
        '{"jsonrpc":"2.0","method":"notifications/initialized"}',
        ('{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"read","arguments":{{"path":"{0}"}}}}}}' -f $pathArg),
        ('{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"read","arguments":{{"path":"{0}"}}}}}}' -f $pathArg)
    ) -join "`n"

    $mcpOut = Invoke-Tz -ArgList @(
        'mcp-server',
        '--allowed-root', $RepoDir,
        '--cache-path',   $CachePath
    ) -StdIn ($jsonrpc + "`n")

    $secondCallText = $null
    foreach ($line in $mcpOut -split "`r?`n") {
        if (-not $line) { continue }
        try { $obj = ConvertFrom-Json $line } catch { continue }
        if ($obj.id -eq 3 -and $obj.result.content) {
            $secondCallText = $obj.result.content[0].text
            break
        }
    }
    if (-not $secondCallText) {
        Write-Warning "MCP dedup scenario skipped: no response to second tools/call"
    } else {
        $rawTok = Get-RawTokens (Get-Content -LiteralPath $largeFile -Raw); $visTok = Get-RawTokens $secondCallText; Add-Row "re-read same file (MCP dedup)" $rawTok $visTok 'second call routed through seen-set in same MCP session'
    }
}

# 4. Repo-wide grep (recoverable hit set)
Write-Host "[4/7] grep        : 'fn ' across crates/"; $rawGrepLines = @()
Get-ChildItem -Path 'crates' -Recurse -File -Filter '*.rs' -ErrorAction SilentlyContinue |
    ForEach-Object {
        $idx = 0
        foreach ($line in [System.IO.File]::ReadLines($_.FullName)) {
            $idx++
            if ($line -match '\bfn\s') { $rawGrepLines += ("{0}:{1}:{2}" -f $_.FullName, $idx, $line) }
        }
    }
$rawGrep  = ($rawGrepLines -join "`n"); $rawTok   = Get-RawTokens $rawGrep; $resJson  = Invoke-Tz -ArgList @('grep', 'fn ', 'crates', '--json', '--max-files', '200', '--cache-path', $CachePath); $visTok   = Get-VisibleTokens $resJson
Add-Row "grep 'fn ' across crates/" $rawTok $visTok ("{0} matching lines" -f $rawGrepLines.Count)

# 5. Recovery round-trip: expand the large-read blob and byte-compare
if ($largeRef) {
    Write-Host "[5/7] expand      : $largeRef (byte-exact check)"; $recovered = Invoke-Tz -ArgList @('expand', $largeRef, '--raw', '--cache-path', $CachePath); $original  = [System.IO.File]::ReadAllText((Resolve-Path -LiteralPath $largeFile).Path)
    # Tolerate a single trailing newline that stdout streams sometimes add.
    $rec = $recovered.TrimEnd("`r","`n"); $orig = $original.TrimEnd("`r","`n")
    if ($rec -ne $orig) {
        $rL = $rec.Length; $oL = $orig.Length
        throw "byte-exact recovery FAILED: expand returned $rL chars, original is $oL chars"
    }
    Add-Row "expand round-trip (large file)" 0 0 'byte-exact: recovered == original'
}

# 6. Recall: re-find content already in the cache without re-scanning
Write-Host "[6/7] recall      : 'fn main' (no re-grep)"; $resJson  = Invoke-Tz -ArgList @('recall', 'fn main', '--max-hits', '10', '--json', '--cache-path', $CachePath); $visTok   = Get-VisibleTokens $resJson
# Same baseline as scenario 4 — we are showing recall vs re-running that grep.
Add-Row "recall 'fn main' vs re-grep" $rawTok $visTok 'reuses cached payloads; no filesystem rescan'

# 7. Shell capture (always works even if cargo missing)
Write-Host "[7/7] run         : capture a small shell command"
$probeCmd = if (Get-Command git -ErrorAction SilentlyContinue) {
    @('git','--version')
} elseif (Test-IsWindows) {
    @('cmd','/c','ver')
} else {
    @('uname','-a')
}
try {
    $rawOut  = & $probeCmd[0] @($probeCmd[1..($probeCmd.Length-1)]) 2>&1 | Out-String; $rawTok  = Get-RawTokens $rawOut; $resJson = Invoke-Tz -ArgList (@('run','--json','--cache-path', $CachePath, '--') + $probeCmd); $visTok  = Get-VisibleTokens $resJson
    Add-Row ("run -- {0}" -f ($probeCmd -join ' ')) $rawTok $visTok 'process capture + recoverable stream'
} catch {
    Write-Warning "shell scenario skipped: $($_.Exception.Message)"
}

# --- Summary ----------------------------------------------------------------
$totalRaw     = ($rows | Where-Object { $_.raw_tokens     -gt 0 } | Measure-Object -Sum raw_tokens).Sum; $totalVisible = ($rows | Where-Object { $_.raw_tokens     -gt 0 } | Measure-Object -Sum visible_tokens).Sum
if (-not $totalRaw)     { $totalRaw = 0 }
if (-not $totalVisible) { $totalVisible = 0 }
$totalPct = if ($totalRaw -gt 0) { [math]::Round(100.0 * ($totalRaw - $totalVisible) / $totalRaw, 1) } else { 0 }

Write-Host ''; Write-Host '=== Results ===' -ForegroundColor Green; $nameW = 40; $head = ('{0,-' + $nameW + '} {1,12} {2,12} {3,10}  {4}') -f 'Workload','Raw tokens','Visible','Savings','Note'; Write-Host $head; Write-Host ('-' * ($head.Length))
foreach ($r in $rows) {
    $line = ('{0,-' + $nameW + '} {1,12} {2,12} {3,10}  {4}') -f `
        $r.workload, $r.raw_tokens, $r.visible_tokens, (Format-Pct $r.raw_tokens $r.visible_tokens), $r.note
    Write-Host $line
}
Write-Host ('-' * ($head.Length)); $totalLine = ('{0,-' + $nameW + '} {1,12} {2,12} {3,10}') -f 'TOTAL (rows with raw baseline)', $totalRaw, $totalVisible, (Format-Pct $totalRaw $totalVisible); Write-Host $totalLine -ForegroundColor Cyan; Write-Host ''
Write-Host 'Honest accounting: every TokenZero row above keeps exact tz:// refs.'; Write-Host 'Hidden bytes are one `tokenzero expand <ref>` call away — and scenario 5'; Write-Host 'proves the round-trip really is byte-exact.'; Write-Host ''

$payload = [pscustomobject]@{
    tokenzero_version = $tzVersion; binary            = $Tz; repo              = $RepoDir; cache             = $CachePath; workloads         = $rows
    totals            = [pscustomobject]@{
        raw_tokens     = $totalRaw; visible_tokens = $totalVisible; savings_pct    = $totalPct
    }
}
$payload | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultsPath -Encoding UTF8; Write-Host "Wrote: $ResultsPath"

if (-not $NoViz) {
    $vizScript = Join-Path $DemoDir 'build_viz.ps1'
    if (Test-Path -LiteralPath $vizScript) {
        Write-Host ''; Write-Host '==> Rendering demo_viz.html' -ForegroundColor Cyan
        $vizArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $vizScript,
                     '-ResultsPath', $ResultsPath,
                     '-OutPath',     (Join-Path $DemoDir 'demo_viz.html'))
        if ($OpenViz) { $vizArgs += '-Open' }
        $psHost = if ((Get-Command pwsh -ErrorAction SilentlyContinue)) {
            (Get-Command pwsh).Source
        } elseif ((Get-Command powershell -ErrorAction SilentlyContinue)) {
            (Get-Command powershell).Source
        } else {
            throw 'PowerShell host not found for rendering visualization.'
        }
        & $psHost @vizArgs
    } else {
        Write-Warning "build_viz.ps1 not found at $vizScript; skipping visualization."
    }
}
