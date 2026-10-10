#Requires -Version 7.0
# Drive, debug and verify Trek on Windows in a disposable mock-only profile — the
# PowerShell 7 twin of the Python `trek-dev` driver next to this script, for the
# verify-trek skill. Same runtime layout, seeded settings, cmd/done protocol and JSON.
#
#   pwsh .agents/skills/verify-trek/scripts/trek-dev.ps1 preflight
#   ... start [--no-build] [--size 1280x820]   status   stop   reset [--dry-run|--yes]
#   ... open settings:appearance   project .   send "Show a short streamed answer"
#   ... control theme paper        screenshot appearance [--output x.png]
#   ... logs [--lines 120] [--grep error]      check [--quick] [--dry-run]
#
# JSON is the default output; add --human anywhere for readable text. Throwaway state
# lives in $env:TEMP\trek-verify-<user>-<hash of this checkout> (TREK_VERIFY_HOME
# overrides the base), never %APPDATA%\trek or the credential store.

$ErrorActionPreference = 'Stop'

$Settings = @(
    '[general]', 'default_agent = "direct:mock"', '',
    '[onboarding]', 'completed = true', '',
    '[import]', 'claude_code = false', 'codex = false', 'opencode = false', '',
    '[notifications]', 'mode = "off"', 'dock_badge = false', 'menu_bar_icon = false', '',
    '[updates]', 'auto_check = false', 'auto_download = false'
) -join "`n"
$Tools = @('git', 'explorer', 'terminal', 'browser', 'sidechat', 'simulator')
$BasicRoutes = @('draft', 'no-project', 'basecamp', 'notes', 'appearance', 'first')
$ControlVerbs = @('range', 'settled', 'glass', 'tint', 'theme', 'pace', 'wait', 'new', 'diff', 'tools', 'editor', 'ide')
$ValueFlags = @('--size', '--timeout', '--output', '--lines', '--grep')

$script:Human = $args -contains '--human'
$Argv = @($args | Where-Object { $_ -notin '--human', '--json' })
$script:Cmd = if ($Argv.Count) { $Argv[0] } else { '' }
$Rest = @($Argv | Select-Object -Skip 1)

function Die([string]$Msg, [string]$Hint, $Details) {
    $out = [ordered]@{ ok = $false; command = $script:Cmd; error = $Msg }
    if ($Hint) { $out.hint = $Hint }
    if ($null -ne $Details) { $out.details = $Details }
    if ($script:Human) {
        [Console]::Error.WriteLine("ERROR: $Msg")
        if ($Hint) { [Console]::Error.WriteLine("Next: $Hint") }
        if ($null -ne $Details) { [Console]::Error.WriteLine(($Details | ConvertTo-Json -Depth 8)) }
    } else {
        [Console]::Error.WriteLine(($out | ConvertTo-Json -Depth 8))
    }
    exit 1
}
function Out-Result($Data) {
    Write-Output (([ordered]@{ ok = $true; command = $script:Cmd } + $Data) | ConvertTo-Json -Depth 8)
    exit 0
}
function Tail([string]$Path, [int]$Lines) {
    if (-not (Test-Path $Path)) { return '' }
    (Get-Content $Path -Tail $Lines -ErrorAction SilentlyContinue) -join "`n"
}
# The value after a --flag in $Rest; $null when absent.
function Flag-Value([string]$Name) {
    $i = [array]::IndexOf($Rest, $Name)
    if ($i -ge 0 -and $i + 1 -lt $Rest.Count) { return $Rest[$i + 1] }
    $null
}
# $Rest without the named flags (boolean flags drop their name, valued flags name + value).
# -NoEnumerate: a single positional must still come back an array, not a bare string.
function Rest-Positionals {
    $out = @()
    for ($i = 0; $i -lt $Rest.Count; $i++) {
        $a = "$($Rest[$i])"
        if ($a -in $ValueFlags) { $i++ } elseif (-not $a.StartsWith('--')) { $out += $a }
    }
    Write-Output -NoEnumerate $out
}
function Dry([string[]]$Would) { Out-Result @{ dry_run = $true; would = $Would } }

# The checkout this run belongs to, and its runtime dir (per user, per checkout).
$top = (git rev-parse --show-toplevel 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $top) { Die 'This command is not running inside a Git checkout.' 'cd to the Trek checkout or worktree, then run the CLI again' }
$Root = (Resolve-Path $top.Trim()).Path
$hash = ([System.Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($Root)) | ForEach-Object { $_.ToString('x2') }) -join ''
$base = if ($env:TREK_VERIFY_HOME) { $env:TREK_VERIFY_HOME } else { [System.IO.Path]::GetTempPath() }
$RunDir = Join-Path $base ("trek-verify-{0}-{1}" -f $env:USERNAME, $hash.Substring(0, 12))
$ShotDir = Join-Path $RunDir 'shot'
$DataDir = Join-Path $RunDir 'data'
$LogPath = Join-Path $RunDir 'trek.log'
$StatePath = Join-Path $RunDir 'state.json'
$LockPath = "$RunDir.lock"
$PendingPath = Join-Path $RunDir 'pending.json'
$BinPath = Join-Path $RunDir 'bin\trek.exe'
$ProvPath = Join-Path $RunDir 'bin\.trek-verify-shots.json'
# Respect a shared CARGO_TARGET_DIR (workers on this laptop use D:\trek-target), else the
# checkout's own target — the artifact must always come from one of these.
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }

# An exclusive lock on a file beside the runtime dir (outside it, so it survives reset).
# FileShare.None is flock(LOCK_EX): held until this process lets go or exits. Unlike flock,
# Open throws while the lock is held — retry so a concurrent command waits instead of dying.
function With-Lock([scriptblock]$Body) {
    New-Item -ItemType Directory -Force (Split-Path $LockPath -Parent) | Out-Null
    $deadline = (Get-Date).AddMinutes(10)
    $stream = $null
    while (-not $stream) {
        try { $stream = [System.IO.File]::Open($LockPath, 'OpenOrCreate', 'ReadWrite', 'None') }
        catch [System.IO.IOException] {
            if ((Get-Date) -gt $deadline) { Die 'The verify-trek lock stayed held for ten minutes.' "another trek-dev.ps1 may be stuck; delete $LockPath only once nothing holds it" }
            Start-Sleep -Milliseconds 100
        }
    }
    try { & $Body } finally { $stream.Dispose() }
}

function Read-State {
    try { $s = Get-Content $StatePath -Raw | ConvertFrom-Json } catch { return $null }
    if ($s.root -ne $Root -or $s.isolated -ne $true) { return $null }
    $s
}
function Get-Signature([int]$ProcId) {
    $p = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcId" -ErrorAction SilentlyContinue
    if (-not $p) { return '' }
    "$($p.CreationDate.ToString('o'))|$($p.CommandLine)"
}
function Test-Alive($State) {
    $sig = Get-Signature ([int]$State.pid)
    return $sig -and $sig -eq $State.process_signature
}
function Need-Running {
    $s = Read-State
    if (-not $s -or -not (Test-Alive $s)) { Die 'The isolated Trek app is not running.' 'run trek-dev.ps1 start' }
    $s
}
function Get-Status {
    $s = Read-State
    $running = $s -and (Test-Alive $s)
    @{ running = [bool]$running; pid = $(if ($running) { $s.pid } else { $null }); isolated = [bool]($running -and $s.isolated)
       root = $Root; runtime_dir = $RunDir; data_dir = $DataDir; log = $LogPath; started_at = $(if ($running) { $s.started_at } else { $null }) }
}

# The app half of a batch: a `done` file's contents, or the reason none came.
function Write-Cmd([string[]]$Lines) {
    $done = Join-Path $ShotDir 'done'
    Remove-Item $done -Force -ErrorAction SilentlyContinue
    @{ pid = $script:StatePid; commands = $Lines; submitted_at = (Get-Date).ToUniversalTime().ToString('o') } | ConvertTo-Json | Set-Content $PendingPath
    $tmp = Join-Path $ShotDir ".cmd.$PID.tmp"
    [System.IO.File]::WriteAllText($tmp, ($Lines -join "`n") + "`n")
    [System.IO.File]::Move($tmp, (Join-Path $ShotDir 'cmd'), $true)
}
function Wait-Done($State, [double]$Timeout) {
    $done = Join-Path $ShotDir 'done'
    $deadline = (Get-Date).AddSeconds($Timeout)
    while ($true) {
        if (Test-Path $done) {
            $result = (Get-Content $done -Raw).Trim()
            Remove-Item $done, $PendingPath -Force -ErrorAction SilentlyContinue
            return $result
        }
        if (-not (Test-Alive $State)) { return '__exited__' }
        if ((Get-Date) -gt $deadline) {
            # Close the boundary race: take a completion written on the last tick; otherwise
            # keep pending.json so no later batch mistakes this one's `done` for its own.
            if (Test-Path $done) {
                $result = (Get-Content $done -Raw).Trim()
                Remove-Item $done, $PendingPath -Force -ErrorAction SilentlyContinue
                return $result
            }
            return '__timeout__'
        }
        Start-Sleep -Milliseconds 100
    }
}
# Public batch: guard against a pending command, send, and die loudly on a bad outcome.
function Send-Batch([string[]]$Lines, [double]$Timeout = 90) {
    $state = Need-Running
    if ((Test-Path $PendingPath) -or (Test-Path (Join-Path $ShotDir 'cmd'))) {
        Die 'A previous verification command has no confirmed completion.' 'run stop, then start, before sending another command'
    }
    $script:StatePid = $state.pid
    Write-Cmd $Lines
    $result = Wait-Done $state $Timeout
    if ($result -eq 'ok') { return }
    if ($result -eq '__exited__') { Die 'Trek exited while handling a verification command.' 'run stop, then start; inspect logs for the cause' (Tail $LogPath 40) }
    if ($result -eq '__timeout__') { Die "Trek did not finish the command within ${Timeout}s." 'run stop, then start; commands remain blocked until the process is recovered' }
    Die 'Trek rejected a verification command.' $null $result
}
# Internal batch for start/stop/check: the caller decides what a failure means.
function Send-Soft($State, [string[]]$Lines, [double]$Timeout) {
    $script:StatePid = $State.pid
    Write-Cmd $Lines
    Wait-Done $State $Timeout
}

function Build-Shots {
    $started = Get-Date
    $env:CARGO_TARGET_DIR = $TargetDir
    Push-Location $Root
    try { $out = & cargo build --locked -p trek-app --features shots --message-format json-render-diagnostics 2>&1 | Out-String }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { Die 'The Trek shots build failed.' 'inspect the captured build output' (($out -split "`n" | Select-Object -Last 80) -join "`n") }
    $exe = $null
    foreach ($line in $out -split "`n") {
        try { $m = $line | ConvertFrom-Json } catch { continue }
        if ($m.reason -eq 'compiler-artifact' -and $m.target.name -eq 'trek' -and $m.target.kind -contains 'bin' -and $m.executable) { $exe = $m.executable }
    }
    if (-not $exe) { Die 'Cargo reported success but did not report the Trek executable artifact.' }
    $exe = (Resolve-Path $exe).Path
    $targetRoot = (Resolve-Path $TargetDir).Path.TrimEnd('\') + '\'
    if (-not $exe.StartsWith($targetRoot, [System.StringComparison]::OrdinalIgnoreCase)) { Die 'Cargo reported a Trek executable outside the controlled target directory.' $null $exe }
    # Copy into the runtime dir: on a shared CARGO_TARGET_DIR another worktree's build can
    # overwrite debug\trek.exe between the build and the launch (same reason as TREK_BIN).
    New-Item -ItemType Directory -Force (Split-Path $BinPath -Parent) | Out-Null
    Copy-Item $exe $BinPath -Force
    $sha = (Get-FileHash $BinPath -Algorithm SHA256).Hash.ToLower()
    @{ root = $Root; binary = $BinPath; sha256 = $sha; feature = 'shots'; built_at = (Get-Date).ToUniversalTime().ToString('o') } |
        ConvertTo-Json | Set-Content $ProvPath
    @{ binary = $BinPath; seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 2) }
}
function Get-VerifiedBinary {
    try { $p = Get-Content $ProvPath -Raw | ConvertFrom-Json } catch { $p = $null }
    if (-not $p -or $p.root -ne $Root -or -not $p.sha256 -or -not $p.binary -or $p.feature -ne 'shots') {
        Die 'The existing Trek binary has no verified shots-build provenance.' 'rerun without --no-build before launching isolated verification'
    }
    if (-not (Test-Path $p.binary)) { Die 'The recorded shots executable is missing.' 'rerun without --no-build' $p.binary }
    if ((Get-FileHash $p.binary -Algorithm SHA256).Hash.ToLower() -ne $p.sha256) {
        Die 'The Trek executable changed after the last verified shots build.' 'rerun without --no-build; an ordinary build may have replaced the binary'
    }
    $p.binary
}
function Seed {
    New-Item -ItemType Directory -Force $ShotDir, $DataDir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $DataDir 'settings.toml'), "$Settings`n")
    Set-Content (Join-Path $RunDir 'ISOLATED_MOCK_ONLY') "Created by verify-trek. TREK_SHOT_DIR isolates data and only permits the mock agent.`n"
}
function End-Trek($State) {
    & taskkill /PID $State.pid /T /F 2>$null | Out-Null
    $deadline = (Get-Date).AddSeconds(10)
    while ((Test-Alive $State) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (Test-Alive $State) { Die 'Could not terminate the isolated Trek process; state was preserved.' $null @{ pid = $State.pid; runtime_dir = $RunDir } }
}
function Start-Trek([bool]$Build, [string]$Size) {
    $current = Get-Status
    if ($current.running) { return $current + @{ already_running = $true } }
    $buildResult = if ($Build) { Build-Shots } else { $null }
    $binary = Get-VerifiedBinary
    Seed
    Remove-Item (Join-Path $ShotDir 'cmd'), (Join-Path $ShotDir 'done'), $PendingPath -Force -ErrorAction SilentlyContinue
    Add-Content $LogPath "`n=== verify-trek start $((Get-Date).ToUniversalTime().ToString('o')) ==="
    # cmd's `>> log 2>&1` merges both streams into one file the way the Python driver's
    # `stdout=log, stderr=STDOUT` does — OS-level redirection, so the log keeps filling
    # for the app's whole life, not just while this script runs. taskkill /T ends both.
    # Arguments goes on cmd's command line verbatim; Start-Process would re-quote it.
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'cmd.exe'
    $psi.Arguments = "/d /c `"$binary`" >> `"$LogPath`" 2>&1"
    $psi.WorkingDirectory = $Root
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    foreach ($kv in @{ TREK_DATA_DIR = $DataDir; TREK_SHOT_DIR = $ShotDir; TREK_MOCK_AGENT = '1'; TREK_BACKGROUND = '1'; TREK_FORCE_ACTIVE = '1'; TREK_WINDOW_SIZE = $Size; RUST_LOG = 'warn,trek=info' }.GetEnumerator()) {
        $psi.EnvironmentVariables[$kv.Key] = $kv.Value
    }
    $proc = [System.Diagnostics.Process]::Start($psi)
    Start-Sleep -Milliseconds 300
    $state = [ordered]@{ root = $Root; pid = $proc.Id; binary = $binary; isolated = $true; mock_only = $true
                        started_at = (Get-Date).ToUniversalTime().ToString('o'); size = $Size }
    $state.process_signature = Get-Signature $proc.Id
    if (-not $state.process_signature) { End-Trek $state; Die 'Trek launched, but its process identity could not be verified.' }
    $state | ConvertTo-Json | Set-Content $StatePath
    if ((Send-Soft $state @('wait 100') 60) -ne 'ok') {
        End-Trek $state
        Die 'Trek launched but its verification command loop did not become ready.' "run the logs command; the log is $LogPath" (Tail $LogPath 40)
    }
    (Get-Status) + @{ already_running = $false; build = $buildResult; safety = 'throwaway data folder, imports off, notifications off, mock agent only' }
}
function Stop-Trek {
    $state = Read-State
    if (-not $state -or -not (Test-Alive $state)) { return @{ stopped = $true; was_running = $false; runtime_dir = $RunDir } }
    if (-not (Test-Path $PendingPath)) { try { Send-Soft $state @('quit') 15 | Out-Null } catch {} }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Test-Alive $state) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (Test-Alive $state) { End-Trek $state }
    $state.stopped_at = (Get-Date).ToUniversalTime().ToString('o')
    $state.pid = $null
    $state | ConvertTo-Json | Set-Content $StatePath
    Remove-Item $PendingPath, (Join-Path $ShotDir 'cmd'), (Join-Path $ShotDir 'done') -Force -ErrorAction SilentlyContinue
    @{ stopped = $true; was_running = $true; runtime_dir = $RunDir }
}
function Route-Line([string]$Route) {
    if ($Route -match '[\r\n]') { Die 'Routes cannot contain line breaks.' }
    if ($Route.StartsWith('tool:')) {
        $tool = $Route.Substring(5)
        if ($Tools -notcontains $tool) { Die "Unknown tool '$tool'." "choose one of: $($Tools -join ', ')" }
        return "tools $tool"
    }
    if ($BasicRoutes -contains $Route -or $Route -match '^(settings|thread|title|project):') { return "route $Route" }
    Die "Unknown route '$Route'." 'use draft, no-project, basecamp, notes, first, settings:<page>, tool:<name>, title:<query>, thread:<id>, or project:<path>'
}
function Invoke-Check([bool]$Quick) {
    $steps = [System.Collections.Generic.List[object]]::new()
    With-Lock {
        foreach ($f in Get-ChildItem "$Root\script\*.ps1", "$Root\script\lib\*.ps1" -ErrorAction SilentlyContinue) {
            $n = "ps1 parses: $($f.Name)"
            try { [void][scriptblock]::Create([System.IO.File]::ReadAllText($f.FullName)); $steps.Add(@{ name = $n; passed = $true }) }
            catch { $steps.Add(@{ name = $n; passed = $false; output = "$_" }); Die "Check failed at: $n." $null @{ steps = $steps } }
        }
        if (-not $Quick) {
            $env:CARGO_TARGET_DIR = $TargetDir
            foreach ($step in @(
                @('workspace tests', 'cargo test --locked -p trek-core -p trek-agents -p trek-app -p trek-mcp'),
                @('clippy', 'cargo clippy --workspace --all-targets --locked -- -A clippy::all -D clippy::correctness -D clippy::suspicious'))) {
                $t = Get-Date
                Push-Location $Root
                try { & ([scriptblock]::Create($step[1])) | Out-Host; $code = $LASTEXITCODE } finally { Pop-Location }
                if ($code -ne 0) { $steps.Add(@{ name = $step[0]; passed = $false }); Die "Check failed at: $($step[0])." $null @{ steps = $steps } }
                $steps.Add(@{ name = $step[0]; passed = $true; seconds = [math]::Round(((Get-Date) - $t).TotalSeconds, 2) })
            }
        }
        $wasRunning = (Get-Status).running
        $b = Build-Shots
        $steps.Add(@{ name = 'shots build'; passed = $true; seconds = $b.seconds; binary = $b.binary })
        # Smoke exactly the executable just built, preserving the caller's running/stopped
        # state but never validating an older process that happened to be open.
        if ($wasRunning) { Stop-Trek | Out-Null }
        Start-Trek $false '1280x820' | Out-Null
        $shots = @()
        try {
            $state = Read-State
            foreach ($route in 'basecamp', 'settings:general') {
                if ((Send-Soft $state @((Route-Line $route), 'wait 250') 30) -ne 'ok') { Die "check smoke failed on route $route" $null (Tail $LogPath 40) }
                $name = "check-$($route -replace ':', '-')"
                if ((Send-Soft $state @("shot $name") 30) -ne 'ok') { Die "check smoke failed on shot $name" $null (Tail $LogPath 40) }
                $art = Join-Path $RunDir 'artifacts'
                New-Item -ItemType Directory -Force $art | Out-Null
                Copy-Item (Join-Path $ShotDir "$name.png") (Join-Path $art "$name.png") -Force
                $shots += (Join-Path $art "$name.png")
            }
            $steps.Add(@{ name = 'isolated UI smoke'; passed = $true; routes = @('basecamp', 'settings:general'); screenshots = $shots })
        } finally { if (-not $wasRunning) { Stop-Trek | Out-Null } }
    }
    Out-Result @{ passed = $true; quick = $Quick; steps = $steps; screenshots = $shots }
}

switch -Regex ($script:Cmd) {
    '^preflight$' {
        $rust = (& rustc --version 2>&1 | Out-String).Trim(); $rustOk = $LASTEXITCODE -eq 0
        $cargo = (& cargo --version 2>&1 | Out-String).Trim(); $cargoOk = $LASTEXITCODE -eq 0
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        Out-Result @{ windows = $true; architecture = $arch; rust = @{ ok = $rustOk; detail = $rust }; cargo = @{ ok = $cargoOk; detail = $cargo }; ready = ($rustOk -and $cargoOk) }
    }
    '^status$' { Out-Result (Get-Status) }
    '^start$' {
        $size = (Flag-Value '--size'); if (-not $size) { $size = '1280x820' }
        if ($size -notmatch '^\d{3,5}x\d{3,5}$') { Die '--size must look like 1280x820.' }
        if ($Rest -contains '--dry-run') { Dry @('build Trek with the shots feature', "launch isolated mock-only app at $RunDir") }
        With-Lock { Out-Result (Start-Trek ($Rest -notcontains '--no-build') $size) }
    }
    '^open$' {
        $pos = Rest-Positionals
        if (-not $pos) { Die 'Missing a route.' 'e.g. basecamp, settings:appearance, tool:git, title:query' }
        $line = Route-Line $pos[0]
        if ($Rest -contains '--dry-run') { Dry @("send '$line' to isolated Trek") }
        With-Lock { Send-Batch @($line) 90 }
        Out-Result @{ opened = $pos[0]; command_sent = $line }
    }
    '^project$' {
        $pos = Rest-Positionals
        if (-not $pos) { Die 'Missing a project folder.' }
        $p = $pos[0]
        if ($p -match '[\r\n]') { Die 'Project paths cannot contain line breaks.' }
        if (-not [IO.Path]::IsPathRooted($p)) { $p = Join-Path $Root $p }
        if (-not (Test-Path $p -PathType Container)) { Die "Project folder does not exist: $p" }
        $p = (Resolve-Path $p).Path
        if ($Rest -contains '--dry-run') { Dry @("add $p only to the throwaway Trek profile", 'open a draft in that project') }
        With-Lock { Send-Batch @("project $p") 90 }
        Out-Result @{ project = $p; opened = $true }
    }
    '^send$' {
        $pos = Rest-Positionals
        if (-not $pos) { Die 'Missing a prompt.' }
        $prompt = $pos[0]
        if ($prompt -match '[\r\n]') { Die 'The shots command protocol accepts one-line prompts only.' 'replace line breaks with spaces and retry' }
        $timeout = 90; if ($v = Flag-Value '--timeout') { $timeout = [double]$v }
        $wait = $Rest -notcontains '--no-wait'
        $lines = @("send $prompt")
        if ($wait) { $lines += "wait idle $([int]($timeout * 1000))" }
        if ($Rest -contains '--dry-run') { Dry @('send prompt to scripted mock agent', $(if ($wait) { 'wait for the mock turn to finish' } else { 'return without waiting' })) }
        With-Lock { Send-Batch $lines ($timeout + 10) }
        Out-Result @{ sent = $true; mock_only = $true; waited = $wait }
    }
    '^control$' {
        $pos = Rest-Positionals
        if (-not $pos) { Die 'Missing a control verb.' "choose one of: $($ControlVerbs -join ', ')" }
        if ($ControlVerbs -notcontains $pos[0]) { Die "Unknown control verb '$($pos[0])'." "choose one of: $($ControlVerbs -join ', ')" }
        $line = ($pos -join ' ').Trim()
        if ($line -match '[\r\n]') { Die 'Control commands cannot contain line breaks.' }
        $timeout = 90; if ($v = Flag-Value '--timeout') { $timeout = [double]$v }
        if ($Rest -contains '--dry-run') { Dry @("send '$line' to isolated Trek") }
        With-Lock { Send-Batch @($line) $timeout }
        Out-Result @{ command_sent = $line }
    }
    '^screenshot$' {
        $pos = Rest-Positionals
        if (-not $pos) { Die 'Missing a screenshot name.' }
        $name = ([IO.Path]::GetFileNameWithoutExtension($pos[0]) -replace '[^A-Za-z0-9._-]+', '-').Trim('-.')
        if (-not $name) { Die 'The screenshot name must contain a letter or number.' }
        $output = (Flag-Value '--output'); if (-not $output) { $output = Join-Path $RunDir "artifacts\$name.png" }
        if (-not [IO.Path]::IsPathRooted($output)) { $output = Join-Path $Root $output }
        if ($Rest -contains '--dry-run') { Dry @("render the isolated Trek window as $name.png", "write $output") }
        With-Lock {
            Send-Batch @("shot $name") 45
            $src = Join-Path $ShotDir "$name.png"
            if (-not (Test-Path $src)) { Die 'Trek reported a successful capture but the PNG is missing.' $null $src }
            New-Item -ItemType Directory -Force (Split-Path $output -Parent) | Out-Null
            Copy-Item $src $output -Force
        }
        Out-Result @{ captured = $true; path = $output; bytes = (Get-Item $output).Length }
    }
    '^logs$' {
        $n = 80; if ($v = Flag-Value '--lines') { $n = [int]$v }
        if ($n -lt 1) { Die '--lines must be at least 1.' }
        $lines = @(Get-Content $LogPath -Tail $n -ErrorAction SilentlyContinue)
        if ($v = Flag-Value '--grep') { $lines = @($lines | Where-Object { $_ -match [regex]::Escape($v) }) }
        Out-Result @{ path = $LogPath; lines = $lines; running = (Get-Status).running }
    }
    '^check$' {
        if ($Rest -contains '--dry-run') {
            Dry @('parse script/*.ps1 and script/lib/*.ps1', 'run cargo tests and clippy', 'build trek-app with shots', 'launch isolated mock-only Trek', 'open and capture Basecamp and General settings')
        }
        Invoke-Check ($Rest -contains '--quick')
    }
    '^stop$' { With-Lock { Out-Result (Stop-Trek) } }
    '^reset$' {
        if ($Rest -contains '--dry-run') { Dry @('stop only the isolated Trek process', "delete $RunDir") }
        if ($Rest -notcontains '--yes') { Die 'Reset needs explicit confirmation.' 'inspect with reset --dry-run, then run reset --yes' }
        With-Lock {
            $s = Stop-Trek
            if (Test-Path $RunDir) { Remove-Item -Recurse -Force $RunDir }
            Out-Result @{ reset = $true; removed = $RunDir; stopped = $s.was_running }
        }
    }
    '^(|help|--help|-h)$' { Get-Content $PSCommandPath -TotalCount 14 | Write-Host; exit 0 }
    default { Die "Unknown command '$script:Cmd'." 'try preflight, start, status, open, project, send, control, screenshot, logs, check, stop or reset' }
}
