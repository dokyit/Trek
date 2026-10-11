<#
.SYNOPSIS
Installs the Devin CLI on Windows.

.DESCRIPTION
Downloads the latest Windows bundle, verifies its SHA256, installs it under
%LOCALAPPDATA%\devin\cli, adds the binary directory
to the user PATH, and then runs `devin setup`.

.NOTES
Usage:
  irm https://static.devin.ai/cli/install.ps1 | iex

This script is generated from a template. Do not edit the published script
in-place.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Ensure TLS 1.2 for older PowerShell (e.g. 5.1)
try {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
} catch {}

$BaseUrl = "https://static.devin.ai/cli"
$BinaryName = "devin"
$ProductSubdir = "cli"
$Distribution = "irm-iex"
$CliDisplayName = ""

# The binary inside the bundle is always "devin.exe" (Cargo [[bin]] target name),
# regardless of channel. $BinaryName is the user-facing binary name.
$CompiledBinName = "devin"

# The namespace root is always "devin" regardless of channel (stable / next /
# insiders). This matches the XDG_DIR_NAMESPACE used on Unix.
$NamespaceName = "devin"

function Write-Info($Message) {
  Write-Host $Message
}

function Ensure-Dir($Path) {
  if (-not (Test-Path -LiteralPath $Path)) {
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
  }
}

function Get-TargetTriple {
  $arch = $env:PROCESSOR_ARCHITECTURE
  if ($arch -eq "ARM64") {
    return "aarch64-pc-windows"
  }
  return "x86_64-pc-windows"
}

$Target = Get-TargetTriple

# Set by the build system for versioned setup scripts (e.g. cli/2026.3.5-1/setup.ps1).
# The top-level setup.ps1 leaves this empty to install the latest promoted version.
$PinnedVersion = ""
$VersionPath = if ($PinnedVersion -ne "") { $PinnedVersion } else { "current" }

$ManifestUrl = if ($Distribution -eq "enterprise-portal") {
  # The portal serves exactly one version; there is no versioned manifest path.
  "$BaseUrl/api/cli/update"
} elseif ($Distribution -eq "irm-iex-enterprise") {
  "$BaseUrl/$VersionPath/manifest-enterprise.json"
} elseif ($Distribution -eq "irm-iex-windsurfcom") {
  "$BaseUrl/$VersionPath/manifest-windsurfcom.json"
} elseif ($Distribution -eq "irm-iex-federal") {
  "$BaseUrl/$VersionPath/manifest-federal.json"
} else {
  "$BaseUrl/$VersionPath/manifest.json"
}

# Use Invoke-WebRequest for compatibility with Windows PowerShell 5.1
$manifestText = (Invoke-WebRequest -Uri $ManifestUrl -UseBasicParsing).Content
$manifest = $manifestText | ConvertFrom-Json

$Version = $manifest.version
if (-not $Version) {
  throw "Failed to parse version from manifest"
}

$platformInfo = $manifest.platforms.$Target
if (-not $platformInfo) {
  throw "No bundle available for platform $Target"
}

$BundleUrl = $platformInfo.url
$ExpectedSha256 = $platformInfo.sha256

if (-not $BundleUrl -or -not $ExpectedSha256) {
  throw "Manifest is missing url/sha256 for platform $Target"
}

$LocalAppData = $env:LOCALAPPDATA
if (-not $LocalAppData) {
  throw "LOCALAPPDATA is not set"
}

$NamespaceRoot = Join-Path $LocalAppData $NamespaceName
$InstallRoot = Join-Path $NamespaceRoot $ProductSubdir
$VersionsDir = Join-Path $InstallRoot "_versions"
$VersionDir = Join-Path $VersionsDir $Version
$VersionBinDir = Join-Path $VersionDir "bin"
$EntryBinDir = Join-Path $InstallRoot "bin"
$EntryExe = Join-Path $EntryBinDir ("{0}.exe" -f $BinaryName)
$MarkerPath = Join-Path $InstallRoot "distribution"

Ensure-Dir $NamespaceRoot
Ensure-Dir $InstallRoot
Ensure-Dir $VersionsDir
Ensure-Dir $EntryBinDir

if ((Test-Path -LiteralPath $EntryExe) -and (Get-Item -LiteralPath $EntryExe).Attributes.ToString().Contains("ReparsePoint")) {
  # avoid weird states if someone had a symlinked entrypoint
  Remove-Item -LiteralPath $EntryExe -Force
}

if (Test-Path -LiteralPath $VersionDir) {
  # Already installed — still refresh the entrypoint and marker below.
} else {
  $TempDir = $env:TEMP
  if (-not $TempDir) {
    $TempDir = (Get-Location).Path
  }

  $BundleFile = Join-Path $TempDir ("{0}-{1}.zip" -f $BinaryName, $Version)
  $ExtractDir = Join-Path $TempDir ("{0}-{1}.tmp" -f $BinaryName, $Version)

  if (Test-Path -LiteralPath $BundleFile) {
    Remove-Item -LiteralPath $BundleFile -Force
  }
  if (Test-Path -LiteralPath $ExtractDir) {
    Remove-Item -LiteralPath $ExtractDir -Recurse -Force
  }

  Write-Info "Downloading $BinaryName v${Version}..."
  Invoke-WebRequest -Uri $BundleUrl -OutFile $BundleFile -UseBasicParsing

  $ActualSha256 = (Get-FileHash -Algorithm SHA256 -Path $BundleFile).Hash.ToLowerInvariant()
  $Expected = $ExpectedSha256.ToLowerInvariant()
  if ($ActualSha256 -ne $Expected) {
    Remove-Item -LiteralPath $BundleFile -Force
    throw "Checksum mismatch. Expected $Expected, got $ActualSha256"
  }

  Ensure-Dir $ExtractDir

  Expand-Archive -Path $BundleFile -DestinationPath $ExtractDir -Force

  if (Test-Path -LiteralPath $VersionDir) {
    Remove-Item -LiteralPath $VersionDir -Recurse -Force
  }
  # Use .NET method instead of Move-Item to avoid PowerShell provider issues
  # when the current working directory is on a different drive (e.g. D:\).
  # Fall back to Copy+Remove if source and destination are on different volumes.
  try {
    [System.IO.Directory]::Move($ExtractDir, $VersionDir)
  } catch [System.IO.IOException] {
    Copy-Item -LiteralPath $ExtractDir -Destination $VersionDir -Recurse -Force
    Remove-Item -LiteralPath $ExtractDir -Recurse -Force
  }

  Remove-Item -LiteralPath $BundleFile -Force
}

$InstalledExe = Join-Path $VersionBinDir ("{0}.exe" -f $CompiledBinName)
if (-not (Test-Path -LiteralPath $InstalledExe)) {
  throw "Installed binary not found at $InstalledExe"
}

Copy-Item -LiteralPath $InstalledExe -Destination $EntryExe -Force

# Write per-install distribution marker next to the entrypoint.
# This is read by the CLI at runtime from current_exe()/../distribution.
Set-Content -LiteralPath $MarkerPath -Value $Distribution

# Federal installs have no baked-in manifest URL; record the release base URL
# so a future updater can derive it at runtime (self-update is not wired up
# yet, but the marker keeps the install forward-compatible).
if ($Distribution -eq "irm-iex-federal") {
  Set-Content -LiteralPath (Join-Path $InstallRoot "base-url") -Value $BaseUrl
}

# Portal installs record the portal-provided identity in the install root so
# the binary can brand itself and reach its portal before any login.
if ($Distribution -eq "enterprise-portal") {
  Set-Content -LiteralPath (Join-Path $InstallRoot "display-name") -Value $CliDisplayName
  Set-Content -LiteralPath (Join-Path $InstallRoot "portal-url") -Value $BaseUrl
}

# Windows self-update runs through the standalone devin-updater.exe, which the
# portal serves alongside the version bundles. Best-effort: the CLI works
# without it, so a failed fetch must not abort the install.
if ($Distribution -eq "enterprise-portal") {
  $UpdaterUrl = "$BaseUrl/devin_cli/$Version/devin-updater-$Target.exe"
  try {
    Invoke-WebRequest -Uri $UpdaterUrl -OutFile (Join-Path $EntryBinDir "devin-updater.exe") -UseBasicParsing
  } catch {
    Write-Host "Warning: could not download devin-updater.exe ($_). Auto-update will be unavailable; re-run this installer to retry." -ForegroundColor Yellow
  }
}

Write-Host "`u{2713} Installed $BinaryName v${Version}" -ForegroundColor Green

# Add to PATH (User) if needed, and clean up the old (pre-product-subdir) entry.
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $UserPath) { $UserPath = "" }

$OldEntryBinDir = Join-Path $NamespaceRoot "bin"
if ($OldEntryBinDir -ne $EntryBinDir -and $UserPath -like "*${OldEntryBinDir}*") {
  $NewPath = ($UserPath -split ";" | Where-Object { $_ -ne $OldEntryBinDir }) -join ";"
  [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
  $UserPath = $NewPath
}

if ($UserPath -notlike "*${EntryBinDir}*") {
  if ($UserPath -eq "") {
    $NewPath = $EntryBinDir
  } else {
    $NewPath = "$UserPath;$EntryBinDir"
  }
  [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
}

# Update current session PATH as well.
$env:Path = "$EntryBinDir;$env:Path"

& $EntryExe setup
