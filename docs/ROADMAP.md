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

- Double-click a clip → second window previewing its whole link group (video + audio). Done: a double-click on a timeline clip, Enter on the selected clip, the Edit menu or the properties panel opens it; one clip editor at a time, and a draft not applied is asked about before another clip opens, the window closes or the project is left.
- Trim, crop, rotate/flip, fit/fill, speed, volume and fades, still duration in the pop-out, applied as one undoable `ApplyClipSession` command. Done: a trim bar over the whole source with I and O at the playhead, the crop as pixels cut from each side as the picture shows, turns that keep a mirror the same way on screen, and every change previewed at once.
- *Apply to project* (undoable) and *Export as file*. Done: Export as file writes an MP4 at the clip's own frame rate and its cropped, turned size, never over a media file of the project; a sound-only clip waits for M4's audio export.
- Both windows stay responsive; edits in the pop-out preview live. Stale-session banner and deleted-clip handling work. Done: the engine draws both previews from one frame cache and decoder pool and one plays at a time; the banner offers Reload from project and Keep my draft, a draft without changes of its own follows the clips, and a deleted or unlinked clip turns Apply off.

**Done when**: you can fix one clip in the pop-out and see the main timeline update, without ever leaving the project. Holds: `a_clip_fixed_in_the_clip_editor_updates_the_timeline_in_one_step` in `dusk-app`'s history tests, and the same in the window (trim and turn the clip in the clip editor, Apply: the timeline clip changes; Undo and Redo from the Edit menu take it back and forth, and the clip editor follows). Memory measured as in M1, figures in REQUIREMENTS.md; the growth when both windows redraw together on Vulkan is AMD's Vulkan driver (ARCHITECTURE.md, "Known issues").

## M4 — Export, compress, extract

- Export dialog: container (MP4/MKV/MOV, WebM as VP9/AV1), resolution presets, Quality 0–100 mapped per encoder, Advanced target bitrate for any encoder and CRF only where the encoder has one (SVT-AV1, VP9, and x264 and x265 in a user's own `ffmpeg.exe`), encoders shown by availability. Done: one dialog for the timeline and the clip editor (ARCHITECTURE.md, "Export details"), which names the encoder each codec will use.
- Compress tool: open a single file, target quality or target size (bitrate ladder + one corrective re-encode), export, no project. Done: File → Compress a video… (Ctrl+M), one dialog (ARCHITECTURE.md, "Compress tool paths"), and `dusq compress --size`.
- Optional external GPL `ffmpeg.exe` export path (raw frames piped). Done: Advanced → *Use my own ffmpeg.exe…* offers its `libx264` and `libx265` beside Dusk's own encoders for the session (ARCHITECTURE.md, "Optional GPL encoders"); M5 keeps the path.
- Encoder probing by real session at 640×480 on first export, fall-through on open failure, per-encoder limits table with fixed values; per-encoder memory measured (SVT-AV1 threads and lookahead fixed) and the encoder line in REQUIREMENTS.md updated. Done: measured with `dusk-media`'s `encoder_memory` example; SVT-AV1 works on two pictures at once (one, without its lookahead, above 1080p), which took it from 4.7 GB to 1.0 GB at 4K, and VP9 on four threads instead of one.
- CPU path in dusq (normalize → tone-map/rotate in Rust → encoder format) verified against the GPU path by the per-plane PSNR test (≥ 45 dB unscaled, ≥ 40 dB scaled); `--threads`; dusq memory ceiling measured per size class and recorded in REQUIREMENTS.md. The path, `--threads` and the PSNR test are done (DECISIONS.md, "Equivalence of the two paths": every plane 56 to 67 dB), and so are the ceilings (REQUIREMENTS.md: 158 MB at 1080p, 446 MB at 4K, 974 MB at 8K).
- Audio-only export (MP3/AAC/Opus/WAV), including the clip editor's Export as file for a sound-only clip. Done: *Sound only* in the export dialog writes the mix of the timeline or of the clip, at 48 kHz.
- `dusq` (the `dusk-cli` binary) with `compress` and `extract-audio`, built without the `gpu` feature on the CPU transcode path. Done, `--size` included.

**Done when**: a 2 GB phone video can be compressed to a 25 MB file from the compress tool in one dialog. Holds: a 2.1 GB, 5.5-minute portrait 4K H.264 video at 50 Mbit/s with sound, made for the test, came out at 23.5 MB, 360 × 640 and upright, in 197 s from one *Compress a video* dialog, which started at 25 MB, peaking at 531 MB private (REQUIREMENTS.md, "During export").

## M5 — Keyboard and settings

- Every common action has a shortcut; shortcuts are remappable and stored in a plain text file. Done: every action has a name and keys in one table, and `shortcuts.txt` in Dusk's settings folder keeps the keys the user changed.
- Shortcut list panel (`?` key and a toolbar button), searchable. Done, grouped, and it changes keys too.
- Settings: cache size, default export preset, optional external `ffmpeg.exe` path. Done: File → Settings… (Ctrl+,), applied at once and kept in `settings.txt`.

**Done when**: a full edit can be done without touching the mouse except for dragging clips. Holds: a test (`dusk-app/tests/keyboard.rs`) starts the real binary on a saved project and, with keys posted to its window, splits the clip, deletes the second half, switches the first off and trims its start, then closes and saves with Enter; the saved file holds the edit. A run with keys alone, no clicks and no menu, set a clip to fill from the properties panel, split another and deleted its end, turned the first in the clip editor (opened with Enter) and applied that with Tab and Enter, then exported with Tab to Export, Enter, Enter and the save dialog. Posted keys cannot hold Shift, Ctrl or Alt, so the chords (undo, save, import, the media keys) rest on the shortcut table's tests and the hands-on checks. On the way, Sequence settings' frame rates became reachable with Tab, the timeline began following the playhead that keys move, and numbers typed in dialogs stopped being lost when a button was clicked.

## M6 — Release 0.1

- Theme and icon polish against THEME.md; app icon and logo. About dialog with `AboutSlint`, license notices, and a link to the releases page. Done: the logo is drawn by `scripts/make-logo.py` at every size Windows asks for and built into `dusk.exe`, the theme pass fixed four things (DECISIONS.md, "The theme pass"), and Help → About Dusk (Shift+F1) has the licenses and the releases page.
- Missing-media relink dialog. Error messages that say what to do. Done: a project opens with its missing files listed, File → Find missing media… (Ctrl+Shift+M) relinks a file and the others beside it in one undoable step, and every message was gone through.
- Installer (Inno Setup with solid LZMA2, picked at M0; not MSI) containing `dusk.exe`, `dusq.exe`, the five FFmpeg DLLs and the third-party license files (the bundled fonts' OFL requires shipping theirs); startup time, download and installed size measured against targets. Done: `scripts/build-release.ps1` and `installer/dusk.iss`; CI builds the installer on every push and installs, runs and uninstalls it (`scripts/check-installer.ps1`); the sizes and the start-up are in REQUIREMENTS.md.
- Docs: README with screenshots, shortcuts reference, build instructions, contributing guide. Done: README.md, `docs/SHORTCUTS.md` (a test keeps it equal to the shortcut table), CONTRIBUTING.md, and SETUP.md's "Release".

**Done when**: a stranger can download Dusk, open a clip, cut it, and export it without asking a question. Holds as far as tests can show it: CI installs the installer as a user would and runs both programs from where it put them, and the release build, started for the first time with nothing set up, says what to do first ("Import video, audio or photos with Import, or drop them on the window. Then drag them onto the timeline."). From there a 12 s 1080p clip with sound was brought in with the Import button and the system's dialog, dragged onto the timeline, split with Edit → Split at the playhead, its second half clicked and deleted with Delete, and exported with Export and Export… into a 6 s H.264 and AAC file, meeting no dialog but the system's and the export's. Whether a stranger asks a question is for a stranger to show, which is the last hands-on check on PR #6.

## After 0.1

Every item under "Later" in FEATURES.md, and the two things that stand between 0.1 and a stranger (an unsigned download, and an installed size that is mostly FFmpeg), placed into milestones. The order follows what depends on what:

- Packaging comes first. It is the first thing a stranger meets, and the FFmpeg that M7 builds is the one every later milestone links: subtitle, GIF and image encoders, and the Linux and macOS builds.
- Timeline comforts (M8) change no invariant, so they come before the milestone that does.
- **Layers (M9) is the one deep change.** Today at most one video clip is visible at any frame (ARCHITECTURE.md, "Fit and rotation"), and the decoder budget, the compositor and the clip editor all lean on that rule. Crossfades, picture-in-picture and titles over video all break it, so it is reworked once, in M9, and the other two build on it.
- Keyframes need the values they animate (M10's scale and position), and subtitles need M11's text drawing.
- Proxies wait for the laptop measurements of M7, which say how urgent they are.

Rules that hold for every milestone below, as for M0 to M6:

- A change to an invariant or to the design is written into ARCHITECTURE.md before it is built, and its reasons into DECISIONS.md.
- Memory is measured against REQUIREMENTS.md (idle, during and after playback, and export where it changed), and any new budget line is measured before it is written down.
- The project file format changes at most once per release. Its version goes up then, and every older version still opens (a 0.1 project opens in every later Dusk); the tests pin each version's format byte for byte.
- Each milestone keeps its hands-on checks in its pull request.

## 0.2 — Lighter and signed

### M7 — Slim, signed, and measured on a laptop

1. **Integrated graphics.** REQUIREMENTS.md names Intel Iris Xe as the class Dusk must be smooth on, and nothing has been measured on one yet (ARCHITECTURE.md, Open questions 1). On an Iris Xe laptop (a 12th-gen Core i7 is that class), run M1's measurements: `DUSK_STATS` while 1080p30 and 1080p60 clips play, `playback_probe` for memory and scrub latency, a 4K clip, and both windows open at once. Also export through `h264_qsv` and `hevc_qsv` with `encoder_memory`, whose 200 MB line in REQUIREMENTS.md is still provisional. 12th-gen graphics decode AV1 but cannot encode it, so `av1_qsv` should fail its probe and fall through to SVT-AV1, which the run also checks. `DUSK_STATS` gains a first line naming the graphics adapter, so a run on a laptop with two GPUs says which one it measured (`WGPU_POWER_PREF=low` picks the integrated one). The figures answer Open question 1: keep the texture path, or switch to the pre-approved readback.
2. **An FFmpeg of Dusk's own.** Build FFmpeg 8.1 starting from BtbN's build scripts, which already produce the current pin, with a configuration of Dusk's own. A workflow of its own runs the build when the pin changes, not on every push, and publishes the DLLs as a release asset of Dusk's repository, together with the exact sources of FFmpeg and of the LGPL libraries built into it, and the configuration. The LGPL asks for those sources, and `licenses/FFmpeg.txt` points to them. `scripts/ffmpeg-pin.psd1` then names that asset, which removes the dependency on BtbN keeping a build downloadable (SETUP.md's pin moves continue until then).
   - **Kept:** all of FFmpeg's own demuxers and decoders, since Dusk imports "any format FFmpeg can demux and decode" (REQUIREMENTS.md §8), plus dav1d for AV1 and `lcms2` for photo color.
   - **Kept:** the encoders and muxers Dusk writes with today. These are OpenH264, Kvazaar, SVT-AV1, libvpx-vp9, FFmpeg's own AAC, libopus, libmp3lame and PCM, the hardware wrappers, and the MP4, MOV, Matroska, WebM, Ogg, M4A, MP3 and WAV muxers.
   - **Kept:** the ones later milestones need, chosen now so the pin moves once. For M13 these are `mov_text`, SubRip, WebVTT and ASS, with their muxers. For M15 they are PNG, MJPEG and GIF, with the image sequence muxer.
   - **Dropped:** network protocols, encoders and muxers Dusk does not write, and external libraries nothing in Dusk uses.
   - The NVENC headers stay at the version that needs driver 570, so GTX 900/1000 cards keep working when the FFmpeg version moves on.
3. **Code signing.** Sign `dusk.exe`, `dusq.exe`, the FFmpeg DLLs and the installer in the release workflow, so Windows names a publisher.
   - **First choice: SignPath Foundation.** It signs for open-source projects at no cost, from builds that CI makes from the public repository, which Dusk's release workflow already does. It asks for a code signing policy on the project's page, and the publisher Windows shows is SignPath Foundation.
   - **If SignPath declines:** Certum's open-source certificate (a cloud key, roughly €50 a year in 2026; check that it covers Authenticode first) or a commercial OV certificate (about $150–300 a year).
   - Microsoft's Artifact Signing (formerly Trusted Signing) serves individual developers only in the US and Canada.
   - A signature does not silence SmartScreen at once, because reputation still builds with downloads, so the README's note stays until the warning stops.
   - `scripts/check-installer.ps1` checks that every signature is valid.

**Done when**: the installer CI builds is signed and installs a Dusk whose files all carry valid signatures; Dusk runs on its own FFmpeg with every test passing, including `testdata`'s messenger audio, HEIC grid, HLG and Display P3 files; the installed size is measured against the ~60 MB target, with what stands in the way recorded in DECISIONS.md if it lands above; and the integrated-graphics figures are in REQUIREMENTS.md.

### M8 — Timeline comforts

- **Waveforms on audio clips.** `dusk-media` reads a file's sound into peaks (an 8-bit low and high value per short slice of time) as a job on a worker, cancellable and reporting progress, made one file at a time like thumbnails, so it never needs more than the audio decoders the pool allows. Peaks live in memory under a cap of their own, about 0.7 MB for an hour of sound at 100 slices a second, and are read again when a project opens. A disk cache waits until reading them is shown to be slow. The timeline draws each clip's peaks at its zoom, in a way chosen by measuring the UI thread.
- **Snapping.** A clip being dragged and a trim handle snap to cuts, the playhead and markers within a few pixels, the playhead to cuts and markers, and a snap guide shows in `signal` (THEME.md). One key turns snapping off and on, in the shortcut table like every action. Snapping only chooses the position; the commands and their invariants are unchanged.
- **Markers.** A marker is a named point on the timeline, added at the playhead with a key, renamed and removed, with keys to go to the previous and next one. Markers are undoable commands and are saved in the project file, which makes format version 2 of release 0.2.

**Done when**: five clips are cut to the beats of a song using its waveform, markers and snapping, without zooming in to single frames; a 0.1 project opens and saves as version 2; and the peaks' memory for an hour of sound is measured.

## 0.3 — Layers

### M9 — Layers and crossfades

- **The design change, written first.** The compositor draws the visible clips of every unmuted video track from the bottom up, starting at the topmost clip that covers the whole frame opaquely, so a frame that shows one clip still costs one clip. "At most one video clip is visible" becomes "a clip is visible where nothing opaque covers it". The decoder budget follows: in the 1080p class, one decoder for each visible clip (two at most with two video tracks) plus the lookahead's. Above it, two software decoders are allowed while two clips show at once, which stays under the single-source ceiling in software (DECISIONS.md, "Decoder sizes"). These are measured before REQUIREMENTS.md changes.
- **Crossfades at a cut.** A crossfade belongs to the cut between two adjacent clips on one track, so clips on a track still never overlap. While it plays, the outgoing clip runs past its out point and the incoming one starts before its in point, so both need source to spare there. A crossfade longer than that source allows is refused with the length that would fit.
  - A trim that eats into that spare source shortens the crossfade, and the UI says so, as a cut at the gap does.
  - A cut between two linked clips also crossfades their sound, as linked clips are edited as a group.
  - A crossfade on an audio track alone is a sound crossfade.
  - "Cut" is no transition at all, as now.
  - The other way, overlapping clips on two tracks with fades, needs no model change but leaves the user to line up two tracks for every transition; it is kept in DECISIONS.md as the option not taken.
- **The clip editor** shows its group without the crossfades at its ends, which belong to the cuts and are edited on the timeline.
- The crossfade's length and where the cut lies are saved in the project file (format version 3, release 0.3).

**Done when**: two 1080p clips joined by a one-second crossfade play without a skipped frame and export as previewed, with memory within the re-measured playback budget; the same with two 4K clips is usable.

### M10 — Picture-in-picture and basic color

- **Scale and position.** A video clip gets a size and a place in the frame (`ClipEdits::Video` gains a transform). It is drawn there over the tracks below, with fit or fill deciding the picture inside its rectangle. The clip editor edits it with a box in its preview and with number fields, and so does the properties panel. A clip that covers the whole frame keeps being the opaque case of M9.
- **Brightness, contrast and saturation**, three sliders per video clip in the clip editor and the properties panel, applied by the compositor to the SDR picture after color step 3 and before quantizing. Like step 3 they have a reference function in `dusk-core`'s `color` module that a GPU test holds the shader to. Three sliders and no more (VISION.md: no color grading suite). dusq applies no edits and does not change.
- Opacity as a control the user sets is not in FEATURES.md. The compositor gains it for crossfades, and whether to show it is the maintainer's decision.

**Done when**: a phone clip shrunk into a corner over a screen recording, with its brightness raised, exports as previewed, and memory is measured with two 1080p clips visible for a whole minute.

### M11 — Text and titles

- **Drawing text.** Titles need shaping, so that Arabic letters join and right-to-left lines run the right way (the maintainer writes Arabic), so a plain glyph rasterizer will not do. Candidates are `cosmic-text` (shaping through rustybuzz, bidirectional text, font fallback) and rustybuzz with `unicode-bidi` and a rasterizer, each MIT/Apache; the choice is measured for size and speed and recorded in DECISIONS.md. Inter has no Arabic, so Dusk bundles an OFL font that does (Noto Sans Arabic or IBM Plex Sans Arabic) for portable projects, and offers the system's fonts as well. Text is laid out and rasterized on the CPU into a glyph atlas the compositor draws from, with a byte cap of its own, at the output's size, so exported text is sharp at any size.
- **Title clips.** A title is a clip on a video track with no media behind it: its text, font, size, color, place and background, which is either none (text over the clip below) or a solid color (a title card). Its length is free, like a still's. The invariant becomes "video tracks hold video clips and titles".
- **Editing titles** happens in the clip editor: a double-click on a title opens it there, as a clip opens, with its text and style beside the preview.

**Done when**: an opening title card and a lower third, one in English and one in Arabic, over a clip, export as previewed, with the Arabic joined and right to left; and the atlas's memory is measured.

## 0.4 — Motion and words

### M12 — Keyframes

- Volume, scale and position (FEATURES.md) can change over a clip's length: a value is either fixed or a list of points in time, with straight lines between them. Where a point's time is kept is decided first, in ARCHITECTURE.md. In source time a point stays on its picture when the clip's start is trimmed or its speed changes; in the clip's own time it stays a fixed distance from the clip's start. Stills and titles have no source time either way.
- Points show on clips in `signal` (THEME.md: keyframes are signal) and are added at the playhead, in the clip editor first.
- The mixer applies volume points per sample, so a change of volume makes no clicks; the compositor applies scale and position per frame.
- Saved in the project file (format version 4, release 0.4).

**Done when**: music dips under a voice twice through volume points, and a picture-in-picture slides in from the side, exported as previewed.

### M13 — Subtitles

- **SRT import** onto a subtitle track, a third kind of track that holds only subtitle clips. Each cue is a clip that moves, trims and splits like any other, and whose text is edited in the clip editor.
- **Burned in**, drawn by M11's text drawing, Arabic included.
- **Soft subtitles** written as a subtitle stream of the file: `mov_text` in MP4 and MOV, SubRip or ASS in MKV, WebVTT in WebM, from the encoders M7's FFmpeg keeps. The export dialog offers none, burned in or soft.

**Done when**: an SRT file is imported, a line is retimed and corrected, and the edit is exported once with the subtitles burned in and once with them soft; the soft track shows in VLC.

## 0.5 — Bigger sources, more outputs

### M14 — Proxies for 4K

- **Proxies** for sources above the 1080p class. Dusk offers one when a 4K file is imported, and makes it on a worker through the CPU transcode path, cancellable and reporting progress. The proxy is a 1080p-class file with short groups of pictures for quick seeking, kept in Dusk's own folder beside the autosaves and keyed by the source's path, size and modification time, under a disk cap set in Settings.
  - A proxy is made from the source and never replaces it; the source is still never modified or copied (REQUIREMENTS.md §3).
  - Playback and scrubbing use the proxy, and with it the 1080p class's two decoders and lookahead; export always reads the original.
- **Zero-copy hardware decoding** (Open questions 2) is decided here from M7's laptop figures. If proxies make 4K smooth on the laptop, it stays later; if not, a spike shares D3D11 surfaces with wgpu's Vulkan device.

**Done when**: a 4K phone video plays without a skipped frame on the Iris Xe laptop through its proxy and exports from the original; the proxy's disk use and the time to make it are recorded.

### M15 — Export queue, GIF and image sequences

- **Export queue.** Exports and compress jobs wait in a list and run one at a time on the export thread, each from its own snapshot, so editing goes on meanwhile. Each can be cancelled or removed, and one that fails says why. Whether "batch export" also means several files in one `dusq` command is the maintainer's decision.
- **GIF.** The palette is made in Rust, because Dusk does not ship libavfilter (no `palettegen`) and libimagequant is GPL. An MIT/Apache quantizer such as `color_quant` makes it, with optional dithering, and FFmpeg's own GIF encoder from M7's build writes the file. The presets keep GIFs small (480p at 15 fps to start).
- **Image sequences.** PNG or JPEG, one file per frame, written into a `name.part` folder that is renamed when the export succeeds, as files are.

**Done when**: a 10-second clip exports as a GIF that plays in a browser and as a PNG sequence, and three queued exports run in order while the timeline is edited.

### M16 — HDR passthrough

- HLG and PQ sources export as HDR in 10 bits: the compositor keeps their light instead of tone-mapping it, writes BT.2020 10-bit YUV (P010, YUV420P10) and hands the mastering display and MaxCLL on to the file.
- It is offered only where a 10-bit encoder exists: HEVC Main10 on NVENC, QSV and AMF, SVT-AV1 (always present) and VP9 profile 2. OpenH264 and Kvazaar are 8-bit only.
- The preview stays tone-mapped on SDR displays. SDR clips in an HDR export are placed at BT.2408's reference white (203 nits).
- dusq can keep HDR too, through its 16-bit path. "All exports are 8-bit SDR" becomes the rule for SDR exports only, a change to CLAUDE.md that the maintainer signs off.
- The equivalence test gains a 10-bit case.

**Done when**: an iPhone HLG clip exports as HLG HEVC that a player shows as HDR, with metadata matching the source.

## 0.6 — Linux

### M17 — Linux

- **The platform layer** grows Linux's own pieces: file dialogs through the desktop portal (which GNOME, KDE and the others provide), the console's Ctrl+C for dusq, and the key codes shortcuts match by. Settings and autosave folders already follow XDG.
- **FFmpeg** comes from M7's build workflow, extended to Linux (BtbN builds Linux too, so the same scripts serve). VAAPI (Intel and AMD) joins NVENC in the encoder order and the limits table.
- **wgpu** runs on Vulkan, and cpal plays through ALSA, which PipeWire and PulseAudio also serve.
- **The package** is an AppImage: one file that runs on most distributions without installing. Linux asks for no signature, so nothing stands between a download and a first run.
- **Trying it on Windows.** WSL2 with WSLg builds Dusk, runs its tests and `dusq`, and opens its window, which is enough for most of the work. It is not a real Linux desktop: its graphics are Windows' own passed through (or drawn in software), it has no VAAPI, and its file dialogs and drag and drop are not a desktop's, so its figures do not count. The done-when runs on a real install, such as Ubuntu started from a USB stick.
- CI builds and tests Linux beside Windows.

**Done when**: M6's done-when holds on the current Ubuntu LTS on real hardware (a stranger downloads Dusk, opens a clip, cuts it and exports it without asking a question), with memory measured there.

## macOS, for someone with a Mac

### M18 — macOS

Left for a contributor (or a later maintainer) who has a Mac, since Dusk cannot be tried there without one. CI's macOS runners can build it and run its tests, but someone has to use it on a real Mac.

- **The platform layer:** `NSOpenPanel` and `NSSavePanel` for file dialogs, the console's Ctrl+C, and the key codes shortcuts match by.
- **FFmpeg** comes from M7's build workflow on a macOS runner (BtbN builds no macOS FFmpeg). VideoToolbox joins the encoder order and the limits table.
- **wgpu** runs on Metal and cpal on Core Audio.
- **Why a download is blocked.** Browsers mark every downloaded file as coming from the internet, GitHub releases included. For a marked app, macOS checks that it is signed with an Apple Developer ID certificate and notarized (scanned by Apple). Only Apple issues those certificates, through its Developer Program, $99 a year, with no waiver for open-source projects. An unsigned app still runs, but macOS refuses it the first time and the user has to allow it in System Settings → Privacy & Security → Open Anyway. A build compiled on the Mac itself carries no mark, so developers never see this.
- **The plan:** ship a `.dmg` without a Developer ID first. Apple silicon still needs every program to carry a basic signature of its own, which the Rust toolchain adds by itself. The README shows the Open Anyway steps with a screenshot. If macOS users come, the Developer Program is the next step, and the release workflow then signs and notarizes on a macOS runner (`codesign`, `notarytool`).

**Done when**: M6's done-when holds on macOS on Apple silicon, tried by someone with a Mac, with memory measured there.

## Choices for the maintainer

Decisions these milestones need from the maintainer; each is asked when its milestone starts.

1. **M7:** apply to SignPath Foundation, or buy a certificate.
2. **M10:** whether opacity becomes a control the user sets (it is not in FEATURES.md).
3. **M11:** which Arabic-capable font Dusk bundles (its size counts toward the installed size), and whether projects may use system fonts, which another machine may not have.
4. **M15:** whether batch export includes several files in one `dusq` command.
5. **M16:** allowing 10-bit HDR exports, which changes CLAUDE.md's "all exports are 8-bit SDR".
6. **M18:** joining Apple's Developer Program ($99 a year) to sign and notarize the macOS build, once macOS users ask for it; the build ships without it first.
