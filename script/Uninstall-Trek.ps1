<#
.SYNOPSIS
    Trek Uninstaller for Windows.

.DESCRIPTION
    Uninstalls Trek from the user's computer ($env:LOCALAPPDATA\Programs\Trek by default).
    Terminates running Trek processes, deletes desktop and Start Menu shortcuts,
    removes the Add/Remove Programs registry entry, and removes the program directory.

.PARAMETER InstallDir
    Directory where Trek is installed. Defaults to $env:LOCALAPPDATA\Programs\Trek.

.PARAMETER Silent
    Run silently without user prompts or confirmation dialogs.

.PARAMETER Force
    Force termination of processes and deletion without prompting.
#>

[CmdletBinding()]
param (
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\Trek"),
    [switch]$Silent,
    [switch]$Force
)

$ErrorActionPreference = "Continue"

# 1. User confirmation if interactive
if (-not $Silent -and -not $Force) {
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction SilentlyContinue
        $msg = "Are you sure you want to uninstall Trek from your computer?"
        $title = "Uninstall Trek"
        $response = [System.Windows.Forms.MessageBox]::Show($msg, $title, [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
        if ($response -ne [System.Windows.Forms.DialogResult]::Yes) {
            Write-Host "Uninstallation canceled by user."
            exit 0
        }
    } catch {
        # Fallback to console prompt
        $confirm = Read-Host "Are you sure you want to uninstall Trek? (y/N)"
        if ($confirm -notmatch "^[yY]") {
            Write-Host "Uninstallation canceled."
            exit 0
        }
    }
}

Write-Host "Uninstalling Trek..."

# 2. Terminate running Trek processes
$processes = @("trek", "trek-mcp", "trek-update")
foreach ($p in $processes) {
    Get-Process -Name $p -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# 3. Remove Start Menu shortcut
$startMenuPrograms = [Environment]::GetFolderPath("Programs")
if ($startMenuPrograms) {
    $shortcut = Join-Path $startMenuPrograms "Trek.lnk"
    if (Test-Path $shortcut) {
        Remove-Item $shortcut -Force -ErrorAction SilentlyContinue
    }
    $folderShortcut = Join-Path $startMenuPrograms "Trek\Trek.lnk"
    if (Test-Path $folderShortcut) {
        Remove-Item $folderShortcut -Force -ErrorAction SilentlyContinue
    }
    $folder = Join-Path $startMenuPrograms "Trek"
    if (Test-Path $folder) {
        Remove-Item $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# 4. Remove Desktop shortcut
$desktop = [Environment]::GetFolderPath("Desktop")
if ($desktop) {
    $desktopShortcut = Join-Path $desktop "Trek.lnk"
    if (Test-Path $desktopShortcut) {
        Remove-Item $desktopShortcut -Force -ErrorAction SilentlyContinue
    }
}

# 5. Remove Uninstall registry entries
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Trek"
if (Test-Path $uninstallKey) {
    Remove-Item $uninstallKey -Recurse -Force -ErrorAction SilentlyContinue
}

# 6. Delete installation directory
if (Test-Path $InstallDir) {
    Get-ChildItem -Path $InstallDir -Exclude "Uninstall-Trek.ps1" -Recurse -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

    $cmd = "timeout /t 1 /nobreak >nul & rmdir /s /q `"$InstallDir`""
    Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$cmd`"" -WindowStyle Hidden
}

Write-Host "Trek has been uninstalled."

if (-not $Silent) {
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction SilentlyContinue
        [System.Windows.Forms.MessageBox]::Show("Trek has been successfully uninstalled from your computer.", "Uninstall Trek", [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Information) | Out-Null
    } catch {
        # Console output already printed
    }
}
