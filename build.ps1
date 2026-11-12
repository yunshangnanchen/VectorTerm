# =====================================================================
# VectorTerm build entry (PowerShell wrapper).
#
# All build logic lives in build.cmd. This file exists so that
#   ./build.ps1 --release
# behaves exactly like
#   build.cmd --release
# while keeping PowerShell argument arrays intact (quoted paths, "--",
# "--nocapture", etc.). It never builds a command string, so arguments
# containing spaces or metacharacters cannot be mangled by cmd.exe.
#
# ASCII-only on purpose: Windows PowerShell 5.1 reads BOM-less .ps1 files
# with the system ANSI codepage, where UTF-8 Chinese can corrupt quotes.
# =====================================================================

# Native commands write to stderr for ordinary progress output. With 'Stop',
# PS 5.1 turns those lines into terminating NativeCommandError records.
$ErrorActionPreference = 'Continue'
# cargo / rustc emit UTF-8; PS 5.1 otherwise decodes with the OEM codepage.
try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
} catch { }

$entry = Join-Path $PSScriptRoot 'build.cmd'
if (-not (Test-Path $entry)) {
    Write-Host "[build] build.cmd not found: $entry" -ForegroundColor Red
    exit 1
}

& $entry @args
$code = $LASTEXITCODE
if ($null -eq $code) { $code = 0 }
exit $code
