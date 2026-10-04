# Dusk — Vision

## Vision

**Dusk** is a free, open-source, native desktop video editor built in Rust. The name comes from its palette: deep violet overhead, magenta and pink at the horizon, a last stripe of gold. It exists because the good editors are either paid, or browser-based and full of ads. It is built first for its author's own daily use, then shared with anyone who wants it. It must be light: light to install, light to open, and light on RAM and CPU. No accounts, no cloud, no ads, no telemetry.

The one thing it does better than anything else is **in-place clip editing**: click any clip in the timeline and a pop-out window opens for light editing of just that clip (trim, crop, rotate, speed, volume). The result is applied back into the project, or saved as its own file, without ever leaving the project or creating a second one.

## Scope guardrails (what it is not)

- Not a Premiere / DaVinci competitor. No VFX, no motion graphics, no color grading suite.
- Not a web app, not a subscription, not a cloud service.
- Not a plugin platform (at least not at first). Features are built in, few, and fast.

## Core promises

1. **Light**: small binary, fast start, low memory. Measured, not assumed.
2. **In-place clip editing**: edit one clip in a pop-out window; apply back or export.
3. **Merge, export, compress**: combine clips with audio, export to multiple formats and qualities, or compress a single file.
4. **Yours**: runs fully offline, project files are plain text you own.

## Decisions so far

- **Name**: **Dusk** everywhere users see it (app, window title, docs, binary `dusk.exe`). **dusq** is the command-line binary (`dusq compress video.mp4`) and the handle wherever `dusk` is taken: crate names, GitHub org / repo, domain, social handles.
- **Language**: Rust (no C++).
- **UI**: Slint under the Slint Royalty-free License 2.0 (attribution in the About dialog and on the download page), with WGPU rendering for the video preview.
- **Media engine**: FFmpeg (LGPL shared build) via `ffmpeg-next` bindings.
- **GPU**: `wgpu` for frame rendering and preview.
- **Platform**: Windows first; Linux and macOS later.
- **License**: own code under MIT. No GPL dependencies in the default build (no x264/x265); encode with hardware encoders (NVENC / QSV / AMF), OpenH264, Kvazaar and SVT-AV1. GPL encoders only through an optional, user-supplied external `ffmpeg.exe`.
