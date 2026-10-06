<#
.SYNOPSIS
    Tries the installer as a user would, as CI does after building it: installs it silently into
    a folder of its own, checks what it put where, runs both programs from there with no FFmpeg on
    PATH, uninstalls it and checks that nothing is left.
    From the repository root, after scripts\build-release.ps1:
    powershell -ExecutionPolicy Bypass -File scripts\check-installer.ps1

.DESCRIPTION
    Meant for machines without Dusk, such as CI's: while it runs, Dusk is in the current user's
    Start menu and opens .dusk files, and it ends by uninstalling Dusk. It refuses to run where
    Dusk is installed. It prints the download and installed sizes (docs/REQUIREMENTS.md).

.PARAMETER Setup
    The installer to check; by default the one in target\installer.
#>
param([string]$Setup = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $root 'target\release-stage\Dusk'
if (-not $Setup) {
    $found = @(Get-ChildItem -LiteralPath (Join-Path $root 'target\installer') -Filter 'dusk-*-setup.exe' -ErrorAction SilentlyContinue)
    if ($found.Count -ne 1) { throw 'Expected one installer in target\installer: run scripts\build-release.ps1 with Inno Setup first, or pass -Setup.' }
    $Setup = $found[0].FullName
}

# The id the installer registers Dusk under, from the installer's own script.
$iss = Get-Content -LiteralPath (Join-Path $root 'installer\dusk.iss')
$appId = ($iss | Select-String -Pattern '^AppId=\{(\{[0-9A-Fa-f-]+\})').Matches[0].Groups[1].Value
$uninstallKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\${appId}_is1"
foreach ($hive in 'HKCU:', 'HKLM:') {
    if (Test-Path -LiteralPath "$hive\$uninstallKey") {
        throw 'Dusk is installed on this computer, and this check ends by uninstalling it; run it where Dusk is not installed, as CI does.'
    }
}

# A folder of the check's own, with no short (8.3) names in it, so paths compare as written.
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $temp 'dusk-installer-check'
$dir = Join-Path $work 'Dusk'
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Force -Path $work | Out-Null
$log = Join-Path $work 'install.log'

'Download {0}: {1:N1} MB' -f (Split-Path -Leaf $Setup), ((Get-Item -LiteralPath $Setup).Length / 1e6)
$p = Start-Process -FilePath $Setup -Wait -PassThru -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`""
if ($p.ExitCode -ne 0) {
    Get-Content -LiteralPath $log -Tail 30 -ErrorAction SilentlyContinue
    throw "The installer failed with exit code $($p.ExitCode)."
}
$files = @(Get-ChildItem -LiteralPath $dir -Recurse -File)
'Installed {0}: {1:N1} MB in {2} files' -f $dir, (($files | Measure-Object -Property Length -Sum).Sum / 1e6), $files.Count

# Every staged file, as staged.
$wrong = @()
foreach ($file in Get-ChildItem -LiteralPath $stage -Recurse -File) {
    $relative = $file.FullName.Substring($stage.Length + 1)
    $there = Join-Path $dir $relative
    if (-not (Test-Path -LiteralPath $there) -or (Get-Item -LiteralPath $there).Length -ne $file.Length) { $wrong += $relative }
}
if ($wrong) { throw "Not installed as staged: $($wrong -join ', ')." }

# The Start menu entry and the .dusk association, for the current user.
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Dusk.lnk'
if (-not (Test-Path -LiteralPath $shortcut)) { throw "There is no Start menu entry at $shortcut." }
$classes = 'HKCU:\Software\Classes'
if ((Get-ItemProperty -LiteralPath "$classes\.dusk").'(default)' -ne 'Dusk.Project') { throw '.dusk files are not registered as Dusk projects.' }
$open = (Get-ItemProperty -LiteralPath "$classes\Dusk.Project\shell\open\command").'(default)'
if ($open -ne ('"{0}" "%1"' -f (Join-Path $dir 'dusk.exe'))) { throw "Opening a .dusk file runs $open." }

# Both programs, from the installed folder alone.
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and $_ -notmatch 'ffmpeg' }) -join ';'
Remove-Item Env:FFMPEG_DIR -ErrorAction SilentlyContinue
$dusq = Join-Path $dir 'dusq.exe'
& $dusq --version
if ($LASTEXITCODE -ne 0) { throw "dusq did not start from the installed folder (exit code $LASTEXITCODE)." }
$compressed = Join-Path $work 'compressed.mp4'
& $dusq compress (Join-Path $root 'testdata\sample-h264-aac.mp4') -o $compressed --quality small
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $compressed)) { throw "dusq could not compress a video from the installed folder (exit code $LASTEXITCODE)." }

$env:DUSK_STATE_DIR = Join-Path $work 'state'
$env:DUSK_SETTINGS_DIR = Join-Path $work 'settings'
$dusk = Start-Process -FilePath (Join-Path $dir 'dusk.exe') -PassThru
$null = $dusk.Handle  # keeps the exit code readable once it exits, in Windows PowerShell too
$clock = [Diagnostics.Stopwatch]::StartNew()
# Its window is the one titled "Untitled — Dusk"; an untitled helper window of winit's shows first.
while ($dusk.MainWindowTitle -notlike '*Dusk' -and -not $dusk.HasExited -and $clock.Elapsed.TotalSeconds -lt 60) {
    Start-Sleep -Milliseconds 100
    $dusk.Refresh()
}
if ($dusk.HasExited) { throw "dusk.exe exited with code $($dusk.ExitCode) before showing its window." }
if ($dusk.MainWindowTitle -notlike '*Dusk') { $dusk.Kill(); throw 'dusk.exe showed no window within a minute.' }
'dusk.exe showed its window after {0:N1} s' -f $clock.Elapsed.TotalSeconds
$null = $dusk.CloseMainWindow()
if (-not $dusk.WaitForExit(30000)) { $dusk.Kill(); throw 'dusk.exe did not close when its window was closed.' }
if ($dusk.ExitCode -ne 0) { throw "dusk.exe closed with exit code $($dusk.ExitCode)." }

# Uninstalling. The uninstaller hands over to a copy of itself in the temporary folder, which
# removes the folder it ran from; wait for that.
$p = Start-Process -FilePath (Join-Path $dir 'unins000.exe') -Wait -PassThru -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
if ($p.ExitCode -ne 0) { throw "The uninstaller failed with exit code $($p.ExitCode)." }
$clock.Restart()
while ((Test-Path -LiteralPath $dir) -and $clock.Elapsed.TotalSeconds -lt 60) { Start-Sleep -Milliseconds 250 }
$left = @($dir, $shortcut, "$classes\.dusk", "$classes\Dusk.Project", "HKCU:\$uninstallKey") | Where-Object { Test-Path -LiteralPath $_ }
if ($left) { throw "Uninstalling left $($left -join ', ')." }
Remove-Item -LiteralPath $work -Recurse -Force
'Uninstalled, and nothing is left.'
