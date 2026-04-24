# docs/build.ps1 — Windows dev loop for the mdBook site.
#
# Mirrors docs/build.sh: copies chapter sources (in priority order:
# .omne/cfg/*.md  ->  docs/src-authored/*.md) into docs/src/, then invokes
# `mdbook build`. Authored files under docs/src/ (SUMMARY.md, introduction.md,
# pack-template.md) are preserved because their names do not collide with any
# copied source.
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $here '..')
$cfgDir = Join-Path $repoRoot '.omne/cfg'
$vendorDir = Join-Path $here 'src-authored'
$srcDir = Join-Path $here 'src'

New-Item -ItemType Directory -Force -Path $srcDir | Out-Null

if (Test-Path $cfgDir) {
    $sourceDir = $cfgDir
    $sourceLabel = '.omne/cfg'
    Write-Host "info: chapter source = $cfgDir"
} elseif (Test-Path $vendorDir) {
    $sourceDir = $vendorDir
    $sourceLabel = 'docs/src-authored'
    Write-Host "info: chapter source = $vendorDir (vendored fallback — .omne/cfg/ not present)"
} else {
    Write-Error "error: neither $cfgDir nor $vendorDir exists — cannot assemble docs/src/"
    exit 1
}

Get-ChildItem -Path $sourceDir -Filter '*.md' -File | ForEach-Object {
    if ($_.Name -ieq 'README.md' -or $_.Name -ieq 'pack-template.md') { return }
    $dest = Join-Path $srcDir $_.Name
    Copy-Item -LiteralPath $_.FullName -Destination $dest -Force
    # Prepend AUTO-GENERATED banner at build time so the copy trail is obvious
    # when a reader lands on the generated file in docs/src/.
    $banner = "<!-- AUTO-GENERATED from $sourceLabel/$($_.Name). DO NOT EDIT HERE. Edit the source and re-run build.sh / build.ps1. -->"
    $body = Get-Content -LiteralPath $dest -Raw
    Set-Content -LiteralPath $dest -Value "$banner`n`n$body" -NoNewline
}

# Keep the vendored fallback in step with the SSOT whenever .omne/cfg/ wins.
if ($sourceDir -eq $cfgDir -and (Test-Path $vendorDir)) {
    Get-ChildItem -Path $cfgDir -Filter '*.md' -File | ForEach-Object {
        if ($_.Name -ieq 'README.md' -or $_.Name -ieq 'pack-template.md') { return }
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $vendorDir $_.Name) -Force
    }
}

Push-Location $here
try {
    $mdbook = Get-Command mdbook -ErrorAction SilentlyContinue
    if (-not $mdbook) {
        Write-Error 'error: mdbook not on PATH. Install with: cargo install mdbook --locked'
        exit 127
    }
    & mdbook build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
