# Dusk — Setup (Windows)

Everything needed to build and run Dusk from a fresh checkout. Every new setup step goes here.

## Prerequisites

| Tool | Version | Why |
|------|---------|-----|
| Windows 10/11, x64 | | Target platform |
| Rust, stable, MSVC toolchain (`x86_64-pc-windows-msvc`) | 1.92 or newer | Minimum for Slint 1.18 |
| Visual Studio Build Tools, "Desktop development with C++" | 2022 or newer | MSVC linker for the Rust MSVC target |
| LLVM | 20 or newer | `bindgen` (used by `ffmpeg-sys-next`) needs `libclang.dll`. `winget install LLVM.LLVM` |
| PowerShell | 5.1 (built in) or 7 | Setup scripts |

Check with `rustc --version` and `Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll"`.

## FFmpeg (pinned)

Dusk links one exact FFmpeg build. The pin lives in `scripts/ffmpeg-pin.psd1`, which the scripts read; this table mirrors it.

| | |
|---|---|
| Source | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), win64, LGPL, shared |
| Release tag | `autobuild-2026-10-03-18-14` |
| File | `ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1.zip` |
| Size | 80,991,898 bytes |
| SHA-256 | `11a4b44bc69721274909619a779d82544c8b83a6557c5b1be92dce9a41a968be` |
| License | LGPL-3.0-or-later (BtbN configures with `--enable-version3`) |
| NVENC driver floor | 570 (GTX 900/1000 cards still qualify; their last driver branch is 580) |

Why this file: it is the first 8.1 build with `lcms2`, which photo color needs (BtbN enabled it on 2026-10-02). BtbN keeps only its 14 newest builds, plus the last build of each month for two years, so this file disappears around 2026-10-17. Keep the downloaded copy (the setup script leaves it in `%LOCALAPPDATA%\dusk\ffmpeg-cache\`), and move the pin to the 8.1 asset of the last October 2026 build (normally `autobuild-2026-10-31-*`) once it is published.

The setup script refuses any file whose size or SHA-256 differs, and refuses a build that lacks `--enable-lcms2`, `--enable-libopenh264`, `--enable-libkvazaar`, `--enable-libsvtav1`, `--enable-libvpx` or `--enable-version3`, or that contains `--enable-gpl`, `--enable-libx264` or `--enable-libx265`. It reads these from the configuration string compiled into `avutil`; it never runs an FFmpeg executable.

Do not point `FFMPEG_DIR` at any other FFmpeg on your machine (for example a Chocolatey `ffmpeg`): those are usually GPL builds without import libraries.

## Steps

One time, from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1
```

This downloads the pinned zip (about 81 MB) into the cache, verifies it, and unpacks it to `.deps\ffmpeg\ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1\` (`bin\`, `include\`, `lib\`). `.deps\` is git-ignored. If BtbN has already deleted the file, pass a kept copy: `-ZipPath C:\path\to\the.zip` (it must keep the `.zip` extension; the script copies it into the cache).

In every shell you build or run Dusk from:

```powershell
. .\scripts\dev-env.ps1
```

If the execution policy blocks it, run `Set-ExecutionPolicy -Scope Process Bypass` in that shell first. Then `cargo build`, `cargo run -p dusk-app`, `cargo test --workspace` work as listed in `CLAUDE.md`.

## How Windows finds the FFmpeg DLLs

Dusk links FFmpeg dynamically, so `avcodec`, `avformat`, `avutil`, `swscale` and `swresample` must be found when a Dusk binary starts. Windows looks in the executable's own folder first, then in the Windows system folders and the current directory, then in the folders on PATH.

- **Development**: `dev-env.ps1` puts the pinned build's `bin\` first on PATH for that shell, and `cargo run` and `cargo test` inherit it. Without it, binaries fail to start with a missing-DLL error.
- **IDEs and other launchers**: start them from a shell that ran `dev-env.ps1` (for example `code .`). To make it permanent for your user instead, run this once from the repository root (Dusk's scripts never change persistent settings). It keeps your user PATH as an expandable value and does not add the folder twice. Run it again after every pin change, because the folder name changes:

  ```powershell
  $dir = (Resolve-Path '.deps\ffmpeg\ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1').Path
  $userPath = (Get-Item 'HKCU:\Environment').GetValue('Path', '', 'DoNotExpandEnvironmentNames')
  if (($userPath -split ';') -notcontains "$dir\bin") {
      Set-ItemProperty 'HKCU:\Environment' -Name Path -Value "$dir\bin;$userPath" -Type ExpandString
  }
  [Environment]::SetEnvironmentVariable('FFMPEG_DIR', $dir, 'User')   # last: this call also tells Windows the environment changed
  ```

- **Release (M6)**: the installer puts the five DLLs next to `dusk.exe` and `dusq.exe`, so nothing depends on PATH.

## Changing the FFmpeg pin

Changing the pin is a documented decision, not a bump.

1. Pick the new asset on the BtbN releases page; prefer the last build of a month (normally `autobuild-YYYY-MM-<last day>-*`), which BtbN keeps for two years, over a daily build, which it deletes after about two weeks.
2. Update `Tag`, `FileName`, `Url`, `Size` and `Sha256` in `scripts/ffmpeg-pin.psd1`. GitHub shows each asset's `sha256` digest on the release page and in the releases API.
3. Run `scripts\setup-ffmpeg.ps1`; it rejects the build if a required library is missing.
4. Update the table above, the NVENC driver floor (BtbN builds after 8.1 use newer NVENC headers that need driver 610 or newer), and the "FFmpeg (Windows)" section of `docs/ARCHITECTURE.md`.

## CI

CI runs the same `scripts\setup-ffmpeg.ps1`, caching `%LOCALAPPDATA%\dusk\ffmpeg-cache\`, then exports `FFMPEG_DIR` and adds the build's `bin\` to the path for later steps. Hosted Windows runners already have LLVM. A second job builds the installer (below) on every push, installing Inno Setup with Chocolatey when the runner lacks it, tries it with `scripts\check-installer.ps1` and keeps it as the `dusk-installer` artifact.

## Release

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
```

This builds `dusk.exe` and `dusq.exe` in release (dusq on its own), stages `target\release-stage\Dusk`, the folder the installer installs (the two programs, the five FFmpeg DLLs and `licenses\`), and, when [Inno Setup 6](https://jrsoftware.org/isinfo.php) is installed (`winget install JRSoftware.InnoSetup`), compiles `installer\dusk.iss` into `target\installer\dusk-<version>-setup.exe`. It needs Python 3, which writes `licenses\THIRD-PARTY-NOTICES.txt` from cargo's own record of the crates in both programs (`scripts\third-party-notices.py`), and it copies `installer\ffmpeg-libraries.txt`, the licenses of the libraries built into FFmpeg's DLLs, to `licenses\FFmpeg-libraries.txt`. It refuses to go on when the shell's `FFMPEG_DIR` names another build than the pinned one (a shell set up for an older pin), or when `ffmpeg-libraries.txt` was written for another build. It removes earlier installers from `target\installer` first, and without Inno Setup it stops after staging, which is enough to measure the installed size and to run Dusk the way the installer leaves it. The version is the workspace's, in `Cargo.toml`; a pre-release such as `0.1.0-rc.1` keeps its numbers alone as the installer's file version, which Windows wants as numbers.

`scripts\check-installer.ps1` then tries the installer as a user would: it installs it silently into a folder of its own, checks that every staged file arrived, that the Start menu has Dusk and that `.dusk` files open with it, runs `dusq` (a small compression) and `dusk.exe` from there with no FFmpeg on the path, uninstalls, and checks that nothing is left. It tries the installer of the version in `Cargo.toml` (or `-Setup`), prints the download and installed sizes, and uninstalls Dusk on its way out when a check fails. Since it ends by uninstalling Dusk, it refuses to run on a computer where Dusk is installed; CI builds and tries the installer for every push to a branch.

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`, which refuses a tag that does not name the version in `Cargo.toml`, builds the installer and drafts a GitHub release with it. Before publishing one:

- Check that `installer\ffmpeg-libraries.txt` belongs to the pinned FFmpeg. The BtbN download carries none of the licenses of the libraries built into its DLLs, so `python scripts\ffmpeg-licenses.py` fetches them: it takes BtbN's build scripts at the pinned release tag, asks them which libraries the win64 LGPL shared build of that FFmpeg version contains (`scripts\btbn-components.sh`, with the scripts' own logic), and reads each library's license files at the commit built, from its own repository. Two of those libraries, rav1e and librsvg, are written in Rust and carry Rust crates of their own: the script checks out their Cargo files and sources at the commit, lets cargo resolve their builds for Windows in a cargo folder of its own (fetching some tens of MB of crates, removed afterwards), and lists the crates' licenses as `scripts\third-party-notices.py` does Dusk's. It needs the network, Git's bash, cargo and an authenticated `gh`; it asks again when a connection fails, and stops with what failed rather than write a shorter list. Run it, and commit the result, whenever the pin moves.
- Make FFmpeg's exact source available beside the release, as its LGPL asks: `FFmpeg.txt` points at the commit and BtbN's build scripts, and attaching FFmpeg's source archive for that commit to the release is the surest way.
- Put the Slint badge on the download page, as Slint's Royalty-free License asks.
- Neither the installer nor the programs are code-signed, so Windows' SmartScreen warns on the first run; the README says how to get past it.
