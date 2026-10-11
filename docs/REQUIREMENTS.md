# Dusk — Non-Functional Requirements

Constraints that shape the architecture. Targets are measured at every milestone, not at the end.

The tables give each target and the latest figure measured against it on each machine, with the milestone it was measured at. When a figure is measured again, the new one replaces it here and the old one moves to [DECISIONS.md](DECISIONS.md), "Measurements", which also says how each was taken. Memory figures are process private bytes: the app's in MB of 2^20 bytes, as Task Manager and `playback_probe` count them; encoder lines, dusq's and file sizes in MB of 10^6 bytes.

The machines:

- **Desktop**: Ryzen 5 2600X (6 cores), Radeon RX 6750 XT on Vulkan, Windows 11. The development machine for M0 to M6.
- **Laptop**: Core i7-13620H (6 performance and 4 efficient cores), its Intel UHD Graphics (Xe-LP, the Iris Xe class) on Vulkan, 16 GB of RAM, Windows 10, on mains power in Windows' Balanced plan.

## 1. Lightweight (top priority)

### Memory

| Budget | Target | Desktop | Laptop |
|---|---|---|---|
| Idle | under ~250 MB with a 1080p project open and nothing playing | 233 MB (M6); the empty window 200 MB (M1) | 124–126 MB; the empty window 119 MB (M7) |
| During 1080p playback | idle + frame cache cap + 300 MB decoders + 64 MB thumbnails: about 800 MB for 8-bit footage and 950 MB for 10-bit at the default cache | 681 MB at most (M6); 703 MB around the cuts of three clips with music (M2) | 560 MB at most at 30 fps, 563 MB at 60 fps (M7) |
| After playback stops | within 10 s: idle + frame cache cap + the preview's working set | 644 MB (M6) | 518–520 MB (M7) |
| 4K sources | one video decoder, no lookahead; playback under ~1.2 GB | the decoder 150 MB (H.264) to 200 MB (10-bit HEVC) (M1) | the decoder 145 MB (H.264) to 276 MB (10-bit HEVC); playing 725 MB (H.264) and 902 MB (10-bit HEVC) at most (M7) |
| Clip editor | no frame memory of its own; its window about 60 MB once opened | 295 MB open, 730 MB playing, 693–695 MB after (M3) | 200 MB open; opened after playback, its window 55 MB, 622 MB playing in it, 628 MB playing in the main window beside it, 587 MB after (M7) |
| Frame cache | default 384 MB, 128 to 4096 MB in Settings; a smaller cap frees what lies above it at once | at 128 MB: 235, 421 and 376 MB idle, playing and after; 384 → 128 MB freed 269 MB within 3 s (M5) | not measured |
| dusq | under the app's playback budget for the same size class | 158 MB at 1080p, 446 MB 4K H.264, 351 MB 4K 10-bit HEVC, 974 MB 8K HEVC (M4) | through `h264_qsv`: 246 MB at 1080p, 557 MB 4K H.264, 683 MB 4K 10-bit HEVC, 906 MB 8K HEVC (M7) |
| During export | Dusk: the playback budget; the encoder: its line, in the helper's own process | a 10 s 4K export through the helper (`h264_amf`, `hevc_amf` or SVT-AV1): Dusk 532–552 MB at most, back to about 140 MB after (M7); compressing a 2.1 GB 4K phone video to 25 MB: 531 MB at most (M4, before the helper) | not measured in the app |

The encoder lines, from the peak private bytes an encoder adds while an export feeds it (`dusk-media`'s `encoder_memory`, grainy pictures at High). Since M7 an encoder runs in Dusk's helper, `dusq`, which peaks near its encoder's figure: 137 MB through `h264_amf` and 138 MB through `hevc_amf` for a 4K export, 994 MB through SVT-AV1, and 188 MB for the encoder probe, once a session.

| Encoder | Line | Desktop, 1080p / 4K | Laptop, 1080p / 4K |
|---|---|---|---|
| `h264_amf` | 150 MB | 61 / 122 MB (M4) | no AMD graphics |
| `hevc_amf` | 150 MB | 58 / 125 MB (M4) | no AMD graphics |
| `h264_qsv` | 200 MB at 1080p, 500 MB at 4K | no Intel graphics | 170 / 454 MB (M7) |
| `hevc_qsv` | 300 MB at 1080p, 750 MB at 4K | no Intel graphics | 272 / 699 MB (M7) |
| NVENC, the hardware AV1 encoders | 200 MB, provisional | not measured | `av1_qsv` does not open on this graphics; AV1 falls through to SVT-AV1 |
| OpenH264 | 150 MB | 39 / 129 MB (M4) | 37 / 127 MB (M7) |
| Kvazaar | 400 MB | 109 / 369 MB (M4) | 123 / 375 MB (M7) |
| VP9 | 350 MB at 1080p, 1.1 GB at 4K | 332 / 1,082 MB (M4) | 331 / 1,081 MB (M7) |
| SVT-AV1 | 550 MB at 1080p, 1.1 GB at 4K | 526 / 1,008 MB (M4) | 526 / 1,048 MB (M7) |

The hardware encoders keep some memory after their session closes, which the next session reuses, so it does not grow export after export but stays until its process ends: AMF about 25 MB at 1080p and 60 MB at 4K, `h264_qsv` about 70 and 235 MB, `hevc_qsv` about 145 and 375 MB. Since M7 that process is the helper, which ends with the export, so Dusk keeps none of it.

What these figures assume:

- GPU textures on integrated graphics count, since that memory is system RAM.
- Memory never grows with project length or session length.
- The decoder budget holds at most 2 video decoders and 4 audio decoders in the 1080p class, and one larger source in a single pool of up to about 550 MB (ARCHITECTURE.md, "Decoder pool"). The reverse-playback buffer and an export's frames in flight live inside the cache cap.
- The preview's working set is what it keeps for the next playback rather than freeing: three output frames and two upload buffers (about 15 MB for a 1080p source in a 1280-wide preview) and the UI's copies (DECISIONS.md, "The compositor keeps its GPU memory").
- Playback and the pop-out preview pause while exporting, and export decoders follow the pool rules. An export through the helper or through the user's own `ffmpeg.exe` has no encoder line in Dusk: that encoder runs in its own process. Sound alone is still encoded in Dusk's.
- Known exception, outside Dusk: on AMD's Vulkan driver, each moment both windows redraw together leaves about 0.4–1.5 MB behind for good (ARCHITECTURE.md, "Known issues").

### Size and start

| Budget | Target | Measured |
|---|---|---|
| Download | under ~60 MB, which needs LZMA: an Inno Setup installer with solid LZMA2 | about 31.1 MB with Dusk's own FFmpeg (M7: the staged files as one LZMA2 stream at preset 9, before the installer's own overhead; 0.1's FFmpeg DLLs in their place, 31.3 MB); 46.5 MB (M6) |
| Installed | under ~160 MB for 0.1; under ~60 MB long-term | about 108.9 MB with Dusk's own FFmpeg (M7): staged 104.5 MB (the five FFmpeg DLLs 76.4 MB, `dusk.exe` 25.8 MB, `dusq.exe` 0.7 MB, `licenses` 1.6 MB) and Inno Setup's uninstaller 4.4 MB (DECISIONS.md, "An FFmpeg of Dusk's own"); about 163.1 MB at 0.1 (M6), over its approximate target and accepted (DECISIONS.md, "The installer's sizes") |
| Cold start | under 2 s to Dusk's first window | Desktop 0.48–0.62 s (M6); CI's runner, drawing with WARP, 1.3 s (M6); Laptop 0.16–0.26 s (M7) |
| Idle CPU | near 0% when not playing or exporting | Dusk's own threads use none measurable (M1). AMD's driver spins a core under a customized Radeon Software profile, which resetting the profile fixes (DECISIONS.md, "Idle CPU"). Laptop: 0 ms of CPU in 10 s, with or without a clip open (M7) |

## 2. Responsive

- The UI thread never blocks. Decoding, thumbnails, waveform analysis, and export run on worker threads.
- Video is decoded in software in 0.1: D3D11VA with the copy back to system memory was no faster and held many times the memory (DECISIONS.md, "Video is decoded in software in 0.1"). Hardware decoding returns with zero-copy decoding, or sooner where software decoding cannot keep up.
- 1080p scrubbing is smooth on integrated graphics (Intel Iris Xe class).
- 4K is usable at MVP (may stutter); proxies come later.

| Measure | Target | Desktop | Laptop |
|---|---|---|---|
| Jump to an exact frame, 1080p H.264 with a 2 s GOP | smooth scrubbing | 72 ms median, 123 ms at worst (M1) | 95 ms median, 192 ms at worst (M7) |
| Dragging the playhead | every frame drawn; the exact frame soon after release | every frame; 24–26 ms after release (M1) | every frame; 56–58 ms after release (M7) |
| 1080p30 playback | every frame shown | none skipped in 5 minutes (M1) | 2 of 8,962 skipped in 5 minutes (M7) |
| 1080p60 playback | every frame shown | all but 3 of 1196 (M1) | all but 7 of 3,529 (M7) |
| 1080p60 in the preview (`DUSK_STATS`) | 60 frames a second drawn | 57–60, 0.05 ms of UI time a frame (M1) | 54–60 (57.8 on average), 0.02 ms of UI time a frame (M7) |
| 4K playback | usable | not recorded | H.264: 29–30 frames a second in the app. 10-bit HEVC: 18–31 (23–29 on average); at 60 fps, faster than this CPU decodes it, 29–43 with the picture up to 2 s behind the sound (M7; DECISIONS.md, "Falling behind while playing"). 1080p 10-bit HEVC plays whole |

## 3. Non-destructive
- Source media is never modified or copied.
- A project is a list of instructions (which clips, where, with which edits). Rendering happens only at export.
- The in-place clip editor applies instructions to the clip, not a re-encoded file, unless the user explicitly exports.

## 4. Offline and private
- No network code of any kind: no accounts, no telemetry, no analytics, no update checks. FFmpeg's protocol whitelist is `file` only, so neither `dusk` nor `dusq` opens URLs. The About dialog may contain a link to the releases page that opens the user's browser; Dusk itself never makes a request.

## 5. Robust
- Autosave every ~30 seconds; crash recovery on next start.
- A failed or cancelled export never corrupts the project or leaves half-written files in place of the user's output (`.part` file, renamed on success).
- Missing media is reported and relinkable, not silently dropped: a project opens with its missing files listed to be found, one find relinks the others beside it, and relinking undoes like any edit (BEHAVIOR.md, "Missing media").

## 6. Keyboard-first
- Every common action has a shortcut; shortcuts are discoverable (shortcut list) and remappable: every action has a key in one table, changed in the shortcut list or in `shortcuts.txt`, and every control is reached with Tab (BEHAVIOR.md, "Keyboard and settings").

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
- Default build contains no GPL dependencies. FFmpeg is Dusk's own pinned LGPL shared 8.1 build (see `docs/SETUP.md`), without x264/x265. Encoding via hardware encoders (NVENC / QSV / AMF), OpenH264, Kvazaar (HEVC), SVT-AV1, VP9.
- Slint under the **Slint Royalty-free License 2.0**: the `AboutSlint` widget is shown in the About dialog (reachable from the top-level menu) and the Slint badge appears on the download page.
- GPL encoders only through a user-supplied external `ffmpeg.exe` that Dusk pipes frames to; nothing GPL is linked or shipped.
- Patents: the shipped `avcodec` contains FFmpeg's H.264/HEVC decoders plus OpenH264 and Kvazaar compiled from source, none covered by a patent license (Cisco's coverage applies only to its own downloaded binary). Same position as every open-source media app; revisit only if Dusk is ever sold (see ARCHITECTURE.md).

## 10. Project file
- Plain text JSON, human-readable, with a schema version field, friendly to git. Media referenced by id, listed once.
