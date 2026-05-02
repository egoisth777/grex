<#
.SYNOPSIS
    One-shot drift cleanup for the grex working tree (v1.2.6).

.DESCRIPTION
    Removes filesystem fossils that historically appeared in `git status`
    after upgrading from <= v1.2.5:

      - claude-statusline-probe.txt and similar CWD probes dropped by
        external tooling (e.g. cc-cfg statusline scripts that resolved
        $env:TEMP without a separator)
      - crates/grex/.grex/ (per-workspace runtime state directory that
        leaked into the repo when the binary was invoked from inside
        a crate root)

    Then verifies the repo's .gitignore matches the expected v1.2.6
    contents and runs `git add --renormalize .` so any stale CRLF/NUL
    blobs surface for manual review.

    This script is RUN-ONCE after upgrading. Subsequent prevention is
    handled by .gitignore + .gitattributes — re-running is harmless but
    not required.

.EXAMPLE
    pwsh -File scripts/cleanup-drift.ps1
#>

[CmdletBinding()]
param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Step([string]$msg) { Write-Host "[cleanup-drift] $msg" -ForegroundColor Cyan }
function Write-Ok  ([string]$msg) { Write-Host "[cleanup-drift] OK    $msg" -ForegroundColor Green }
function Write-Warn2([string]$msg) { Write-Host "[cleanup-drift] WARN  $msg" -ForegroundColor Yellow }

Write-Step "Repo root: $RepoRoot"

# 1. Remove CWD-fossil statusline probes (any *statusline-probe*.txt at repo root).
Write-Step 'Scanning for statusline-probe fossils...'
$probes = Get-ChildItem -LiteralPath $RepoRoot -Force -File -Filter '*statusline-probe*.txt' -ErrorAction SilentlyContinue
if ($probes) {
    foreach ($p in $probes) {
        Remove-Item -LiteralPath $p.FullName -Force
        Write-Ok "removed $($p.Name)"
    }
} else {
    Write-Ok 'no statusline-probe fossils found'
}

# 1b. Defensive: also remove the literal name 'claude-statusline-probe.txt'
#     in case it slipped past the filter (e.g. odd unicode in the actual fossil).
$literal = Join-Path $RepoRoot 'claude-statusline-probe.txt'
if (Test-Path -LiteralPath $literal) {
    Remove-Item -LiteralPath $literal -Force
    Write-Ok 'removed claude-statusline-probe.txt'
}

# 2. Remove crates/grex/.grex/ (runtime state that should never be committed).
$runtimeDir = Join-Path $RepoRoot 'crates/grex/.grex'
if (Test-Path -LiteralPath $runtimeDir) {
    Remove-Item -LiteralPath $runtimeDir -Recurse -Force
    Write-Ok 'removed crates/grex/.grex/'
} else {
    Write-Ok 'crates/grex/.grex/ already absent'
}

# 3. Verify .gitignore contains the v1.2.6 drift-prevention markers.
Write-Step 'Verifying .gitignore drift-prevention entries...'
$gitignorePath = Join-Path $RepoRoot '.gitignore'
if (-not (Test-Path -LiteralPath $gitignorePath)) {
    throw ".gitignore not found at $gitignorePath"
}
$content = Get-Content -LiteralPath $gitignorePath -Raw
$required = @(
    '.grex/',
    '**/.grex/',
    'claude-statusline-probe.txt',
    '*statusline-probe*.txt'
)
$missing = @()
foreach ($pat in $required) {
    if ($content -notmatch [regex]::Escape($pat)) { $missing += $pat }
}
if ($missing.Count -gt 0) {
    Write-Warn2 ".gitignore is missing expected entries: $($missing -join ', ')"
    Write-Warn2 'Re-run after upgrading to v1.2.6 .gitignore.'
    exit 2
}
Write-Ok '.gitignore contains all required drift-prevention entries'

# 4. Renormalize line endings via git so any latent CRLF/NUL blobs surface
#    in `git status` for manual review.
Write-Step 'Running: git -C <repo> add --renormalize .'
$git = Get-Command git -ErrorAction SilentlyContinue
if (-not $git) {
    Write-Warn2 'git not on PATH — skipping renormalize step (run it manually).'
} else {
    & git -C $RepoRoot add --renormalize .
    if ($LASTEXITCODE -ne 0) {
        Write-Warn2 "git add --renormalize . exited $LASTEXITCODE"
    } else {
        Write-Ok 'renormalize complete — review `git status` for any flagged blobs'
    }
}

Write-Step 'Done. v1.2.6 drift cleanup complete.'
exit 0
