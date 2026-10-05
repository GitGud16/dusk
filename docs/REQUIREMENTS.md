# Dusk — Non-Functional Requirements

Constraints that shape the architecture. Targets are measured at every milestone, not at the end. Memory figures are process private bytes.

## 1. Lightweight (top priority)
- **Idle**: under ~250 MB with a 1080p project open and nothing playing. Measured at M1 (release build, Radeon RX 6750 XT on Vulkan): 229 MB with the M1 window (timeline, transport) and a 1080p clip open once its decoder has closed, 254 MB while it decodes, 200 MB for the empty window. Of those 201 MB the GPU driver's device takes about 80 MB and the UI toolkit and window about 100 MB, so the empty window is the floor of this stack; Dusk's own idle share with a clip open is about 20 MB. Measured at M2, same machine: 224 MB with a 1080p project open (three clips and music, their thumbnails in the bin) once its decoders have closed. Integrated GPUs and other vendors are measured when available.
- **During 1080p playback**: under idle + frame cache cap + 300 MB decoder budget + 64 MB thumbnails. The reverse-playback buffer lives inside the cache cap. The decoder budget holds at most 2 video decoders and 4 audio decoders; 0.1 decodes video in software, where a 1080p decoder peaks at about 32 MB (H.264) to 51 MB (10-bit HEVC), measured at M1 (a hardware pool would be about 65 MB for 1080p 8-bit, about 133 MB for 10-bit HEVC). GPU textures on integrated graphics count. With the default cache cap this is about 950 MB worst case (10-bit) and about 800 MB for typical 8-bit footage. Measured at M1 over 5 minutes of 1080p30 H.264 with sound: 681 to 684 MB, flat from the first minute on (the frame cache fills to its cap within seconds); 682 MB for 1080p60. Measured at M2 over 2 minutes of three 1080p30 H.264 clips cut one after another, with music on the second audio track: 669 to 677 MB while playing through a clip, 703 MB at most, around the cuts, where the next clip's decoder is open beside the current one.
- **4K sources**: one video decoder, no lookahead. In software it peaks at about 150 MB (H.264) to 200 MB (10-bit HEVC), measured at M1; a hardware pool would reach about 500 MB for 3840-wide 10-bit HEVC and about 535 MB at 4096 wide, so the decoder line for one such source keeps a ceiling of about 550 MB and playback stays under about 1.2 GB. 4K is usable, not smooth, in 0.1.
- **dusq**: one software decoder, as in the app (`min(4, cores / 2)` threads for the 1080p class, 2 above it, 1 above 9 Mpx), scaling and normalizing right after decode, a 4-frame queue, no frame cache. Ceiling is measured at M4 per size class (1080p, 4K, 8K) and recorded here; before the encoder line it is expected well under the app's playback budget for the same source.
- **During export**: the playback budget plus an encoder line, provisionally 200 MB for hardware encoders, OpenH264 and Kvazaar and 600 MB for SVT-AV1 at 1080p (likely low at 4K; fix its thread count and lookahead when measuring), measured per encoder at M4. Playback and the pop-out preview pause while exporting; export frames in flight live inside the cache cap and export decoders follow the pool rules.
- **After playback stops**: back to under idle + frame cache cap + the preview's working set within 10 s (decoders idle for 5 s are closed). Memory must never grow with project length or session length. The working set is what the preview keeps for the next playback, reused rather than freed: three output frames and two upload buffers (about 15 MB for a 1080p source in a 1280-wide preview) and the UI's copies. It is part of the budget because graphics drivers keep GPU memory that is freed at frame rate: at M1, allocating per frame left 45 MB behind, and freeing the set once the preview went idle left more, not less. Measured at M1: 639 MB 10 s after 5 minutes of 1080p playback, which is idle (229 MB), the full 384 MB frame cache and 26 MB of working set; it does not grow with playing time (the same after 45 s and after 5 minutes). Measured at M2: 644 MB 10 s after the 2 minutes of three clips and music, idle (224 MB) plus the cache and 36 MB.
- Frame cache cap is user-configurable, default 384 MB. The cache owns its frames (copied out of decoder pools), so eviction always frees memory.
- **Download**: under ~60 MB, which requires LZMA compression: an Inno Setup installer with solid LZMA2, picked at M0 (MSI only offers MSZIP/LZX, so no MSI). **Installed**: under ~160 MB for 0.1: the five FFmpeg DLLs Dusk loads (130.3 MB for the pinned 8.1 build), `dusk.exe` (about 15–20 MB) and `dusq.exe` (about 5 MB), sharing the DLLs. Long-term target under ~60 MB installed, via the slim custom FFmpeg build planned for 0.2. Both measured from M0 onward. At M0 (release build, 1 MB = 10⁶ bytes): installed 149.9 MB, with `dusk.exe` at 19.4 MB (1.5 MB of it bundled fonts) and `dusq.exe` at 0.2 MB (no commands yet); download estimate 44.5 MB (all seven files as one solid LZMA2 stream, before installer overhead). At M1: installed 151.0 MB with `dusk.exe` at 20.4 MB; download estimate 44.8 MB.
- Cold start under 2 seconds. Measured at M1: first window after 0.76–0.88 s (release build, Radeon RX 6750 XT).
- Idle CPU near 0% when not playing or exporting. Measured at M1: Dusk's own threads use no measurable CPU when idle. On the M1 machine a thread of AMD's Vulkan and Direct3D drivers spins one core whenever a window with a GPU swapchain sits idle, a bare Slint window included: the driver bug where a customized Radeon Software profile makes a worker thread poll instead of wait ([GPUOpen-Drivers/AMD-Gfx-Drivers#106](https://github.com/GPUOpen-Drivers/AMD-Gfx-Drivers/issues/106)). Resetting the profile to its defaults is the reported workaround, and it works: on the M1 machine, switching Radeon Software's global gaming experience from Quality to Default brought an idle Dusk with a clip open to 0 ms of CPU in 5 s. Dusk cannot avoid the bug from the application side.

## 2. Responsive
- The UI thread never blocks. Decoding, thumbnails, waveform analysis, and export run on worker threads.
- Video is decoded in software in 0.1: measured at M1, D3D11VA with the copy back to system memory was no faster and held many times the memory (ARCHITECTURE.md, "Decoder pool"). Hardware decoding returns with zero-copy decoding, or sooner where software decoding cannot keep up.
- 1080p scrubbing is smooth on integrated graphics (Intel Iris Xe class). Measured at M1 on a Radeon RX 6750 XT (integrated graphics not measured yet): jumping to an exact frame of 1080p H.264 with a 2 s GOP takes 72 ms at the median and 123 ms at worst; while the playhead is dragged every frame is drawn, and the exact frame arrives 24–26 ms after release. 1080p playback shows every frame at 30 fps (none skipped in 5 minutes) and all but 3 of 1196 at 60 fps.
- 4K is usable at MVP (may stutter); proxies come later.

## 3. Non-destructive
- Source media is never modified or copied.
- A project is a list of instructions (which clips, where, with which edits). Rendering happens only at export.
- The in-place clip editor applies instructions to the clip, not a re-encoded file, unless the user explicitly exports.

## 4. Offline and private
- No network code of any kind: no accounts, no telemetry, no analytics, no update checks. FFmpeg's protocol whitelist is `file` only, so neither `dusk` nor `dusq` opens URLs. The About dialog may contain a link to the releases page that opens the user's browser; Dusk itself never makes a request.

## 5. Robust
- Autosave every ~30 seconds; crash recovery on next start.
- A failed or cancelled export never corrupts the project or leaves half-written files in place of the user's output (`.part` file, renamed on success).
- Missing media is reported and relinkable, not silently dropped.

## 6. Keyboard-first
- Every common action has a shortcut; shortcuts are discoverable (shortcut list) and remappable.

## 7. Platform
- Windows 10/11 first. Code stays portable (no Windows-only APIs outside a thin platform layer) so Linux and macOS follow.

## 8. Media support
- Any format FFmpeg can demux and decode, including WhatsApp/Telegram audio (Opus, AAC, AMR, MP3).
- Variable-frame-rate video (phone recordings) is handled by timestamp, never by frame counting.
- 8-bit and 10-bit sources. HDR (HLG/PQ) is tone-mapped to SDR for preview and export in 0.1, in both `dusk` and `dusq`; all 0.1 exports are 8-bit SDR. HDR passthrough is later.
- Still images are clips with an explicit duration (default 5 s). Embedded color profiles are honored for sRGB, Display P3 and the other profiles FFmpeg can name; Adobe RGB and ProPhoto photos are treated as sRGB in 0.1.
- Rotation metadata is honored on import; clips that don't match the sequence are fitted with bars by default.
- Audio files with embedded cover art import as audio only.

## 9. Licensing
- Own code: MIT.
- Default build contains no GPL dependencies. FFmpeg is the pinned BtbN LGPL shared 8.1 build (see `docs/SETUP.md`) without x264/x265. Encoding via hardware encoders (NVENC / QSV / AMF), OpenH264, Kvazaar (HEVC), SVT-AV1, VP9.
- Slint under the **Slint Royalty-free License 2.0**: the `AboutSlint` widget is shown in the About dialog (reachable from the top-level menu) and the Slint badge appears on the download page.
- GPL encoders only through a user-supplied external `ffmpeg.exe` that Dusk pipes frames to; nothing GPL is linked or shipped.
- Patents: the shipped `avcodec` contains FFmpeg's H.264/HEVC decoders plus OpenH264 and Kvazaar compiled from source, none covered by a patent license (Cisco's coverage applies only to its own downloaded binary). Same position as every open-source media app; revisit only if Dusk is ever sold (see ARCHITECTURE.md).

## 10. Project file
- Plain text JSON, human-readable, with a schema version field, friendly to git. Media referenced by id, listed once.
