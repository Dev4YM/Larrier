<#
.SYNOPSIS
  Build the optimized Larrier program into dist/, named and hashed the way
  releases are.

.DESCRIPTION
  Produces dist/Larrier-<version>-windows-x64.exe and its .sha256, and
  checks that the program reports the workspace version. Releases use
  scripts/release.ps1, which builds the same way.

.EXAMPLE
  ./scripts/build.ps1
  ./scripts/build.ps1 -Cli          # also the command line, as dist/larrier.exe
#>
param(
    [switch] $Cli
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

$meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$version = ($meta.packages | Where-Object name -eq 'larrier-app').version

# The browser engine's bindings build with CMake and Ninja.
. "$PSScriptRoot/ninja.ps1"

$packages = @('-p', 'larrier-app')
if ($Cli) { $packages += @('-p', 'larrier-cli') }
& cargo build --release --locked @packages
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force dist | Out-Null
$name = "Larrier-$version-windows-x64.exe"
Copy-Item target/release/larrier-desktop.exe "dist/$name" -Force
$hash = (Get-FileHash "dist/$name" -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "dist/$name.sha256" -Value "$hash  $name" -NoNewline -Encoding ascii
if ($Cli) { Copy-Item target/release/larrier.exe dist/larrier.exe -Force }

$reported = & "dist/$name" --version | Out-String
if ($reported -notmatch [regex]::Escape($version)) { Write-Host "build: the program reports '$($reported.Trim())', expected $version" -ForegroundColor Red; exit 1 }
Write-Host "dist/$name  ($hash)" -ForegroundColor Green
