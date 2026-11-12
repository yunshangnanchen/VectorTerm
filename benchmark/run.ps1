# VectorTerm performance benchmark runner (Windows PowerShell 5.1)
# Usage: powershell -ExecutionPolicy Bypass -File benchmark\run.ps1
# Options: -SkipWindow (skip real-window FPS), -SkipStartup (skip cold start / memory)
# NOTE: this file is ASCII-only on purpose. Windows PowerShell 5.1 reads .ps1
# as the system ANSI codepage unless there is a BOM, so UTF-8 Chinese text
# would corrupt string literals.
param(
    [switch]$SkipWindow,
    [switch]$SkipStartup
)

# 'Continue' on purpose: with 'Stop', PowerShell 5.1 treats cargo's stderr
# warnings as terminating NativeCommandError. Native exit codes are checked
# explicitly below; critical cmdlets get -ErrorAction Stop individually.
$ErrorActionPreference = 'Continue'
# Rust prints UTF-8; PS 5.1 otherwise decodes it with the OEM codepage and
# the Chinese benchmark labels turn into mojibake.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
$data = Join-Path $PSScriptRoot 'data'
$cfgDir = Join-Path $PSScriptRoot 'configs'
New-Item -ItemType Directory -Force -Path $data | Out-Null
Set-Location $root

# Start-Process throws when the environment block contains names that differ
# only by case (HTTPS_PROXY vs https_proxy). Enumerating Env: itself throws in
# that state, so just drop the common lowercase duplicates. Process scope only
# - the user's real environment is untouched.
foreach ($n in @('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')) {
    try { [Environment]::SetEnvironmentVariable($n, $null, 'Process') } catch { }
}

# All cargo invocations go through the project build entry: it initializes
# the MSVC environment (vcvars64 + CRATE_CC_NO_DEFAULTS) that cc-rs needs on
# VS 2026. Calling cargo directly here would fail with D8050 on a cold cache.
$build = Join-Path $root 'build.cmd'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

function Write-Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }

# ---------------------------------------------------------------------------
Write-Step '1/5 Generate fixtures (full-screen colored text + 2 MiB payload)'
$esc = [char]27
$line = -join (1..100 | ForEach-Object { "$esc[38;5;196mW$esc[0m$esc[48;5;24;38;5;231mM$esc[0m" })
$screen = (1..60 | ForEach-Object { $line }) -join "`r`n"
[IO.File]::WriteAllText((Join-Path $data 'bench-screen.txt'), $screen)
"bench-screen.txt: $((Get-Item (Join-Path $data 'bench-screen.txt')).Length) bytes / 60 lines x 200 cells"

$payloadPath = Join-Path $data 'bench-payload.txt'
if (-not (Test-Path $payloadPath) -or (Get-Item $payloadPath).Length -ne 2MB) {
    $unit = "The quick brown fox jumps over the lazy dog 0123456789 abcdefghijklmnop`r`n"
    $sb = New-Object System.Text.StringBuilder
    while ($sb.Length -lt 2MB) { [void]$sb.Append($unit) }
    [IO.File]::WriteAllText($payloadPath, $sb.ToString())
}
"bench-payload.txt: $((Get-Item $payloadPath).Length) bytes"

# ---------------------------------------------------------------------------
Write-Step '2/5 Build release (debug numbers are NOT comparable)'
& $build --release
if ($LASTEXITCODE -ne 0) { throw 'build --release failed' }
& $build test --release --no-run
if ($LASTEXITCODE -ne 0) { throw 'build test --release --no-run failed' }

# ---------------------------------------------------------------------------
Write-Step '3/5 In-process benchmarks: frame stages / GPU submit / parse / burst / ConPTY'
$env:VECTORTERM_BENCH_MIB = '2'
$benchLog = Join-Path $data "run-$stamp.txt"
# No "2>&1" here: PowerShell 5.1 turns native stderr lines into
# ErrorRecords, which would pollute the captured benchmark log.
& $build test --release bench_ -- --ignored --nocapture | Tee-Object -FilePath $benchLog | Select-String -Pattern '^===|CSV|MB/s|test result|over budget' | ForEach-Object { $_.Line }
$outDir = if ($env:VECTORTERM_BENCH_OUT) { $env:VECTORTERM_BENCH_OUT } else { 'benchmark\data' }
"Full output -> $benchLog"
"CSV        -> $outDir\bench-frames.csv / bench-parse.csv"

# ---------------------------------------------------------------------------
$exe = Join-Path $root 'target\release\VectorTerm.exe'
$summary = @()
$summary += "VectorTerm benchmark run $stamp"
$summary += "OS: $((Get-CimInstance Win32_OperatingSystem).Caption)"
$summary += "CPU: $((Get-CimInstance Win32_Processor).Name)"

if (-not $SkipWindow) {
    Write-Step '4/5 Real-window FPS (2 sizes x 2 backends x 2 runs, includes present)'
    $env:VECTORTERM_PROFILE_FRAMES = '300'
    foreach ($size in @('960x640', '1920x1080')) {
        $env:VECTORTERM_PROFILE_SIZE = $size
        foreach ($mode in @('gpu', 'cpu')) {
            $cfg = Join-Path $cfgDir "bench-$mode.lua"
            for ($i = 1; $i -le 2; $i++) {
                $err = Join-Path $data "run-profile-$mode-$size-$i.txt"
                Start-Process -FilePath $exe -ArgumentList @('--config', "`"$cfg`"") `
                    -RedirectStandardError $err -PassThru -Wait -ErrorAction Stop | Out-Null
                $hits = @(Get-Content $err | Where-Object { $_ -match 'backend=' })
                foreach ($h in $hits) {
                    $row = "$size $mode run$i $h"
                    Write-Host $row
                    $summary += $row
                }
            }
        }
    }
    Remove-Item Env:\VECTORTERM_PROFILE_FRAMES -ErrorAction SilentlyContinue
    Remove-Item Env:\VECTORTERM_PROFILE_SIZE -ErrorAction SilentlyContinue

    # Verify the fixture really reached the parser, otherwise FPS numbers are
    # measuring an empty screen (this exact mistake invalidated an earlier run).
    $verify = Join-Path $cfgDir 'bench-verify.lua'
    $env:VECTORTERM_PROFILE_FRAMES = '60'
    $env:VECTORTERM_PROFILE_SIZE = '1920x1080'
    $sessionLog = Join-Path $data 'session-bench.log'
    if (Test-Path $sessionLog) { Remove-Item $sessionLog -Force }
    Start-Process -FilePath $exe -ArgumentList @('--config', "`"$verify`"") `
        -RedirectStandardError (Join-Path $data 'run-verify.txt') -PassThru -Wait -ErrorAction Stop | Out-Null
    $got = if (Test-Path $sessionLog) { (Get-Item $sessionLog).Length } else { 0 }
    $expected = (Get-Item (Join-Path $data 'bench-screen.txt')).Length
    $verdict = if ($got -ge $expected * 0.9) { 'OK' } else { 'FAILED - discard FPS data' }
    Write-Host "session_log $got / fixture $expected bytes : $verdict"
    $summary += "verify: session_log=$got source=$expected $verdict"
    Remove-Item Env:\VECTORTERM_PROFILE_FRAMES -ErrorAction SilentlyContinue
    Remove-Item Env:\VECTORTERM_PROFILE_SIZE -ErrorAction SilentlyContinue
}

if (-not $SkipStartup) {
    Write-Step '5/5 Cold-start first frame / peak working set'
    $env:VECTORTERM_PROFILE_FRAMES = '180'
    $env:VECTORTERM_PROFILE_SIZE = '1920x1080'
    foreach ($mode in @('gpu', 'cpu')) {
        $cfg = Join-Path $cfgDir "bench-$mode.lua"
        for ($i = 1; $i -le 2; $i++) {
            $err = Join-Path $data "run-startup-$mode-$i.txt"
            $sw = [Diagnostics.Stopwatch]::StartNew()
            $p = Start-Process -FilePath $exe -ArgumentList @('--config', "`"$cfg`"") `
                -RedirectStandardError $err -PassThru -ErrorAction Stop
            $maxWs = 0
            while ($sw.ElapsedMilliseconds -lt 20000) {
                try {
                    $p.Refresh()
                    if ($p.HasExited) { break }
                    if ($p.WorkingSet64 -gt $maxWs) { $maxWs = $p.WorkingSet64 }
                } catch { break }
                Start-Sleep -Milliseconds 25
            }
            if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
            $log = if (Test-Path $err) { Get-Content $err -Raw } else { '' }
            $ff = if ($log -match 'first_frame_ms=([\d.]+)') { $matches[1] } else { '?' }
            $row = "startup mode=$mode run=$i first_frame_ms=$ff peak_ws_MB=$([math]::Round($maxWs / 1MB, 1))"
            Write-Host $row
            $summary += $row
        }
    }
    Remove-Item Env:\VECTORTERM_PROFILE_FRAMES -ErrorAction SilentlyContinue
    Remove-Item Env:\VECTORTERM_PROFILE_SIZE -ErrorAction SilentlyContinue
}

$summaryPath = Join-Path $data 'latest-summary.txt'
[IO.File]::WriteAllLines($summaryPath, $summary)
Write-Host "`nDone. Summary -> $summaryPath" -ForegroundColor Green
