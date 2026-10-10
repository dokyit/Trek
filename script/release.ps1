#Requires -Version 7.0
# Build and publish the Windows half of a Trek release (docs/RELEASING.md). The PowerShell
# counterpart of script/release.sh: the Mac publishes first, this attaches Windows artifacts
# to the release it already made.
#
#   pwsh script/release.ps1 <version> [-Channel stable|beta|nightly] [-Notes FILE] [-Publish]
#
# Sets the workspace version for the build, builds trek.exe and trek-mcp.exe (release,
# --locked), smokes trek-core, packs Trek-<version>-windows-x86_64.zip flat, signs it with
# minisign (trusted comment "Trek <version> windows-x86_64") and writes the channel manifest
# to dist/release/<version>/. With -Publish it uploads to the channel's EXISTING GitHub
# release - zip and .minisig first, the merged <channel>.json last, --clobber - like
# release.sh does. It never creates a release, tag or commit, and always puts Cargo.toml and
# Cargo.lock back; without -Publish nothing leaves the machine.
#
# Environment (same names as release.sh):
#   TREK_MINISIGN_KEY          secret key (default ~/.trek-signing/minisign.key); a locked one
#                              takes TREK_MINISIGN_PASSWORD on stdin
#   TREK_MINISIGN_PASSWORD     the key's password (the workflow feeds it from secrets)
#   TREK_RELEASE_REPO          GitHub repository (default dokyit/Trek)
#   TREK_RELEASE_DOWNLOAD_URL  where the manifest says the archive lives (default: the GitHub
#                              release); for testing against a local server
#   BUILD=0                    reuse the binaries already in the release target dir - for
#                              exercising this script, never for a real build (-Publish refuses)
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Version,

    [ValidateSet('stable', 'beta', 'nightly')]
    [string]$Channel = 'stable',

    # Release notes file; without one, the git log since the channel's last tag (release.sh's
    # rule). Only used when there is no existing manifest to merge into.
    [string]$Notes,

    # Minisign secret key (default $env:TREK_MINISIGN_KEY, else ~/.trek-signing/minisign.key)
    [string]$Key,

    # Public key the signature is verified against (default assets/update/minisign.pub).
    # -Publish insists on the shipped key: a release users can't verify is worse than none.
    [string]$PubKey,

    # The minisign binary (default: "minisign" on PATH; the workflow passes the downloaded one).
    [string]$Minisign = 'minisign',

    # An existing <channel>.json to merge into, instead of a fresh manifest - dry runs only;
    # -Publish always downloads the release's own.
    [string]$ExistingManifest,

    # Upload the zip, signature and merged manifest to the release that must already exist.
    [switch]$Publish,

    # Skip the `cargo test -p trek-core --locked` smoke (still runs by default, like the
    # workflow).
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
. (Join-Path $PSScriptRoot 'lib\release-manifest.ps1')

function Say([string]$Msg) { Write-Host $Msg }
function Die([string]$Msg) { [Console]::Error.WriteLine("release: $Msg"); exit 1 }

if ([string]::IsNullOrWhiteSpace($Version)) {
    [Console]::Error.WriteLine('usage: pwsh script/release.ps1 <version> [-Channel stable|beta|nightly] [-Notes FILE] [-Publish]')
    exit 2
}
$Version = $Version.TrimStart('v')
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') { Die "$Version is not a semver version" }
$pre = if ($Version.Contains('-')) { $Version.Substring($Version.IndexOf('-') + 1) } else { '' }
$channelBad = switch ($Channel) {
    'stable'   { [bool]$pre }
    { $_ -in 'beta', 'nightly' } { -not $pre.StartsWith($_) }
}
if ($channelBad) {
    $msg = if ($Channel -eq 'stable') { "stable versions have no pre-release suffix ($Version)" } else { "$Channel versions look like 1.2.3-$Channel.N ($Version)" }
    # A dry run may build any version (script/release.ps1 0.0.0-test); publishing may not.
    if ($Publish) { Die $msg } else { Say " ! $msg - allowed only because this is a dry run" }
}
if ($Notes -and -not (Test-Path $Notes)) { Die "no notes file at $Notes" }
if ($Publish -and $env:BUILD -eq '0') { Die 'BUILD=0 only exercises a dry run; -Publish builds for real' }
if ($Publish -and $PubKey) { Die '-Publish verifies against assets/update/minisign.pub, not a test key' }
if ($Publish -and $ExistingManifest) { Die '-Publish downloads the release''s own manifest; -ExistingManifest is for dry runs' }
if (-not $PubKey) { $PubKey = 'assets\update\minisign.pub' }

$Repo = if ($env:TREK_RELEASE_REPO) { $env:TREK_RELEASE_REPO } else { 'dokyit/Trek' }
$Platform = 'windows-x86_64'
$Name = "Trek-$Version-$Platform.zip"
$ReleaseTag = if ($Channel -eq 'stable') { "v$Version" } else { $Channel }
$Out = "dist\release\$Version"
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$BinDir = Join-Path $TargetDir 'release'

# ---------- checks ----------
if (-not $Key) { $Key = $env:TREK_MINISIGN_KEY }
if (-not $Key) {
    $default = Join-Path $HOME '.trek-signing\minisign.key'
    if (Test-Path $default) { $Key = $default }
}
$haveKey = $Key -and (Test-Path $Key)
if ($Key -and -not $haveKey) { Die "no minisign secret key at $Key (docs/RELEASING.md)" }
if ($Publish -and -not $haveKey) { Die 'publishing needs a minisign key (TREK_MINISIGN_KEY); an unsigned update is refused by every Trek' }
if ($haveKey) {
    if (Test-Path $Minisign -PathType Leaf) { $Minisign = (Resolve-Path $Minisign).Path }
    elseif (-not (Get-Command $Minisign -ErrorAction SilentlyContinue)) { Die "minisign isn't installed (see docs/RELEASING.md)" }
}

if ($Publish) {
    & gh auth status *> $null
    if ($LASTEXITCODE -ne 0) { Die "gh isn't logged in" }
    & gh release view $ReleaseTag --repo $Repo *> $null
    if ($LASTEXITCODE -ne 0) { Die "release $ReleaseTag doesn't exist - the Mac's script/release.sh publishes first" }
    if (git status --porcelain) { Die 'the working tree has changes; commit or stash them first' }
}

# ---------- version ----------
# Set for the build, put back on the way out - release.ps1 never commits or tags (the Mac's
# release.sh already did for stable).
$backup = Join-Path ([IO.Path]::GetTempPath()) "trek-release-$PID"
New-Item -ItemType Directory -Force $backup | Out-Null
Copy-Item Cargo.toml, Cargo.lock $backup
try {
    $current = $null
    $toml = Get-Content Cargo.toml
    $inPkg = $false
    foreach ($line in $toml) {
        if ($line -match '^\s*\[') { $inPkg = $line -match '^\[workspace\.package\]' }
        elseif ($inPkg -and $line -match '^\s*version\s*=') {
            $current = ($line -replace '^\s*version\s*=\s*"([^"]*)".*$', '$1')
        }
        if ($null -ne $current) { break }
    }
    if (-not $current) { Die 'no [workspace.package] version in Cargo.toml' }
    if ($Version -ne $current) {
        # The Mac's script committed (stable) or tagged (beta, nightly) a tree that already has the
        # version; a runner building a tree that doesn't has the wrong ref and would ship a Trek
        # whose own version disagrees with its manifest.
        if ($Publish -and $env:GITHUB_ACTIONS) { Die "the checked-out tree is version $current, not $Version - wrong ref?" }
        Say " version $current -> $Version"
        $inPkg = $false
        $toml = $toml | ForEach-Object {
            if ($_ -match '^\s*\[') { $inPkg = $_ -match '^\[workspace\.package\]' }
            elseif ($inPkg -and $_ -match '^\s*version\s*=') { $_ = "version = `"$Version`"" }
            $_
        }
        [IO.File]::WriteAllText((Resolve-Path Cargo.toml).Path, ($toml -join "`n") + "`n")
        # Workspace versions live in Cargo.lock too; refresh it so --locked builds.
        & cargo update --workspace --offline 2> $null
        if ($LASTEXITCODE -ne 0) {
            & cargo update --workspace
            if ($LASTEXITCODE -ne 0) { Die 'could not update Cargo.lock for the version bump' }
        }
    }

    # ---------- build ----------
    $packages = @('trek-app', 'trek-mcp')
    # TODO(wp/updater): add 'trek-update' once the helper crate exists on this base - the
    # updater worker adds it; the zip layout is fixed with it in mind.
    if (Test-Path 'crates\trek-update') { $packages += 'trek-update' }
    if ($env:BUILD -eq '0') {
        Say ' build: BUILD=0, reusing the binaries already in the target dir'
    } else {
        $buildArgs = @('--release', '--locked') + ($packages | ForEach-Object { '-p'; $_ })
        Say " build: cargo build $($buildArgs -join ' ')"
        & cargo build @buildArgs
        if ($LASTEXITCODE -ne 0) { Die 'cargo build failed' }
    }
    if (-not $SkipTests) {
        Say ' smoke: cargo test -p trek-core --locked'
        & cargo test -p trek-core --locked
        if ($LASTEXITCODE -ne 0) { Die 'trek-core tests failed' }
    }

    # ---------- archive, checksum, signature ----------
    $exes = @('trek.exe', 'trek-mcp.exe')
    # TODO(wp/updater): include trek-update.exe once the crate exists.
    if (Test-Path 'crates\trek-update') { $exes += 'trek-update.exe' }
    $staged = @()
    foreach ($exe in $exes) {
        $p = Join-Path $BinDir $exe
        if (-not (Test-Path $p)) { Die "$p missing after build" }
        $staged += $p
    }
    Remove-Item $Out -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $Out | Out-Null
    Compress-Archive -Path $staged -DestinationPath "$Out\$Name" -Force
    # The layout is a contract with the updater: flat, exactly these files.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead((Resolve-Path "$Out\$Name").Path)
    try { $entries = @($zip.Entries | ForEach-Object FullName | Sort-Object) } finally { $zip.Dispose() }
    if (($entries -join '|') -ne (($exes | Sort-Object) -join '|')) {
        Die "$Name holds [$($entries -join ', ')], expected exactly [$($exes -join ', ')]"
    }
    $sha = (Get-FileHash "$Out\$Name" -Algorithm SHA256).Hash.ToLowerInvariant()

    $signature = ''
    if ($haveKey) {
        # Whether the key has a password: minisign's key names its KDF ("Sc", scrypt) or none.
        $keyLines = Get-Content $Key
        $keyLocked = $false
        try { $keyLocked = [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($keyLines[1])[2..3]) -eq 'Sc' } catch {}
        if (-not $keyLocked) {
            Say " ! $Key has no password: anything running as you can read it and sign updates"
            Say '   every installed Trek accepts. Give it one: docs/RELEASING.md, "Update signing key".'
        }
        $signArgs = @('-S', '-s', $Key, '-m', "$Out\$Name", '-x', "$Out\$Name.minisig", '-t', "Trek $Version $Platform")
        if ($keyLocked -and $env:TREK_MINISIGN_PASSWORD) {
            $env:TREK_MINISIGN_PASSWORD | & $Minisign @signArgs | Out-Null
        } else {
            # No password known: minisign asks on stdin itself (dry run with an unlocked key
            # reads nothing at all).
            & $Minisign @signArgs | Out-Null
        }
        if ($LASTEXITCODE -ne 0) { Die 'minisign signing failed' }
        & $Minisign -V -q -p $PubKey -m "$Out\$Name" -x "$Out\$Name.minisig"
        if ($LASTEXITCODE -ne 0) { Die "$Key doesn't match $PubKey; Trek would refuse this release" }
        # minisign.exe writes CRLF; the Mac's manifest has LF, and so should this one.
        $signature = (Get-Content "$Out\$Name.minisig" -Raw).Trim() -replace "`r`n", "`n"
        Say " $Name  sha256 $sha  (signature verified with $PubKey)"
    } else {
        Say " ! no minisign key - $Name is left unsigned; Trek would refuse this update"
    }

    # ---------- notes ----------
    if ($Notes) {
        Copy-Item $Notes "$Out\notes.md"
    } else {
        # The most recent tag, by when it was made: git's version sort puts 0.4.0-beta.1 after 0.4.0.
        $since = switch ($Channel) {
            'stable'  { git tag --list 'v[0-9]*' --sort=-creatordate | Where-Object { $_ -notmatch '-' } | Select-Object -First 1 }
            'nightly' { if (git rev-parse -q --verify refs/tags/nightly 2> $null) { 'nightly' } else { git tag --list 'v[0-9]*' --sort=-creatordate | Select-Object -First 1 } }
            default   { git tag --list 'v[0-9]*' --sort=-creatordate | Select-Object -First 1 }
        }
        $range = 'HEAD'; if ($since) { $range = "$since..HEAD" }
        $log = git log --no-merges --invert-grep --grep='^Release v' --format='- %s' $range
        $text = if ($log) { ($log -join "`n") + "`n" } else { "- Maintenance release.`n" }
        [IO.File]::WriteAllText((Join-Path $Out 'notes.md'), $text)
    }

    # ---------- manifest ----------
    $base = if ($env:TREK_RELEASE_DOWNLOAD_URL) { $env:TREK_RELEASE_DOWNLOAD_URL } else { "https://github.com/$Repo/releases/download/$ReleaseTag" }
    $url = "$($base.TrimEnd('/'))/$Name"
    $manifestPath = Join-Path $Out "$Channel.json"
    $existing = $null
    if ($Publish) {
        $dl = Join-Path $backup 'manifest'
        New-Item -ItemType Directory -Force $dl | Out-Null
        & gh release download $ReleaseTag --repo $Repo -p "$Channel.json" -D $dl --clobber
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path (Join-Path $dl "$Channel.json"))) {
            Die "no $Channel.json on release $ReleaseTag - the Mac's release.sh publishes it first"
        }
        $existing = Get-Content (Join-Path $dl "$Channel.json") -Raw | ConvertFrom-Json
    } elseif ($ExistingManifest) {
        if (-not (Test-Path $ExistingManifest)) { Die "no manifest at $ExistingManifest" }
        $existing = Get-Content $ExistingManifest -Raw | ConvertFrom-Json
    }
    $merged = Merge-TrekReleaseManifest -Manifest $existing -Version $Version -Platform $Platform `
        -Url $url -Sha256 $sha -Signature $signature -Notes ((Get-Content "$Out\notes.md" -Raw).Trim())
    if ($existing -and -not ($merged.platforms.PSObject.Properties.Name | Where-Object { $_ -like 'darwin-*' })) {
        # The Mac publishes first, always; a manifest without its entry is the wrong release or a
        # half-published one, and attaching Windows to it would ship an update only Windows sees.
        $msg = 'the existing manifest has no darwin-* entry - is this the right release, did the Mac publish?'
        if ($Publish) { Die $msg } else { Say " ! $msg" }
    }
    [IO.File]::WriteAllText($manifestPath, ($merged | ConvertTo-Json -Depth 8) + "`n")
    Say " $manifestPath -> $url"

    $assets = @("$Out\$Name") + $(if ($signature) { @("$Out\$Name.minisig") } else { @() }) + @($manifestPath)
    if (-not $Publish) {
        Say " dry run: built $Out; not publishing. Would upload to ${Repo}'s release $ReleaseTag ($Channel):"
        foreach ($a in $assets) { Say "   $(Split-Path $a -Leaf)" }
        exit 0
    }

    # ---------- publish ----------
    # Archive and signature first, the manifest that points at them last (release.sh's order):
    # a client that fetches the manifest mid-upload finds the zip already there.
    & gh release upload $ReleaseTag --repo $Repo --clobber "$Out\$Name" "$Out\$Name.minisig"
    if ($LASTEXITCODE -ne 0) { Die 'uploading the archive failed' }
    & gh release upload $ReleaseTag --repo $Repo --clobber $manifestPath
    if ($LASTEXITCODE -ne 0) { Die 'uploading the manifest failed - the release still has the previous one' }
    # Stale Windows archives under the moving beta/nightly tags get removed, like release.sh
    # does for the Mac's.
    $assetNames = & gh release view $ReleaseTag --repo $Repo --json assets --jq '.assets[].name'
    foreach ($asset in $assetNames) {
        if ($asset -like "Trek-*-$Platform.zip*" -and $asset -ne $Name -and $asset -ne "$Name.minisig") {
            & gh release delete-asset $ReleaseTag $asset --repo $Repo -y
        }
    }
    Say " published Trek $Version for $Platform on $Channel`: https://github.com/$Repo/releases/tag/$ReleaseTag"
} finally {
    Copy-Item "$backup\Cargo.toml", "$backup\Cargo.lock" . -Force
    Remove-Item $backup -Recurse -Force -ErrorAction SilentlyContinue
}
