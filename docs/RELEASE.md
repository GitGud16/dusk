# Dusk — Release

What ships and how a release is built and checked: the logo and icon, the theme every view follows, the About dialog, the installer, the licenses, and the docs. How to build a release on a machine is in [SETUP.md](SETUP.md), "Release"; why the release is the way it is, in [DECISIONS.md](DECISIONS.md), "Release"; the download, installed size and cold start against their budgets, in [REQUIREMENTS.md](REQUIREMENTS.md). Like [ARCHITECTURE.md](ARCHITECTURE.md), this file says what Dusk does now.

## Logo and icon

The logo of THEME.md, the four brand colors as horizontal bands (violet on top, then magenta, pink and a thin gold stripe) in a rounded square, is drawn by `scripts/make-logo.py` from its geometry with Python's standard library alone. It writes `dusk-app/assets/dusk.ico` at each size Windows asks an icon for (16, 20, 24, 32, 40, 48, 64 and 256 pixels), each drawn on whole pixels rather than scaled down; `dusk-app/assets/dusk.svg`, the mark as shapes, which the windows' title bars and the toolbar (beside the wordmark) show at the size asked for; and `docs/images/logo.png`, the README's. `dusk-app/build.rs` writes the icon and the version information (what the file is, its version from `Cargo.toml`, its license) into a resource file, which the MSVC linker takes like an object file, so the build needs no resource compiler and no crate for it; the icon shows in Explorer, the taskbar and the installer's shortcuts, and a test reads both back from the built `dusk.exe` with Windows' own calls. `dusq.exe`, a command-line tool, keeps Windows' plain icon.

## Theme

Every view (the main window, the clip editor, every dialog) takes every color, font size, weight, border and radius from the theme's tokens (THEME.md); the two pill shapes, the export's progress bar and the clip editor's trim handles, are rounded by half their own size. Sizes that belong to one view's layout, such as a list's row height or a dialog's width, are written in that view. Both status lines show three kinds of message: what happened in `text-2`, warnings (an edit that did more than was asked, such as a cut at the gap) in `signal`, and errors in `alert`.

## About

Help → About Dusk (Shift+F1) opens a dialog (`about`): the mark and the wordmark, the version, the MIT license, what Dusk is built with (Slint, with the `AboutSlint` widget its license asks for; FFmpeg under the LGPL, linked dynamically; Inter and JetBrains Mono under the OFL), a button that opens the third-party notices installed beside Dusk, and one that opens the releases page in the browser. The system opens both, so Dusk itself still makes no request (REQUIREMENTS.md, "Offline and private"). The releases page goes through Slint's own `Platform.open-url`, which hands the link to the system (the `webbrowser` crate Slint already uses, on `ShellExecuteW`) and says whether it took; when it did not, the status line gives the address. *Licenses* shows the `licenses` folder beside `dusk.exe` in Explorer and appears only when that folder is there, which a development build lacks. Dusk sets Slint's palette to dark at start, so `AboutSlint` shows its dark-background logo whatever Windows' own choice.

## Installer

`installer/dusk.iss` builds one Inno Setup 6 installer with solid LZMA2 compression: `dusk.exe`, `dusq.exe` and the five FFmpeg DLLs beside them, so nothing depends on PATH, and a `licenses` folder. It installs for the current user without administrator rights (`%LOCALAPPDATA%\Programs\Dusk`), or for all users when asked, adds a Start menu entry (and a desktop one if asked), opens `.dusk` projects with Dusk if asked, and uninstalls cleanly; settings and autosaves are the user's data and stay. Neither the installer nor the binaries are code-signed, so Windows' SmartScreen warns about the download; the README says how to get past it, and signing is planned (ROADMAP.md, M7).

## Licenses

The `licenses` folder holds Dusk's MIT license, FFmpeg's LGPL (with the GPL it builds on), the fonts' OFL, Slint's license, and:

- `THIRD-PARTY-NOTICES.txt`, the license of every Rust crate in the two binaries, written by `scripts/third-party-notices.py` from `cargo metadata` (asked for Windows' packages alone) and the crates' own license files;
- `FFmpeg.txt`, which names Dusk's own FFmpeg build with its SHA-256 and release, FFmpeg's commit and the source tarball published beside the build, the BtbN image and scripts it was compiled with, and the configuration, which the release script reads from the avutil DLL itself;
- `FFmpeg-libraries.txt` (from `installer/ffmpeg-libraries.txt`), the license texts of the libraries built into FFmpeg's DLLs. `scripts/ffmpeg-licenses.py` fetches them for the pinned build: it asks BtbN's build scripts at the commit `scripts/ffmpeg-source.txt` names which libraries the build contains, with their own logic (`scripts/btbn-components.sh`), leaves out those Dusk's build drops, and reads each one's license files at the commit built, from its own repository. One of them, librsvg, is written in Rust, so the file also lists the crates its build for Windows depends on as cargo resolves it, with the code that lists Dusk's own (SETUP.md, "Release").

## Building and checking it

`scripts/build-release.ps1` builds both binaries in release (dusq on its own, as CI does), stages the files and runs the Inno Setup compiler. CI builds the installer on every push to a branch and keeps it as an artifact, and a pushed `v*` tag that names the version in `Cargo.toml` makes a draft release with the installer attached, for the maintainer to publish. `scripts/check-installer.ps1` tries every installer CI builds: it installs it silently into a folder of its own, checks that every staged file arrived, that the Start menu has Dusk and that `.dusk` files open with it, compresses a clip with the installed `dusq` and opens the installed `dusk.exe` with no FFmpeg on PATH, then uninstalls and checks that nothing is left. It refuses to run where Dusk is installed, since it ends by uninstalling it. The cold start, the download and the installed size are measured against REQUIREMENTS.md.

## Docs

The README says what Dusk is, with screenshots of a demo project made from FFmpeg's own test sources (a gradient, a Mandelbrot set, a game of life, a chord), how to install it and how a first edit goes. `docs/SHORTCUTS.md` lists every action with its keys; a test in `keymap` writes it from `Keymap` when `DUSK_WRITE_SHORTCUTS` is set and otherwise fails when the two differ. `CONTRIBUTING.md` gives contributors the rules from CLAUDE.md (setup, the checks CI runs, code style, commits), and SETUP.md says how to build the installer.
