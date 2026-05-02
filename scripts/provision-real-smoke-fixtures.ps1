<#
.SYNOPSIS
    Provision (or audit) the 6 real-smoke GitHub fixtures under egoisth777.

.DESCRIPTION
    Real-smoke harness regression gates B1-B15 require six dedicated GitHub
    repositories that exercise leaf packs, flat meta packs, sub-pack-under-meta
    layouts, A<->B cycles, and an intentionally-malformed manifest.

    The fixture set is HARDCODED in this script. The script will NEVER touch
    repositories outside this list:

        egoisth777/grex-test-leaf
        egoisth777/grex-test-meta-flat
        egoisth777/grex-test-meta-nested
        egoisth777/grex-test-cycle-a
        egoisth777/grex-test-cycle-b
        egoisth777/grex-test-broken-manifest

    Modes:
      (default)  Provision: create any missing fixture from its seed dir under
                 scripts/real-smoke-fixtures/<name>/. Idempotent — if the GH
                 repo already exists, skip it. Newly-created repos use SSH
                 remotes (git@github.com:egoisth777/<name>.git) and a `main`
                 branch with the seed content committed.

      -Check     Audit only. For each fixture: verify the GH repo exists, SSH
                 clone works, and required seed files are present. NO writes.

      -DryRun    Print the actions that would be taken (provision or check)
                 without invoking gh/git side effects.

    Prerequisites:
      - `gh` CLI on PATH and `gh auth status` already authenticated as a user
        with create-repo permission on the egoisth777 namespace.
      - `git` on PATH with SSH key configured for github.com.

.EXAMPLE
    pwsh -File scripts/provision-real-smoke-fixtures.ps1

    Provision any missing fixture (idempotent; existing repos are skipped).

.EXAMPLE
    pwsh -File scripts/provision-real-smoke-fixtures.ps1 -Check

    Audit all six fixtures; exit 0 if all green, exit nonzero on first red.

.EXAMPLE
    pwsh -File scripts/provision-real-smoke-fixtures.ps1 -DryRun

    Print the actions that would be taken without acting.
#>

[CmdletBinding(DefaultParameterSetName = 'Provision')]
param(
    [Parameter(ParameterSetName = 'Check')]
    [switch]$Check,

    [Parameter(ParameterSetName = 'Provision')]
    [Parameter(ParameterSetName = 'Check')]
    [switch]$DryRun,

    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,

    [string]$Owner = 'egoisth777'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# --- Hardcoded fixture list (DO NOT EXPAND). ----------------------------------
# Each entry: name (also the seed-dir name + GH repo name) and a list of
# required seed files used by -Check mode to confirm push integrity.

$Fixtures = @(
    @{
        Name      = 'grex-test-leaf'
        Required  = @('README.md', '.grex/pack.yaml', 'LICENSE',
                      'pseudo-config.toml', 'pseudo-data.txt')
    },
    @{
        Name      = 'grex-test-meta-flat'
        Required  = @('README.md', '.grex/pack.yaml',
                      'pseudo-data-1.txt', 'pseudo-data-2.txt', 'pseudo-data-3.txt')
    },
    @{
        Name      = 'grex-test-meta-nested'
        Required  = @('README.md', '.grex/pack.yaml',
                      'pseudo-config.toml', 'pseudo-data.txt')
    },
    @{
        Name      = 'grex-test-cycle-a'
        Required  = @('README.md', '.grex/pack.yaml',
                      'pseudo-marker-a.txt', 'LICENSE')
    },
    @{
        Name      = 'grex-test-cycle-b'
        Required  = @('README.md', '.grex/pack.yaml',
                      'pseudo-marker-b.txt', 'LICENSE')
    },
    @{
        Name      = 'grex-test-broken-manifest'
        Required  = @('README.md', '.grex/pack.yaml', '.gitignore',
                      'pseudo-ignored-dir/marker.txt', 'pseudo-data.txt')
    }
)

$SeedRoot = Join-Path $PSScriptRoot 'real-smoke-fixtures'

# --- Helpers ------------------------------------------------------------------

function Write-Step([string]$msg) { Write-Host "[real-smoke] $msg" -ForegroundColor Cyan }
function Write-Ok  ([string]$msg) { Write-Host "[real-smoke] OK    $msg" -ForegroundColor Green }
function Write-Skip([string]$msg) { Write-Host "[real-smoke] skip  $msg" -ForegroundColor DarkGray }
function Write-Warn2([string]$msg) { Write-Host "[real-smoke] WARN  $msg" -ForegroundColor Yellow }
function Write-Err ([string]$msg) { Write-Host "[real-smoke] FAIL  $msg" -ForegroundColor Red }
function Write-DryRun([string]$msg) { Write-Host "[real-smoke] DRY   $msg" -ForegroundColor Magenta }

function Assert-Tool([string]$exe) {
    $cmd = Get-Command $exe -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "$exe is not on PATH. Aborting." }
}

function Test-GhRepoExists {
    param([string]$FullName)
    & gh repo view $FullName --json name 2>$null | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Invoke-Native {
    param(
        [string]$Exe,
        [string[]]$Args,
        [string]$Cwd,
        [switch]$AllowFailure
    )
    Push-Location $Cwd
    try {
        & $Exe @Args
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if (-not $AllowFailure -and $code -ne 0) {
        throw "$Exe $($Args -join ' ') exited $code (cwd=$Cwd)"
    }
    return $code
}

# --- Provision ----------------------------------------------------------------

function Invoke-Provision {
    param([hashtable]$Fixture)

    $name     = $Fixture.Name
    $fullName = "$Owner/$name"
    $seedDir  = Join-Path $SeedRoot $name

    if (-not (Test-Path -LiteralPath $seedDir -PathType Container)) {
        throw "seed dir missing: $seedDir"
    }

    if ($DryRun) {
        Write-DryRun "would provision $fullName from $seedDir (if not already on GH)"
        return
    }

    if (Test-GhRepoExists -FullName $fullName) {
        Write-Skip "$fullName already exists on GitHub"
        return
    }

    Write-Step "creating $fullName from $seedDir"

    # Initialize a local git repo in the seed dir if it isn't one already.
    # We do this in the seed dir directly because `gh repo create --source` expects
    # an existing local repo. Seed dirs are tracked in this repo only as plain
    # files; the .git/ subdir we create here is local-only and harmless (it sits
    # under scripts/real-smoke-fixtures/<name>/.git which is fine for a one-shot
    # operator run).

    if (-not (Test-Path -LiteralPath (Join-Path $seedDir '.git'))) {
        Invoke-Native -Exe 'git' -Args @('init', '-b', 'main') -Cwd $seedDir | Out-Null
        Invoke-Native -Exe 'git' -Args @('add', '-A') -Cwd $seedDir | Out-Null
        Invoke-Native -Exe 'git' -Args @(
            'commit', '-m', "chore: seed $name fixture for real-smoke harness"
        ) -Cwd $seedDir | Out-Null
    }

    # gh repo create with --source pushes the existing local repo, then sets
    # the remote. Use --remote=origin and --push so the operator only runs one
    # command. After this, switch the remote to SSH (gh defaults to https).
    Invoke-Native -Exe 'gh' -Args @(
        'repo', 'create', $fullName,
        '--public',
        '--source', $seedDir,
        '--remote', 'origin',
        '--push'
    ) -Cwd $seedDir | Out-Null

    $sshUrl = "git@github.com:$Owner/$name.git"
    Invoke-Native -Exe 'git' -Args @('remote', 'set-url', 'origin', $sshUrl) -Cwd $seedDir | Out-Null

    # Verify the remote is now SSH.
    Push-Location $seedDir
    try {
        $remotes = & git remote -v
    } finally {
        Pop-Location
    }
    if ($remotes -notmatch [regex]::Escape($sshUrl)) {
        throw "remote rewrite to SSH failed for $fullName (got: $remotes)"
    }

    Write-Ok "$fullName created + pushed (SSH remote: $sshUrl)"
}

# --- Check (audit) ------------------------------------------------------------

function Invoke-Check {
    param([hashtable]$Fixture)

    $name     = $Fixture.Name
    $fullName = "$Owner/$name"
    $sshUrl   = "git@github.com:$Owner/$name.git"
    $required = $Fixture.Required

    if ($DryRun) {
        Write-DryRun "would audit $fullName (gh repo view + ssh clone + seed files: $($required -join ', '))"
        return
    }

    Write-Step "audit $fullName"

    # 1. gh repo view
    if (-not (Test-GhRepoExists -FullName $fullName)) {
        Write-Err "$fullName : gh repo view failed (repo missing or no permission)"
        throw "fixture missing on GitHub: $fullName"
    }

    # 2. SSH clone to a temp dir
    $tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("grex-real-smoke-audit-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $tmpRoot -Force | Out-Null
    $cloneDir = Join-Path $tmpRoot $name

    try {
        $rc = Invoke-Native -Exe 'git' -Args @('clone', '--depth', '1', $sshUrl, $cloneDir) -Cwd $tmpRoot -AllowFailure
        if ($rc -ne 0) {
            Write-Err "$fullName : SSH clone failed (exit $rc) — verify SSH key + repo visibility"
            throw "ssh clone failed for $fullName"
        }

        # 3. Required seed files
        $missing = @()
        foreach ($rel in $required) {
            $fp = Join-Path $cloneDir $rel
            if (-not (Test-Path -LiteralPath $fp)) { $missing += $rel }
        }
        if ($missing.Count -gt 0) {
            Write-Err "$fullName : missing required seed files: $($missing -join ', ')"
            throw "seed files missing in $fullName"
        }

        Write-Ok "$fullName : present on GH, SSH clone OK, seed files present"
    } finally {
        if (Test-Path -LiteralPath $tmpRoot) {
            Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

# --- Main --------------------------------------------------------------------

Write-Step "repo root: $RepoRoot"
Write-Step "seed root: $SeedRoot"
Write-Step "owner    : $Owner"
Write-Step ("mode     : " + $(if ($Check) { 'CHECK (audit only)' } else { 'PROVISION' }) + $(if ($DryRun) { ' + DRY-RUN' } else { '' }))

if (-not $DryRun) {
    Assert-Tool 'gh'
    Assert-Tool 'git'
}

if (-not (Test-Path -LiteralPath $SeedRoot -PathType Container)) {
    throw "seed root missing: $SeedRoot"
}

$fail = 0
foreach ($fx in $Fixtures) {
    try {
        if ($Check) {
            Invoke-Check -Fixture $fx
        } else {
            Invoke-Provision -Fixture $fx
        }
    } catch {
        $fail++
        Write-Err "$($fx.Name): $($_.Exception.Message)"
        # On Check we want to short-circuit on first red per spec; on Provision
        # we keep going so the operator sees all failures in one run.
        if ($Check) { break }
    }
}

Write-Host ''
if ($fail -gt 0) {
    Write-Err "$fail fixture(s) failed."
    exit 1
}

Write-Ok ("all " + $Fixtures.Count + " fixture(s) green")
exit 0
