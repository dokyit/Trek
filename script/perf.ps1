# Trek's performance measurements and stress run, against a headless, isolated Trek (mock agent
# only, a throwaway data folder): nothing real is read, touched or typed into. Each run launches
# a release build with TREK_PERF=1, drives it through the shots harness (crates/trek-app/src/shots.rs)
# and reads back the `perf:` lines it logs (crates/trek-app/src/perf.rs). See docs/PERF.md.
#
#   pwsh script/perf.ps1                       # every measurement, three runs each, medians
#   pwsh script/perf.ps1 startup basecamp      # some of them: startup basecamp scroll type inbox stream
#   pwsh script/perf.ps1 stress                # the long run: 30 minutes, memory and handles
#
# Knobs (env): TREK_BIN=<trek.exe> (a release build with --features shots; copy it out of a
# shared target dir first) · BUILD=0 skips building · RUNS=3 · SIZE=1280x820 · OUT=dist/perf ·
# STRESS_MIN=30 · KEEP=1 keeps the scratch folders · SEED_THREADS=200 · SEED_LONG=400 ·
# SEED_BIN=<perf_seed.exe> (a built `cargo build --release -p trek-core --example perf_seed`).
[CmdletBinding()]
param([Parameter(ValueFromRemainingArguments)] [string[]]$What)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$Runs = if ($env:RUNS) { [int]$env:RUNS } else { 3 }
$Size = if ($env:SIZE) { $env:SIZE } else { '1280x820' }
$Out = if ($env:OUT) { $env:OUT } else { 'dist/perf' }
$StressMin = if ($env:STRESS_MIN) { [int]$env:STRESS_MIN } else { 30 }
$Keep = $env:KEEP -eq '1'
$All = @('startup', 'basecamp', 'scroll', 'type', 'inbox', 'stream')
if (-not $What) { $What = $All }
foreach ($w in $What) { if ($w -notin $All + 'stress') { throw "unknown measurement '$w' (one of: $($All -join ' ') stress)" } }

$Scratch = Join-Path ([IO.Path]::GetTempPath()) ('trek-perf-' + [IO.Path]::GetRandomFileName().Replace('.', ''))
New-Item -ItemType Directory -Force $Scratch, $Out | Out-Null
$Results = [Collections.Generic.List[object]]::new()

function Say([string]$Msg) { Write-Host "$(Get-Date -Format 'HH:mm:ss')  $Msg" }

function Get-Trek {
    if ($env:TREK_BIN) { return (Resolve-Path $env:TREK_BIN).Path }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    if ($env:BUILD -ne '0') {
        Say 'building trek (release, --features shots)'
        & cargo build --release -p trek-app --features shots --locked -q | Out-Host
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
    # Copied out: another build on a shared target dir may overwrite it mid-run.
    $bin = Join-Path $Scratch 'trek.exe'
    Copy-Item (Join-Path $target 'release\trek.exe') $bin
    $bin
}

# The 200-thread data folder, made once through trek-core's own API (examples/perf_seed.rs);
# each run gets a copy.
function New-Seed {
    $seed = Join-Path $Scratch 'seed'
    if (Test-Path $seed) { return $seed }
    Say 'seeding the 200-thread data folder'
    $env:TREK_DATA_DIR = $seed
    try {
        if ($env:SEED_BIN) { & $env:SEED_BIN | Out-Host }
        else { & cargo run --release -p trek-core --example perf_seed --locked -q | Out-Host }
        if ($LASTEXITCODE -ne 0) { throw 'perf_seed failed' }
    } finally { Remove-Item Env:TREK_DATA_DIR }
    $seed
}

function Write-Settings([string]$Data) {
    New-Item -ItemType Directory -Force $Data | Out-Null
    $settings = @(
        '[general]', 'default_agent = "direct:mock"', '',
        '[onboarding]', 'completed = true', '',
        '[import]', 'claude_code = false', 'codex = false', 'opencode = false', '',
        '[notifications]', 'mode = "off"', 'dock_badge = false', 'menu_bar_icon = false', '',
        '[updates]', 'auto_check = false', 'auto_download = false'
    ) -join "`n"
    [IO.File]::WriteAllText("$Data\settings.toml", "$settings`n")
}

# A fresh data folder (empty, or a copy of the seed) and Trek launched on it.
function Start-Trek([string]$Bin, [string]$Name, [switch]$Seeded) {
    $work = Join-Path $Scratch $Name
    $data = Join-Path $work 'data'
    $shot = Join-Path $work 'shot'
    if ($Seeded) { Copy-Item -Recurse (New-Seed) $data } else { New-Item -ItemType Directory -Force $data | Out-Null }
    Write-Settings $data
    New-Item -ItemType Directory -Force $shot | Out-Null
    $psi = [Diagnostics.ProcessStartInfo]::new($Bin)
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($kv in @{
            TREK_DATA_DIR = $data; TREK_SHOT_DIR = $shot; TREK_PERF = '1'; TREK_MOCK_AGENT = '1'
            TREK_BACKGROUND = '1'; TREK_FORCE_ACTIVE = '1'; TREK_WINDOW_SIZE = $Size; RUST_LOG = 'warn,trek=info'
        }.GetEnumerator()) { $psi.EnvironmentVariables[$kv.Key] = $kv.Value }
    $p = [Diagnostics.Process]::Start($psi)
    # Drained so a chatty run never blocks on a full pipe; the log file has everything.
    $null = $p.StandardOutput.ReadToEndAsync()
    $null = $p.StandardError.ReadToEndAsync()
    [pscustomobject]@{ Proc = $p; Data = $data; Shot = $shot; Work = $work }
}

function Send-Batch($T, [string[]]$Lines, [int]$TimeoutSec = 600) {
    Remove-Item "$($T.Shot)\done" -Force -ErrorAction SilentlyContinue
    $tmp = Join-Path $T.Shot '.cmd.tmp'
    [IO.File]::WriteAllText($tmp, (($Lines | ForEach-Object { $_.Replace('@DATA@', $T.Data) }) -join "`n") + "`n")
    [IO.File]::Move($tmp, (Join-Path $T.Shot 'cmd'), $true)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while (-not (Test-Path "$($T.Shot)\done")) {
        if ($T.Proc.HasExited) { Start-Sleep -Milliseconds 300; if (Test-Path "$($T.Shot)\done") { break }; throw "Trek exited mid-batch (exit $($T.Proc.ExitCode)); see $($T.Data)\logs" }
        if ((Get-Date) -gt $deadline) { throw "batch timed out: $($Lines -join '; ')" }
        Start-Sleep -Milliseconds 100
    }
    $done = (Get-Content "$($T.Shot)\done" -Raw).Trim()
    if ($done -ne 'ok') { throw "batch failed: $done" }
}

# The `perf:` lines logged so far, as objects: Kind plus each key=value.
function Read-Perf($T) {
    $log = Get-ChildItem "$($T.Data)\logs\trek-*.log" -ErrorAction SilentlyContinue
    if (-not $log) { return @() }
    # Shared: Trek has the file open for writing.
    $fs = [IO.FileStream]::new($log[-1].FullName, 'Open', 'Read', 'ReadWrite')
    try { $text = [IO.StreamReader]::new($fs).ReadToEnd() } finally { $fs.Dispose() }
    foreach ($line in $text -split "`r?`n") {
        # ` perf: <kind> k=v …` (not the `trek::perf:` target before it).
        if ($line -notmatch '\sperf: (\w+)(.*)$') { continue }
        $o = [ordered]@{ Kind = $Matches[1] }
        foreach ($m in [regex]::Matches($Matches[2], '(\w+)=(\S+)')) { $o[$m.Groups[1].Value] = $m.Groups[2].Value }
        [pscustomobject]$o
    }
}

function Wait-Perf($T, [string]$Kind, [int]$TimeoutSec = 60, [scriptblock]$Where = { $true }) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $hit = Read-Perf $T | Where-Object { $_.Kind -eq $Kind } | Where-Object $Where | Select-Object -Last 1
        if ($hit) { return $hit }
        if ($T.Proc.HasExited) { throw "Trek exited (exit $($T.Proc.ExitCode)) before logging perf: $Kind" }
        Start-Sleep -Milliseconds 100
    }
    throw "no perf: $Kind within ${TimeoutSec}s"
}

# Quit as the menu does and wait; $script:QuitMs is how long from `quit` to the process gone.
function Stop-Trek($T) {
    $script:QuitMs = $null
    if (-not $T.Proc.HasExited) {
        $asked = [Diagnostics.Stopwatch]::StartNew()
        try { Send-Batch $T @('quit') 30 } catch {}
        if (-not $T.Proc.WaitForExit(20000)) { try { $T.Proc.Kill() } catch {}; return -1 }
        $script:QuitMs = $asked.ElapsedMilliseconds
    }
    $T.Proc.WaitForExit()
    $T.Proc.ExitCode
}

function Add-Result([string]$Name, [int]$Run, [hashtable]$Values) {
    foreach ($k in $Values.Keys) { $Results.Add([pscustomobject]@{ Measure = $Name; Run = $Run; Key = $k; Value = [double]$Values[$k] }) }
    Say ("  {0} #{1}: {2}" -f $Name, $Run, (($Values.GetEnumerator() | Sort-Object Key | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
}

function Measure-Startup($Bin, [int]$Run, [switch]$Seeded) {
    $name = if ($Seeded) { 'startup-200' } else { 'startup-empty' }
    $t = Start-Trek $Bin "$name-$Run" -Seeded:$Seeded
    try {
        $s = Wait-Perf $t 'startup' 60
        # The wall clock of the first paint, less the process's own start: the loader's time too.
        $launched = [DateTimeOffset]::new($t.Proc.StartTime).ToUnixTimeMilliseconds()
        $v = @{ from_process_start_ms = ([double]$s.at - $launched) }
        # Each phase is when it ended, in ms after `main` began (see main.rs).
        foreach ($p in $s.PSObject.Properties) { if ($p.Name -notin 'Kind', 'at') { $v["$($p.Name)_ms"] = $p.Value } }
        Add-Result $name $Run $v
    } finally { $null = Stop-Trek $t }
}

function Measure-Basecamp($Bin, [int]$Run) {
    $t = Start-Trek $Bin "basecamp-$Run" -Seeded
    try {
        $null = Wait-Perf $t 'startup' 60
        # Before the warm-up pass (10 s after launch): the first open reads everything.
        Send-Batch $t @('route basecamp', 'range all', 'wait 1500', 'perf frames 3000 basecamp-all')
        $b = Wait-Perf $t 'basecamp' 30
        $f = Wait-Perf $t 'frames' 10 { $_.label -eq 'basecamp-all' }
        Add-Result 'basecamp-all' $Run @{ pass_all_ms = $b.all; threads = $b.threads; frame_p50_ms = $f.frame_p50; frame_p95_ms = $f.frame_p95 }
    } finally { $null = Stop-Trek $t }
}

function Measure-Scroll($Bin, [int]$Run) {
    $t = Start-Trek $Bin "scroll-$Run" -Seeded
    try {
        $null = Wait-Perf $t 'startup' 60
        # The thread opens at its end; the wheel goes up through it, 100 px a frame for 10 s. The
        # list has no id of its own: the pointer rests on the last answer (item 1998 of the seed's
        # 2,000), and stays there as the rows move under it.
        Send-Batch $t @('route title:Long transcript', 'wait 3000', 'perf scroll answer#1998 -100 600 transcript-up') 300
        $s = Wait-Perf $t 'scroll' 10
        Add-Result 'scroll-2000' $Run @{ frame_p50_ms = $s.frame_p50; frame_p95_ms = $s.frame_p95; frame_max_ms = $s.frame_max; over_budget = $s.frame_over; frames = $s.frames; latency_p50_ms = $s.latency_p50; latency_p95_ms = $s.latency_p95; unpainted = $s.unpainted }
    } finally { $null = Stop-Trek $t }
}

function Measure-Type($Bin, [int]$Run) {
    $t = Start-Trek $Bin "type-$Run" -Seeded
    try {
        $null = Wait-Perf $t 'startup' 60
        $proj = '@DATA@\projects\atlas-api'
        # Halfway down a 3,000-line Rust file (the seed's), at the start of a line.
        Send-Batch $t @('editor on', "editor root $proj", "editor open $proj\src\pipeline.rs 1500", 'wait 2000', 'perf type 200 editor', 'shot typed') 300
        $s = Wait-Perf $t 'type' 10
        Add-Result 'type-editor' $Run @{ latency_p50_ms = $s.latency_p50; latency_p95_ms = $s.latency_p95; latency_max_ms = $s.latency_max; frame_p50_ms = $s.frame_p50; frame_p95_ms = $s.frame_p95; unpainted = $s.unpainted }
    } finally { $null = Stop-Trek $t }
}

function Measure-Inbox($Bin, [int]$Run) {
    $t = Start-Trek $Bin "inbox-$Run" -Seeded
    try {
        $null = Wait-Perf $t 'startup' 60
        # Four threads with two background sub-agents each: eight running, beside 200 in the inbox.
        $lines = foreach ($p in 'atlas-api', 'harbor-web', 'lumen-cli', 'orbit-mobile') { "route project:@DATA@\projects\$p"; 'send subagents 20s'; 'wait 500' }
        Send-Batch $t (@($lines) + @('wait 1500', 'perf frames 8000 inbox-subagents')) 120
        $f = Wait-Perf $t 'frames' 10 { $_.label -eq 'inbox-subagents' }
        $p = Get-Process -Id $t.Proc.Id
        $v = @{ frame_p50_ms = $f.frame_p50; frame_p95_ms = $f.frame_p95; frame_max_ms = $f.frame_max; frames = $f.frames; over_budget = $f.frame_over; lag_p95_ms = $f.lag_p95; lag_max_ms = $f.lag_max; working_set_mb = [math]::Round($p.WorkingSet64 / 1MB) }
        # Quitting with the eight still at work: sessions ended, the process tree let go.
        $exit = Stop-Trek $t
        $v.quit_ms = $script:QuitMs
        $v.quit_exit = $exit
        Add-Result 'inbox-8-subagents' $Run $v
    } finally { $null = Stop-Trek $t }
}

# A long answer streaming in the thread on screen (two minutes of `mock:stream`), with Mica on and
# off: the frames of the transcript growing, and what the backdrop costs on top.
function Measure-Stream($Bin, [int]$Run) {
    $t = Start-Trek $Bin "stream-$Run" -Seeded
    try {
        $null = Wait-Perf $t 'startup' 60
        Send-Batch $t @('pace 1', 'route project:@DATA@\projects\atlas-api', 'send mock:stream 150s', 'wait 120000', 'glass on', 'wait 500', 'perf frames 3000 stream-glass', 'glass off', 'wait 500', 'perf frames 3000 stream-plain') 300
        foreach ($g in 'glass', 'plain') {
            $f = Wait-Perf $t 'frames' 10 { $_.label -eq "stream-$g" }
            Add-Result "stream-2min-$g" $Run @{ frame_p50_ms = $f.frame_p50; frame_p95_ms = $f.frame_p95; frame_max_ms = $f.frame_max; frames = $f.frames; lag_p95_ms = $f.lag_p95 }
        }
        # Then, finished, the wheel up through it.
        Send-Batch $t @('wait idle 120000', 'wait 500', 'elements') 200
        # The answer's last part on screen (a build before parts: the answer's one row).
        $part = (Select-String "$($t.Shot)\elements.txt" -Pattern '^(answer-part#\d+|answer#\d+) ' | Select-Object -Last 1).Matches[0].Groups[1].Value
        # (Builds before 2026-10-10 aimed the wheel at the middle of the row, off the window for
        # a tall one: every step goes unpainted there, at 500 ms each. Not worth waiting for.)
        try {
            Send-Batch $t @("perf scroll $part -60 300 long-answer-up") 400
            $s = Wait-Perf $t 'scroll' 10 { $_.label -eq 'long-answer-up' }
            Add-Result 'scroll-long-answer' $Run @{ frame_p50_ms = $s.frame_p50; frame_p95_ms = $s.frame_p95; frame_max_ms = $s.frame_max; frames = $s.frames; latency_p50_ms = $s.latency_p50; latency_p95_ms = $s.latency_p95; unpainted = $s.unpainted }
        } catch { Say "  scroll-long-answer #${Run}: $_" }
    } finally { $null = Stop-Trek $t }
}

function Invoke-Stress($Bin) {
    $t = Start-Trek $Bin 'stress' -Seeded
    $samples = [Collections.Generic.List[object]]::new()
    $sample = {
        $p = Get-Process -Id $t.Proc.Id
        $samples.Add([pscustomobject]@{ Minute = [math]::Round(((Get-Date) - $p.StartTime).TotalMinutes, 1); WorkingSetMB = [math]::Round($p.WorkingSet64 / 1MB); PrivateMB = [math]::Round($p.PrivateMemorySize64 / 1MB); Handles = $p.HandleCount; Threads = $p.Threads.Count })
        $samples[-1]
    }
    try {
        $null = Wait-Perf $t 'startup' 60
        Start-Sleep -Seconds 12
        Say "  start: $(& $sample)"
        $dur = "$($StressMin)m"
        $lines = @('pace 1')
        foreach ($p in 'atlas-api', 'harbor-web', 'lumen-cli') { $lines += "route project:@DATA@\projects\$p"; $lines += "send mock:stream $dur"; $lines += 'wait 500' }
        # Sub-agents beside them, reporting back near the end.
        $lines += 'route project:@DATA@\projects\orbit-mobile'; $lines += "send subagents $([math]::Max(1, $StressMin - 2))m"
        Send-Batch $t $lines 120
        $end = (Get-Date).AddMinutes($StressMin)
        $minute = 0
        while ((Get-Date) -lt $end) {
            Start-Sleep -Seconds 55
            $minute++
            # Now and then a look round, as someone would: Basecamp, the long thread, and one of
            # the streaming threads (titled by its prompt), timed as it streams.
            # A stream that has finished is retitled: then there's no streaming thread to look at.
            $s = & $sample
            try {
                Send-Batch $t @('route basecamp', 'wait 1000', 'route title:Long transcript', 'wait 1000', 'route title:mock:stream', 'wait 500', "perf frames 3000 stream-$minute") 120
                $f = Wait-Perf $t 'frames' 10 { $_.label -eq "stream-$minute" }
                $s | Add-Member -NotePropertyMembers @{ FrameP95 = [double]$f.frame_p95; FrameMax = [double]$f.frame_max; Frames = [int]$f.frames; LagP95 = [double]$f.lag_p95; LagMax = [double]$f.lag_max }
            } catch { Say "  (no streaming thread to look at: $_)" }
            Say "  $s"
        }
        Send-Batch $t @('wait idle 300000', 'perf frames 3000 stress-end') 400
        Say "  idle: $(& $sample)"
    } finally {
        $exit = Stop-Trek $t
        $leaks = Select-String -Path "$($t.Data)\logs\*.log" -Pattern 'leaked|panic' -SimpleMatch -ErrorAction SilentlyContinue
        Say "  quit: exit $exit; $(@($leaks).Count) leak/panic lines in the log"
        # Every column, though the first sample (before any streaming) has no frame times.
        $samples | Select-Object Minute, WorkingSetMB, PrivateMB, Handles, Threads, Frames, FrameP95, FrameMax, LagP95, LagMax | Export-Csv -NoTypeInformation "$Out\stress.csv"
        [pscustomobject]@{ Exit = $exit; LeakLines = @($leaks).Count; First = $samples[0]; Last = $samples[-1]; Peak = ($samples | Sort-Object WorkingSetMB | Select-Object -Last 1) } | ConvertTo-Json | Set-Content "$Out\stress.json"
    }
}

$bin = Get-Trek
Say "trek: $bin"
try {
    if ('stress' -in $What) { Invoke-Stress $bin }
    foreach ($run in 1..$Runs) {
        if ('startup' -in $What) { Measure-Startup $bin $run; Measure-Startup $bin $run -Seeded }
        if ('basecamp' -in $What) { Measure-Basecamp $bin $run }
        if ('scroll' -in $What) { Measure-Scroll $bin $run }
        if ('type' -in $What) { Measure-Type $bin $run }
        if ('inbox' -in $What) { Measure-Inbox $bin $run }
        if ('stream' -in $What) { Measure-Stream $bin $run }
    }
    if ($Results.Count) {
        $Results | Export-Csv -NoTypeInformation "$Out\runs.csv"
        $medians = $Results | Group-Object Measure, Key | ForEach-Object {
            $v = @($_.Group.Value | Sort-Object)
            [pscustomobject]@{ Measure = $_.Group[0].Measure; Key = $_.Group[0].Key; Median = $v[[math]::Floor(($v.Count - 1) / 2)]; Runs = ($_.Group.Value -join ' / ') }
        }
        $medians | Export-Csv -NoTypeInformation "$Out\medians.csv"
        $medians | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
    }
} finally {
    if (-not $Keep) { Remove-Item -Recurse -Force $Scratch -ErrorAction SilentlyContinue } else { Say "kept $Scratch" }
}
