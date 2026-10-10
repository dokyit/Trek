# Shared release-manifest merge for the Windows release: dot-sourced by script/release.ps1
# and by .github/workflows/release-windows.yml. Tests: script/tests/release-manifest.Tests.ps1.
#
# A channel manifest (<channel>.json, written by script/release.sh on the Mac) looks like:
#
#   { "version": "0.2.0", "notes": "…", "pub_date": "2026-10-01T00:00:00Z",
#     "platforms": { "darwin-aarch64": { "url": "…", "sha256": "…",
#                                      "signature": "<the .minisig file>" } } }
#
# release.sh publishes the manifest with only its own platform; the Windows build attaches its
# zip to the same release and merges the windows-x86_64 entry in. The manifest's version must
# match the version being built: the merge refuses a mismatch rather than pointing users at a
# different version's archive.

# PowerShell's ConvertFrom-Json turns "2026-10-10T01:02:03Z" into a [datetime]; write it back
# the way the manifest had it.
function Format-TrekManifestValue($Value) {
    if ($Value -is [datetime]) {
        return $Value.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")
    }
    "$Value"
}

function Merge-TrekReleaseManifest {
    [CmdletBinding()]
    param(
        # The existing manifest - ConvertFrom-Json output or its JSON text - or $null/empty
        # for a fresh one.
        [AllowNull()]
        [AllowEmptyString()]
        $Manifest,

        # The release version as the manifest writes it: "0.4.0", "0.4.0-beta.1" (no "v").
        [Parameter(Mandatory)]
        [string]$Version,

        # The platforms key - "windows-x86_64" (trek-core update::platform_key()).
        [Parameter(Mandatory)]
        [string]$Platform,

        # Where the artifact is served from (the release's download URL for the zip).
        [Parameter(Mandatory)]
        [string]$Url,

        # Hex SHA-256 of the artifact.
        [Parameter(Mandatory)]
        [string]$Sha256,

        # The .minisig file's contents. Empty is allowed for a local dry run without a key;
        # such a manifest must never be published (Trek refuses unsigned updates).
        [AllowEmptyString()]
        [string]$Signature = '',

        # Notes for a fresh manifest only; an existing manifest keeps its own.
        [AllowEmptyString()]
        [string]$Notes = ''
    )

    $ErrorActionPreference = 'Stop'

    $version = $Version.TrimStart('v')
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') {
        throw "release: $Version is not a semver version"
    }
    if ($Sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "release: bad sha256 '$Sha256'"
    }
    if ([string]::IsNullOrWhiteSpace($Url)) {
        throw 'release: empty artifact url'
    }
    if ($Manifest -is [string]) {
        $Manifest = if ([string]::IsNullOrWhiteSpace($Manifest)) { $null } else { $Manifest | ConvertFrom-Json }
    }

    $artifact = [pscustomobject][ordered]@{
        url       = $Url
        sha256    = $Sha256.ToLowerInvariant()
        signature = $Signature
    }
    $now = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")

    if ($null -eq $Manifest) {
        return [pscustomobject][ordered]@{
            version   = $version
            notes     = $Notes
            pub_date  = $now
            platforms = [pscustomobject][ordered]@{ $Platform = $artifact }
        }
    }

    $existingVersion = Format-TrekManifestValue $Manifest.version
    $existingVersion = $existingVersion.TrimStart('v')
    if ($existingVersion -ne $version) {
        throw "release: manifest is for $existingVersion, not $version - refusing to merge another version's artifact"
    }

    # Keep every existing platform entry (the Mac's darwin-* first and foremost), replacing
    # only ours in place so a re-run of the same release updates the entry, not appends it.
    $platforms = [ordered]@{}
    $replaced = $false
    if ($null -ne $Manifest.platforms) {
        foreach ($prop in $Manifest.platforms.PSObject.Properties) {
            if ($prop.Name -eq $Platform) {
                $platforms[$Platform] = $artifact
                $replaced = $true
                continue
            }
            # Copy only the fields Trek reads, so nothing stray passes through.
            $a = $prop.Value
            $platforms[$prop.Name] = [pscustomobject][ordered]@{
                url       = "$($a.url)"
                sha256    = "$($a.sha256)"
                signature = "$($a.signature)"
            }
        }
    }
    if (-not $replaced) {
        $platforms[$Platform] = $artifact
    }

    [pscustomobject][ordered]@{
        version   = $existingVersion
        notes     = Format-TrekManifestValue $Manifest.notes
        pub_date  = if ($Manifest.pub_date) { Format-TrekManifestValue $Manifest.pub_date } else { $now }
        platforms = [pscustomobject]$platforms
    }
}
