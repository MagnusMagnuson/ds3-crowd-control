# Builds a release zip that runs without .NET, Rust or the C++ build tools installed.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -Version 0.1.0
param([Parameter(Mandatory = $true)][string]$Version)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$name = "ds3-crowd-control-v$Version"
$out = Join-Path $root "dist\$name"
$zip = Join-Path $root "dist\$name.zip"

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
if (Test-Path $zip) { Remove-Item -Force $zip }

dotnet publish (Join-Path $root 'src\Ds3CrowdControl.Manager') `
    -c Release -r win-x64 --self-contained true `
    -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true `
    -o $out
if ($LASTEXITCODE -ne 0) { throw 'dotnet publish failed' }

Get-ChildItem $out -Filter *.pdb | Remove-Item
Copy-Item (Join-Path $root 'README.md') $out
Copy-Item (Join-Path $root 'THIRD-PARTY-NOTICES.md') $out
Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zip
Write-Host "Wrote $zip"
