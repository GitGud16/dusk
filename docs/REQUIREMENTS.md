# Dusk — Non-Functional Requirements

Constraints that shape the architecture. Targets are measured at every milestone, not at the end. Memory figures are process private bytes.

## 1. Lightweight (top priority)
- **Idle**: under ~200 MB with a 1080p project open and nothing playing.
- **During 1080p playback**: under idle + frame cache cap + 300 MB decoder budget + 64 MB thumbnails. The reverse-playback buffer lives inside the cache cap. The decoder budget holds at most 2 video decoders (a hardware pool is about 65 MB for 1080p 8-bit, about 133 MB for 10-bit HEVC) and 4 audio decoders. GPU textures on integrated graphics count. With the default cache cap this is about 950 MB worst case (10-bit) and about 800 MB for typical 8-bit footage.
- **4K sources**: one video decoder, no lookahead; its pool reaches about 500 MB for 3840-wide 10-bit HEVC and about 535 MB at 4096 wide, so the decoder line for one such source is a measured ceiling of about 550 MB and playback may reach about 1.2 GB. 4K is usable, not smooth, in 0.1.
- **dusq**: one decoder (hardware only for the 1080p class, software above it with 2 threads, 1 thread above 9 Mpx), scaling and normalizing right after decode, a 4-frame queue, no frame cache. Ceiling is measured at M4 per size class (1080p, 4K, 8K) and recorded here; before the encoder line it is expected well under the app's playback budget for the same source.
- **During export**: the playback budget plus an encoder line, provisionally 200 MB for hardware encoders, OpenH264 and Kvazaar and 600 MB for SVT-AV1 at 1080p (likely low at 4K; fix its thread count and lookahead when measuring), measured per encoder at M4. Playback and the pop-out preview pause while exporting; export frames in flight live inside the cache cap and export decoders follow the pool rules.
- **After playback stops**: back to under idle + frame cache cap within 10 s (decoders idle for 5 s are closed). Memory must never grow with project length or session length.
- Frame cache cap is user-configurable, default 384 MB. The cache owns its frames (copied out of decoder pools), so eviction always frees memory.
- **Download**: under ~60 MB, which requires LZMA compression: an Inno Setup installer with solid LZMA2, picked at M0 (MSI only offers MSZIP/LZX, so no MSI). **Installed**: under ~160 MB for 0.1: the five FFmpeg DLLs Dusk loads (measured 120–130 MB), `dusk.exe` (about 15–20 MB) and `dusq.exe` (about 5 MB), sharing the DLLs. Long-term target under ~60 MB installed, via the slim custom FFmpeg build planned for 0.2. Both measured from M0 onward.
- Cold start under 2 seconds.
- Idle CPU near 0% when not playing or exporting.

## 2. Responsive
- The UI thread never blocks. Decoding, thumbnails, waveform analysis, and export run on worker threads.
- Hardware video decoding (D3D11VA on Windows) with per-file software fallback.
- 1080p scrubbing is smooth on integrated graphics (Intel Iris Xe class).
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
