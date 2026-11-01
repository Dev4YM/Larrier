<#
.SYNOPSIS
  Run the Larrier desktop app from source.

.DESCRIPTION
  Builds and starts the app (debug unless -Release). With -Sandbox it runs
  against a fresh data folder under %TEMP%, so your own sessions and
  settings are never touched; -DataDir picks the folder yourself (for example
  the demo home from scripts/demo/make_demo.py).

.EXAMPLE
  ./scripts/dev.ps1
  ./scripts/dev.ps1 -Sandbox
  ./scripts/dev.ps1 -DataDir $env:TEMP\larrier-demo\home -DevTools 9333
#>
param(
    [switch] $Release,
    # Run with a new, empty data folder.
    [switch] $Sandbox,
    # Run with this data folder (LARRIER_HOME).
    [string] $DataDir,
    # Open the WebView's DevTools protocol on this port.
    [int] $DevTools,
    # Arguments passed to the app.
    [Parameter(ValueFromRemainingArguments)] [string[]] $AppArgs
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

if ($Sandbox) {
    $DataDir = Join-Path ([IO.Path]::GetTempPath()) "larrier-sandbox-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    New-Item -ItemType Directory -Force $DataDir | Out-Null
    Write-Host "Sandbox data folder: $DataDir"
}
if ($DataDir) { $env:LARRIER_HOME = (Resolve-Path $DataDir).Path }
if ($DevTools) { $env:LARRIER_DEVTOOLS_PORT = "$DevTools" }

$cargo = @('run', '-p', 'larrier-app')
if ($Release) { $cargo += '--release' }
if ($AppArgs) { $cargo += @('--') + $AppArgs }
& cargo @cargo
exit $LASTEXITCODE
