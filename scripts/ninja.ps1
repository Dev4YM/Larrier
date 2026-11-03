<#
.SYNOPSIS
  Put Ninja on PATH: on Windows the browser engine's bindings are built
  with CMake and Ninja, and Visual Studio's C++ tools ship a copy.

.DESCRIPTION
  Dot-source it before building (. ./scripts/ninja.ps1). On GitHub Actions
  it also adds Ninja to the job's PATH for the steps after it.
#>
if (Get-Command ninja -ErrorAction SilentlyContinue) { return }
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$found = $null
if (Test-Path $vswhere) {
    foreach ($vs in & $vswhere -all -prerelease -products * -property installationPath) {
        $dir = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'
        if (Test-Path (Join-Path $dir 'ninja.exe')) { $found = $dir; break }
    }
}
if (-not $found) {
    Write-Host "ninja: not found. Install Visual Studio's C++ CMake tools, or Ninja itself (winget install Ninja-build.Ninja)." -ForegroundColor Red
    exit 1
}
$env:PATH = "$found;$env:PATH"
if ($env:GITHUB_PATH) { Add-Content -Path $env:GITHUB_PATH -Value $found }
