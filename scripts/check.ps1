<#
.SYNOPSIS
  Build, test and lint the whole workspace: what a pull request must pass.

.EXAMPLE
  ./scripts/check.ps1
  ./scripts/check.ps1 -Quick        # tests only
  ./scripts/check.ps1 -Ignored      # also the tests that need real agents or the network
#>
param(
    [switch] $Quick,
    [switch] $Ignored
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

function Step([string] $name, [string[]] $cargo) {
    Write-Host "==> $name" -ForegroundColor Cyan
    & cargo @cargo
    if ($LASTEXITCODE -ne 0) { Write-Host "check: $name failed" -ForegroundColor Red; exit $LASTEXITCODE }
}

if (-not $Quick) { Step 'Build' @('build', '--workspace', '--locked') }
Step 'Test' @('test', '--workspace', '--locked')
if ($Ignored) { Step 'Ignored tests' @('test', '--workspace', '--locked', '--', '--ignored') }
if (-not $Quick) { Step 'Clippy' @('clippy', '--workspace', '--locked') }
Write-Host 'All checks passed.' -ForegroundColor Green
