# Dusk — Roadmap

Milestones, not dates. Each one ends with something that runs. Memory and responsiveness targets from REQUIREMENTS.md are checked at every milestone, not at the end.

## M0 — Foundation

- Cargo workspace with `dusk-core`, `dusk-media`, `dusk-render`, `dusk-audio`, `dusk-engine`, `dusk-app`, `dusk-cli` (`dusq`).
- FFmpeg (the pinned BtbN LGPL shared 8.1 build from `scripts/ffmpeg-pin.psd1`, installed by `scripts/setup-ffmpeg.ps1`) linked and loading on Windows; a smoke test prints a file's streams. Confirm the build has `lcms2` (hard requirement for photo color), `libopenh264`, `libkvazaar`, `libsvtav1` and `libvpx`. `docs/SETUP.md` records the steps (download, `FFMPEG_DIR`, LLVM) and the NVENC driver floor (570) that the pin implies. `crt-static` set.
- Ship set is avcodec, avformat, avutil, swscale, swresample (measured 120–130 MB installed, 51–54 MB zipped); confirm with the pinned build, confirm `libkvazaar` is present, and pick NSIS or Inno Setup. SETUP.md covers `FFMPEG_DIR` and LLVM for `bindgen`.
- Slint used under the Royalty-free License 2.0; an About dialog stub with the `AboutSlint` widget exists from day one.
- Git: `main` branch, first commit contains the docs and CLAUDE.md.
- Slint window opens with the femtovg-wgpu renderer (Slint 1.18.1, `unstable-wgpu-30`); the Dusk dark theme tokens defined in `.slint`.
- CI builds on Windows; `cargo fmt --check`, `cargo clippy` and `cargo test` pass for the workspace and for `dusk-engine` with `--no-default-features`; `dusk-cli` builds alone and the wgpu check passes.
- MIT license, README with the one-paragraph vision.

**Done when**: `cargo run` opens an empty, themed Dusk window and the test suite probes a sample MP4.

## M1 — Vertical slice

Import one file, see it as a linked video clip and audio clip on one video track and one audio track, play it with sound, scrub, trim, export.

1. Decode one frame (D3D11VA, software fallback) and show it in the preview through a `wgpu` texture. If 60 fps texture updates don't hold on integrated graphics, switch to the pre-approved fallback (GPU readback of the composited RGBA texture into a Slint `SharedPixelBuffer`) and record the decision in ARCHITECTURE.md.
2. Playback with audio as the clock; play/pause, scrub, frame-step; J/K/L.
3. Timeline with a ruler, a playhead, one video track and one audio track, and one link group with draggable trim handles that move both clips.
4. Export to MP4 (hardware encoder or software fallback) with a progress bar and cancel; `.part` file and rename.
5. Measure: idle memory, memory during and 10 s after 5 minutes of playback, scrub latency on 1080p, download and installed size.

**Done when**: a 1080p clip plays smoothly with sound, can be trimmed, and exports to a playable MP4. Memory stays within the idle, during-playback and after-playback budgets in REQUIREMENTS.md.

## M2 — A real timeline

- 2 video + 2 audio tracks; move, trim, split at playhead, delete with and without ripple (ripple shifts all unlocked tracks). Track lock and mute.
- Linked video + audio clips on import, kept in lock-step (shared trim, position, length, speed); unlink; Alt+Delete; per-clip enable/disable; still images with duration, decoded then scaled through swscale into NV12 (full-chroma entry for native-size export); grid HEIC stitching tile by tile via the ffi module; full 8-way orientation (from the first decoded frame for every still format, display matrix for video, stream group for HEIC) and fit/fill; ICC-tagged wide-gamut photos handled (flag only for RGB profiles, retry without on failure, grid profiles via ffi), progressive-JPEG peak, texture-size clamp; VFR snapped to standard rates.
- Sequence settings dialog (frame rate, resolution) with the match-first-clip prompt and the rate-change conversion.
- Decoder pool with the visibility rule and size budget; 4K single-decoder rule.
- Per-clip volume, mute, fades; detach audio.
- Undo/redo for everything. Project save/load (JSON). Autosave and crash recovery.
- Media bin with thumbnails; drag and drop import; WhatsApp/Telegram audio formats verified.
- Variable-speed playback (0.1x to 32x, both directions; slow reverse via GOP buffer, fast via keyframes).

**Done when**: a three-clip edit with music survives save, quit, reopen, and undo history behaves.

## M3 — Pop-out clip editor (signature feature)

- Click a clip → second window previewing its whole link group (video + audio).
- Trim, crop, rotate/flip, fit/fill, speed, volume and fades, still duration in the pop-out, applied as one undoable `ApplyClipSession` command.
- *Apply to project* (undoable) and *Export as file*.
- Both windows stay responsive; edits in the pop-out preview live. Stale-session banner and deleted-clip handling work.

**Done when**: you can fix one clip in the pop-out and see the main timeline update, without ever leaving the project.

## M4 — Export, compress, extract

- Export dialog: container (MP4/MKV/MOV, WebM as VP9/AV1), resolution presets, Quality 0–100 mapped per encoder, Advanced target bitrate for any encoder and CRF only where the encoder has one (SVT-AV1, VP9), encoders shown by availability.
- Compress tool: open a single file, target quality or target size (bitrate ladder + one corrective re-encode), export, no project.
- Optional external GPL `ffmpeg.exe` export path (raw frames piped).
- Encoder probing by real session at 640×480 on first export, fall-through on open failure, per-encoder limits table with fixed values; per-encoder memory measured (SVT-AV1 threads and lookahead fixed) and the encoder line in REQUIREMENTS.md updated.
- CPU path in dusq (normalize → tone-map/rotate in Rust → encoder format) verified against the GPU path by the per-plane PSNR test (≥ 45 dB unscaled, ≥ 40 dB scaled); `--threads`; dusq memory ceiling measured per size class and recorded in REQUIREMENTS.md.
- Audio-only export (MP3/AAC/Opus/WAV).
- `dusq` (the `dusk-cli` binary) with `compress` and `extract-audio`, built without the `gpu` feature on the CPU transcode path.

**Done when**: a 2 GB phone video can be compressed to a 25 MB file from the compress tool in one dialog.

## M5 — Keyboard and settings

- Every common action has a shortcut; shortcuts are remappable and stored in a plain text file.
- Shortcut list panel (`?` key and a toolbar button), searchable.
- Settings: cache size, default export preset, optional external `ffmpeg.exe` path.

**Done when**: a full edit can be done without touching the mouse except for dragging clips.

## M6 — Release 0.1

- Theme and icon polish against THEME.md; app icon and logo. About dialog with `AboutSlint`, license notices, and a link to the releases page.
- Missing-media relink dialog. Error messages that say what to do.
- Installer (NSIS or Inno Setup with LZMA, or a 7z self-extracting archive; not MSI) containing `dusk.exe`, `dusq.exe`, the five FFmpeg DLLs and the third-party license files (the bundled fonts' OFL requires shipping theirs); startup time, download and installed size measured against targets.
- Docs: README with screenshots, shortcuts reference, build instructions, contributing guide.

**Done when**: a stranger can download Dusk, open a clip, cut it, and export it without asking a question.

## 0.2

- Slim custom FFmpeg build (only the demuxers, decoders and encoders Dusk uses) to reach the ~60 MB installed target.
- Zero-copy hardware decode (D3D11 surfaces shared with wgpu) if measurements show the copy matters.

## Later (from FEATURES.md)

Transitions, titles, color adjustments, picture-in-picture, keyframes, waveforms on the timeline, snapping and markers, 4K proxies, HDR passthrough, subtitles, export queue, GIF export, Linux and macOS builds.
