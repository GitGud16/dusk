# Features

Written as user stories. **MVP** is what the first usable version must do. **Later** is agreed direction, not yet scheduled.

## MVP

### Import
- I can drag and drop video, audio, and image files into a media bin.
- A file with video and audio lands as two linked clips (video track + audio track) that move together.
- Still images become clips with an editable duration (default 5 s).
- I can import any audio format FFmpeg can read, including voice notes and audio files from WhatsApp and Telegram (Opus in `.opus` / `.ogg` / `.oga`, `.m4a` / `.aac`, `.amr`, `.mp3`).

### Timeline
- 2 video tracks + 2 audio tracks.
- The sequence takes its frame rate and size from the first video clip I add (it asks me to match); I can change both later in Sequence settings.
- A clip that doesn't match the sequence is fitted with bars by default; I can switch it to fill (crop to cover).
- I can place, move, trim the ends of, and split clips at the playhead.
- I can delete a clip with or without ripple (closing the gap across all unlocked tracks).
- Linked video and audio move, trim, split and delete together; Alt+Delete removes just one. I can lock or mute a track.
- Rotated phone video shows upright, and portrait clips get bars instead of stretching.
- I can undo and redo any action.

### Preview
- Play / pause, scrub, and step frame by frame, with audio in sync.
- I can play at variable speed, from very slow (frame-by-frame, 0.1x) to very fast (8x, 16x, 32x), forwards and backwards (J/K/L style).

### Audio
- Per-clip volume, mute, fade in / fade out.
- I can detach (unlink) audio from a video clip and use it on its own.
- I can disable a single clip (hide video / silence audio) or mute a whole track.
- I can take a video and export only its audio to a file (MP3 / AAC / Opus / WAV).

### In-place clip editor (the signature feature)
- I click a clip in the timeline and a pop-out window opens for just that clip, including its linked audio or video.
- In it I can trim, crop, rotate / flip, change speed, adjust volume and fades, or set a still image's duration.
- If I change the clip in the main window meanwhile, the pop-out tells me and lets me reload or keep my draft.
- I can **Apply to project** (the clip in the timeline updates) or **Export as file** (a standalone file is written), without leaving the main project.

### Export
- Containers: MP4, MKV, WebM (VP9/AV1 only), MOV. Only combinations the installed encoders support are offered.
- Presets: 1080p / 720p / 480p and High / Medium / Small quality (one Quality 0–100 slider underneath, mapped per encoder).
- Advanced: target bitrate for any encoder, CRF where the encoder supports it.
- Hardware encoding (NVENC / QSV / AMF) when available; software fallback (OpenH264, Kvazaar, SVT-AV1, VP9).
- Optional: use my own GPL `ffmpeg.exe` for x264/x265 exports.
- Progress bar with cancel.

### Compress tool
- I can open a single video, pick a target quality or a target file size, and export. No project needed. If the size is impossible, it tells me the smallest it can do.

### Command line (`dusq`)
- `dusq compress in.mp4 --size 25MB` and `dusq extract-audio in.mp4` do the same jobs without opening the app, with no GPU needed.

### Shortcuts
- Every common action has a keyboard shortcut.
- A **Shortcuts** button / `?` opens a searchable list of all shortcuts.
- Shortcuts are remappable (settings stored in a plain text file).

### Projects
- Save / load as a human-readable text file that references media (never copies or modifies it).
- Autosave and crash recovery.

## Later

- Transitions (cut, crossfade)
- Text and titles
- Basic color adjustments (brightness, contrast, saturation)
- Picture-in-picture: scale and position clips
- Keyframes for volume, scale, position
- Audio waveform display on the timeline
- Snapping and markers
- Proxies for smooth 4K editing
- HDR passthrough export (0.1 tone-maps HDR to SDR)
- Subtitles: SRT import, burn-in, soft subtitles
- Export queue / batch export
- GIF and image-sequence export
- Linux and macOS builds
