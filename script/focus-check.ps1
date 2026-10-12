# Does opening a project in Trek open any console window, or take the foreground from Trek's own?
#
# Trek is a GUI program with no console, so a child it starts without CREATE_NO_WINDOW gets a
# console window of its own, and each new one takes the foreground. A run from a terminal never
# shows it (the children share the terminal's console), so this starts Trek the way Explorer does:
# from a process with no console (DETACHED_PROCESS), on a scratch repository, in a scratch data
# folder and home (nothing of the user's is read or written), and then only WATCHES: it lists
# windows and the foreground window, and never activates, clicks or types.
#
#   pwsh script/focus-check.ps1 -Exe D:\trek-target\release\trek.exe
#   pwsh script/focus-check.ps1 -Exe <copy of trek.exe> -Background   # Trek opens behind other windows
#
# Use a release build (a debug one has a console of its own, which its children share). Copy the
# exe out of the shared target folder first: other builds overwrite it. Exit 0 when no console
# window (ConsoleWindowClass, or Windows Terminal's) opened and the foreground never went to one.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Exe,
    [switch]$Background,
    [int]$Seconds = 15
)

$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Watch {
    delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();

    public class Win {
        public long Hwnd; public string Class; public string Title; public uint Pid;
        public override string ToString() { return string.Format("{0:x} {1} pid={2} \"{3}\"", Hwnd, Class, Pid, Title); }
    }

    public static Win Describe(IntPtr h) {
        var c = new StringBuilder(256); var t = new StringBuilder(256); uint pid;
        GetClassName(h, c, c.Capacity); GetWindowText(h, t, t.Capacity); GetWindowThreadProcessId(h, out pid);
        return new Win { Hwnd = h.ToInt64(), Class = c.ToString(), Title = t.ToString(), Pid = pid };
    }

    /// Every visible top-level window.
    public static List<Win> Visible() {
        var all = new List<Win>();
        EnumWindows((h, l) => { if (IsWindowVisible(h)) all.Add(Describe(h)); return true; }, IntPtr.Zero);
        return all;
    }

    public static Win Foreground() {
        var h = GetForegroundWindow();
        return h == IntPtr.Zero ? null : Describe(h);
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags,
        IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct PROCESSENTRY32 {
        public int dwSize, cntUsage, th32ProcessID; public IntPtr th32DefaultHeapID;
        public int th32ModuleID, cntThreads, th32ParentProcessID, pcPriClassBase, dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExeFile;
    }
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Process32FirstW(IntPtr snap, ref PROCESSENTRY32 e);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Process32NextW(IntPtr snap, ref PROCESSENTRY32 e);

    /// "name:pid" of every process whose parent is `parent` right now.
    public static List<string> ChildrenOf(int parent) {
        var found = new List<string>();
        var snap = CreateToolhelp32Snapshot(0x2, 0);
        var e = new PROCESSENTRY32(); e.dwSize = Marshal.SizeOf(e);
        if (Process32FirstW(snap, ref e)) {
            do { if (e.th32ParentProcessID == parent) found.Add(e.szExeFile + ":" + e.th32ProcessID); } while (Process32NextW(snap, ref e));
        }
        CloseHandle(snap);
        return found;
    }

    /// Start `exe` with no console at all (DETACHED_PROCESS), inheriting this process's environment.
    public static int StartWithoutConsole(string exe, string args, string cwd) {
        var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi;
        var line = new StringBuilder("\"" + exe + "\" " + args);
        if (!CreateProcessW(exe, line, IntPtr.Zero, IntPtr.Zero, false, 0x00000008, IntPtr.Zero, cwd, ref si, out pi))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
        return pi.dwProcessId;
    }
}
'@

function Is-Console($w) { $w -and ($w.Class -eq 'ConsoleWindowClass' -or $w.Class -eq 'CASCADIA_HOSTING_WINDOW_CLASS') }

# Everything under one scratch folder of its own.
$root = Join-Path ([IO.Path]::GetTempPath()) "trek-focus-$PID"
$home_ = Join-Path $root 'home'; $data = Join-Path $root 'data'; $repo = Join-Path $root 'repo'
$null = New-Item -ItemType Directory -Force $home_, $data, $repo, (Join-Path $home_ 'AppData\Roaming'), (Join-Path $home_ 'AppData\Local')

Push-Location $repo
git init -q -b main
"hello`n" | Set-Content -NoNewline notes.txt
git add notes.txt
git -c user.name=Scratch -c user.email=scratch@example.invalid commit -q -m 'a first commit'
git checkout -q -b feature
"more`n" | Add-Content notes.txt
Pop-Location

# The default agent is the mock (never a real one), onboarding is done, and the repo is the user's.
@"
user_projects = ['$repo']

[general]
default_agent = "direct:mock"

[onboarding]
completed = true
"@ | Set-Content (Join-Path $data 'settings.toml')

$saved = @{}
$vars = @{
    TREK_DATA_DIR = $data; USERPROFILE = $home_; HOME = $home_
    APPDATA = (Join-Path $home_ 'AppData\Roaming'); LOCALAPPDATA = (Join-Path $home_ 'AppData\Local')
    TREK_BACKGROUND = $(if ($Background) { '1' } else { $null })
}
foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }

$before = [Watch]::Visible()
$known = [System.Collections.Generic.HashSet[long]]::new()
foreach ($w in $before) { [void]$known.Add($w.Hwnd) }
$fg0 = [Watch]::Foreground()
Write-Host "foreground before: $fg0"

# Opening a file in the repo makes Trek read the repo with git (the nine processes of read_git_info).
$trek = [Watch]::StartWithoutConsole($Exe, "`"$(Join-Path $repo 'notes.txt')`"", $root)
foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
Write-Host "started trek pid $trek with no console ($(if ($Background) { 'background' } else { 'foreground' }))"

$opened = @{}; $fgSeen = [System.Collections.Generic.List[object]]::new(); $last = $null
$started = [System.Collections.Generic.HashSet[string]]::new()
$until = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $until) {
    foreach ($c in [Watch]::ChildrenOf($trek)) { [void]$started.Add($c) }
    foreach ($w in [Watch]::Visible()) {
        if ($known.Add($w.Hwnd)) { $opened[$w.Hwnd] = $w }
    }
    $fg = [Watch]::Foreground()
    if ($fg -and (-not $last -or $fg.Hwnd -ne $last.Hwnd)) { $fgSeen.Add($fg); $last = $fg }
    Start-Sleep -Milliseconds 25
}
$fg1 = [Watch]::Foreground()

$trekWindows = @($opened.Values | Where-Object { $_.Pid -eq $trek })
$consoles = @($opened.Values | Where-Object { Is-Console $_ })
$fgConsole = @($fgSeen | Where-Object { Is-Console $_ })
Write-Host "new windows ($($opened.Count)):"
$opened.Values | ForEach-Object { Write-Host "  $_" }
Write-Host "foreground, in order:"
$fgSeen | ForEach-Object { Write-Host "  $_" }
Write-Host "foreground after: $fg1"
$gits = @($started | Where-Object { $_ -like 'git.exe:*' })
Write-Host "processes trek started ($($started.Count), $($gits.Count) of them git): $(($started | ForEach-Object { $_ -replace ':\d+$' } | Group-Object | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', ')"
Write-Host "trek's own windows: $($trekWindows.Count)"
Write-Host "console windows opened: $($consoles.Count)"
Write-Host "foreground went to a console window: $($fgConsole.Count -gt 0)"

# Only what this script started: Trek and what it started, by its pid.
& taskkill /PID $trek /T /F *> $null
Start-Sleep -Milliseconds 500
if ($root.StartsWith([IO.Path]::GetTempPath()) -and (Split-Path $root -Leaf) -like 'trek-focus-*') {
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}
if ($consoles.Count -gt 0 -or $fgConsole.Count -gt 0) { exit 1 }
