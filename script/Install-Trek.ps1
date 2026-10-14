<#
.SYNOPSIS
    Trek Setup Guide and Installer for Windows.

.DESCRIPTION
    Installs Trek for the current user into their user folder (defaults to
    $env:LOCALAPPDATA\Programs\Trek). Does not require administrator privileges.
    Presents an interactive setup guide and provides three completion options:
      1. Finish and open up Trek
      2. Finish and open up folder
      3. Finish and close installer

.PARAMETER InstallDir
    Target directory for installation (default: $env:LOCALAPPDATA\Programs\Trek).

.PARAMETER Silent
    Perform installation silently without GUI.

.PARAMETER ActionOnFinish
    In silent mode, what to do on finish: OpenTrek, OpenFolder, or Close.
#>

[CmdletBinding()]
param (
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\Trek"),
    [switch]$Silent,
    [ValidateSet("OpenTrek", "OpenFolder", "Close")]
    [string]$ActionOnFinish = "OpenTrek"
)

$ErrorActionPreference = "Stop"

function Find-TrekSources {
    $candidates = @(
        $PSScriptRoot,
        (Get-Location).Path,
        (Join-Path $PSScriptRoot "..\target\release"),
        (Join-Path $PSScriptRoot "..\target\x86_64-pc-windows-msvc\release")
    )
    foreach ($dir in $candidates) {
        if ($dir -and (Test-Path (Join-Path $dir "trek.exe"))) {
            return (Resolve-Path $dir).Path
        }
    }
    return $null
}

function Install-TrekFiles([string]$targetDir) {
    if (-not (Test-Path $targetDir)) {
        New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
    }

    $sourceDir = Find-TrekSources
    if ($sourceDir) {
        Write-Host "Installing from local source: $sourceDir -> $targetDir"
        $filesToCopy = @("trek.exe", "trek-mcp.exe", "trek-update.exe", "Uninstall-Trek.ps1")
        foreach ($f in $filesToCopy) {
            $src = Join-Path $sourceDir $f
            if (Test-Path $src) {
                Copy-Item -Path $src -Destination (Join-Path $targetDir $f) -Force
            }
        }
    } else {
        Write-Host "Downloading latest Trek release for Windows..."
        $zipUrl = "https://github.com/dokyit/Trek/releases/latest/download/Trek-windows-x86_64.zip"
        $tempZip = Join-Path $env:TEMP "Trek-windows-x86_64.zip"
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $zipUrl -OutFile $tempZip -UseBasicParsing

        Write-Host "Extracting Trek to $targetDir..."
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [System.IO.Compression.ZipFile]::ExtractToDirectory($tempZip, $targetDir, $true)
        Remove-Item $tempZip -Force -ErrorAction SilentlyContinue
    }

    $uninstallSrc = Join-Path $PSScriptRoot "Uninstall-Trek.ps1"
    if (Test-Path $uninstallSrc) {
        Copy-Item -Path $uninstallSrc -Destination (Join-Path $targetDir "Uninstall-Trek.ps1") -Force -ErrorAction SilentlyContinue
    }

    # Create Start Menu shortcut
    $startMenuPrograms = [Environment]::GetFolderPath("Programs")
    if (Test-Path $startMenuPrograms) {
        $shortcutPath = Join-Path $startMenuPrograms "Trek.lnk"
        $targetExe = Join-Path $targetDir "trek.exe"
        if (Test-Path $targetExe) {
            $wscript = New-Object -ComObject WScript.Shell
            $shortcut = $wscript.CreateShortcut($shortcutPath)
            $shortcut.TargetPath = $targetExe
            $shortcut.WorkingDirectory = $targetDir
            $shortcut.Description = "Trek AI Harness"
            $shortcut.Save()
        }
    }

    Register-WindowsUninstall -targetDir $targetDir
}

function Register-WindowsUninstall([string]$targetDir) {
    $regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Trek"
    if (-not (Test-Path $regPath)) {
        New-Item -Path $regPath -Force | Out-Null
    }
    $targetExe = Join-Path $targetDir "trek.exe"
    $uninstallerScript = Join-Path $targetDir "Uninstall-Trek.ps1"

    $version = "0.4.3"
    if (Test-Path $targetExe) {
        $fileVersion = (Get-Item $targetExe).VersionInfo.ProductVersion
        if ($fileVersion) { $version = $fileVersion }
    }

    Set-ItemProperty -Path $regPath -Name "DisplayName" -Value "Trek" -Force
    Set-ItemProperty -Path $regPath -Name "DisplayVersion" -Value $version -Force
    Set-ItemProperty -Path $regPath -Name "Publisher" -Value "Trek" -Force
    Set-ItemProperty -Path $regPath -Name "DisplayIcon" -Value "$targetExe,0" -Force
    Set-ItemProperty -Path $regPath -Name "InstallLocation" -Value $targetDir -Force
    Set-ItemProperty -Path $regPath -Name "UninstallString" -Value "powershell.exe -ExecutionPolicy Bypass -File `"$uninstallerScript`"" -Force
    Set-ItemProperty -Path $regPath -Name "QuietUninstallString" -Value "powershell.exe -ExecutionPolicy Bypass -File `"$uninstallerScript`" -Silent" -Force
    Set-ItemProperty -Path $regPath -Name "NoModify" -Value 1 -Type DWord -Force
    Set-ItemProperty -Path $regPath -Name "NoRepair" -Value 1 -Type DWord -Force
}

function Create-DesktopShortcut([string]$targetDir) {
    $desktop = [Environment]::GetFolderPath("Desktop")
    $shortcutPath = Join-Path $desktop "Trek.lnk"
    $targetExe = Join-Path $targetDir "trek.exe"
    if (Test-Path $targetExe) {
        $wscript = New-Object -ComObject WScript.Shell
        $shortcut = $wscript.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $targetExe
        $shortcut.WorkingDirectory = $targetDir
        $shortcut.Description = "Trek AI Harness"
        $shortcut.Save()
    }
}

function Run-FinishChoice([string]$choice, [string]$targetDir) {
    switch ($choice) {
        "OpenTrek" {
            $exe = Join-Path $targetDir "trek.exe"
            if (Test-Path $exe) {
                Start-Process -FilePath $exe -WorkingDirectory $targetDir
            }
        }
        "OpenFolder" {
            Start-Process -FilePath "explorer.exe" -ArgumentList "`"$targetDir`""
        }
        "Close" {
            # Clean exit
        }
    }
}

if ($Silent) {
    Write-Host "Installing Trek to $InstallDir..."
    Install-TrekFiles $InstallDir
    Create-DesktopShortcut $InstallDir
    Write-Host "Installation complete."
    Run-FinishChoice $ActionOnFinish $InstallDir
    exit 0
}

# Interactive Guide using Windows Forms
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = "Trek Setup Guide"
$form.Size = New-Object System.Drawing.Size(560, 440)
$form.StartPosition = "CenterScreen"
$form.FormBorderStyle = "FixedDialog"
$form.MaximizeBox = $false
$form.MinimizeBox = $true
$form.BackColor = [System.Drawing.Color]::FromArgb(250, 250, 252)
$form.Font = New-Object System.Drawing.Font("Segoe UI", 9)

# Header Panel
$header = New-Object System.Windows.Forms.Panel
$header.Dock = "Top"
$header.Height = 70
$header.BackColor = [System.Drawing.Color]::FromArgb(24, 24, 27)

$lblTitle = New-Object System.Windows.Forms.Label
$lblTitle.Text = "Trek Setup Guide"
$lblTitle.Font = New-Object System.Drawing.Font("Segoe UI", 14, [System.Drawing.FontStyle]::Bold)
$lblTitle.ForeColor = [System.Drawing.Color]::White
$lblTitle.Location = New-Object System.Drawing.Point(24, 14)
$lblTitle.AutoSize = $true
$header.Controls.Add($lblTitle)

$lblSubtitle = New-Object System.Windows.Forms.Label
$lblSubtitle.Text = "Install Trek for Windows"
$lblSubtitle.Font = New-Object System.Drawing.Font("Segoe UI", 9)
$lblSubtitle.ForeColor = [System.Drawing.Color]::FromArgb(161, 161, 170)
$lblSubtitle.Location = New-Object System.Drawing.Point(25, 42)
$lblSubtitle.AutoSize = $true
$header.Controls.Add($lblSubtitle)
$form.Controls.Add($header)

# Content Container Panel
$content = New-Object System.Windows.Forms.Panel
$content.Location = New-Object System.Drawing.Point(24, 85)
$content.Size = New-Object System.Drawing.Size(496, 260)
$form.Controls.Add($content)

# Page 1: Destination Selection
$pnlStep1 = New-Object System.Windows.Forms.Panel
$pnlStep1.Dock = "Fill"
$content.Controls.Add($pnlStep1)

$lblInfo = New-Object System.Windows.Forms.Label
$lblInfo.Text = "Trek installs into your user account folder so background updates and agents work smoothly without needing administrator access."
$lblInfo.Location = New-Object System.Drawing.Point(0, 10)
$lblInfo.Size = New-Object System.Drawing.Size(490, 40)
$pnlStep1.Controls.Add($lblInfo)

$lblFolder = New-Object System.Windows.Forms.Label
$lblFolder.Text = "Installation folder:"
$lblFolder.Font = New-Object System.Drawing.Font("Segoe UI", 9, [System.Drawing.FontStyle]::Bold)
$lblFolder.Location = New-Object System.Drawing.Point(0, 60)
$lblFolder.AutoSize = $true
$pnlStep1.Controls.Add($lblFolder)

$txtDir = New-Object System.Windows.Forms.TextBox
$txtDir.Text = $InstallDir
$txtDir.Location = New-Object System.Drawing.Point(3, 85)
$txtDir.Size = New-Object System.Drawing.Size(390, 24)
$pnlStep1.Controls.Add($txtDir)

$btnBrowse = New-Object System.Windows.Forms.Button
$btnBrowse.Text = "Browse..."
$btnBrowse.Location = New-Object System.Drawing.Point(400, 83)
$btnBrowse.Size = New-Object System.Drawing.Size(90, 28)
$btnBrowse.Add_Click({
    $fbd = New-Object System.Windows.Forms.FolderBrowserDialog
    $fbd.SelectedPath = $txtDir.Text
    if ($fbd.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
        $txtDir.Text = $fbd.SelectedPath
    }
})
$pnlStep1.Controls.Add($btnBrowse)

$chkDesktop = New-Object System.Windows.Forms.CheckBox
$chkDesktop.Text = "Create Desktop shortcut"
$chkDesktop.Checked = $true
$chkDesktop.Location = New-Object System.Drawing.Point(3, 130)
$chkDesktop.AutoSize = $true
$pnlStep1.Controls.Add($chkDesktop)

$prgBar = New-Object System.Windows.Forms.ProgressBar
$prgBar.Location = New-Object System.Drawing.Point(3, 175)
$prgBar.Size = New-Object System.Drawing.Size(487, 20)
$prgBar.Style = "Marquee"
$prgBar.Visible = $false
$pnlStep1.Controls.Add($prgBar)

$lblStatus = New-Object System.Windows.Forms.Label
$lblStatus.Location = New-Object System.Drawing.Point(3, 202)
$lblStatus.Size = New-Object System.Drawing.Size(487, 20)
$lblStatus.ForeColor = [System.Drawing.Color]::FromArgb(75, 85, 99)
$pnlStep1.Controls.Add($lblStatus)

# Page 2: Completion with 3 options
$pnlStep2 = New-Object System.Windows.Forms.Panel
$pnlStep2.Dock = "Fill"
$pnlStep2.Visible = $false
$content.Controls.Add($pnlStep2)

$lblDone = New-Object System.Windows.Forms.Label
$lblDone.Text = "Installation Complete!"
$lblDone.Font = New-Object System.Drawing.Font("Segoe UI", 12, [System.Drawing.FontStyle]::Bold)
$lblDone.ForeColor = [System.Drawing.Color]::FromArgb(22, 101, 52)
$lblDone.Location = New-Object System.Drawing.Point(0, 10)
$lblDone.AutoSize = $true
$pnlStep2.Controls.Add($lblDone)

$lblDoneSub = New-Object System.Windows.Forms.Label
$lblDoneSub.Text = "Trek has been installed successfully. Choose what to do next:"
$lblDoneSub.Location = New-Object System.Drawing.Point(0, 38)
$lblDoneSub.AutoSize = $true
$pnlStep2.Controls.Add($lblDoneSub)

$grpOptions = New-Object System.Windows.Forms.GroupBox
$grpOptions.Text = "Finish Options"
$grpOptions.Location = New-Object System.Drawing.Point(0, 65)
$grpOptions.Size = New-Object System.Drawing.Size(490, 160)
$pnlStep2.Controls.Add($grpOptions)

$rb1 = New-Object System.Windows.Forms.RadioButton
$rb1.Text = "Finish and open up Trek"
$rb1.Checked = $true
$rb1.Font = New-Object System.Drawing.Font("Segoe UI", 9.5, [System.Drawing.FontStyle]::Bold)
$rb1.Location = New-Object System.Drawing.Point(20, 30)
$rb1.AutoSize = $true
$grpOptions.Controls.Add($rb1)

$rb2 = New-Object System.Windows.Forms.RadioButton
$rb2.Text = "Finish and open up folder"
$rb2.Location = New-Object System.Drawing.Point(20, 72)
$rb2.AutoSize = $true
$grpOptions.Controls.Add($rb2)

$rb3 = New-Object System.Windows.Forms.RadioButton
$rb3.Text = "Finish and close installer"
$rb3.Location = New-Object System.Drawing.Point(20, 114)
$rb3.AutoSize = $true
$grpOptions.Controls.Add($rb3)

# Bottom Actions
$btnAction = New-Object System.Windows.Forms.Button
$btnAction.Text = "Install Trek"
$btnAction.Size = New-Object System.Drawing.Size(120, 32)
$btnAction.Location = New-Object System.Drawing.Point(400, 355)
$btnAction.BackColor = [System.Drawing.Color]::FromArgb(37, 99, 235)
$btnAction.ForeColor = [System.Drawing.Color]::White
$btnAction.FlatStyle = "Flat"
$btnAction.FlatAppearance.BorderSize = 0
$form.Controls.Add($btnAction)

$btnCancel = New-Object System.Windows.Forms.Button
$btnCancel.Text = "Cancel"
$btnCancel.Size = New-Object System.Drawing.Size(85, 32)
$btnCancel.Location = New-Object System.Drawing.Point(305, 355)
$btnCancel.Add_Click({ $form.Close() })
$form.Controls.Add($btnCancel)

$btnAction.Add_Click({
    if ($btnAction.Text -eq "Install Trek") {
        $btnAction.Enabled = $false
        $btnBrowse.Enabled = $false
        $txtDir.Enabled = $false
        $chkDesktop.Enabled = $false
        $prgBar.Visible = $true
        $lblStatus.Text = "Installing Trek files..."
        $form.Refresh()

        $target = $txtDir.Text.Trim()
        try {
            Install-TrekFiles $target
            if ($chkDesktop.Checked) {
                Create-DesktopShortcut $target
            }

            $pnlStep1.Visible = $false
            $pnlStep2.Visible = $true
            $btnCancel.Visible = $false
            $btnAction.Text = "Finish"
            $btnAction.Enabled = $true
        } catch {
            $lblStatus.ForeColor = [System.Drawing.Color]::Red
            $lblStatus.Text = "Error: $($_.Exception.Message)"
            $btnAction.Enabled = $true
            $btnBrowse.Enabled = $true
            $txtDir.Enabled = $true
            $prgBar.Visible = $false
        }
    } elseif ($btnAction.Text -eq "Finish") {
        $target = $txtDir.Text.Trim()
        if ($rb1.Checked) {
            Run-FinishChoice "OpenTrek" $target
        } elseif ($rb2.Checked) {
            Run-FinishChoice "OpenFolder" $target
        } else {
            Run-FinishChoice "Close" $target
        }
        $form.Close()
    }
})

$form.ShowDialog() | Out-Null
