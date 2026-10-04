<#
.SYNOPSIS
    Fetches, verifies and unpacks the pinned FFmpeg build into .deps\ffmpeg\.

.DESCRIPTION
    Reads the pin from scripts\ffmpeg-pin.psd1. Downloads the zip once into a cache outside
    the repo (or uses -ZipPath), checks its size and SHA-256, extracts it to
    .deps\ffmpeg\<asset name>\, and checks that the five shipped DLLs, their import libraries
    and the headers are present and that the build was configured with the required flags.
    It never runs an FFmpeg executable. Safe to run again: a verified install is re-checked
    and kept. Works in Windows PowerShell 5.1 and PowerShell 7.

.PARAMETER ZipPath
    Use this local copy of the pinned zip instead of downloading it (for example a copy kept
    after BtbN deleted the build). It must end in .zip and match the pinned SHA-256; it is
    copied into the cache so later runs find it.

.PARAMETER CacheDir
    Where the downloaded zip is kept. Default: %LOCALAPPDATA%\dusk\ffmpeg-cache.
    Keep this file: BtbN keeps only its 14 newest builds (plus month-end builds for two years).

.PARAMETER Force
    Re-extract even if .deps\ffmpeg\<asset name>\ is already verified.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1
#>
[CmdletBinding()]
param(
    [string]$ZipPath,
    [string]$CacheDir = (Join-Path $env:LOCALAPPDATA 'dusk\ffmpeg-cache'),
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow in 5.1 with the progress bar on

$repoRoot = Split-Path -Parent $PSScriptRoot
$pin = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'ffmpeg-pin.psd1')
foreach ($key in 'Tag', 'FileName', 'Url', 'Size', 'Sha256', 'RequiredConfig', 'ForbiddenConfig', 'ShipDlls') {
    if (-not $pin -or -not $pin.ContainsKey($key)) { throw "scripts\ffmpeg-pin.psd1 is missing '$key'." }
}
$assetName = [IO.Path]::GetFileNameWithoutExtension($pin.FileName)
$installRoot = Join-Path $repoRoot '.deps\ffmpeg'
$installDir = Join-Path $installRoot $assetName
$marker = Join-Path $installDir '.dusk-pin-sha256'

function Test-Marker {
    (Test-Path -LiteralPath $marker) -and (([string](Get-Content -LiteralPath $marker -Raw)).Trim() -eq $pin.Sha256)
}

function Test-ZipFile([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $item = Get-Item -LiteralPath $path
    if ($item.Length -ne $pin.Size) {
        Write-Warning "$path is $($item.Length) bytes, expected $($pin.Size)."
        return $false
    }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $pin.Sha256) {
        Write-Warning "$path has SHA-256 $hash, expected $($pin.Sha256)."
        return $false
    }
    return $true
}

# Leftovers from an interrupted earlier run.
if (Test-Path -LiteralPath $installRoot) {
    Get-ChildItem -LiteralPath $installRoot -Directory -Force |
        Where-Object { $_.Name -like '.staging-*' -or $_.Name -like '.old-*' } |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
}

if (-not $Force -and (Test-Marker)) {
    Write-Host "FFmpeg $assetName is already installed; re-checking it."
} else {
    $hadVerifiedInstall = Test-Marker

    # 1. Get a verified copy of the zip into the cache.
    New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
    $zip = Join-Path $CacheDir $pin.FileName
    if ($ZipPath) {
        if (-not (Test-ZipFile $ZipPath)) {
            throw "The zip at $ZipPath does not match the pin in scripts\ffmpeg-pin.psd1. Use the exact file $($pin.FileName)."
        }
        $source = (Resolve-Path -LiteralPath $ZipPath).Path
        if ($source -ne $zip) { Copy-Item -LiteralPath $source -Destination $zip -Force }
    } elseif (Test-ZipFile $zip) {
        Write-Host "Using cached $zip"
    } else {
        $part = "$zip.part"
        if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        Write-Host "Downloading $($pin.FileName) ($([math]::Round($pin.Size / 1e6, 1)) MB) from $($pin.Url)"
        try {
            Invoke-WebRequest -Uri $pin.Url -OutFile $part -UseBasicParsing
        } catch {
            if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
            throw "Download failed: $($_.Exception.Message). If BtbN has deleted this build, pass a kept copy with -ZipPath, or move the pin to a newer build (a documented decision; see docs/SETUP.md)."
        }
        if (-not (Test-ZipFile $part)) {
            Remove-Item -LiteralPath $part -Force
            throw 'The downloaded file does not match the pinned size and SHA-256. Nothing was installed.'
        }
        Move-Item -LiteralPath $part -Destination $zip -Force
    }

    # 2. Extract into a staging folder, then swap it in. The old install is renamed aside rather
    #    than deleted in place, so a locked DLL can never leave a half-deleted install behind.
    New-Item -ItemType Directory -Force -Path $installRoot | Out-Null
    $staging = Join-Path $installRoot ('.staging-' + [guid]::NewGuid().ToString('N'))
    $old = $null
    try {
        Write-Host "Extracting to $installDir"
        Expand-Archive -LiteralPath $zip -DestinationPath $staging
        $top = @(Get-ChildItem -LiteralPath $staging -Directory)
        if ($top.Count -ne 1) { throw "Expected one top-level folder in $($pin.FileName), found $($top.Count)." }
        if (Test-Path -LiteralPath $installDir) {
            if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker -Force }
            $old = Join-Path $installRoot ('.old-' + [guid]::NewGuid().ToString('N'))
            try {
                Rename-Item -LiteralPath $installDir -NewName (Split-Path -Leaf $old)
            } catch {
                if ($hadVerifiedInstall) { Set-Content -LiteralPath $marker -Value $pin.Sha256 -Encoding ASCII }
                $old = $null
                throw "Could not replace $installDir; something is using its DLLs. Close Dusk, running tests and editors started from a dev-env shell, then run this script again. ($($_.Exception.Message))"
            }
        }
        try {
            Move-Item -LiteralPath $top[0].FullName -Destination $installDir
        } catch {
            if ($old) { Rename-Item -LiteralPath $old -NewName $assetName; $old = $null }
            throw
        }
    } finally {
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
        if ($old -and (Test-Path -LiteralPath $old)) { Remove-Item -LiteralPath $old -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

# 3 and 4. Check what Dusk needs, every run. Any failure removes the marker, so dev-env.ps1
#          never accepts an incomplete install.
try {
    $bin = Join-Path $installDir 'bin'
    $lib = Join-Path $installDir 'lib'
    $include = Join-Path $installDir 'include'
    $missing = @()
    foreach ($prefix in $pin.ShipDlls) {
        $dll = @(Get-ChildItem -LiteralPath $bin -Filter "$prefix*.dll" -ErrorAction SilentlyContinue)
        if ($dll.Count -ne 1) { $missing += "bin\$prefix*.dll" }
        $stem = $prefix.TrimEnd('-')
        if (-not (Test-Path -LiteralPath (Join-Path $lib "$stem.lib"))) { $missing += "lib\$stem.lib" }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $include 'libavcodec\avcodec.h'))) { $missing += 'include\libavcodec\avcodec.h' }
    if ($missing.Count -gt 0) { throw "The FFmpeg install is missing: $($missing -join ', ')." }

    # FFmpeg embeds its configure arguments as one NUL-terminated string; every LGPL-3 build
    # contains --enable-version3, so anchor on it and read out to the surrounding NULs.
    $avutil = @(Get-ChildItem -LiteralPath $bin -Filter 'avutil-*.dll')[0].FullName
    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($avutil))
    $anchor = $text.IndexOf('--enable-version3')
    if ($anchor -lt 0) { throw "No configuration string with --enable-version3 in $avutil; this is not the pinned LGPL-3 build." }
    $start = $text.LastIndexOf([char]0, $anchor) + 1
    $end = $text.IndexOf([char]0, $anchor)
    if ($end -lt 0) { $end = $text.Length }
    $config = $text.Substring($start, $end - $start)
    $absent = @($pin.RequiredConfig | Where-Object { -not $config.Contains($_) })
    $present = @($pin.ForbiddenConfig | Where-Object { $config.Contains($_) })
    if ($absent.Count -gt 0) { throw "The FFmpeg build lacks required options: $($absent -join ', ')." }
    if ($present.Count -gt 0) { throw "The FFmpeg build contains forbidden options: $($present -join ', ')." }
} catch {
    if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue }
    throw "$($_.Exception.Message) Run scripts\setup-ffmpeg.ps1 -Force to reinstall."
}

Set-Content -LiteralPath $marker -Value $pin.Sha256 -Encoding ASCII
Write-Host ''
Write-Host "FFmpeg ready: $installDir"
Write-Host "Configured with: $($pin.RequiredConfig -join ' ')"
Write-Host 'Next, in each shell you build or run Dusk from:  . .\scripts\dev-env.ps1'
