<#
.SYNOPSIS
    Builds Dusk's release: both binaries, the folder the installer installs, and the installer.
    From the repository root:  powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

.DESCRIPTION
    1. Builds dusk.exe and dusq.exe in release, dusq on its own (a workspace build would turn
       on the gpu feature for it; see CLAUDE.md).
    2. Stages target\release-stage\Dusk, the installed folder: the two programs, the five FFmpeg
       DLLs beside them, and licenses\ (Dusk's MIT license, FFmpeg's LGPL with the GPL it builds
       on and where its source is, Slint's license, the fonts' OFL, and THIRD-PARTY-NOTICES.txt
       from scripts\third-party-notices.py).
    3. With Inno Setup 6 (-Iscc, ISCC.exe on PATH, or its default folder), compiles
       installer\dusk.iss into target\installer\dusk-<version>-setup.exe.
    It prints the installed size and the installer's size (docs/REQUIREMENTS.md, "Download").
    Needs the pinned FFmpeg (scripts\setup-ffmpeg.ps1), Python 3 and cargo; it reads crates
    from cargo's registry and never goes to the network itself.

.PARAMETER Iscc
    The Inno Setup compiler, when it is not on PATH or in its default folder.

.PARAMETER NoBuild
    Stages and packs what target\release already holds.
#>
param([string]$Iscc = '', [switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
. (Join-Path $PSScriptRoot 'dev-env.ps1')
if (-not $env:FFMPEG_DIR) { throw 'The pinned FFmpeg is not installed: run scripts\setup-ffmpeg.ps1 first.' }

$version = (Select-String -LiteralPath 'Cargo.toml' -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "Dusk $version"

function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)." }
}

if (-not $NoBuild) {
    Invoke-Native 'Building dusk.exe' { cargo build -p dusk-app --release }
    Invoke-Native 'Building dusq.exe' { cargo build -p dusk-cli --release }
}

# The installed folder, made afresh.
$stage = Join-Path $root 'target\release-stage\Dusk'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
$licenses = Join-Path $stage 'licenses'
New-Item -ItemType Directory -Force -Path $licenses | Out-Null
foreach ($exe in 'dusk.exe', 'dusq.exe') {
    Copy-Item -LiteralPath (Join-Path $root "target\release\$exe") -Destination $stage
}
$pin = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'ffmpeg-pin.psd1')
$ffmpegBin = Join-Path $env:FFMPEG_DIR 'bin'
foreach ($prefix in $pin.ShipDlls) {
    $dll = @(Get-ChildItem -LiteralPath $ffmpegBin -Filter "$prefix*.dll")
    if ($dll.Count -ne 1) { throw "Expected one $prefix*.dll in $ffmpegBin." }
    Copy-Item -LiteralPath $dll[0].FullName -Destination $stage
}

# Licenses. Slint's crate carries its own license and the GPL text FFmpeg's LGPL builds on.
# Windows' packages only: a build fetches no other platform's crates, so --offline would fail
# on them.
$metadata = cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-msvc --offline | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
$slint = @($metadata.packages | Where-Object { $_.name -eq 'slint' })[0]
$slintLicenses = Join-Path (Split-Path -Parent $slint.manifest_path) 'LICENSES'
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $licenses 'Dusk.txt')
Copy-Item -LiteralPath (Join-Path $env:FFMPEG_DIR 'LICENSE.txt') -Destination (Join-Path $licenses 'FFmpeg-LGPL-3.0.txt')
Copy-Item -LiteralPath (Join-Path $slintLicenses 'GPL-3.0-only.txt') -Destination (Join-Path $licenses 'GPL-3.0.txt')
Copy-Item -LiteralPath (Join-Path $slintLicenses 'LicenseRef-Slint-Royalty-free-2.0.md') -Destination (Join-Path $licenses 'Slint.txt')
Copy-Item -LiteralPath (Join-Path $root 'dusk-app\ui\fonts\Inter\LICENSE.txt') -Destination (Join-Path $licenses 'Inter.txt')
Copy-Item -LiteralPath (Join-Path $root 'dusk-app\ui\fonts\JetBrainsMono\OFL.txt') -Destination (Join-Path $licenses 'JetBrains-Mono.txt')
Invoke-Native 'Writing the third-party notices' {
    python (Join-Path $PSScriptRoot 'third-party-notices.py') (Join-Path $licenses 'THIRD-PARTY-NOTICES.txt')
}

# FFmpeg's notice: this build, where its source is, and how it was configured, as avutil says.
$avutil = @(Get-ChildItem -LiteralPath $ffmpegBin -Filter 'avutil-*.dll')[0].FullName
$text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($avutil))
$anchor = $text.IndexOf('--enable-version3')
$start = $text.LastIndexOf([char]0, $anchor) + 1
$config = $text.Substring($start, $text.IndexOf([char]0, $anchor) - $start)
if ($pin.FileName -notmatch '^ffmpeg-(n[0-9.]+-[0-9]+-g([0-9a-f]+))-') { throw "Cannot read FFmpeg's version from $($pin.FileName)." }
$ffmpegVersion, $commit = $Matches[1], $Matches[2]
$dlls = (@(Get-ChildItem -LiteralPath $stage -Filter '*.dll') | ForEach-Object Name) -join ', '
@"
FFmpeg

dusk.exe and dusq.exe use five libraries of FFmpeg (https://ffmpeg.org): avcodec, avformat,
avutil, swscale and swresample, linked dynamically. They are the DLLs beside the two programs
($dlls), and you may replace them with your own build of the same FFmpeg version.

FFmpeg is free software under the GNU Lesser General Public License, version 3 or later
(FFmpeg-LGPL-3.0.txt), which builds on the GNU General Public License, version 3
(GPL-3.0.txt). It comes with no warranty.

These DLLs come from $($pin.FileName), release $($pin.Tag) of BtbN's FFmpeg-Builds
(SHA-256 $($pin.Sha256)): FFmpeg $ffmpegVersion. Its source is
https://github.com/FFmpeg/FFmpeg/tree/$commit
and the scripts that built it, which name the source of every library below, are
https://github.com/BtbN/FFmpeg-Builds/tree/$($pin.Tag)

It was configured with:

$config

The libraries those options name are built into the DLLs where FFmpeg uses them, each
under its own license, as its source gives it.
"@ | Set-Content -LiteralPath (Join-Path $licenses 'FFmpeg.txt') -Encoding UTF8

$installed = (Get-ChildItem -LiteralPath $stage -Recurse -File | Measure-Object -Property Length -Sum).Sum
Write-Host ('Staged {0}: {1:N1} MB installed' -f $stage, ($installed / 1e6))

# The installer, when Inno Setup is at hand.
if (-not $Iscc) {
    $found = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($found) { $Iscc = $found.Source }
    elseif (Test-Path -LiteralPath "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") { $Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
}
if (-not $Iscc) {
    Write-Warning 'Inno Setup 6 was not found, so only the installed folder was made. Install it, or pass -Iscc.'
    return
}
$out = Join-Path $root 'target\installer'
Invoke-Native 'Compiling the installer' {
    & $Iscc /Q "/DAppVersion=$version" "/DStage=$stage" "/O$out" (Join-Path $root 'installer\dusk.iss')
}
$setup = Join-Path $out "dusk-$version-setup.exe"
Write-Host ('Installer {0}: {1:N1} MB' -f $setup, ((Get-Item -LiteralPath $setup).Length / 1e6))
