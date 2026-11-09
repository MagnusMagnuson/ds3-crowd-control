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

# The compilers write the folders they built from into the files, for error messages and
# debugging. Those name the builder's Windows user, and many people use their real name for
# it, so the release leaves them out: Rust gets its paths rewritten, and .NET writes no debug
# information at all. The encoded form keeps folder names with spaces in one piece, and it
# replaces the rustflags in .cargo\config.toml, so crt-static is repeated here.
# When several prefixes match, rustc uses the last one, so the more specific ones come later.
$remaps = @("$($env:USERPROFILE)=~")
foreach ($folder in @($env:CARGO_HOME, $env:RUSTUP_HOME)) {
    if ($folder) { $remaps += "$folder=~" }
}
$remaps += "$root=."
$flags = @('-C', 'target-feature=+crt-static') + ($remaps | ForEach-Object { "--remap-path-prefix=$_" })
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
try {
    dotnet publish (Join-Path $root 'src\Ds3CrowdControl.Manager') `
        -c Release -r win-x64 --self-contained true `
        -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true `
        -p:DebugType=none -p:DebugSymbols=false `
        -o $out
    if ($LASTEXITCODE -ne 0) { throw 'dotnet publish failed' }
}
finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
}

# Stop if the user folder, user name or PC name slipped through anyway, in either of the
# text encodings Windows programs use. Only readable text counts, as the strings tool finds
# it, since a short name like "ann" turns up by chance in compressed data. Names count only
# where they stand on their own, so "ann" is not found inside "announce" either.
$checks = @(
    [regex]::Escape($env:USERPROFILE),
    "(?<![\p{L}\p{N}_])$([regex]::Escape($env:USERNAME))(?![\p{L}\p{N}_])",
    "(?<![\p{L}\p{N}_])$([regex]::Escape($env:COMPUTERNAME))(?![\p{L}\p{N}_])"
)
foreach ($file in Get-ChildItem $out -File) {
    $bytes = [IO.File]::ReadAllBytes($file.FullName)
    $texts = @(
        [Text.Encoding]::GetEncoding(28591).GetString($bytes),
        [Text.Encoding]::Unicode.GetString($bytes),
        [Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1)
    )
    $readable = ($texts | ForEach-Object { [regex]::Matches($_, '[ -~]{6,}') | ForEach-Object { $_.Value } }) -join "`n"
    foreach ($check in $checks) {
        foreach ($text in @($readable)) {
            if ([regex]::IsMatch($text, $check, 'IgnoreCase')) {
                throw "$($file.Name) still contains your Windows user folder, user name or PC name"
            }
        }
    }
}

Get-ChildItem $out -Filter *.pdb | Remove-Item
Copy-Item (Join-Path $root 'README.md') $out
Copy-Item (Join-Path $root 'THIRD-PARTY-NOTICES.md') $out
Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zip
Write-Host "Wrote $zip"
