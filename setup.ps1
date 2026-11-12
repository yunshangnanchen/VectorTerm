# =====================================================================
# VectorTerm bootstrap: fetch wezterm-term and its in-repo path dependencies
# (shallow clone + sparse-checkout), then apply patches/ idempotently.
# Run once. build.cmd auto-invokes this when sources are missing.
#
# ASCII-only on purpose: Windows PowerShell 5.1 reads BOM-less .ps1 files
# with the system ANSI codepage, where UTF-8 Chinese can corrupt quotes.
# =====================================================================
# Native git writes to stderr for expected conditions (e.g. an already
# applied patch). With 'Stop', PS 5.1 turns those into terminating errors,
# so use 'Continue' and check $LASTEXITCODE explicitly instead.
$ErrorActionPreference = 'Continue'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$tag = '20240203-110809-5046fc22'
$dirs = @('term', 'termwiz', 'bidi', 'wezterm-dynamic', 'color-types',
          'wezterm-blob-leases', 'wezterm-input-types', 'filedescriptor', 'vtparse')
$upstream = Join-Path $PSScriptRoot '.wezterm-src'

# Idempotent: --check succeeds only when the patch is not applied yet.
function Apply-UpstreamPatches {
    $patchDir = Join-Path $PSScriptRoot 'patches'
    if (-not (Test-Path $patchDir)) { return }
    Get-ChildItem (Join-Path $patchDir '*.patch') | ForEach-Object {
        $p = $_.FullName
        & git -C $upstream apply --check $p 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
            & git -C $upstream apply $p | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "failed to apply patch: $($_.Name)" }
            Write-Host "[setup] applied patch: $($_.Name)"
        }
    }
}

if (Test-Path (Join-Path $upstream 'term\src\lib.rs')) {
    Write-Host '[setup] wezterm-term sources present, checking patches'
    Apply-UpstreamPatches
    exit 0
}

if (Test-Path $upstream) {
    Write-Host "[setup] removing incomplete $upstream"
    Remove-Item -Recurse -Force $upstream -ErrorAction Stop
}

Write-Host "[setup] cloning wezterm at tag $tag (needs network)..."
& git clone --depth 1 --filter=blob:none --sparse --branch $tag https://github.com/wezterm/wezterm $upstream
if ($LASTEXITCODE -ne 0) { throw 'git clone failed; check network access and retry' }

Push-Location $upstream -ErrorAction Stop
try {
    & git sparse-checkout set @dirs
    if ($LASTEXITCODE -ne 0) { throw 'git sparse-checkout failed' }
    # Remove the upstream workspace manifests so cargo does not claim this
    # directory as a workspace root; every sub-crate builds on its own.
    Remove-Item 'Cargo.toml', 'Cargo.lock' -ErrorAction SilentlyContinue
} finally {
    Pop-Location
}

Apply-UpstreamPatches
Write-Host "[setup] wezterm-term sources ready (tag $tag)"
