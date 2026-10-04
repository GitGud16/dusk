# Dusk

Dusk is a free, open-source, native desktop video editor built in Rust. It exists because the good editors are either paid, or browser-based and full of ads. It must be light: light to install, light to open, and light on RAM and CPU. No accounts, no cloud, no ads, no telemetry. The one thing it does better than anything else is in-place clip editing: click any clip in the timeline and a pop-out window opens for light editing of just that clip (trim, crop, rotate, speed, volume), and the result is applied back into the project or saved as its own file.

**Status**: early development, milestone M0 (foundation). Windows first; Linux and macOS later. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

Windows 10/11 with Rust, LLVM and the pinned FFmpeg build; [docs/SETUP.md](docs/SETUP.md) has the details. In short:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1   # once
. .\scripts\dev-env.ps1                                             # in every new shell
cargo run -p dusk-app
```

## Design

[docs/VISION.md](docs/VISION.md), [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/FEATURES.md](docs/FEATURES.md) and [docs/THEME.md](docs/THEME.md).

## License

Dusk's own code is MIT licensed ([LICENSE](LICENSE)). It links the LGPL build of FFmpeg dynamically and uses Slint under the Slint Royalty-free License 2.0.
