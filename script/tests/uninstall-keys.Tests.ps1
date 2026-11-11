#Requires -Version 5.1
# Tests for the registry part of script/Uninstall-Trek.ps1 (Remove-TrekKeys): the keys Trek writes
# for itself go, and nothing else does. Plain assertions, no framework. They run on keys of their
# own under HKCU:\Software, removed after; the uninstaller itself isn't run (only its function is
# taken from it, parsed).
#
#   pwsh script/tests/uninstall-keys.Tests.ps1

$ErrorActionPreference = 'Stop'
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$uninstaller = Join-Path $Root 'script\Uninstall-Trek.ps1'
$ast = [System.Management.Automation.Language.Parser]::ParseFile($uninstaller, [ref]$null, [ref]$null)
$fn = $ast.Find({
        param($n)
        $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Remove-TrekKeys'
    }, $true)
if (-not $fn) { throw "Remove-TrekKeys isn't in $uninstaller" }
. ([scriptblock]::Create($fn.Extent.Text))

$script:Failures = 0
function Check([string]$Name, [bool]$Cond) {
    if ($Cond) { Write-Host "  ok   $Name" }
    else { Write-Host "  FAIL $Name"; $script:Failures++ }
}

$scratch = "HKCU:\Software\trek-uninstall-test-$([guid]::NewGuid().ToString('N'))"
$classes = Join-Path $scratch 'Classes'
$exe = 'C:\Users\someone\AppData\Local\Programs\Trek\trek.exe'
$protocol = Join-Path $classes 'trek'
$identity = Join-Path $classes 'AppUserModelId\dev.trek.Trek'
$otherIdentity = Join-Path $classes 'AppUserModelId\other.App'
$otherClass = Join-Path $classes 'trekker'

# The keys as Trek writes them (winsys.rs), its links opening `$OpenWith`, and two that aren't Trek's.
function Seed([string]$OpenWith) {
    Remove-Item -LiteralPath $classes -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -Path (Join-Path $protocol 'shell\open\command') -Force | Out-Null
    New-Item -Path (Join-Path $protocol 'DefaultIcon') -Force | Out-Null
    Set-Item -LiteralPath $protocol -Value 'URL:Trek'
    Set-ItemProperty -LiteralPath $protocol -Name 'URL Protocol' -Value ''
    Set-Item -LiteralPath (Join-Path $protocol 'shell\open\command') -Value "`"$OpenWith`" --link `"%1`""
    New-Item -Path $identity -Force | Out-Null
    Set-ItemProperty -LiteralPath $identity -Name 'DisplayName' -Value 'Trek'
    New-Item -Path $otherIdentity -Force | Out-Null
    New-Item -Path $otherClass -Force | Out-Null
}

try {
    Write-Host "this install's keys"
    Seed $exe
    Remove-TrekKeys -Classes $classes -Exe $exe
    Check 'protocol removed'             (-not (Test-Path -LiteralPath $protocol))
    Check 'identity removed'             (-not (Test-Path -LiteralPath $identity))
    Check "another app's identity kept"  (Test-Path -LiteralPath $otherIdentity)
    Check 'a key that starts trek kept'  (Test-Path -LiteralPath $otherClass)

    Write-Host 'the same exe, spelled another way'
    Seed $exe.ToUpperInvariant()
    Remove-TrekKeys -Classes $classes -Exe 'C:\Users\someone\AppData\Local\Programs\Trek\.\trek.exe'
    Check 'protocol removed'             (-not (Test-Path -LiteralPath $protocol))

    Write-Host 'links another Trek has taken since'
    Seed 'D:\Elsewhere\Trek\trek.exe'
    Remove-TrekKeys -Classes $classes -Exe $exe
    Check 'protocol kept'                (Test-Path -LiteralPath (Join-Path $protocol 'shell\open\command'))
    Check 'identity removed'             (-not (Test-Path -LiteralPath $identity))

    Write-Host 'nothing there'
    Remove-Item -LiteralPath $classes -Recurse -Force
    New-Item -Path $classes -Force | Out-Null
    $failed = $false
    try { Remove-TrekKeys -Classes $classes -Exe $exe } catch { $failed = $true }
    Check 'no error'                     (-not $failed)
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
Check 'scratch keys gone'                (-not (Test-Path -LiteralPath $scratch))

if ($script:Failures -gt 0) {
    Write-Host "$script:Failures failed"
    exit 1
}
Write-Host 'all passed'
