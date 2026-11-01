<#
.SYNOPSIS
  Check that this machine has what developing Larrier needs, and say how to
  get what's missing. Changes nothing.

.EXAMPLE
  ./scripts/setup.ps1
#>
$ErrorActionPreference = 'Continue'
Set-Location (Split-Path $PSScriptRoot)
$missing = 0

function Report([string] $what, [bool] $ok, [string] $detail, [string] $fix, [switch] $Optional) {
    if ($ok) {
        Write-Host ("  ok   {0,-18} {1}" -f $what, $detail) -ForegroundColor Green
    } elseif ($Optional) {
        Write-Host ("  --   {0,-18} {1}" -f $what, $fix) -ForegroundColor DarkYellow
    } else {
        Write-Host ("  no   {0,-18} {1}" -f $what, $fix) -ForegroundColor Red
        $script:missing++
    }
}

function Version-Of([string] $cmd, [string[]] $argv) {
    $c = Get-Command $cmd -ErrorAction SilentlyContinue
    if (-not $c) { return $null }
    try { return ((& $cmd @argv 2>&1) | Select-Object -First 1).ToString().Trim() } catch { return 'found' }
}

Write-Host 'Required'
$rustc = Version-Of 'rustc' @('--version')
$rustOk = $false
if ($rustc -match '(\d+)\.(\d+)') { $rustOk = ([int]$Matches[1] -gt 1) -or ([int]$Matches[2] -ge 85) }
Report 'Rust 1.85+' $rustOk $rustc 'Install from https://rustup.rs, then: rustup update stable'
$cargo = Version-Of 'cargo' @('--version')
Report 'Cargo' ([bool]$cargo) $cargo 'Comes with Rust (https://rustup.rs)'
$git = Version-Of 'git' @('--version')
Report 'Git' ([bool]$git) $git 'https://git-scm.com/downloads'
if ($IsWindows -or $env:OS -eq 'Windows_NT') {
    $wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
    if (-not $wv) { $wv = Get-ItemProperty 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue }
    Report 'WebView2' ([bool]$wv) $(if ($wv) { $wv.pv }) 'https://developer.microsoft.com/microsoft-edge/webview2/'
    $msvc = Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    Report 'MSVC build tools' $msvc 'Visual Studio installer found' 'Install "Desktop development with C++" from https://visualstudio.microsoft.com/visual-cpp-build-tools/'
}

Write-Host ''
Write-Host 'Optional'
$pwshOk = $PSVersionTable.PSVersion.Major -ge 7
Report 'PowerShell 7' $pwshOk "$($PSVersionTable.PSVersion)" 'https://aka.ms/powershell (the scripts also run in Windows PowerShell 5)' -Optional
$py = Version-Of 'python' @('--version')
Report 'Python 3.10+' ([bool]$py) $py 'For scripts/demo: https://www.python.org' -Optional
if ($py) {
    & python -c "import PIL, numpy" 2>$null
    Report 'Pillow + NumPy' ($LASTEXITCODE -eq 0) 'for compose_hero.py' 'pip install pillow numpy' -Optional
}
$gh = Version-Of 'gh' @('--version')
Report 'GitHub CLI' ([bool]$gh) $gh 'For releases and pull requests: https://cli.github.com' -Optional

Write-Host ''
if ($missing) { Write-Host "$missing required tool(s) missing." -ForegroundColor Red; exit 1 }
Write-Host 'Ready. Try: ./scripts/dev.ps1 -Sandbox' -ForegroundColor Green
