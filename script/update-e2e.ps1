<#
End-to-end check of the updater on Windows, without publishing anything: run an old Trek from a
scratch install folder against a local feed serving a newer release, and watch it download,
verify, stage, quit, have trek-update.exe swap its folder and start the new version, with no
clicks. Or serve a tampered archive and check it's refused and the install is left alone, or hold
a file in the install open and check the swap is rolled back and the old version started again.

  script/update-e2e.ps1 <old Trek-*-windows-*.zip> <new release dir> [-Tamper checksum|signature] [-Lock] [-Keep]

The release dir holds the new version's zip and its channel manifest (stable.json, beta.json or
nightly.json). Releases are signed with the release key; to test with builds of your own, sign
them with a test key instead and build both versions trusting it:

  script/update-e2e.ps1 -NewKey <key dir>              # prints the key line
  $env:TREK_UPDATE_PUBKEY = '<that line>'              # then build trek, trek-mcp, trek-update
  script/update-e2e.ps1 -Pack <folder with the three .exe files> -Key <key dir> -Out <release dir> [-Channel stable]

Everything happens under $env:TREK_E2E_DIR (default %TEMP%\trek-update-e2e) with its own
TREK_DATA_DIR, so your real Trek data and any installed Trek are never touched. The test Trek
opens behind other windows (TREK_BACKGROUND) and restarts as soon as the update is ready
(TREK_UPDATE_AUTO_RESTART). The feed is `fixture serve` on 127.0.0.1 only. Processes it started
are stopped and the folder removed at the end unless -Keep. Exit 0: PASS.
#>
[CmdletBinding(DefaultParameterSetName = 'Run')]
param(
    [Parameter(ParameterSetName = 'Run', Position = 0, Mandatory)] [string] $Old,
    [Parameter(ParameterSetName = 'Run', Position = 1, Mandatory)] [string] $New,
    [Parameter(ParameterSetName = 'Run')] [ValidateSet('checksum', 'signature')] [string] $Tamper,
    [Parameter(ParameterSetName = 'Run')] [switch] $Lock,
    [Parameter(ParameterSetName = 'Run')] [switch] $Keep,
    [Parameter(ParameterSetName = 'NewKey', Mandatory)] [string] $NewKey,
    [Parameter(ParameterSetName = 'Pack', Mandatory)] [string] $Pack,
    [Parameter(ParameterSetName = 'Pack', Mandatory)] [string] $Key,
    [Parameter(ParameterSetName = 'Pack', Mandatory)] [string] $Out,
    [Parameter(ParameterSetName = 'Pack')] [ValidateSet('stable', 'beta', 'nightly')] [string] $Channel = 'stable'
)
$ErrorActionPreference = 'Stop'
$Repo = Split-Path -Parent $PSScriptRoot
$TrekFiles = 'trek.exe', 'trek-mcp.exe', 'trek-update.exe'

function Say([string] $text) { Write-Host "$(Get-Date -Format HH:mm:ss)  $text" }
function VersionOf([string] $exe) { (Get-Item $exe).VersionInfo.ProductVersion }

# The `fixture` test binary: the feed server and the test signing key. $env:TREK_E2E_FIXTURE names
# a copy of it, for a target folder other checkouts build their own `fixture` into.
function Fixture {
    if ($env:TREK_E2E_FIXTURE) { return $env:TREK_E2E_FIXTURE }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
    $exe = Join-Path $target 'debug\fixture.exe'
    if (-not (Test-Path $exe)) {
        Push-Location $Repo
        try { cargo build -p trek-test-fixtures --locked | Out-Host } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw 'cargo build -p trek-test-fixtures failed' }
    }
    $exe
}

if ($PSCmdlet.ParameterSetName -eq 'NewKey') {
    & (Fixture) minisign-keygen $NewKey
    exit $LASTEXITCODE
}

if ($PSCmdlet.ParameterSetName -eq 'Pack') {
    # The release layout: a flat zip of Trek's three files, signed with the trusted comment
    # "Trek <version> <platform>", and the channel manifest pointing at it.
    $version = VersionOf (Join-Path $Pack 'trek.exe')
    $platform = 'windows-' + $(if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' })
    New-Item -ItemType Directory -Force $Out | Out-Null
    $zip = Join-Path $Out "Trek-$version-$platform.zip"
    Remove-Item -Force -ErrorAction SilentlyContinue $zip, "$zip.minisig"
    Compress-Archive -Path ($TrekFiles | ForEach-Object { Join-Path $Pack $_ }) -DestinationPath $zip
    & (Fixture) minisign-sign (Join-Path $Key 'test.seed') $zip "Trek $version $platform"
    if ($LASTEXITCODE -ne 0) { throw 'signing failed' }
    $manifest = [ordered]@{
        version   = $version
        notes     = "- Test build $version"
        pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        platforms = @{ $platform = [ordered]@{ url = "https://example.invalid/$(Split-Path -Leaf $zip)"; sha256 = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower(); signature = Get-Content -Raw "$zip.minisig" } }
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8NoBOM (Join-Path $Out "$Channel.json")
    Say "packed Trek $version ($platform) into $Out"
    exit 0
}

$Old = (Resolve-Path $Old).Path
$New = (Resolve-Path $New).Path
$Work = if ($env:TREK_E2E_DIR) { $env:TREK_E2E_DIR } else { Join-Path $env:TEMP 'trek-update-e2e' }
$Port = if ($env:TREK_E2E_PORT) { [int] $env:TREK_E2E_PORT } else { 8765 }
$Install = Join-Path $Work 'apps\Trek'
$Data = Join-Path $Work 'data'
$Fixture = Fixture

# Only processes running from the scratch folder: never any other Trek. (CIM, not Get-Process:
# that has no path for a process trek-update started detached.)
function TestProcesses { Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Work, [StringComparison]::OrdinalIgnoreCase) } }
function StopAll($procs) { foreach ($p in @($procs)) { if ($p) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue } } }
$Server = $null
$Held = $null
function Cleanup {
    # Helpers first (one waiting for Trek would swap as soon as Trek is stopped), then Trek.
    foreach ($pass in 1..2) {
        StopAll (TestProcesses | Where-Object { $_.Name -eq 'trek-update.exe' })
        StopAll (TestProcesses)
        Start-Sleep -Milliseconds 300
    }
    if ($script:Held) { $script:Held.Dispose() }
    if ($script:Server) { Stop-Process -Id $script:Server.Id -Force -ErrorAction SilentlyContinue }
    if (-not $Keep) { Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Work }
}

# The run starts by emptying $Work and ends by removing it: only ever a folder this script made.
$Marker = Join-Path $Work '.trek-update-e2e'
if ((Test-Path $Work) -and -not (Test-Path $Marker) -and (Get-ChildItem -Force $Work | Select-Object -First 1)) {
    throw "$Work exists and isn't an update-e2e scratch folder (no .trek-update-e2e in it); name an empty or new folder in TREK_E2E_DIR"
}
$outcome = 'timeout'
try {
    StopAll (TestProcesses)
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Work
    New-Item -ItemType Directory -Force $Install, (Join-Path $Work 'serve'), $Data | Out-Null
    New-Item -ItemType File $Marker | Out-Null
    Expand-Archive -Path $Old -DestinationPath $Install
    $From = VersionOf (Join-Path $Install 'trek.exe')
    $manifestFile = 'stable.json', 'beta.json', 'nightly.json' | ForEach-Object { Join-Path $New $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $manifestFile) { throw "no manifest in $New" }
    $Channel = [IO.Path]::GetFileNameWithoutExtension($manifestFile)
    $archive = Get-ChildItem (Join-Path $New 'Trek-*-windows-*.zip') | Select-Object -First 1
    if (-not $archive) { throw "no Trek-*-windows-*.zip in $New" }
    Copy-Item $archive.FullName (Join-Path $Work 'serve')
    $archive = Join-Path $Work "serve\$($archive.Name)"
    $manifest = Get-Content -Raw $manifestFile | ConvertFrom-Json
    $To = $manifest.version

    if ($Tamper) {
        # Flip one byte in the middle of the archive.
        $bytes = [IO.File]::ReadAllBytes($archive)
        $bytes[[int]($bytes.Length / 2)] = $bytes[[int]($bytes.Length / 2)] -bxor 0xFF
        [IO.File]::WriteAllBytes($archive, $bytes)
    }
    # Point the manifest at the local copy (the URL isn't signed; the archive is). For -Tamper
    # signature the checksum is updated too, so only the signature can catch it.
    foreach ($p in $manifest.platforms.PSObject.Properties) {
        $p.Value.url = "http://127.0.0.1:$Port/$(Split-Path -Leaf $archive)"
        if ($Tamper -eq 'signature') { $p.Value.sha256 = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower() }
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8NoBOM (Join-Path $Work "serve\$Channel.json")
    @"
[updates]
channel = "$Channel"
auto_check = true
auto_download = true
feed_url = "http://127.0.0.1:$Port/{channel}.json"

[onboarding]
completed = true

[import]
claude_code = false
codex = false
opencode = false

[notifications]
mode = "off"
dock_badge = false
menu_bar_icon = false
"@ | Set-Content -Encoding utf8NoBOM (Join-Path $Data 'settings.toml')

    $Server = Start-Process $Fixture -ArgumentList 'serve', "`"$Work\serve`"", $Port -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $Work 'http.log')
    Start-Sleep -Milliseconds 500
    Say "serving $Channel.json ($To) on 127.0.0.1:$Port$(if ($Tamper) { ", archive tampered ($Tamper)" })"
    if ($Lock) {
        # Like an editor or a scan that holds a file: Windows won't rename the folder around it.
        $Held = [IO.File]::Open((Join-Path $Install 'trek-mcp.exe'), 'Open', 'Read', 'Read')
        Say 'holding trek-mcp.exe open in the install'
    }
    $env:TREK_DATA_DIR = $Data
    $env:TREK_BACKGROUND = '1'
    $env:TREK_UPDATE_AUTO_RESTART = '1'
    $env:RUST_LOG = 'warn,trek=info,trek_core=info'
    $first = Start-Process (Join-Path $Install 'trek.exe') -WorkingDirectory $Work -PassThru
    Say "started Trek $From from $Install (pid $($first.Id))"

    $updateLog = Join-Path $Data 'logs\update.log'
    foreach ($i in 1..240) {
        $trekLog = Get-ChildItem (Join-Path $Data 'logs\trek-*.log') -ErrorAction SilentlyContinue | Get-Content -Raw -ErrorAction SilentlyContinue
        if ($trekLog -match 'rejected') { $outcome = 'rejected'; break }
        if ($first.HasExited -and (Test-Path $updateLog)) {
            $u = Get-Content -Raw $updateLog
            if ($u -match 'the new version started') { $outcome = 'updated'; break }
            if ($u -match 'nothing was changed|previous version is back') { $outcome = 'rolled back'; break }
        }
        Start-Sleep -Milliseconds 500
    }
    # Whatever the helper started has a moment to show up.
    Start-Sleep -Seconds 2
    $running = TestProcesses | Where-Object { $_.Name -eq 'trek.exe' }

    Say "outcome: $outcome"
    Write-Host "--- log of Trek $From (pid $($first.Id)) and after"
    Get-ChildItem (Join-Path $Data 'logs\trek-*.log') -ErrorAction SilentlyContinue | Get-Content | Where-Object { $_ -match 'update|Update' } | ForEach-Object { "    $_" }
    Write-Host '--- update.log'
    if (Test-Path $updateLog) { Get-Content $updateLog | ForEach-Object { "    $_" } } else { '    (none)' }
    Write-Host '--- feed requests'
    Get-Content (Join-Path $Work 'http.log') -ErrorAction SilentlyContinue | ForEach-Object { "    $_" }
    Write-Host '--- result'
    $now = VersionOf (Join-Path $Install 'trek.exe')
    "    install version:  $now"
    "    running now:      $(if ($running) { ($running | ForEach-Object { "pid $($_.ProcessId) $($_.ExecutablePath)" }) -join ', ' } else { 'nothing' })"
    "    next to install:  $((Get-ChildItem (Split-Path $Install) | ForEach-Object Name) -join ', ')"
    "    download folders: $(((Get-ChildItem (Join-Path $Data 'updates') -Directory -Filter 'download-*' -ErrorAction SilentlyContinue) | ForEach-Object Name) -join ', ')"

    $pass = if ($Tamper) {
        $outcome -eq 'rejected' -and $now -eq $From
    } elseif ($Lock) {
        # The old version is started again (and, with automatic restarts, soon tries again).
        $u = Get-Content -Raw $updateLog
        $outcome -eq 'rolled back' -and $now -eq $From -and $u -match "couldn't move" -and $u -match 'started .*trek\.exe'
    } else {
        # The previous version moved to Trek.old-<version>, which the new one deletes half a minute after it starts.
        $outcome -eq 'updated' -and $now -eq $To -and $running -and (Get-Content -Raw $updateLog) -match "previous version is in .*Trek\.old-$([regex]::Escape($From))"
    }
    if ($pass) {
        Say $(if ($Tamper) { "PASS: tampered update refused, Trek $From untouched" } elseif ($Lock) { "PASS: the swap failed on a held file, Trek $From is back and running" } else { "PASS: Trek $From updated itself to $To and relaunched" })
        exit 0
    }
    Say 'FAIL'
    exit 1
} finally {
    Cleanup
}
