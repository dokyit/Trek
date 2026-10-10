#Requires -Version 7.0
# Tests for the release-manifest merge the Windows release applies to the Mac's channel
# manifest (script/lib/release-manifest.ps1). Plain assertions, no framework; CI runs this
# file in the Windows job ("Script tests" in .github/workflows/ci.yml).
#
#   pwsh script/tests/release-manifest.Tests.ps1

$ErrorActionPreference = 'Stop'
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
. (Join-Path $Root 'script\lib\release-manifest.ps1')
$Fixtures = Join-Path $PSScriptRoot 'fixtures'

$script:Failures = 0
function Check([string]$Name, [bool]$Cond) {
    if ($Cond) { Write-Host "  ok   $Name" }
    else { Write-Host "  FAIL $Name"; $script:Failures++ }
}
function LoadFixture([string]$Name) {
    Get-Content (Join-Path $Fixtures $Name) -Raw | ConvertFrom-Json
}

$WinSha = 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'
$WinUrl = 'https://github.com/dokyit/Trek/releases/download/v0.4.0/Trek-0.4.0-windows-x86_64.zip'
$WinSig = "untrusted comment: signature from minisign secret key`nRUNewWindowsSignature==`ntrusted comment: Trek 0.4.0 windows-x86_64`nTmV3"

function MergeArgs($Manifest) {
    @{
        Manifest  = $Manifest
        Version   = '0.4.0'
        Platform  = 'windows-x86_64'
        Url       = $WinUrl
        Sha256    = $WinSha
        Signature = $WinSig
    }
}

Write-Host 'add windows to a mac-only manifest'
$mergeArgs = MergeArgs (LoadFixture 'mac-only.json')
$m = Merge-TrekReleaseManifest @mergeArgs
Check 'version kept'                    ($m.version -eq '0.4.0')
Check 'notes kept'                      ($m.notes -eq '- Maintenance release.')
Check 'pub_date kept'                   ($m.pub_date -eq '2026-10-10T01:02:03Z')
Check 'two platforms'                   ($m.platforms.PSObject.Properties.Name.Count -eq 2)
$mac = $m.platforms.'darwin-aarch64'
Check 'mac entry kept'                  ($null -ne $mac)
Check 'mac url untouched'               ($mac.url -eq 'https://github.com/dokyit/Trek/releases/download/v0.4.0/Trek-0.4.0-darwin-aarch64.app.tar.gz')
Check 'mac signature untouched'         ($mac.signature -like '*Trek 0.4.0 darwin-aarch64*')
$win = $m.platforms.'windows-x86_64'
Check 'windows entry added'             ($null -ne $win)
Check 'windows url'                     ($win.url -eq $WinUrl)
Check 'windows sha256 lowercased'       ($win.sha256 -eq $WinSha)
Check 'windows signature'               ($win.signature -eq $WinSig)
Check 'serializes to json'              ($null -ne ($m | ConvertTo-Json -Depth 8))

Write-Host 'replace an existing windows entry (windows listed first in the manifest)'
$mergeArgs = MergeArgs (LoadFixture 'mac-and-windows.json')
$m = Merge-TrekReleaseManifest @mergeArgs
Check 'still two platforms'             ($m.platforms.PSObject.Properties.Name.Count -eq 2)
Check 'mac entry survives replacement'  ($m.platforms.'darwin-aarch64'.signature -like '*Trek 0.4.0 darwin-aarch64*')
Check 'windows sha replaced'            ($m.platforms.'windows-x86_64'.sha256 -eq $WinSha)
Check 'windows signature replaced'      ($m.platforms.'windows-x86_64'.signature -eq $WinSig)
Check 'windows entry kept in place'     (($m.platforms.PSObject.Properties.Name -join ',') -eq 'windows-x86_64,darwin-aarch64')

Write-Host 'a manifest for another version is refused'
$threw = $false
$mergeArgs = MergeArgs (LoadFixture 'wrong-version.json')
try { Merge-TrekReleaseManifest @mergeArgs | Out-Null }
catch { $threw = $_.Exception.Message -like '*0.3.9*0.4.0*' }
Check 'version mismatch throws'         $threw

Write-Host 'the written manifest keeps the Mac entry byte for byte (JSON text in, file out, parsed back)'
$text = Get-Content (Join-Path $Fixtures 'mac-only.json') -Raw
$mergeArgs = MergeArgs $text
$m = Merge-TrekReleaseManifest @mergeArgs
$file = Join-Path ([IO.Path]::GetTempPath()) "trek-manifest-test-$PID.json"
try {
    [IO.File]::WriteAllText($file, ($m | ConvertTo-Json -Depth 8) + "`n")
    $back = Get-Content $file -Raw | ConvertFrom-Json
    $orig = $text | ConvertFrom-Json
    Check 'mac entry identical after a file round trip' ((($back.platforms.'darwin-aarch64' | ConvertTo-Json) -eq ($orig.platforms.'darwin-aarch64' | ConvertTo-Json)))
    Check 'windows entry survives the file'             ($back.platforms.'windows-x86_64'.signature -eq $WinSig)
    Check 'version and pub_date survive the file'       ($back.version -eq '0.4.0' -and (Format-TrekManifestValue $back.pub_date) -eq '2026-10-10T01:02:03Z')
# The raw text matters to Trek's parser too: the date must be written as a string, not a datetime object.
Check 'pub_date is a plain string in the file' ((Get-Content $file -Raw) -match '"pub_date":\s*"2026-10-10T01:02:03Z"')
} finally { Remove-Item $file -ErrorAction SilentlyContinue }

Write-Host 'a manifest with no version is refused'
$threw = $false
$mergeArgs = MergeArgs '{ "platforms": {} }'
try { Merge-TrekReleaseManifest @mergeArgs | Out-Null } catch { $threw = $true }
Check 'missing version throws'          $threw

Write-Host 'fresh manifest (no existing manifest to merge into)'
$mergeArgs = MergeArgs $null
$m = Merge-TrekReleaseManifest @mergeArgs
Check 'version set'                     ($m.version -eq '0.4.0')
Check 'only windows platform'           (($m.platforms.PSObject.Properties.Name -join ',') -eq 'windows-x86_64')
Check 'pub_date stamped'                ($m.pub_date -match '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$')

Write-Host 'rejects a malformed artifact'
foreach ($case in @(
        @{ Name = 'bad sha256'; Sha = 'not-hex' },
        @{ Name = 'empty url';  Url = '' })) {
    $threw = $false
    try {
        $mergedArgs = MergeArgs (LoadFixture 'mac-only.json')
        if ($case.ContainsKey('Sha')) { $mergedArgs.Sha256 = $case.Sha }
        if ($case.ContainsKey('Url')) { $mergedArgs.Url = $case.Url }
        Merge-TrekReleaseManifest @mergedArgs | Out-Null
    } catch { $threw = $true }
    Check $case.Name $threw
}

if ($script:Failures -gt 0) {
    Write-Host "$($script:Failures) test(s) failed"
    exit 1
}
Write-Host 'all tests passed'
