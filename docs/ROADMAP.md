# Dusk — Roadmap

Milestones, not dates. Each one ends with something that runs. Memory and responsiveness targets from REQUIREMENTS.md are checked at every milestone, not at the end.

## M0 — Foundation

- Cargo workspace with `dusk-core`, `dusk-media`, `dusk-render`, `dusk-audio`, `dusk-engine`, `dusk-app`, `dusk-cli` (`dusq`).
- FFmpeg (the pinned BtbN LGPL shared 8.1 build from `scripts/ffmpeg-pin.psd1`, installed by `scripts/setup-ffmpeg.ps1`) linked and loading on Windows; a smoke test prints a file's streams. Confirm the build has `lcms2` (hard requirement for photo color), `libopenh264`, `libkvazaar`, `libsvtav1` and `libvpx`. `docs/SETUP.md` records the steps (download, `FFMPEG_DIR`, LLVM) and the NVENC driver floor (570) that the pin implies. `crt-static` set.
- Ship set is avcodec, avformat, avutil, swscale, swresample (measured 120–130 MB installed, 51–54 MB zipped); confirm with the pinned build (130.3 MB installed, 54.3 MB zipped at M0), confirm `libkvazaar` is present, and pick NSIS or Inno Setup (picked: Inno Setup). SETUP.md covers `FFMPEG_DIR` and LLVM for `bindgen`.
- Slint used under the Royalty-free License 2.0; an About dialog stub with the `AboutSlint` widget exists from day one.
- Git: `main` branch, first commit contains the docs and CLAUDE.md.
- Slint window opens with the femtovg-wgpu renderer (Slint 1.18.1, `unstable-wgpu-30`); the Dusk dark theme tokens defined in `.slint`.
- CI builds on Windows; `cargo fmt --check`, `cargo clippy` and `cargo test` pass for the workspace and for `dusk-engine` with `--no-default-features`; `dusk-cli` builds alone and the wgpu check passes.
- MIT license, README with the one-paragraph vision.

**Done when**: `cargo run` opens an empty, themed Dusk window and the test suite probes a sample MP4.

## M1 — Vertical slice

Import one file, see it as a linked video clip and audio clip on one video track and one audio track, play it with sound, scrub, trim, export. In M1 the file is opened from the command line (`dusk.exe clip.mp4`), which needs no new dependency; drag-and-drop import arrives with the media bin in M2.

1. Startup: `dusk-render` creates the wgpu device and hands it to Slint (Vulkan first, DX12 fallback; see ARCHITECTURE.md, Slint specifics), to bring the first window under the 2 s cold-start target (about 3.0 s at M0). Done: 0.76–0.88 s.
2. Decode one frame (in software for 0.1; see ARCHITECTURE.md, "Decoder pool") and show it in the preview through a `wgpu` texture. If 60 fps texture updates don't hold on integrated graphics, switch to the pre-approved fallback (GPU readback of the composited RGBA texture into a Slint `SharedPixelBuffer`) and record the decision in ARCHITECTURE.md. Done: the texture path stays (57–60 fps of a 1080p60 clip on a discrete GPU); integrated graphics is measured with `DUSK_STATS` when one is available.
3. Playback with audio as the clock; play/pause, scrub, frame-step; J/K/L. Done. L doubles the speed up to 8x (sound up to 4x); J plays backwards at 1x without sound, decoding each group of pictures into the frame cache. Faster and audible reverse, and keyframe stepping for long GOPs, come with the variable-speed playback of M2.
4. Timeline with a ruler, a playhead, one video track and one audio track, and one link group with draggable trim handles that move both clips. Done, with undo and redo and a central shortcut table that the Help menu lists.
5. Export to MP4 (hardware encoder or software fallback) with a progress bar and cancel; `.part` file and rename. Done. Until the export dialog of M4 the file goes beside the source as "<name> export.mp4" (numbered, never replacing a file) at the High quality preset, and the first H.264 encoder in the order that opens at the export's size is used (the 640×480 probe comes with M4). Closing Dusk mid-export cancels it and removes the part file.
6. Measure: idle memory, memory during and 10 s after 5 minutes of playback, scrub latency on 1080p, download and installed size. Done; the figures are in REQUIREMENTS.md. The after-playback budget now counts the preview's working set, which graphics drivers make cheaper to keep than to free (REQUIREMENTS.md, "After playback stops").

**Done when**: a 1080p clip plays smoothly with sound, can be trimmed, and exports to a playable MP4. Memory stays within the idle, during-playback and after-playback budgets in REQUIREMENTS.md.

## M2 — A real timeline

- 2 video + 2 audio tracks; move, trim, split at playhead, delete with and without ripple (ripple shifts all unlocked tracks). Track lock and mute. Done.
- Linked video + audio clips on import, kept in lock-step (shared trim, position, length, speed); unlink; Alt+Delete; per-clip enable/disable; still images with duration, decoded then scaled through swscale into NV12 (full-chroma entry for native-size export); grid HEIC stitching tile by tile via the ffi module; full 8-way orientation (from the first decoded frame for every still format, display matrix for video, stream group for HEIC) and fit/fill; ICC-tagged wide-gamut photos handled (flag only for RGB profiles, retry without on failure, grid profiles via ffi), progressive-JPEG peak, texture-size clamp; VFR snapped to standard rates. Done, HEIC grids included: their orientation and color profile come from the grid, and `scripts/heif-grid.py` builds the test file, since FFmpeg cannot write grids.
- Sequence settings dialog (frame rate, resolution) with the match-first-clip prompt and the rate-change conversion. Done.
- Decoder pool with the visibility rule and size budget; 4K single-decoder rule. Done: the visible clip and the next one within 2 s of playback, made ready by a worker thread; one decoder and no lookahead above the 1080p class; above 9 Mpx one software decoder on one thread, and a source whose pictures would not fit it refused at import.
- Per-clip volume, mute, fades; detach audio. Done.
- Undo/redo for everything. Project save/load (JSON). Autosave and crash recovery. Done; autosaves live in a session folder of their own per running Dusk (ARCHITECTURE.md, Project file and autosave).
- Media bin with thumbnails; import by drag and drop from Explorer, by an Import button (the system file dialog), and from the command line as in M1; WhatsApp/Telegram audio formats verified. Done: the three ways to import; the audio formats, checked on Opus in Ogg, AAC in M4A, AMR-NB and MP3 with cover art in `testdata`; and thumbnails, made one at a time on the engine's thumbnail thread. Decided at M2: Slint's `unstable-winit-030` feature joins the pinned set, because Slint 1.18's winit backend does not pass files dropped from Explorer to the app and its window-event hook does (ARCHITECTURE.md, Slint specifics).
- Variable-speed playback (0.1x to 32x, both directions; slow reverse via GOP buffer, fast via keyframes). Done: L and J double the speed up to 32x, Shift+L and Shift+J slow it down to 0.1x; above 8x forwards or 2x backwards (playback times clip speed) only keyframes are decoded; sound plays backwards from 0.25x to 2x; a group of pictures longer than half the cache is decoded again from its keyframe for each stretch of it that fits.

**Done when**: a three-clip edit with music survives save, quit, reopen, and undo history behaves. Holds: `a_three_clip_edit_with_music_survives_save_and_reopen` in `dusk-app`'s history tests, and the same in the window (edit, save from the menu, quit, reopen: the same timeline, nothing to undo). Memory measured as in M1, figures in REQUIREMENTS.md.

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
- Installer (Inno Setup with solid LZMA2, picked at M0; not MSI) containing `dusk.exe`, `dusq.exe`, the five FFmpeg DLLs and the third-party license files (the bundled fonts' OFL requires shipping theirs); startup time, download and installed size measured against targets.
- Docs: README with screenshots, shortcuts reference, build instructions, contributing guide.

**Done when**: a stranger can download Dusk, open a clip, cut it, and export it without asking a question.

## 0.2

- Slim custom FFmpeg build (only the demuxers, decoders and encoders Dusk uses) to reach the ~60 MB installed target.
- Zero-copy hardware decode (D3D11 surfaces shared with wgpu): at M1 the copy back made D3D11VA no faster than software decoding, at many times the memory.

## Later (from FEATURES.md)

Transitions, titles, color adjustments, picture-in-picture, keyframes, waveforms on the timeline, snapping and markers, 4K proxies, HDR passthrough, subtitles, export queue, GIF export, Linux and macOS builds.
