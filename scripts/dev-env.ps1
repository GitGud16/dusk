<#
.SYNOPSIS
    Sets up the current shell for building and running Dusk. Dot-source it:  . .\scripts\dev-env.ps1

.DESCRIPTION
    For this shell session only (nothing is written to the registry or your profile):
      - FFMPEG_DIR points at the pinned FFmpeg build in .deps\ffmpeg\ (ffmpeg-sys-next reads
        include\ and lib\ from it at build time);
      - the build's bin\ folder is put first on PATH, so Windows finds avcodec, avformat,
        avutil, swscale and swresample when `cargo run` or `cargo test` starts a binary;
      - LIBCLANG_PATH is set to LLVM's bin\ folder if it is unset and LLVM is installed in the
        default place (bindgen needs libclang.dll).
    Run scripts\setup-ffmpeg.ps1 once before the first use.
#>
if ($MyInvocation.InvocationName -ne '.') {
    Write-Warning 'Dot-source this script so it can change your shell:  . .\scripts\dev-env.ps1'
    return
}

$duskRepoRoot = Split-Path -Parent $PSScriptRoot
$duskPin = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'ffmpeg-pin.psd1')
$duskFfmpeg = Join-Path $duskRepoRoot ('.deps\ffmpeg\' + [IO.Path]::GetFileNameWithoutExtension($duskPin.FileName))
$duskMarker = Join-Path $duskFfmpeg '.dusk-pin-sha256'

# Accept the install only if setup-ffmpeg.ps1 verified this exact pin and the DLLs are there.
$duskVerified = (Test-Path -LiteralPath $duskMarker) -and
    (([string](Get-Content -LiteralPath $duskMarker -Raw)).Trim() -eq $duskPin.Sha256) -and
    (@(Get-ChildItem -LiteralPath (Join-Path $duskFfmpeg 'bin') -Filter 'avutil-*.dll' -ErrorAction SilentlyContinue).Count -eq 1)

if (-not $duskVerified) {
    Write-Warning "The pinned FFmpeg build is not installed or not verified. Run:  powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1"
} else {
    $env:FFMPEG_DIR = $duskFfmpeg
    $duskBin = Join-Path $duskFfmpeg 'bin'
    $duskPathParts = @($env:PATH -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ne $duskBin) })
    $env:PATH = (@($duskBin) + $duskPathParts) -join ';'
    Write-Host "FFMPEG_DIR = $env:FFMPEG_DIR (bin\ is first on PATH for this shell)"
}

if (-not $env:LIBCLANG_PATH) {
    $duskLlvm = Join-Path $env:ProgramFiles 'LLVM\bin'
    if (Test-Path -LiteralPath (Join-Path $duskLlvm 'libclang.dll')) {
        $env:LIBCLANG_PATH = $duskLlvm
        Write-Host "LIBCLANG_PATH = $env:LIBCLANG_PATH"
    } else {
        Write-Warning 'libclang.dll not found. Install LLVM (winget install LLVM.LLVM) or set LIBCLANG_PATH.'
    }
} else {
    Write-Host "LIBCLANG_PATH = $env:LIBCLANG_PATH (already set)"
}

Remove-Variable duskRepoRoot, duskPin, duskFfmpeg, duskMarker, duskVerified, duskBin, duskPathParts, duskLlvm -ErrorAction SilentlyContinue
