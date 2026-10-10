# Capture marketing media without touching real data, real agents or a mouse: run a manifest
# under media/ against a headless, isolated Trek (mock agent only) and collect the PNGs and
# videos it declares. The PowerShell 7 port of script/capture.sh for the Windows build.
#
#   pwsh script/capture.ps1 list                 # the suites under media/
#   pwsh script/capture.ps1 check [suite|all]    # lint manifests without launching anything
#   pwsh script/capture.ps1 all                  # every macOS suite (iOS needs a Mac)
#   pwsh script/capture.ps1 mac/readme           # one suite (media/mac/readme.cmds)
#   pwsh script/capture.ps1 mac/parity
#
# Knobs (env): BUILD=0 skips the cargo build · SIZE=1280x820 window in points · FPS=15 for
# video assembly · OUT=dist/media · KEEP=1 keeps the scratch work dir · SHOT_TIMEOUT=900s per
# batch · TREK_SHOT_UNDER=<png> for the backdrop under glass · SHOT_REPO=<path> for what
# @REPO@ stands for.
#
# Each .cmds line is a shots.rs command (see the doc comment in crates/trek-app/src/shots.rs).
# iOS suites (*.list) run xcrun simctl; they stay macOS-only.
[CmdletBinding()]
param([Parameter(Position = 0)] [string]$Command, [Parameter(Position = 1)] [string]$Target)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$Size = if ($env:SIZE) { $env:SIZE } else { '1280x820' }
if (-not $env:FPS) { $env:FPS = '15' }
$Out = if ($env:OUT) { $env:OUT } else { 'dist/media' }
$Keep = $env:KEEP -eq '1'
$ShotTimeout = if ($env:SHOT_TIMEOUT) { [int]$env:SHOT_TIMEOUT } else { 900 }
$script:ShotWork = $null
$script:ShotDir = $null
$script:ShotProc = $null
$script:LogSubs = $null

$Verbs = @('route', 'send', 'attach', 'project', 'diff', 'pair', 'push', 'new', 'settled',
    'glass', 'tint', 'theme', 'tools', 'range', 'usage-demo', 'pace', 'wait', 'record',
    'editor', 'agent-install', 'toast', 'click', 'rclick', 'hover', 'elements', 'type',
    'key', 'scroll', 'resize', 'approve', 'deny', 'answer', 'rewind', 'shot', 'browser', 'quit')

function Say([string]$Msg) { Write-Host "$(Get-Date -Format 'HH:mm:ss')  $Msg" }
function Die([string]$Msg) { [Console]::Error.WriteLine("capture: $Msg"); exit 1 }

function Test-Cmds([string]$File) {
    $bad = $false
    foreach ($raw in [System.IO.File]::ReadLines($File)) {
        $line = ($raw -replace '#.*$', '').Trim()
        if ($line -eq '') { continue }
        $verb = ($line -split ' ', 2)[0]
        if ($Verbs -notcontains $verb) {
            [Console]::Error.WriteLine("  ${File}: unknown command: $line"); $bad = $true
        } elseif ($verb -eq 'route') {
            $route = $line.Substring(5).Trim()
            if ($route -notmatch '^(draft|no-project|basecamp|notes|appearance|first)$' -and $route -notmatch '^(settings|title|project|thread):') {
                [Console]::Error.WriteLine("  ${File}: odd route: $line"); $bad = $true
            }
        }
    }
    -not $bad
}

function Test-ListFile([string]$File) {
    $bad = $false
    foreach ($raw in [System.IO.File]::ReadLines($File)) {
        $line = ($raw -replace '#.*$', '').Trim()
        if ($line -eq '') { continue }
        if ($line -notmatch '[0-9A-Za-z]' -or $line.StartsWith(' -- ') -or $line.StartsWith('-- ')) {
            [Console]::Error.WriteLine("  ${File}: bad line (want 'name [opts] -- launch args'): $line"); $bad = $true
        }
    }
    -not $bad
}

# `check` lints every manifest; `check <suite>` (`mac/parity`, or just `parity`) lints that one;
# `check all` is `check`.
function Test-Manifests([string]$Suite) {
    $ok = $true
    if ($Suite -and $Suite -ne 'all') {
        $name = $Suite -replace '^(mac|ios)/', ''
        $cmds = "media/mac/$name.cmds"
        $list = "media/ios/$name.list"
        if (Test-Path $cmds) { return (Test-Cmds (Resolve-Path $cmds).Path) }
        if (Test-Path $list) { return (Test-ListFile (Resolve-Path $list).Path) }
        Die "no manifest named $Suite (see 'capture.ps1 list')"
    }
    foreach ($f in Get-ChildItem media/mac/*.cmds) { if (-not (Test-Cmds $f.FullName)) { $ok = $false } }
    foreach ($f in Get-ChildItem media/ios/*.list) { if (-not (Test-ListFile $f.FullName)) { $ok = $false } }
    $ok
}

# Build the app with the shots feature once; returns the binary's path. TREK_BIN names a
# specific exe instead — useful on a shared target dir, where another build may overwrite
# debug\trek.exe between `cargo build` and the launch.
function Build-Trek {
    if ($env:TREK_BIN) {
        if (-not (Test-Path $env:TREK_BIN)) { Die "TREK_BIN not found: $env:TREK_BIN" }
        return $env:TREK_BIN
    }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $bin = Join-Path $target 'debug\trek.exe'
    if ($env:BUILD -eq '0') { return $bin }
    Say 'capture: building trek --features shots'
    & cargo build -p trek-app --features shots -q | Out-Host
    if ($LASTEXITCODE -ne 0) { Die 'cargo build failed' }
    if (-not (Test-Path $bin)) { Die "$bin missing after build" }
    $bin
}

# Write $text as the next cmd batch, atomically (a temp file renamed into place) so the app
# never reads a half-written one.
function Send-Batch([string]$Text) {
    $tmp = Join-Path $script:ShotDir '.cmd.tmp'
    [System.IO.File]::WriteAllText($tmp, $Text)
    [System.IO.File]::Move($tmp, (Join-Path $script:ShotDir 'cmd'), $true)
}

# Seed a throwaway data folder and start Trek headless. Isolation comes from TREK_SHOT_DIR
# itself (the process never touches the credential store or real data, and only the mock
# agent can run); the seeded settings make the mock the default agent so a plain `send`
# can't aim at a vendor CLI that isn't there.
function Launch-Trek([string]$Work, [string]$Bin) {
    $script:ShotWork = $Work
    $script:ShotDir = Join-Path $Work 'shot'
    New-Item -ItemType Directory -Force $script:ShotDir, "$Work\data" | Out-Null
    $settings = @(
        '[general]', 'default_agent = "direct:mock"', '',
        '[onboarding]', 'completed = true', '',
        '[import]', 'claude_code = false', 'codex = false', 'opencode = false', '',
        '[notifications]', 'mode = "off"', 'dock_badge = false', 'menu_bar_icon = false', '',
        '[updates]', 'auto_check = false', 'auto_download = false'
    ) -join "`n"
    [System.IO.File]::WriteAllText("$Work\data\settings.toml", "$settings`n")

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Bin
    $psi.WorkingDirectory = (Get-Location).Path
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($kv in @{
            TREK_DATA_DIR     = "$Work\data"
            TREK_SHOT_DIR     = $script:ShotDir
            TREK_MOCK_AGENT   = '1'
            TREK_BACKGROUND   = '1'
            TREK_FORCE_ACTIVE = '1'
            TREK_WINDOW_SIZE  = $Size
            RUST_LOG          = 'warn,trek=info'
        }.GetEnumerator()) {
        $psi.EnvironmentVariables[$kv.Key] = $kv.Value
    }
    $script:ShotProc = [System.Diagnostics.Process]::Start($psi)

    # stdout and stderr, appended to trek.log the way the zsh driver's `> trek.log 2>&1` merges
    # them. Events rather than a pipe read at the end, so a hung batch still leaves a live log.
    $script:LogSubs = @(
        Register-ObjectEvent -InputObject $script:ShotProc -EventName OutputDataReceived -MessageData "$Work\trek.log" -Action {
            if ($null -ne $EventArgs.Data) { Add-Content -Path $Event.MessageData -Value $EventArgs.Data }
        }
        Register-ObjectEvent -InputObject $script:ShotProc -EventName ErrorDataReceived -MessageData "$Work\trek.log" -Action {
            if ($null -ne $EventArgs.Data) { Add-Content -Path $Event.MessageData -Value $EventArgs.Data }
        }
    )
    $script:ShotProc.BeginOutputReadLine()
    $script:ShotProc.BeginErrorReadLine()

    # Readiness probe: a `wait 100` round trip proves the command loop is up.
    Remove-Item "$script:ShotDir\done" -Force -ErrorAction SilentlyContinue
    Send-Batch "wait 100`n"
    $waited = 0
    while (-not (Test-Path "$script:ShotDir\done")) {
        Start-Sleep -Milliseconds 500; $waited++
        if ($waited -ge 120) { Die "Trek didn't answer the shot dir within 60s — see $Work\trek.log" }
        if ($script:ShotProc.HasExited) { Die "Trek exited during launch — see $Work\trek.log" }
    }
}

# Feed $Batch to the app as one cmd batch (comment and blank lines stripped), wait for `done`,
# and fail loudly on any `err` line.
function Invoke-Batch([string]$Batch) {
    if (-not (Test-Path $Batch)) { Die "no such batch file: $Batch" }
    Remove-Item "$script:ShotDir\done" -Force -ErrorAction SilentlyContinue
    # `@REPO@` in a manifest stands for the repository's root (a real project to open).
    $repo = if ($env:SHOT_REPO) { $env:SHOT_REPO } else { (Get-Location).Path }
    $lines = [System.IO.File]::ReadAllLines($Batch) |
        Where-Object { $_ -notmatch '^\s*(#|$)' } |
        ForEach-Object { $_.Replace('@REPO@', $repo) }
    Send-Batch (($lines -join "`n") + "`n")
    $ticks = $ShotTimeout * 4
    $waited = 0
    while (-not (Test-Path "$script:ShotDir\done")) {
        Start-Sleep -Milliseconds 250; $waited++
        if ($waited -ge $ticks) {
            [Console]::Error.WriteLine("capture: batch timed out after ${ShotTimeout}s (manifest still on disk at $script:ShotDir\cmd, app log at $script:ShotWork\trek.log)")
            return $false
        }
        if ($script:ShotProc.HasExited) {
            # `quit` ends the app right after `done` is written — check once more before failing.
            Start-Sleep -Milliseconds 500
            if (Test-Path "$script:ShotDir\done") { break }
            [Console]::Error.WriteLine("capture: Trek exited mid-batch — see $script:ShotWork\trek.log")
            return $false
        }
    }
    $done = (Get-Content "$script:ShotDir\done" -Raw).Trim()
    if ($done -ne 'ok') {
        [Console]::Error.WriteLine('capture: batch failed:')
        foreach ($l in [System.IO.File]::ReadAllLines("$script:ShotDir\done")) { [Console]::Error.WriteLine("  $l") }
        if (Test-Path "$script:ShotWork\trek.log") {
            Get-Content "$script:ShotWork\trek.log" -Tail 20 | ForEach-Object { [Console]::Error.WriteLine("  log: $_") }
        }
        return $false
    }
    $true
}

# Ask for a clean exit first (manifests can end with `quit` themselves; harmless either way),
# then kill whatever's left.
function Stop-Trek {
    if (-not $script:ShotProc) { return }
    if (-not $script:ShotProc.HasExited) {
        Remove-Item "$script:ShotDir\done" -Force -ErrorAction SilentlyContinue
        Send-Batch "quit`n"
        [void]$script:ShotProc.WaitForExit(10000)
        if (-not $script:ShotProc.HasExited) { try { $script:ShotProc.Kill() } catch {} }
    }
    $script:ShotProc = $null
    foreach ($sub in $script:LogSubs) {
        try { Unregister-Event -SourceIdentifier $sub.SourceIdentifier } catch {}
    }
    $script:LogSubs = $null
}

# Every `shot <name>`/`record <name>` the manifest declared should have produced a file.
function Test-Artifacts([string]$File) {
    $missing = $false
    foreach ($raw in [System.IO.File]::ReadLines($File)) {
        $line = ($raw -replace '#.*$', '')
        if ($line.Trim() -eq '') { continue }
        $parts = $line.Trim() -split '\s+'
        if ($parts[0] -eq 'shot' -and $parts[1] -and -not (Test-Path "$script:ShotDir\$($parts[1]).png")) {
            [Console]::Error.WriteLine("  missing: $($parts[1]).png"); $missing = $true
        }
        if ($parts[0] -eq 'record' -and $parts[1] -and $parts[1] -notin 'wait', 'stop' -and -not (Test-Path "$script:ShotDir\$($parts[1]).ffconcat")) {
            [Console]::Error.WriteLine("  missing: $($parts[1]).ffconcat"); $missing = $true
        }
    }
    -not $missing
}

# Assemble <name>.frames/ + <name>.ffconcat into <name>.mp4 (and .gif for short clips).
function Encode([string]$Concat, [string]$OutDir) {
    $base = [System.IO.Path]::GetFileNameWithoutExtension($Concat)
    & ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i $Concat `
        -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' -c:v libx264 -pix_fmt yuv420p -movflags +faststart `
        "$OutDir\$base.mp4" | Out-Host
    if ($LASTEXITCODE -ne 0) { Die "ffmpeg failed encoding $base.mp4" }
    & ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i $Concat `
        -vf 'fps=12,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse' `
        "$OutDir\$base.gif" | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Remove-Item "$OutDir\$base.gif" -Force -ErrorAction SilentlyContinue
        Say "  no gif for $base (too few frames); the mp4 has them"
    }
    $poster = Join-Path $script:ShotDir "$base.frames\f00000.png"
    if (Test-Path $poster) { Copy-Item $poster "$OutDir\$base-poster.png" }
}

function Invoke-MacSuite([string]$Suite) {
    $file = "media\mac\$Suite.cmds"
    if (-not (Test-Path $file)) { Die "no manifest media\mac\$Suite.cmds" }
    if (-not (Test-Cmds $file)) { Die "fix the manifest first (or run 'capture.ps1 check')" }
    $records = [bool]([System.IO.File]::ReadLines($file) | Where-Object { $_ -match '^record\s' } | Select-Object -First 1)
    $ffmpeg = [bool](Get-Command ffmpeg -ErrorAction SilentlyContinue)
    if ($records -and -not $ffmpeg) {
        Say "ffmpeg isn't installed: frame sequences and .ffconcat lists are collected, but no videos are encoded"
    }
    $work = Join-Path ([System.IO.Path]::GetTempPath()) ('trek-capture-mac-' + [System.IO.Path]::GetRandomFileName().Replace('.', ''))
    New-Item -ItemType Directory -Force $work | Out-Null
    try {
        $bin = Build-Trek
        Say "launching (isolated, mock agent, ${Size}pt)"
        Launch-Trek $work $bin
        Say "running $file"
        if (-not (Invoke-Batch $file)) { Die "batch failed (KEEP=1 keeps $work)" }
        if (-not (Test-Artifacts $file)) { Die "declared artifacts missing (KEEP=1 keeps $work)" }
        Stop-Trek
        $outdir = "$Out\$Suite"
        New-Item -ItemType Directory -Force $outdir | Out-Null
        foreach ($png in Get-ChildItem -Recurse -Filter *.png $script:ShotDir) {
            # Frame sequences are intermediates for the video, not artifacts — unless there's no
            # ffmpeg to encode them with, in which case they're what this run produced.
            if ($png.FullName -match '\.frames[\\/]' -and $ffmpeg) { continue }
            $rel = $png.FullName.Substring($script:ShotDir.Length + 1)
            $dest = Join-Path $outdir $rel
            New-Item -ItemType Directory -Force (Split-Path $dest -Parent) | Out-Null
            Copy-Item $png.FullName $dest
            Say "  $rel"
        }
        foreach ($cc in Get-ChildItem -Filter *.ffconcat $script:ShotDir) {
            if ($ffmpeg) {
                Say "  encoding $($cc.BaseName)"
                Encode $cc.FullName $outdir
            } else {
                Copy-Item $cc.FullName $outdir
                Say "  $($cc.Name) (kept; ffmpeg needed for video)"
            }
        }
        Say "published → $outdir"
    } finally {
        Stop-Trek
        if (-not $Keep) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
    }
}

function Show-Suites {
    Get-ChildItem media/mac/*.cmds | ForEach-Object { "  $($_.BaseName)" }
    Get-ChildItem media/ios/*.list | ForEach-Object { "  $($_.BaseName)" }
}

function Show-Help {
    @'
Capture marketing media without touching real data, real agents or a mouse: run a manifest
under media/ against a headless, isolated Trek (mock agent only) and collect the PNGs and
videos it declares.

  pwsh script/capture.ps1 list                 # the suites under media/
  pwsh script/capture.ps1 check [suite|all]    # lint manifests without launching anything
  pwsh script/capture.ps1 all                  # every macOS suite (iOS needs a Mac)
  pwsh script/capture.ps1 mac/parity           # one suite (media/mac/parity.cmds)

Knobs (env): BUILD=0 skips the cargo build · SIZE=1280x820 window in points · FPS=15 for
video assembly · OUT=dist/media · KEEP=1 keeps the scratch work dir · SHOT_TIMEOUT=900s per
batch · TREK_SHOT_UNDER=<png> for the backdrop under glass · SHOT_REPO=<path> for @REPO@.
'@ | Write-Host
}

switch -Regex ($Command) {
    '^list$' { Show-Suites; break }
    '^check$' { if (Test-Manifests $Target) { Say $(if ($Target -and $Target -ne 'all') { "$Target lints clean" } else { 'all manifests lint clean' }) } else { exit 1 }; break }
    '^all$' {
        foreach ($f in Get-ChildItem media/mac/*.cmds) { Invoke-MacSuite $f.BaseName }
        if (Get-ChildItem media/ios/*.list -ErrorAction SilentlyContinue) { Say 'iOS suites need macOS; skipped' }
        break
    }
    '^mac/(.+)$' { Invoke-MacSuite $Matches[1]; break }
    '^ios/(.+)$' { Die 'iOS capture needs macOS (xcrun simctl)' }
    '^(|help|--help|-h)$' { Show-Help; break }
    default {
        if (Test-Path "media\mac\$Command.cmds") { Invoke-MacSuite $Command }
        elseif (Test-Path "media\ios\$Command.list") { Die 'iOS capture needs macOS (xcrun simctl)' }
        else { Show-Help; exit 1 }
    }
}
