# Dusk — Architecture

Rust, Slint UI, FFmpeg via `ffmpeg-next`, `wgpu` rendering, `cpal` audio. Windows first, portable by design.

This file says what Dusk's design is now. Why it is that way (what was measured, what went wrong first, what was turned down) is in [DECISIONS.md](DECISIONS.md) under the same headings, and the budgets and the figures measured against them are in [REQUIREMENTS.md](REQUIREMENTS.md). A change of design is proposed here first; its reasons go in DECISIONS.md.

## Layers

```
┌──────────────────────────────┬──────────────────────────────┐
│ Main window                  │ Clip editor window (pop-out) │   Slint
├──────────────────────────────┴──────────────────────────────┤   dusk-app
│ App layer: commands, undo/redo, view models, shortcuts      │
├─────────────────────────────────────────────────────────────┤
│ Engine: frame cache, decoder pool, playback scheduler,      │   dusk-engine
│ export pipeline (worker threads)                            │
├───────────────────┬───────────────────┬─────────────────────┤
│ Media (FFmpeg)    │ Render (wgpu)     │ Audio (cpal)        │   leaf crates
│ probe/decode/     │ composite frames  │ mix, resample,      │
│ encode/mux        │ given as input    │ output, clock       │
├───────────────────┴───────────────────┴─────────────────────┤
│ Project model: timeline, tracks, clips, edits as data       │   dusk-core
└─────────────────────────────────────────────────────────────┘
```

Dependencies point downward only: `dusk-app` → `dusk-engine` → {`dusk-media`, `dusk-render`, `dusk-audio`} → `dusk-core` → `serde`. The three leaf crates never depend on each other; `dusk-engine` is the only place that moves data between them.

## Workspace layout

| Crate | Responsibility | Must not |
|-------|----------------|----------|
| `dusk-core` | Project model: `Project`, `Sequence`, `Track`, `Clip`, `MediaRef`, edits. Commands with apply/revert. Serialization. Enforcement of timeline invariants. Also the plain data the leaf crates hand each other through `dusk-engine`, since they cannot share types directly: decoded pictures (`Picture`, NV12 or P010 with their color tags). | Touch FFmpeg, wgpu, Slint, or the filesystem beyond serde. |
| `dusk-media` | FFmpeg wrapper: probe files (rotation, VFR, HDR flags, attached pictures), decode video to frames (hardware when possible) and audio to PCM, seek, thumbnails, waveform peaks, encode and mux, enumerate encoders. | Know about the UI, the project model, or other leaf crates. |
| `dusk-render` | GPU compositor: given a `Sequence`, a frame index, and the already-decoded source frames it needs, produce a composited frame (fit, crop, rotate, tone-mapping) as a `wgpu::Texture`. Also readback for export. Creates the wgpu device that Slint renders with too (see Slint specifics). | Decode media or ask for frames; it receives them. |
| `dusk-audio` | Mix already-decoded PCM for a frame range with volume and fades, resample, drive the output device, own the playback clock. | Decode media or know about the UI. |
| `dusk-engine` | Worker-thread orchestration: frame cache, decoder pool, playback scheduler (asks media, feeds render and audio), thumbnail and waveform jobs, export pipeline, cancellation and progress. The **CPU transcode path** (decode → `swscale` scale and normalize → tone-map and rotate in Rust → `swscale` to encoder format → encode) is always compiled; the additive `gpu` feature (default on) adds `dusk-render`, the compositor-based export and playback, whose sound output (`dusk-audio`'s `output` feature) brings in `cpal`, so `dusq` links neither wgpu nor an audio device. | Contain UI code. |
| `dusk-app` | The binary `dusk.exe`. Slint windows, view models, command dispatch, undo stack, shortcuts, settings, autosave. | Contain business logic that belongs in `dusk-core` or orchestration that belongs in `dusk-engine`. |
| `dusk-cli` | The binary **`dusq`**: `dusq compress in.mp4 --size 25MB`, `dusq extract-audio in.mp4`. Uses `dusk-engine` without the `gpu` feature: no Slint, no wgpu, no GPU adapter needed, so it runs headless. Cargo unifies features across a workspace build, so dusq is built and released on its own (`cargo build -p dusk-cli --release`). Separate binary because a Windows GUI-subsystem executable cannot print to a console. | Composite or apply edits; it transcodes. |

## Threading model

- **UI thread**: Slint event loop only. Never decodes, encodes, or reads large files.
- **Engine threads** (`dusk-engine`): a video thread (decodes through the frame cache, draws the previews, follows the playback clock), a mixer thread (mixes the sequence's audio into the output device's ring buffer and starts the clock), short-lived workers for jobs such as probing a file on import or getting the next clip ready while playing (waveforms later), a thumbnail thread that makes the media bin's thumbnails one at a time and waits while anything plays or exports (its decoder would be a third beside playback's two), plus one dedicated export thread when exporting. Communication via channels (`crossbeam-channel`). Results are pushed to the UI with `slint::invoke_from_event_loop`; preview frames pass through a slot that keeps only the newest, so a busy UI thread skips frames instead of queuing them.
- **Audio callback**: the `cpal` output callback, on the device's own thread. Real-time: no allocation, no locks that can block; it copies from a lock-free ring buffer (200 ms) filled by the mixer thread, fills silence when that runs dry, and counts the frames it played.
- Every long job (export, waveform scan, relink) carries a cancellation token and reports progress through a channel.

## Time units

Two kinds of time, never mixed without an explicit conversion:

- **Timeline time** is a `Frame`: an `i64` count of frames at the sequence frame rate (a `Rational`, e.g. 30000/1001). Positions, lengths, fades, and the playhead are all `Frame`. No floats.
- **Media time** is a `MediaTime`: an `i64` in microseconds, like FFmpeg's `AV_TIME_BASE`. Clip in/out points into source files are `MediaTime`, so variable-frame-rate phone video works: the source frame shown at media time *t* is the last decoded frame whose timestamp is ≤ *t*.
- Conversions live in one module in `dusk-core` and round to nearest.

## Core data model (`dusk-core`)

```
Project   { version, media: Vec<MediaRef>, sequence: Sequence }
MediaRef  { id: MediaId, path (relative to project when possible, absolute fallback), info: MediaInfo }
MediaInfo { kind: Video | Audio | Still, duration, has_video, has_audio, frame_rate (snapped), vfr: bool,
            width, height, orientation: Orientation (the 8 EXIF orientations: rotation plus mirror, from the
            display matrix for video, the first decoded frame for JPEG/PNG/WebP/TIFF, or the HEIC stream group),
            pix_fmt, primaries, transfer, matrix, range, hdr: Option<Hlg|Pq> }
Sequence  { frame_rate: Rational, resolution: (u32, u32), tracks: Vec<Track> }
Track     { id, kind: Video | Audio, locked: bool, muted: bool, clips: Vec<Clip> (sorted, non-overlapping) }
Clip      { id: ClipId, media_id: MediaId, source_in: MediaTime, source_out: MediaTime,
            position: Frame, length: Frame, speed: f64 (0.1..=32), enabled: bool,
            link: Option<LinkId>, edits: ClipEdits }
ClipEdits = Video { crop: Rect, rotate: Rot90, flip_h: bool, flip_v: bool, fit: Fit | Fill }
          | Audio { volume_db: f32, fade_in: Frame, fade_out: Frame }
```

Invariants the model enforces (commands that would break one are rejected with a reason, never patched up in the UI):

- A **video track holds only video clips, an audio track only audio clips.** Importing a file with both streams creates one video clip and one audio clip sharing a `LinkId`. Audio files whose only picture stream is attached cover art import as audio only.
- **Linked clips share `source_in`, `source_out`, `position`, `length` and `speed`.** Commands keep a link group in lock-step: move, trim, split, speed and delete apply to the whole group. "Detach audio" in the UI is `Unlink`. Alt+Delete is `Batch[Unlink, RemoveClip]` as one undo step.
- `length` is explicit. For video/audio clips, commands recompute it as `max(1, round(((source_out - source_in) / speed) × frame_rate))`. For stills, `length` is free and defaults to 5 seconds; `source_in/out` are 0.
- Clips on a track are sorted by `position` and **never overlap.** The UI snaps a drag to the nearest valid position before issuing the command. Media dragged from the bin onto an empty timeline starts at frame 0 wherever it is let go, and the outline shown while dragging says so (`dusk-core`'s `drop_place`), so a first clip never leaves black before it; once the timeline has clips, media starts where it is let go.
- **Speed changes and end-trims never push neighbors.** If the new length does not fit before the next clip, the clip is cut at the gap (`source_out` reduced, or `length` reduced for a still) and the UI says so. For a link group the smaller gap on either track applies. On the timeline, dragging a clip's left handle moves its start (`position` and `source_in` change together). Users ripple explicitly to make room.
- **Ripple delete** removes a clip (group) and shifts every clip on all unlocked tracks whose `position` ≥ the deleted end leftward by the deleted length. If any other clip on an unlocked track overlaps the deleted range at all (even partially), the shift would create an overlap, so the command is rejected with "lock that track or use plain delete". Plain delete leaves a gap.
- **Locked tracks.** No command may insert, move, trim, split, delete or edit a clip on a locked track, and ripple shifts skip locked tracks. A group command that would touch a linked partner on a locked track is rejected ("unlock the track or unlink").
- **Fit and rotation.** `orientation` is applied at import, so phone video and photos are upright and mirrored shots are un-mirrored. Video carries it in the display matrix; JPEG, PNG, WebP and TIFF expose it only on decoded frames, so import decodes one frame of every still (needed for the thumbnail anyway); HEIC grids keep it on the stream group, which `dusk-media` reads through its `ffi` module (see below). Any source tagged with primaries other than BT.709 (Display P3 photos from iPhones, BT.2020 video) gets a gamut matrix to BT.709 with clipping, HDR or not. Profiles FFmpeg cannot name (Adobe RGB, ProPhoto) are treated as sRGB in 0.1, which is stated in the docs; deriving the matrix from the profile is Later. A clip whose rotated size differs from the sequence is fitted by default (preserve aspect, centered, black bars); `Fill` crops to cover. Either way a clip is opaque over its whole frame, and the upper video track draws on top, so at any frame at most one video clip is visible: the topmost enabled clip on an unmuted track, or black in a gap. Muting a video track hides all its clips. Scale/position controls are later (picture-in-picture).
- **Streams of unequal length.** A linked pair shares one source range even when the file's video and audio streams start or end at different times. Where the range reaches past a stream, the video clip shows black before the first frame and holds the last frame after the stream ends (the same hold-last-frame rule as VFR), and the audio clip is silent outside its stream.
- **Sequence settings** come from the first video clip added, after rotation (a portrait phone clip makes a portrait sequence); the UI asks to match. VFR sources are snapped to the nearest standard rate (23.976, 24, 25, 29.97, 30, 50, 59.94, 60). Default 1920×1080 at 30/1 when the project starts with audio or stills. Both are editable later. Changing the frame rate requires every track to be unlocked (rejected otherwise). It converts every clip's start and end to time, rounds each to the new rate, and sets `length = end − start`; because abutting clips share a boundary time they round to the same frame, so no overlaps or new gaps appear. A clip that rounds to zero frames is given one frame and every clip on every track that starts at or after that clip's end shifts right by one frame (link groups stay aligned), which is reported. Fades and still lengths convert as durations the same way, link groups move as one, and `source_out` is adjusted if a recomputed length no longer matches.

**Commands**: `enum Command { InsertClips, RemoveClips, MoveClips, TrimClips, SplitClips, SetSpeed, SetVideoEdits, SetAudioEdits, SetStillLength, SetClipEnabled, Unlink, ApplyClipSession, SetTrackLocked, SetTrackMuted, SetSequenceSettings, RelinkMedia, Batch(Vec<Command>), ... }`, each with `apply(&mut Project) -> Result<(), Rejection>` and `revert(&mut Project)`. Plural commands take a link group. The undo stack is a `Vec<Command>` plus a cursor. The UI never mutates the project directly.

## Data flow

### Scrub / show frame at t

UI sets playhead → `dusk-engine` works out which source frames frame t needs → cache hit or decode via `dusk-media` → passes the frames and the `Sequence` to `dusk-render` → composite on GPU → texture handed to Slint via `slint::Image::try_from(wgpu::Texture)`.

- The preview draws the sequence's frame at the largest size that fits the preview area, so a clip is fitted or fills relative to the sequence exactly as on export, and a gap is a black frame of that shape; the preview's own background shows around it.
- A frame drawn at another size than the sequence's (the preview, an export at a preset size) has its sides rounded, so its shape can be a hair off the sequence's (16:9 at 480p is 854×480). Each picture is fitted or made to fill against the sequence's own shape and the result stretched onto the frame, so a picture of the sequence's shape covers the frame whole.
- Each clip's picture goes through one mapping from output pixels to source texels (`dusk-render`'s placement): the file's orientation, the clip's crop in upright pixels, its turns and flips, then fit or fill. The two resampling passes read along whichever source axis the output axis follows, so a quarter turn costs nothing extra.

### Playback

`dusk-audio` starts the output stream and becomes the clock. On each tick it reports the current time; the scheduler in `dusk-engine` fetches and schedules the matching video frame. Video follows audio, never the other way around, so there is no drift. Speed changes the clock rate and the resampler. In practice the mixer thread fills the device's ring buffer before starting it, and the clock counts the frames the device has played, interpolated between callbacks and corrected for the output latency; the video thread reads the clock at least every 10 ms, draws the frame it points at and decodes the next one ahead. When nothing is audible (a muted speed, no device) the clock runs on the system's monotonic clock.

- Forward 0.1x to 8x: sequential decode. Audio is audible from 0.25x to 4x playback speed (resampled, pitch follows speed; no pitch correction in 0.1), muted outside that range.
- **Clip speed vs playback speed.** A clip's `speed` is part of the edit and is always rendered: its audio is resampled with pitch following speed, in preview exactly as on export, at any value in 0.1–32x. The J/K/L playback factor multiplies on top; **muting looks only at the playback factor** (audible from 0.25x to 4x forward, 0.25x to 2x reverse), so an 8x clip at normal playback sounds in preview as it will in the file. The switch to keyframe-only decoding uses the combined rate (above 8x forward or 2x reverse) in preview; export decodes every frame regardless of clip speed.
- Reverse up to 2x: the decoder decodes the GOP containing *t* forward from its keyframe into a reverse buffer (part of the frame cache budget) and frames are emitted backward. Of a GOP that does not fit in half the cache cap (long-GOP screen recordings), only the last frames before *t* that fit are kept; the frames before them are decoded without being copied, and the group is decoded again from its keyframe when playback reaches them. Decoding the next stretch on a worker while the current one plays, as the lookahead does, would smooth it; later. Audio from 0.25x to 2x reverse is reversed PCM; muted faster. The mixer thread mixes the sequence forwards 0.2 s of sound at a time, each stretch ending where the previous one began, and plays each stretch from its end; the clips' decoders stay open from one stretch to the next and are moved back for each.
- **Scrubbing**: while the user drags, show the nearest cached frame immediately and decode an exact frame at most every 16 ms; on release, the exact frame. Audio is silent while scrubbing.
- **Playback ownership**: only one window plays at a time; starting playback in the pop-out pauses the main window and vice versa. If no audio output device exists, a monotonic clock drives playback instead.
- Combined rate faster than 8x forward or 2x reverse: keyframe-only stepping. Audio follows the playback-factor rule above.

### Decoder pool

The decoder pool (`dusk-engine`) budgets decoders by size, not just count. Together they must fit the decoder budget in REQUIREMENTS.md: 300 MB for the 1080p class, and for one larger source a single pool of up to about 550 MB. Opening a decoder that would exceed it closes the least recently used first, and any decoder idle for 5 s is closed. These rules are the app's; `dusq` has its own (see "Compress tool paths").

**Video size classes**, by frame area:

- **Up to 2.1 Mpx (the 1080p class): at most 2 video decoders.** At most one video clip is visible at a time (see "Fit and rotation"), so one decoder serves the visible clip and the other the next clip that will become visible, once it is within 2 s. While playing forwards, a short-lived worker thread opens that next clip's decoder (or takes the pool's decoder of its media when no clip in view uses it), decodes up to the frame the clip comes into view with and hands both back: the frame goes into the cache and the decoder joins the pool at the cut, so neither that frame nor the ones after it wait for decoding. One worker runs at a time and its decoder counts toward the two.
- **Above 2.1 Mpx and up to 9 Mpx (4K): one decoder and no lookahead**, which is why 4K is "usable" rather than "smooth" in 0.1.
- **Above 9 Mpx (6K/8K video): one software decoder on one thread**, opened only if its pictures fit the decoder and cache budgets. Such a decoder holds up to 6 pictures for a conforming stream at that size besides the 3 Dusk holds, so import accepts the source when 9 of its 4:2:0 pictures fit under the ceiling: 8-bit 8K does, 10-bit 8K and 10-bit 6K do not. A source that does not fit is refused with a message suggesting `dusq compress` to make a smaller copy first. A stream that keeps more pictures than its level allows is not caught by this estimate.

**Audio**: at most 4 audio decoders (their buffers are small).

**Threads** are set explicitly: 1 for hardware decoders, `min(4, cores / 2)` for software decoders, 1 above 9 Mpx.

**Software decoding.** In 0.1 video is decoded in software. The D3D11VA path stays in `dusk-media` (`Acceleration::Hardware`) for zero-copy D3D11-to-wgpu sharing, a later optimization where hardware decoding can pay off, and for machines where software decoding cannot keep up. A hardware decoder allocates its whole frame pool on open (on integrated graphics that pool is system RAM); the size classes above are drawn for those pools. Decoded frames are uploaded to `wgpu` as NV12 or P010.

#### Stills

Stills are different, and the 9 Mpx rule applies to video only. Most decoders produce the native size (JPEG can decode at 1/2, 1/4 or 1/8 and does when the target allows), so a photo (typically 12–50 Mpx) is decoded, scaled at once on the CPU through the same `swscale` settings as dusq to the size the current render needs (sequence size for the timeline, native for a native-size export or a crop), the decoded frame is released, and only the scaled frame stays in the cache under a key that includes its size.

- Timeline entries are NV12 like video frames. A native-size export of an RGB or 4:4:4 photo uses a full-chroma planar 4:4:4 8-bit entry instead (`SWS_FULL_CHR_H_INP` set for RGB input), so chroma is subsampled only once, in the export conversion pass.
- Every still entry carries its own color tags (matrix, range, chroma siting, primaries, transfer). Entries keep their source's chroma siting (JPEG's is centered), which `Picture` carries and the compositor reads, video frames included.
- The key includes its size, pixel layout and crop, so an NV12 entry, a 4:4:4 entry and two different crops of the same photo never collide. A still is decoded again when the crop or target size changes.
- Native-size crop entries are clamped to the GPU's maximum texture dimension (downscaled, aspect preserved) so they can always be uploaded.
- The brief decode peak is computed from the actual pixel format (1.5 bytes per pixel for 8-bit YUV 4:2:0, 8 bytes for a 16-bit RGBA PNG, so a 50 Mpx photo peaks anywhere from 75 to 400 MB), plus about twice the frame size for progressive JPEGs (the decoder holds coefficient buffers that reduced-size decoding does not shrink). It must fit in 192 MB, half the default cache cap, whatever cap the user sets (see "Settings"), otherwise the still is refused with a message; it is budgeted as a transient inside the cap.

#### HEIC grids

iPhone HEIC photos are stored as grids of HEVC tiles; `dusk-media` reads the grid geometry through its ffi module, decodes the tiles one at a time and copies each into the stitched frame in Rust.

- The probe takes a file as a HEIF still when FFmpeg finds a tile grid in it or its brand says so (`heic`, `heix`, `heim`, `heis`, `mif1`, `avif`; not the sequence brands).
- FFmpeg (since 7.1) presents a grid as a tile-grid stream group: each tile's stream and where it goes on the canvas, the picture's window on that canvas (iPhones crop the 4096×3072 of their 512-pixel tiles to 4032×3024), and the grid's display matrix (`irot`, `imir`) and color profile.
- The stitched frame has the window's size and the tiles' format and color tags, so the scaling, orientation and peak rules for stills apply to it unchanged.
- FFmpeg can read grids but not write them, so the test file is assembled by `scripts/heif-grid.py` from tiles FFmpeg encodes.

### Color and scaling

In this order in both paths (the compositor, and dusq's CPU path), matching what a single `swscale` context does:

1. Resample each plane from its own resolution to the output size, luma and each chroma plane separately, with the tagged chroma siting (default left, MPEG-2 style, for 4:2:0) applied as a sampling offset, using a separable Catmull-Rom (B=0, C=0.5) whose support widens in proportion to the downscale ratio so big downscales do not alias (the shader runs it as a horizontal and a vertical pass per plane).
2. Convert to full-range RGB using the tagged matrix and range (defaults when untagged: BT.709 limited when width ≥ 1280 or height > 576, BT.601 limited otherwise).
3. Only for sources whose primaries are not BT.709 or whose transfer is HDR, otherwise skipped entirely: linearize (sRGB curve for photos, BT.1886 for SDR video, HLG inverse OETF then OOTF with system gamma 1.2 at the 1000-nit reference, or PQ EOTF, for HDR) → for HDR, clamp to the source peak and apply the BT.2390 EETF to PQ-encoded max-RGB with a 100-nit target, where the source peak is MaxCLL from the content light level metadata when plausible (above 0 and at most 10 000 nits), else the mastering display's maximum luminance, else 1000 nits → gamut matrix to BT.709 with clipping → re-encode: SDR sources with the same curve they were linearized with (sRGB for photos, BT.1886 for video), HDR sources with inverse BT.1886.
4. Quantize to 8-bit.

Step 3's reference math (gamut matrices from chromaticities, the sRGB, BT.1886, PQ and HLG curves, the BT.2390 EETF on max-RGB) is `dusk-core`'s `color` module, which dusq's CPU path calls and the compositor's shader mirrors; a GPU test holds the shader to it within two 8-bit codes. Steps 1–2 read the tags of the frame they are given, which for a still is the cache entry's own tags (a JPEG entry is full-range BT.601, for example), not the source file's. The preview shows the RGB after step 4.

For export, `dusk-render` runs one more GPU pass that converts RGB to limited-range BT.709 YUV in the layout the chosen encoder accepts (NV12 for the hardware encoders, planar YUV420P for the software ones; 10-bit layouts such as P010 and YUV420P10 arrive with HDR passthrough, later), with chroma downsampled by the same Catmull-Rom and sited left, and the output tagged accordingly, before readback, so the encoder never sees RGB. 10-bit and HDR (HLG/PQ) sources are tone-mapped to SDR BT.709 in the compositor for both preview and export in 0.1; HDR passthrough is later.

### Edit

UI interaction → `Command` → `apply` on the project (or a `Rejection` shown to the user) → undo stack push → view models refresh → preview invalidated for the affected time range only.

### Export

The export thread in `dusk-engine` takes a snapshot (a clone of the `Sequence` and the media list) when it starts and renders from that, so edits made during the export do not affect it. It iterates frames: render at full resolution → YUV conversion pass → readback → encode → mux via `dusk-media`. Audio is mixed offline on the same thread. While an export runs, playback and the pop-out preview are paused, scrubbing shows cached frames only (no decoding), and thumbnail jobs wait, so export decoders obey the same pool rules and the export fits the export memory budget; timeline editing stays live. Output is written to `name.ext.part` and renamed on success, so a cancelled or failed export never leaves a broken file where the user expects a video.

## Pop-out clip editor

Opening a clip opens its whole link group. `ClipEditSession { seen: Vec<Clip>, draft: ClipDraft }` with `ClipDraft { source_in, source_out, speed, still_length, video: Option<VideoEdits>, audio: Option<AudioEdits> }` copied from the group (`dusk-core`'s `session` module). `seen` is the group's clips as the session last saw them, compared with the project after each edit, so the project needs no version counter.

- **Preview.** A second Slint window is bound to the session and previews just that group with the draft applied, through the same engine: the engine keeps two previews, the main window's and the clip editor's, each with a project, a size and a compositor of its own, drawn by the one video thread from the one frame cache and decoder pool, and one transport plays either, so starting one stops the other.
- ***Apply to project*** emits one `ApplyClipSession { group, before, after }` command that sets trim, speed, edits and recomputed `length` on every clip in the group and undoes as a single step; `before` is taken from the project at apply time so undo is exact even if the clips changed meanwhile. In the pop-out `position` is fixed, so extending the start grows the clip to the right, and the fit rule above applies (cut at the gap, user informed).
- ***Export as file*** builds a temporary one-group `Sequence` at the source's snapped frame rate and rotated, cropped resolution (crop dimensions rounded down to the chosen encoder's alignment), and runs the normal export pipeline. The picture fills that frame, which has its own shape, so the rounding crops a pixel instead of leaving a bar. A still exports as a video of its `still_length` at the sequence rate; an audio-only group exports as an audio file in the audio-extraction formats. Export as file opens the export dialog (see "Export details") over the clip editor, with the group's own size and the presets below it; the save dialog then suggests "<source> edit" with the format's extension beside the project file, or beside the source while the project is unsaved. Dusk refuses a path that is one of the project's media files, so an export can never replace a source.

### The window

A double-click on a timeline clip opens it, as do Enter on the selected clip, the Edit menu and the properties panel. There is one clip editor at a time. Opening a clip of another group while the draft has changes not applied asks *Apply*, *Discard* or *Cancel* first, and so do closing the window and leaving the project (New, Open, Quit), the latter before the question about unsaved changes.

- Trimming is a bar over the whole source with in and out handles, and I and O set them at the playhead; trim points snap to the source's own frames (the sequence's for sound).
- The crop is shown and typed as the pixels cut from each side of the picture as it shows, after the turn and the flips, and kept as the model's rectangle of the upright picture; a quarter turn of a mirrored picture swaps the two flips, so the mirror stays the same way on screen.
- A trim or a speed change that shortens the clip shortens its fades, fade-in first, as a trim on the timeline does.
- Every change shows in the preview at once, while it plays too; sound edits are heard from the next play, since the mixer takes the project when playback starts, as in the main window.
- A change that a rule of the timeline refuses (a crop larger than the picture, fades longer than the clip) is not taken, and the window says why.

### Stale sessions

If a clip in the group is trimmed, moved, re-sped or edited in the main window while the pop-out is open, the pop-out shows a banner with *Reload from project* and *Keep my draft* (Ctrl+R and Ctrl+K); a draft without changes of its own simply follows the clips, without a banner. Apply stays available under the banner and lays the draft over the clips as they are now. If the group's structure changes (unlinked, or any member deleted), Apply is disabled with a notice to reopen the clip, and Export as file keeps working from the draft. A split keeps the first half under the clip's id and in its link group, so it counts as an edit of the group: the pop-out offers Reload or Keep my draft, and a kept draft applied later is cut at the gap where the second half starts.

## Export details

### The export dialog

Export (Ctrl+E, the toolbar, the File menu) opens it over the main window for the timeline, and *Export as file* over the clip editor for its clip. It offers *Video* or *Sound only* (each only when there is a picture or sound), then the file format, the codec with the encoder that will write it (on the graphics card, or in software), the picture size (the sequence's own, then each preset below its short side, so nothing is enlarged), the Quality presets and slider, the sound codec when the format holds two, and an Advanced section; for sound alone, the sound file format. The first time it opens in a session it shows that it is checking the encoders while the probe below runs, and Export waits for it. Export… asks where with the system's save dialog, which gives the file the format's extension whatever is typed, so what is written matches its name. The dialog starts from the last export's choices in the session, and keeps its top edge while rows appear and go, so a choice never moves the rows above it.

### Containers and codecs

MP4 (H.264, HEVC, AV1), MOV (H.264, HEVC), MKV (anything), WebM (VP9 via `libvpx-vp9` if present, AV1 via any available AV1 encoder; never H.264). Audio: AAC for MP4 and MOV, Opus for WebM, AAC or Opus for MKV. Sound alone: MP3, AAC (in an `.m4a` file), Opus (in Ogg) or WAV. Exported sound is mixed and written at 48 kHz stereo, the rate Opus needs and the others take. The export dialog only offers combinations the loaded FFmpeg can actually produce.

### Encoder selection order

H.264 `h264_nvenc` → `h264_qsv` → `h264_amf` → `libopenh264`; HEVC `hevc_nvenc` → `hevc_qsv` → `hevc_amf` → `libkvazaar`; AV1 `av1_nvenc` → `av1_qsv` → `av1_amf` → `libsvtav1`. The hardware wrappers are always present in the build, so availability is probed by opening a real encoding session at 640×480 (NVENC and AMF reject very small frames) on a worker thread the first time the export dialog opens or dusq runs, and the result is cached for the session; this keeps the probe off the startup path. FFmpeg cannot report an encoder's size limits, so the probe only answers whether the encoder opens; the limits table holds fixed values per encoder (conservative ones for AMF, where older AMD cards cap H.264 at 4096×2160), and the fall-through on open failure covers whatever the table gets wrong. If the chosen encoder then fails to open at the real export size, the export falls through to the next encoder in the order and tells the user which one it used.

### Encoder constraints

These live in a per-encoder table in `dusk-media` next to the quality mapping: accepted pixel formats, bit depth, maximum width and height, alignment, maximum frame area, and maximum luma samples per second. Every export, including native-size ones, is clamped to that table:

- frame area per codec (H.264 ≤ 9.4 Mpx at level 5.2, HEVC and AV1 ≤ 35.6 Mpx, VP9 ≤ 35.6 Mpx at level 6.2);
- luma sample rate (H.264 level 5.2 allows about 531 M samples/s, which binds at 4096×2160 at 60 fps, so beyond that the frame rate or the size comes down);
- each side's maximum (hardware H.264 encoders cap width and height at 4096, so a 5120×1440 frame is downscaled even though its area passes);
- alignment (multiples of 8 for Kvazaar, 2 for the rest).

Downscaling preserves aspect. OpenH264 and Kvazaar are 8-bit only, which fits: **all 0.1 exports are 8-bit SDR**, from both binaries.

### Quality

The user sees one *Quality* slider, 0–100, with presets High (80), Medium (60), Small (40). `dusk-media` maps it per encoder to that encoder's own control: a constant-quality mode where one exists (NVENC, QSV, SVT-AV1, VP9, Kvazaar QP; AMF through its quality/QP options), and a bitrate ladder keyed by resolution and quality where none does (OpenH264). The mapping table lives in `dusk-media` (`formats`) and is tested per encoder.

- The slider sets an H.264 or HEVC quantizer of 37 − 0.2 × level (21 at High) and an AV1 or VP9 one of 51 − 0.3 × level (27 at High), as NVENC's `cq`, QSV's ICQ, AMF's QP (AV1 on AMF counts to 255, so four times that), Kvazaar's QP, SVT-AV1's and VP9's CRF. OpenH264 gets 0.03 + 0.0015 × level bits a pixel (0.15 at High).
- A target bitrate peaks at 1.2 times it with a buffer of one peak second, except on SVT-AV1, which takes a peak only in its constant-quality mode and refuses to open with one otherwise, so it gets the target alone with a buffer of one second.
- SVT-AV1 runs at preset 8 on at most two pictures at once (`lp=2`), and above the 1080p class on one without its lookahead (`lp=1:lookahead=0`), which bounds its memory (REQUIREMENTS.md, "During export").
- VP9 runs at `good` and `cpu-used` 4 on 4 threads with row threading (FFmpeg's libraries default to one thread, which libvpx takes as it is).
- Kvazaar runs at its `fast` preset.
- The hardware AV1 mappings have not run on a card that has those encoders yet.

An **Advanced** section offers a target bitrate for every encoder and a raw CRF only for encoders that have one. Resolution presets (1080p/720p/480p) and the compress ladder refer to the **short side**, so a portrait sequence at 1080p exports as 1080×1920. Every export dimension is rounded down to the chosen encoder's alignment, and the per-encoder area and side caps above apply to stills and any other native-size export.

### Compress tool paths

#### In the app

The app's compress tool builds a one-clip sequence and uses the normal GPU export path (same compositor, same tone-mapping, same ladder), always at a fixed frame rate. File → *Compress a video…* (Ctrl+M) asks for a video with the system's open dialog and reads it on a worker while one dialog opens, which shows its length, picture size and file size and aims it either at a file size, in whole megabytes with 8, 25, 50 and 100 MB at hand (25 MB to start with, or half a video already smaller than that), or at the Quality slider at the video's own size. For a size it says what the file will come out as (the picture size the ladder gives, the video and sound bitrates), and below the floor it names the smallest size, rounded up to a whole megabyte, with a button to take it, and Compress waits. The file is MP4 with H.264 and AAC, which every player takes, saved with the system's save dialog as "<name> compressed.mp4" beside the video by default, never over the video or another media file of the project, and the project is never touched (`dusk-engine`'s `compress` module and `Engine::compress`). Progress and cancelling are the export's; a second pass is announced (`ExportEvent::Again`), and the end says the file's actual size.

#### dusq's CPU path

`dusq compress` uses the CPU path (`dusk-engine`'s `transcode` module), in this order:

1. Decode in software, as the app does in 0.1: `min(4, cores / 2)` threads for the 1080p class, 2 above it, 1 above 9 Mpx; `--threads N` overrides, with a note that more threads mean more memory.
2. `swscale` performs step 1 of "Color and scaling" (per-plane resampling with siting, explicit defaults, Catmull-Rom with widening support; never swscale's own defaults), scaling to the output size transposed for 90°/270° orientations since rotation comes after, and normalizes every decoder output into one layout, planar 16-bit YUV 4:4:4 (`yuv444p16`) that keeps the source's matrix and range and only shifts its values up.
3. Rust (`dusk-core`'s `planar` module) performs step 2 with `yuv_to_rgb` (swscale's own RGB output is not exact enough; DECISIONS.md), step 3 when needed and step 4, then the orientation, so the output is upright 8-bit SDR RGB with no rotation tag, in bands of rows on up to 4 threads. Step 3 runs through `SdrConverter`, which splits the reference functions into tables for what acts on one channel (every 16-bit value decoded, the tone curve above the knee and the output curve both looked up by square root) and the little that needs the whole pixel; it stays within 0.02 of an 8-bit code of the reference.
4. `swscale` converts it to the encoder's pixel format (limited-range BT.709, chroma sited left).
5. Encode.

A decoding thread does all of this up to the encoder's conversion and hands frames on through a 4-frame bounded queue; the calling thread encodes them, with the source's first sound stream at 48 kHz (AAC at 128 kbps, Opus for WebM). No frame cache. dusq passes variable-frame-rate timestamps through unchanged unless `--fps` is given, on a 90 kHz clock, and always hands the encoder the source's average frame rate so hardware rate control can hit the target size; a packet that comes from the encoder without a duration gets one frame at that rate. With `--fps`, each output frame shows the last source frame that has started by then.

#### The command line

`dusq compress <video>` (`-o`, `--quality`, `--short-side`, `--format`, `--codec`, `--fps`, `--threads`, `--overwrite`) and `dusq extract-audio <file>` (`-o`, `--format` mp3, aac, opus or wav, `--overwrite`), `--` ending the options for a file whose name starts with a dash, and `--size` (such as 25MB, 800KB or 1.5GB, a bare number being megabytes) compresses to a size instead of a quality. By default a job writes beside its input under a name that replaces no file, and dusq never writes over its input, even with `--overwrite`. Ctrl+C stops the job and removes its part file (Windows' console handler; other systems with their builds), exiting with 130. FFmpeg's own log is kept off the console; Kvazaar and SVT-AV1 still print their settings, since they write to the console themselves and read `SVT_LOG` from an environment the DLLs copied before dusq ran. The CLI prints an estimate of how long a job will take.

#### Equivalence of the two paths

The two paths cannot be bit-identical even with the same kernel, so equivalence is a tolerance: pre-encode YUV frames from both paths on the same source and output size must reach PSNR ≥ 45 dB when unscaled and ≥ 40 dB when scaled, computed per plane with every plane required to pass, on a sample set that includes an untagged HD clip, a rotated phone clip, a Display P3 photo, an HDR clip and a 4K-to-480p downscale. A `dusk-engine` test (`equivalence`) checks it in CI with the `gpu` feature on: it builds both paths' frames the way an export and dusq do, from `testdata`'s untagged HD clip, turned phone clip, Display P3 photo and HLG clip, and a 4K clip it writes.

### Target file size

For the compress tool: video bitrate = (target bytes × 8 × 0.97 − audio bitrate × duration) / duration.

- Resolution is capped by a bitrate ladder: ≥ 6 Mbps 1080p, 2.5–6 Mbps 720p, 1–2.5 Mbps 480p, 0.5–1 Mbps 360p, below that 240p.
- Audio is AAC 128 kbps, 64 kbps when video is under 1.5 Mbps, 48 kbps when video is under 0.5 Mbps.
- Single-pass VBR with `maxrate` 1.2× and a matching buffer, since hardware encoders cannot two-pass. If the output overshoots by more than 3%, re-encode once with the bitrate scaled down by the overshoot; after that, keep the result and report its actual size. Both passes write the same part file, so the file is renamed into place only once. On a short clip the container's own share of the file is more than the 3% the plan leaves it, so the first pass comes out over and the second runs.
- The floor is 240p at 200 kbps video plus 48 kbps audio; a target below that floor shows "smallest possible is X MB" and asks whether to use it: the compress dialog offers it with a button, and `dusq` asks on a console and otherwise says which `--size` would do. The sizes said are rounded up, so taking one is never refused.

The planning (`dusk-engine`'s `size` module) is shared by both paths.

### Optional GPL encoders

The export dialog's Advanced section takes a user-supplied `ffmpeg.exe` (a GPL build), picked with the system's file dialog and asked for its encoders (`ffmpeg -encoders`) on a worker thread. When it has `libx264` or `libx265`, the dialog offers it for H.264 or HEVC beside Dusk's own encoder for the rest of the session, and the settings keep the path.

- Dusk still renders every frame itself: each is drawn and converted to limited-range BT.709 NV12 exactly as for its own encoders and piped raw into the program's standard input, tagged as such (chroma sited left). The frames reach the program through a thread of their own, one waiting at most, so a cancel is heard even when the program stops reading.
- The program reads the sound from a WAV file mixed first, beside the part file (`name.ext.part.wav`), writes the part file, and reports `-progress` on its standard output, which drives the progress bar.
- The quality comes from the encoder table that serves Dusk's own encoders (`dusk-media`'s `EXTERNAL_ENCODERS`, which no export of Dusk's own ever tries): the slider's H.264/HEVC quantizer as x264's and x265's CRF, a raw CRF in Advanced, or a target bitrate peaking at 1.2 times it; the table's limits are the codecs' levels.
- The program's inputs are limited to the pipe and files (`-protocol_whitelist`), and files go to it as `file:` URLs, so it never opens a URL.
- Cancelling stops the program; when it fails, its last lines of errors are shown; either way the part file and the WAV are removed. Its memory is its own process's, outside Dusk's.
- No DLL swapping, no link-time dependency, and the GPL binary is a separate program the user installed. The tests drive the pinned build's own `ffmpeg.exe` through the same path with OpenH264, since getting a GPL build is the user's choice.

## Memory discipline

- In the app, one budget for frames: decoded frames, the reverse-playback buffer and in-flight export frames all live inside the frame cache cap (default 384 MB, user setting), an LRU keyed by `(media_id, timestamp, size)` (size matters for stills, which are cached at the size they were rendered at). Nothing else holds frames.
- Thumbnails have their own small cap (64 MB): each is one decoded frame (a video's keyframe a tenth of the way in, at most a second in; a photo decoded just large enough) shrunk on the CPU to 128 px on its long side by averaging the samples each pixel covers, taken through color steps 2 to 4 with `dusk-core`'s reference math (`yuv_to_rgb`, then `color::to_sdr_bt709`, the shader's step 3 for one pixel) and turned upright; the app keeps them as RGBA images, about 50 KB each, the oldest going first over the cap.
- **The cache owns its memory.** Every decoded frame is copied into a cache-owned NV12 or P010 buffer and the decoder's frame is released at once, so evicting from the cache really frees memory instead of returning it to a decoder's private pool.
- Frames are kept as **NV12 / P010**, not RGBA, and converted in the shader: half the bytes. On integrated GPUs texture memory is system RAM, so it counts.
- **The compositor keeps its GPU memory.** Graphics drivers keep memory that is freed and allocated again at frame rate, so the compositor allocates once and reuses: its upload and intermediate textures, two upload buffers written in turn, and three output frames in rotation (a frame handed out stays intact until two newer ones have been drawn; the preview only shows the newest, and since the compositor and Slint share one queue a reused frame is never shown half-drawn). The export pass keeps its targets and readback buffers the same way.
- Decoder pools are a separate budget in REQUIREMENTS.md (300 MB for the 1080p class, the single ~550 MB ceiling for one larger source), enforced by the decoder pool rules above. Encoders have their own line in the export budget (REQUIREMENTS.md, "During export").

## Project file and autosave

JSON (via serde), pretty-printed, with a top-level `"version": 1`. Human-readable and diffable.

- Media is referenced by `MediaId`, listed once in `media`, never copied. Media in the project file's folder or below it is saved relative to that folder with `/` between parts, so the folder can move as a whole; other media keeps its absolute path.
- Positions, lengths and fades are frames at the sequence rate, source times and durations microseconds, rates fractions such as `"30000/1001"`, rotation degrees; whole numbers stay within 2^53, which keeps every sum Dusk forms from them from overflowing. Floats are read back to the last bit (`serde_json`'s `float_roundtrip`), since a clip's length must keep matching its speed.
- The format is defined in `dusk-core`'s `file` module on types of its own, apart from the model, and a test pins version 1 byte for byte, so a model change cannot change the format unnoticed; any change to the format raises the version, once per release (a format that only an unreleased build wrote is not kept).
- Reading a file checks it against every timeline invariant (the commands' clip checks, no overlaps, video tracks before audio tracks, unique ids, link groups in lock-step); a file that breaks one is refused with the reason, never patched up, and a file from a newer format version says so. Clips listed out of order are put in order, which changes nothing on the timeline.

Saving and autosaving write `name.part` and rename it over the file once it is flushed, as exports do, so a crash mid-write never leaves a broken project; the writes run in order on a file worker thread, never on the UI thread.

Autosave is a full snapshot written every 30 s while the project has unsaved changes, into Dusk's own folder (`%LOCALAPPDATA%\Dusk\autosave` on Windows; `DUSK_STATE_DIR` moves it, which the tests use) rather than next to the project. Every running Dusk keeps a session there: a record naming its project file, locked (the standard library's file locks, which the system releases when a process dies) for as long as that Dusk runs, and its autosave. Relative media paths in an autosave are relative to the project's folder, as in the project file.

- A clean exit removes the session's files, and so does undoing back to the saved state (the autosave is removed at the next tick).
- A record nobody holds belongs to a Dusk that stopped without closing its project, and at the next start its autosave is offered for recovery (Recover, Discard, or Not now, which keeps it for the next start), unless the project file was saved after it or there is no autosave, in which case it is cleared away. A recovered project opens with unsaved changes.
- Closing with unsaved changes asks Save, Don't save or Cancel; choosing Don't save ends the session normally, so nothing is offered next time.

## Keyboard and settings

Dusk can be used from the keyboard alone, except to drag clips. `dusk-app/tests/keyboard.rs` holds it to that: it starts the real binary on a saved project, edits it with keys posted to the window and saves it through the question that closing asks.

### Shortcuts

One table in `dusk-app` (`shortcuts::ACTIONS`, one entry an action with its group) holds every action with its default keys and what it does; menu items and buttons name the same actions, and a test keeps every action they name in the table. The keys in use are `keymap::Keymap`, which both windows' key handling, menus and shortcut list go through. Every action has a stable name (`play-pause`, `split`, `mute-v1`), which the shortcuts file and the shortcut list use.

- Keys are written as modifiers and a key joined by `+`, such as `Ctrl+Shift+Z`, where the key is a letter, a digit, a punctuation character or a named key (`Space`, `Enter`, `Esc`, `Tab`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Right`, `Up`, `Down`, `F1` to `F12`, and `Comma` and `Plus`, since those two characters set keys and modifiers apart in the file).
- Shift with a punctuation key stands for the character it types on a US layout, as a press of it arrives (`Shift+/` is `?`).
- Letter and digit keys match by the key's place on the keyboard (its virtual-key code), so they work under any layout; punctuation matches by the character typed, with or without Shift, and where the layout types something else on that key (Arabic types ؟ where a US layout types ?), by what the key types on a US layout.

### Remapping

`shortcuts.txt` in Dusk's settings folder holds the keys the user changed, one action a line (`split = S`, or `redo = Ctrl+Shift+Z, Ctrl+Y` for two keys); every other action is written there as a comment with its default keys, so the file names every action, and a later version's new defaults still reach a user who never changed them. The first time Dusk starts it writes the file with every action as a comment, ready to change.

- A line Dusk cannot read (an action it does not know, a key it cannot read, a key an earlier line took) is skipped and reported when Dusk starts, and that action keeps its default. What Dusk cannot read is said in the status line and at the top of the shortcut list, which stays where a later message would cover the status line.
- The shortcut list changes keys too: pick an action and press its new keys, Tab among them; a key another action has is taken from it only when pressed a second time; *Remove* and *Default* do what they say. Dusk writes the file when keys change there (at once, on the file worker) and reads it once when it starts.

### The shortcut list

It opens with `?` and F1, a toolbar button and Help → Keyboard shortcuts, in either window. It groups the actions (Playback, Editing, Clip editor, Tracks, View, Project), narrows them to what one types (what they do, their keys, or their name in the file), and changes keys as above. In the main window it is a dialog (`shortcut_dialog`, with its logic tested apart from the window) whose search field has the keys when it opens; Up and Down pick an action, Enter or a double-click waits for its new keys (the keys leave the search field meanwhile, and Escape stops waiting), and the list keeps the picked action in view. The clip editor's `?` shows the list without changing it.

### Without the mouse

Tab and Shift+Tab move through every control of a panel or dialog, which shows a focus ring in `primary-bright`; the arrow keys change a segmented choice or a slider, Space or Enter presses a button, and Escape gives the keys back to the shortcuts. Keys the focused control does not use go on to the shortcuts, so Space still plays while a button has focus.

- Every control in `widgets.slint` has a focus scope that Tab reaches and a click does not take, so the mouse leaves the keys with the shortcuts. Number fields select their number when reached and count with Up and Down. The frame rates in Sequence settings are one control for Tab, chosen with the arrow keys (Left and Right along a row, Up and Down between rows).
- A click on a button leaves the keyboard in a number field, so the number fields of the export, compress and sequence dialogs, whose numbers are only choices until a button takes them, report each number as it is typed. The Settings dialog applies a typed cache size as it closes. The clip editor's and the properties panel's fields apply on Enter or when the keyboard leaves them, since the preview shows whether a number took and a speed taken as it is typed would cut the clip at the gap on the way.
- Slint walks the whole window with Tab, so while a dialog is open a per-window `Modal` global keeps the controls behind it out of reach, which keeps Tab in the dialog and Enter off any button behind it, and the keys go back to the window whenever a dialog opens or closes. A question (closing with unsaved changes, say) is drawn over every dialog and takes the keys first, and while it is open only its own buttons are reached with Tab (`Modal` has a question level). While a dialog is open the menu's other actions wait, and the project keys the clip editor passes on say so there, except Quit, which asks its question over the dialog.
- Keys for editing without the mouse (`navigation`): Up and Down go to the previous or next cut (any clip's start or end); Shift+Left and Shift+Right move a second; D selects the clip at the playhead, the topmost first and the next one down when pressed again; Ctrl+Shift+A selects nothing; Alt+Up and Alt+Down pick the previous or next media in the bin; I and O start and end the selected clip at the playhead, as they do in the clip editor; and placing media moves the playhead to the end of the clip it placed, so placing again puts the next clip after it.
- The playhead can stand just past the last frame, where the preview is black, so that placing appends and the last cut can be reached; End still goes to the last frame. Keys that move the playhead scroll a zoomed-in timeline to keep it in view, as playback does. The media bin scrolls to keep the selected media in view.
- Shift+Left and Shift+Right work in the clip editor too; the cut and selection keys mean nothing there.

### Settings

File → Settings… (Ctrl+,) holds:

- the frame cache's size (384 MB by default, applied at once: a smaller cap gives back what lies above it; the field counts in steps of 64 MB with Up and Down);
- the export dialog's default (file format, codec, picture size, quality), which it starts from in each session until an export there changes it;
- the user's own `ffmpeg.exe` (its path, whether to use it, and what it has, checked on a worker when Dusk starts and when it is picked), which the export dialog's Advanced section also picks; both places keep it.

`settings.txt` holds them as `name = value` lines, with comments saying what each takes; a value Dusk cannot read is reported and its default used. The dialog (`settings_dialog`, with how each control changes the settings tested apart from the window) writes the file on the file worker after every change, and the first start writes it with the defaults, ready to change.

- Changing the export default makes the next export start from it, not from the session's last export. The export dialog's choice between Dusk's encoder and the user's program lasts for the session, as its other choices do, while the Settings switch is what the next session starts with.
- A program picked in either place is kept once it lists its encoders as `ffmpeg` does (with or without x264 and x265), and one that does not leaves the program in use, if any, as it was; a check overtaken by a later pick or by *Forget* changes nothing. When the kept program cannot be run as Dusk starts (on a drive that is not there, say), the status line says so and the setting stays, so it works again once the drive is back.
- Each line of either file that Dusk cannot read is reported with what Dusk does instead (the default is used, or the line is skipped); the status line shows the first and counts the rest.
- The limit on decoding one still stays at half the default cache (192 MB) whatever size the cache is given, so which photos a project can use does not depend on the setting.

### Where

Dusk's settings folder is `%APPDATA%\Dusk` on Windows (the roaming profile, so settings follow the user, while autosaves stay in `%LOCALAPPDATA%\Dusk`), and `$XDG_CONFIG_HOME/dusk` (`~/.config/dusk`) elsewhere; `DUSK_SETTINGS_DIR` moves it, which the tests use. Both files are plain UTF-8 text, small enough to read before the window shows, once, when Dusk starts; Dusk writes them on the file worker as `name.part` renamed into place, as it saves projects.

## Missing media

A project whose media files cannot be found opens anyway, and a dialog then lists them, each with *Find…*. The media bin marks a missing file and says how to find it, with the key that does it; File → Find missing media… (Ctrl+Shift+M) opens the dialog again.

- **What a found file must hold.** A file picked there is read on a worker and used only if it holds what the project's clips use: the same kind of media, sound where a clip plays some, every clip's source range inside it (a project file whose clips reach past their media's end is refused when read, so a shorter file would leave a project that cannot be opened again), and a picture that every crop of it fits, since crops are kept in its upright pixels.
- **Relinking is a command.** `dusk-core`'s `RelinkMedia` (the media's new path and what the file holds) applies the new path and what the file holds to the project, then runs the clips' own checks (`check_clip`) on every clip of it, so source ranges and crops follow the same rules as any edit, and reports a file of another kind, one without the sound a clip plays, one too short and one too small for a crop each in their own words. It undoes like any edit, and the project has unsaved changes.
- **The others beside it.** Dusk then looks in the picked file's folder, under their own names, for the other missing files that were in the same folder as the one found, so a folder that moved, or a drive with a new letter, is relinked in one step. A missing file from another folder is not looked for there, since a file of its name in another folder is another file (cameras name files the same on every card). The picked file, and the files beside it named as those other missing ones in any case (one back at its own path too, as when a drive comes back), are read on a thread of their own, which says how far it has got and stops when the list closes or another find starts; the picked file must fit, the others that fit come along in the same undoable step, and those that do not are counted in a warning.
- **One file, one media.** The picked file is not found again for another missing file of its name, a file the project already uses is not taken, and a name two missing files share finds neither. A find whose media the project no longer names as it did when the file was asked for (another project opened, an edit undone meanwhile) changes nothing.
- **Checking.** Which files are missing is checked whenever the project's media paths change (opening a project or recovering one, a relink, undoing it, an import), on a short-lived thread, since asking about a file on a drive that went away can keep the system waiting. A newer check overtakes one under way and does what that one was to do, so the engine's report of a missing file, which can arrive first as a project opens, does not keep the list from opening. The engine reports a file gone at every frame it cannot show, so those reports start one check at a time, and one more after it when they came while it ran. When a question or another dialog is up as the list is to open (recovering an autosave as a project opens, say), the list opens once none is left open.
- **The dialog** (`missing`, whose matching and relinking are tested apart from the window) lists them with the folder each was in; Up and Down or a click pick one, and Find… (or Enter, or a double-click) asks for it with the system's dialog, which shows that file's name alone in its old folder when the folder is still there.
- **The engine.** It remembers which file each media's frames and decoder came from, and forgets them when a project names another file for that media (one relinked, or an id given to new media after an import was undone). Either preview names the file each time it asks for a frame, so the clip editor, still drafting from an older project, cannot put another file's frames in the cache under that id. The bin's thumbnails likewise belong to the file they were made from, and the bin asks for a thumbnail again once its file is found. A file that goes away under an open decoder fails as FFmpeg failing to read it; the engine then names it as missing too, for a picture (letting its decoder go), for sound while it plays, and for an export. A frame it cannot decode shows black, after saying why, rather than leaving the last picture on screen.

## Error messages

Every message a user can see says what happened and what they can do (CLAUDE.md, "Code style").

- A media file that goes away while Dusk runs (an unplugged drive) is named with how to find it, and the bin marks it; `EngineError::missing_file` tells the app which file.
- The engine's errors reach the status line as sentences, capitalized like the app's own.
- Where a message is passed on in a place its advice cannot be followed, the advice is left out: a project file that breaks a timeline rule names the rule (`Rejection::what`) without saying to trim a clip in a project that does not open, a failed recovery says what is wrong with the autosave (`FileError::what`), and a file picked in Find that cannot be read asks only for the file the project used.
- `dusk-app/tests/messages.rs` reads every crate's sources for gaps of spaces in strings (a line's lost `\`), following strings across lines as Rust reads them.

## Release

- **Logo and icon.** The logo of THEME.md, the four brand colors as horizontal bands (violet on top, then magenta, pink and a thin gold stripe) in a rounded square, is drawn by `scripts/make-logo.py` from its geometry with Python's standard library alone. It writes `dusk-app/assets/dusk.ico` at each size Windows asks an icon for (16, 20, 24, 32, 40, 48, 64 and 256 pixels), each drawn on whole pixels rather than scaled down; `dusk-app/assets/dusk.svg`, the mark as shapes, which the windows' title bars and the toolbar (beside the wordmark) show at the size asked for; and `docs/images/logo.png`, the README's. `dusk-app/build.rs` writes the icon and the version information (what the file is, its version from `Cargo.toml`, its license) into a resource file, which the MSVC linker takes like an object file, so the build needs no resource compiler and no crate for it; the icon shows in Explorer, the taskbar and the installer's shortcuts, and a test reads both back from the built `dusk.exe` with Windows' own calls. `dusq.exe`, a command-line tool, keeps Windows' plain icon.
- **Theme.** Every view (the main window, the clip editor, every dialog) takes every color, font size, weight, border and radius from the theme's tokens (THEME.md); the two pill shapes, the export's progress bar and the clip editor's trim handles, are rounded by half their own size. Sizes that belong to one view's layout, such as a list's row height or a dialog's width, are written in that view. Both status lines show three kinds of message: what happened in `text-2`, warnings (an edit that did more than was asked, such as a cut at the gap) in `signal`, and errors in `alert`.
- **About.** Help → About Dusk (Shift+F1) opens a dialog (`about`): the mark and the wordmark, the version, the MIT license, what Dusk is built with (Slint, with the `AboutSlint` widget its license asks for; FFmpeg under the LGPL, linked dynamically; Inter and JetBrains Mono under the OFL), a button that opens the third-party notices installed beside Dusk, and one that opens the releases page in the browser. The system opens both, so Dusk itself still makes no request (REQUIREMENTS.md, "Offline and private"). The releases page goes through Slint's own `Platform.open-url`, which hands the link to the system (the `webbrowser` crate Slint already uses, on `ShellExecuteW`) and says whether it took; when it did not, the status line gives the address. *Licenses* shows the `licenses` folder beside `dusk.exe` in Explorer and appears only when that folder is there, which a development build lacks. Dusk sets Slint's palette to dark at start, so `AboutSlint` shows its dark-background logo whatever Windows' own choice.
- **Installer.** `installer/dusk.iss` builds one Inno Setup 6 installer with solid LZMA2 compression: `dusk.exe`, `dusq.exe` and the five FFmpeg DLLs beside them, so nothing depends on PATH, and a `licenses` folder. It installs for the current user without administrator rights (`%LOCALAPPDATA%\Programs\Dusk`), or for all users when asked, adds a Start menu entry (and a desktop one if asked), opens `.dusk` projects with Dusk if asked, and uninstalls cleanly; settings and autosaves are the user's data and stay. Neither the installer nor the binaries are code-signed, so Windows' SmartScreen warns about the download; the README says how to get past it, and signing is planned (ROADMAP.md, M7).
- **Licenses.** The `licenses` folder holds Dusk's MIT license, FFmpeg's LGPL (with the GPL it builds on), the fonts' OFL, Slint's license, and:
  - `THIRD-PARTY-NOTICES.txt`, the license of every Rust crate in the two binaries, written by `scripts/third-party-notices.py` from `cargo metadata` (asked for Windows' packages alone) and the crates' own license files;
  - `FFmpeg.txt`, which names the exact BtbN download with its SHA-256, FFmpeg's commit, BtbN's build scripts for that release and the configuration, which the release script reads from the avutil DLL itself;
  - `FFmpeg-libraries.txt` (from `installer/ffmpeg-libraries.txt`), the license texts of the libraries built into FFmpeg's DLLs, which the BtbN download lacks. `scripts/ffmpeg-licenses.py` fetches them for the pinned build: it asks BtbN's build scripts at the pinned tag which libraries the build contains, with their own logic (`scripts/btbn-components.sh`), and reads each one's license files at the commit built, from its own repository. Two of them, rav1e and librsvg, are written in Rust, so the file also lists the crates their builds for Windows depend on as cargo resolves them, with the code that lists Dusk's own (SETUP.md, "Release").
- **Building and checking it.** `scripts/build-release.ps1` builds both binaries in release (dusq on its own, as CI does), stages the files and runs the Inno Setup compiler. CI builds the installer on every push to a branch and keeps it as an artifact, and a pushed `v*` tag that names the version in `Cargo.toml` makes a draft release with the installer attached, for the maintainer to publish. `scripts/check-installer.ps1` tries every installer CI builds: it installs it silently into a folder of its own, checks that every staged file arrived, that the Start menu has Dusk and that `.dusk` files open with it, compresses a clip with the installed `dusq` and opens the installed `dusk.exe` with no FFmpeg on PATH, then uninstalls and checks that nothing is left. It refuses to run where Dusk is installed, since it ends by uninstalling it. The cold start, the download and the installed size are measured against REQUIREMENTS.md.
- **Docs.** The README says what Dusk is, with screenshots of a demo project made from FFmpeg's own test sources (a gradient, a Mandelbrot set, a game of life, a chord), how to install it and how a first edit goes. `docs/SHORTCUTS.md` lists every action with its keys; a test in `keymap` writes it from `Keymap` when `DUSK_WRITE_SHORTCUTS` is set and otherwise fails when the two differ. `CONTRIBUTING.md` gives contributors the rules from CLAUDE.md (setup, the checks CI runs, code style, commits), and SETUP.md says how to build the installer.

## FFmpeg (Windows)

- Link dynamically to the prebuilt **BtbN LGPL shared** FFmpeg, pinned to one exact **8.1** asset (tag, file name, size and SHA-256 in `scripts/ffmpeg-pin.psd1`, mirrored in `docs/SETUP.md`, installed and verified by `scripts/setup-ffmpeg.ps1`); `ffmpeg-next` finds it through `FFMPEG_DIR` at build time. That build includes `libopenh264`, `libkvazaar` and `libsvtav1`. NVENC in 8.1 needs NVIDIA driver 570 or newer, which GTX 900/1000 cards still receive (their last branch is 580). BtbN's 9.0 and master builds are compiled against newer NVENC headers that need driver 610 and would drop those cards (the floor comes from the header choice, not from FFmpeg itself; a custom build could keep 570), so moving the pin is a deliberate, documented decision.
- FFmpeg's protocol whitelist is set to `file` in both binaries, so neither `dusk` nor `dusq` will ever fetch a URL handed to it.
- The MSVC runtime is linked statically (`crt-static`); the BtbN DLLs are MinGW-built and need no runtime of their own, so the installer ships nothing but Dusk and the DLLs.
- `ffmpeg-next` exposes neither swscale's scaler parameters, color matrix and range, and chroma positions, nor the HEIC grid geometry. `dusk-media` has one `ffi` module, the only place `unsafe` FFmpeg calls are allowed. For swscale it uses the legacy setup sequence, which in FFmpeg 8.1 is the only one where these overrides take effect: `sws_alloc_context` → `av_opt_set` (`sws_flags`, scaler parameters, chroma positions, source and destination range) → `sws_init_context` → `sws_setColorspaceDetails` → `sws_scale`. Dusk calls `sws_scale` on that context; it does not reach for `sws_scale_frame`. This way swscale never falls back to its own slightly different bicubic or its BT.601 default.
- Still decoders are opened with the ICC-profile flag (`AV_CODEC_FLAG2_ICC_PROFILES`) so a Display P3 iPhone JPEG, or a PNG/WebP/TIFF with an embedded profile, gets primaries tags. The flag makes FFmpeg refuse files whose profile is grayscale or CMYK (common for PNGs from Photoshop and GIMP), so `dusk-media` decodes a still without the flag first, reads the color-space field of the profile header from the frame's ICC side data, decodes again with the flag only for an RGB profile, and keeps the first decode if that one fails (an untagged photo is sRGB). Decoded video frames carry their primaries, transfer and, for HDR, the light levels (MaxCLL, mastering maximum) the tone mapping's source peak comes from; FFmpeg's mastering-display structs are declared in the `ffi` module, since `ffmpeg-sys-next` does not bind them. For HEIC the profile may sit on the grid's stream group rather than on the tiles; the `ffi` module reads it there, next to the orientation, and hands an RGB one to each tile's decoder with its packet, decoded with the flag, so FFmpeg's ICC support tags the tiles as it tags JPEGs. `lcms2` in the FFmpeg build is a hard requirement (if a pinned build lacks it, pick or build one that has it).
- Ship only the DLLs Dusk loads: `avcodec`, `avformat`, `avutil`, `swscale`, `swresample` (disable `ffmpeg-next`'s `filter` and `device` features). They are most of the installed size (REQUIREMENTS.md, "Download" and "Installed"), which an Inno Setup installer with solid LZMA2 compression brings under the download target; MSI cannot use LZMA. Building needs LLVM/Clang for `bindgen`; `docs/SETUP.md` records that. A slimmer FFmpeg build of Dusk's own is planned (ROADMAP.md, M7), toward the ~60 MB installed target.
- **Patents**: `avcodec` contains FFmpeg's own H.264/HEVC decoders plus OpenH264 and Kvazaar compiled from source. None of that is covered by a patent license (Cisco's coverage applies only to Cisco's separately downloaded binary, which Dusk cannot fetch because it has no network code). This is the position of every open-source media application and is not a 0.1 concern. If Dusk is ever sold commercially, the options are: hardware-only H.264/HEVC, dropping OpenH264, or having the user download Cisco's binary themselves from Cisco through a browser link (Firefox-style; Cisco's coverage applies only to that download, and Dusk has no network code).

## Slint specifics

- **License**: Slint Royalty-free License 2.0. Condition: the `AboutSlint` widget in an About dialog reachable from the top-level menu, and the Slint badge on the download page. Both are shipped.
- **Renderer**: Slint **1.18.1**, pinned exactly, with `default-features = false` and features `std`, `compat-1-18`, `backend-winit`, `renderer-femtovg-wgpu`, `unstable-wgpu-30`, `accessibility` and `unstable-winit-030`. That is the FemtoVG renderer on wgpu; the Skia and OpenGL renderers are not built. Slint renames its `unstable-wgpu-NN` features between minor versions, so upgrading Slint, that feature and `wgpu` is one documented decision.
- **One device.** The app selects the renderer with `slint::BackendSelector::new().require_wgpu_30(config).select()`, and the `wgpu` crate is 30, the version that feature expects. `config` is `WGPUConfiguration::Manual { instance, adapter, device, queue }` with objects that `dusk-render` creates, so the compositor and Slint share one device and queue, as the preview texture requires.
  - On Windows `dusk-render` creates a Vulkan-only instance first and falls back to a DX12-only instance when Vulkan finds no adapter; the `WGPU_BACKEND` environment variable overrides the choice for testing, and `WGPU_POWER_PREF` the adapter wgpu prefers when there are two.
  - A machine without any hardware adapter (a VM, a CI runner) gets WARP, Direct3D 12's software rasterizer: slow, but Dusk starts. Slint renders on such a CPU adapter only when `SLINT_WGPU_CPU` is set, so `dusk-app` sets it at startup; the choice of adapter stays with `dusk-render`, which prefers hardware.
  - The device asks for `wgpu::Limits::default()` (the WebGPU baseline every D3D12 GPU meets, where Slint's default settings ask only for WebGL2-level limits) and for no optional feature a target GPU might lack, since device creation would fail: NV12 planes upload as `R8Unorm`/`Rg8Unorm` and P010 planes as `R16Uint`/`Rg16Uint`, read with `textureLoad` by the Catmull-Rom shader, which filters by hand anyway.
  - Slint keeps the `Manual` objects in thread-local state that is destroyed at exit after wgpu's own thread-locals, so `dusk-app` keeps one reference to them alive for the whole process; otherwise destroying the queue during that teardown panics.
  - Sharing the device has one race: Slint reconfigures a window's surface (on a resize, or when it went stale) after waiting for the queue to empty, and if the compositor submits during that wait, wgpu reports that the GPU did not come idle and, by default, panics. The surface stays as it was and Slint configures it again on the next frame, so `dusk-render`'s device error handler logs that one error and panics on any other.
- **Importing**: files dropped on the window from Explorer arrive through winit's window events, which Slint 1.18 does not pass on to the application; the `unstable-winit-030` feature exposes them through `WinitWindowAccessor::on_winit_window_event`, and like `unstable-wgpu-NN` its name can change between Slint minor versions. The Import button opens the system's own file dialog, as do opening and saving projects: on Windows `IFileOpenDialog` and `IFileSaveDialog` through the `windows` crate, which Slint, cpal and wgpu already depend on, in `dusk-app`'s platform module; other platforms add theirs with their builds. Files named on the command line are imported as well. The same hook supplies each key's virtual-key code, which shortcuts use for letter and digit keys, as Windows' own shortcuts do: under a non-Latin keyboard layout (Arabic, Cyrillic, Greek) the L key types another letter, and with Shift sometimes punctuation, yet still plays.
- **The video preview** is a Slint `Image` fed from a `wgpu::Texture` that `dusk-render` produced on the same device and queue. The pre-approved fallback, should integrated graphics not hold 60 fps updates this way (Open questions 1), is reading the composited RGBA texture back from the GPU each frame into a `slint::SharedPixelBuffer<Rgba8Pixel>` (Slint images accept only RGB/RGBA; the compositor already produces RGBA, so no CPU color conversion is involved). Running `dusk.exe clip.mp4` with the `DUSK_STATS` environment variable set prints the frames received, the draws and the UI time per second, which is the check to run.
- Two windows (main, clip editor) are two Slint component instances sharing the app state through `Rc<RefCell<AppState>>` on the UI thread.
- **Fonts**: bundle Inter for UI text and JetBrains Mono for timecodes and numeric readouts (Slint cannot enable OpenType tabular figures, so a monospaced font does that job).

## Open questions

1. Does Slint's WGPU integration allow efficient per-frame texture updates at 60 fps on integrated graphics? Yes on discrete graphics (DECISIONS.md, "Slint specifics"); integrated graphics is still to be measured with `DUSK_STATS`.
2. Can D3D11 decoded surfaces be shared with `wgpu` without a system-memory round trip? Later optimization; not needed for 0.1.
3. Commands with hand-written `revert` versus snapshotting the model with a persistent data structure (`im` crate). Start with commands; switch if revert logic gets error-prone.

## Known issues

- **Two windows on AMD's Vulkan driver.** Each time the window that presents switches (an action that redraws both windows, not while one plays or is worked in alone), AMD's Vulkan driver keeps about 0.2 MB for good. Dusk cannot present both windows in one call (Slint presents each window by itself), and DX12 costs about 250 MB more at idle, so Dusk stays on Vulkan and the loss stays; a driver that fixes it needs no change in Dusk. The investigation is in DECISIONS.md, "Known issues".
