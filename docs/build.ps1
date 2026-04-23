# docs/build.ps1 — Windows dev loop for the mdBook site.
#
# Mirrors docs/build.sh: copies .omne/cfg/*.md (excluding README.md) into
# docs/src/, then invokes `mdbook build`. Authored files under docs/src/
# (SUMMARY.md, introduction.md) are preserved because their names do not
# collide with any source under .omne/cfg/.
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $here '..')
$cfgDir = Join-Path $repoRoot '.omne/cfg'
$srcDir = Join-Path $here 'src'

if (-not (Test-Path $cfgDir)) {
    Write-Error "error: $cfgDir not found"
    exit 1
}

New-Item -ItemType Directory -Force -Path $srcDir | Out-Null

Get-ChildItem -Path $cfgDir -Filter '*.md' -File | ForEach-Object {
    if ($_.Name -ieq 'README.md') { return }
    $dest = Join-Path $srcDir $_.Name
    Copy-Item -LiteralPath $_.FullName -Destination $dest -Force
    # Prepend AUTO-GENERATED banner at build time so the copy trail is obvious
    # when a reader lands on the generated file in docs/src/.
    $banner = "<!-- AUTO-GENERATED from .omne/cfg/$($_.Name). DO NOT EDIT HERE. Edit the source and re-run build.sh / build.ps1. -->"
    $body = Get-Content -LiteralPath $dest -Raw
    Set-Content -LiteralPath $dest -Value "$banner`n`n$body" -NoNewline
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
