# Dusk — Decisions

Why the design in [ARCHITECTURE.md](ARCHITECTURE.md) is the way it is: what was measured, what went wrong first, and what was turned down. ARCHITECTURE.md says what Dusk does now; this file keeps the reasons, so the design can be read without the history and the history is not lost.

Entries sit under the ARCHITECTURE.md heading they explain, and each names the milestone it was decided or found at (ROADMAP.md). Figures were measured on the development machine unless an entry says otherwise: a Ryzen 5 2600X with a Radeon RX 6750 XT on Windows 11, release builds. Budgets, and the figures measured against them at each milestone, are in [REQUIREMENTS.md](REQUIREMENTS.md).

When a decision is made or a finding changes the design, ARCHITECTURE.md gets the rule and this file gets the reason, under the same heading.

## Core data model

### Media dropped on an empty timeline starts at frame 0 (M6)

An empty timeline is fitted to the window, so it is about a second wide, and a first clip let go a little to the right of its start would leave black before it in the export. So `drop_place` puts media dropped on an empty timeline at frame 0 and the outline shown while dragging says so. Once the timeline has clips, media starts where it is let go.

## Data flow

### Fitting against the sequence's shape (M4)

At a preset size the frame's sides are rounded (16:9 at 480p is 854×480), so its shape is a hair off the sequence's. Fitting each picture against the frame itself left a sliver of a bar on the right of an export and squeezed the picture by a pixel; the equivalence test between the GPU and CPU paths found it. Pictures are now fitted or made to fill against the sequence's own shape, and the result is stretched onto the frame.

### Reverse playback of long groups of pictures (M2)

Reverse playback decodes each group of pictures forwards from its keyframe and plays the frames backwards. A group longer than half the cache (long-GOP screen recordings) is decoded again from its keyframe for each stretch of it that fits, keeping only the last frames before the playhead. On a 20 s single-group 1080p file played backwards at 1x for 12 s:

- this showed 146 of 360 frames, still for 1.6 s at most;
- keeping as many frames as the whole cache holds showed 201, still for up to 2.3 s, at the cost of everything else in the cache;
- keyframe stepping would show one frame, the group's keyframe, for all of it.

Half the cache was chosen. Decoding the next stretch on a worker while the current one plays, as the lookahead does, would smooth it; later.

### Decoder sizes (M1)

A software decoder peaks at about 32 MB for 1080p H.264, 51 MB for 1080p 10-bit HEVC, 150 MB for 4K H.264 and 200 MB for 4K 10-bit HEVC, and gives it all back when closed. A hardware (D3D11VA) decoder allocates its frame pool on open: about 65 MB for 1080p 8-bit, about 133 MB for 1080p 10-bit HEVC, about 500 MB for 3840-wide 10-bit HEVC and about 535 MB at 4096 wide; on integrated graphics that pool is system RAM. The size classes and the ~550 MB single-source ceiling were drawn for those hardware pools, so they hold whichever kind of decoder is open.

### Video is decoded in software in 0.1 (M1)

With every frame copied back to system memory (`av_hwframe_transfer_data`), D3D11VA was no faster than FFmpeg's software decoders on 4 threads (1080p H.264: 274 against 278 fps; 4K 10-bit HEVC: 45 against 43 fps), yet it held about 200 MB at 1080p and 640 MB at 4K, and the driver kept 180 to 270 MB of it after the decoder closed, more with every reopen. So 0.1 decodes in software. The D3D11VA path stays in `dusk-media` for zero-copy D3D11-to-wgpu sharing, where hardware decoding can pay off (Open questions 2), and for machines where software decoding cannot keep up.

### The next clip is made ready on a worker (M2)

Getting the next clip ready on the video thread itself, a frame per tick, stalled playback, since a single frame of 1080p HEVC can take tens of milliseconds. A short-lived worker now opens the next clip's decoder and decodes up to its first frame. On noisy 20 Mbps 1080p HEVC with groups of pictures of about 2 s, cutting 1 to 1.5 s into a group, 40 of 180 frames were skipped over two cuts without the lookahead, and none with it.

### One thread above 9 Mpx, and the 9 Mpx boundary (M2)

Every further decoding thread holds pictures of its own: one 8K HEVC stream peaked at 1,141 MB on 4 threads and 845 MB on one. So a source above 9 Mpx gets one software decoder on one thread, and is accepted when 9 of its 4:2:0 pictures fit under the ceiling (the decoder's 6 for a conforming stream, Dusk's 3). 8-bit 8K comes to 448 MB and fits; a low-delay 8K HEVC stream peaked at 320 MB. 10-bit 8K and 10-bit 6K do not fit. A stream that keeps more pictures than its level allows (an encoder's 16-picture pyramid, the 845 MB above) is not caught by the estimate.

9 Mpx is where a 10-bit hardware pool crosses the ceiling. 8-bit sources would fit up to about 18 Mpx, but 0.1 deliberately uses the single boundary; a later version can base the class on the computed pool size instead.

### Stills keep their source's chroma siting (M4)

A still entry used to be resampled to left-sited chroma before the compositor resampled it again, which blurred the colored edges of every JPEG. The equivalence test found it. Entries now keep their source's siting (JPEG's is centered), which `Picture` carries and the compositor reads, video frames included.

## Pop-out clip editor

### No version counter (M3)

The session keeps `seen`, the group's clips as it last saw them, and compares them with the project after each edit. That tells a stale draft from a fresh one without the project counting its versions.

### The window and stale sessions (M3)

Decided at M3: one clip editor at a time; a draft not applied is asked about (*Apply*, *Discard*, *Cancel*) before another clip opens, the window closes or the project is left; a draft without changes of its own follows the clips without a banner; and a split counts as an edit of the group, since the first half keeps the clip's id and link group, so a kept draft applied later is cut at the gap where the second half starts.

## Export details

### The export dialog (M4)

Decided at M4: one dialog serves the timeline and the clip editor, and it names the encoder each codec will use.

### The Quality mapping (M4)

The slider's H.264/HEVC quantizer, 37 − 0.2 × level, gives 21 at High, the quantizer M1 exported at. AV1 and VP9 get 51 − 0.3 × level (27 at High).

### SVT-AV1 and a peak bitrate (M4)

SVT-AV1 takes a peak bitrate only in its constant-quality mode and refuses to open with one otherwise, so a target bitrate reaches it alone, with a buffer of one second.

### SVT-AV1's memory (M4)

Left to itself, SVT-AV1 chose level 5 on the six-core machine and took 1.3 GB at 1080p and 4.7 GB at 4K. On at most two pictures at once (`lp=2`), and above the 1080p class on one without its lookahead (`lp=1:lookahead=0`), it takes about 0.5 GB at 1080p and 1.0 GB at 4K. Without the lookahead a file comes out about 6% larger at the same CRF, so it stays on at 1080p.

### VP9 on four threads (M4)

FFmpeg's libraries default to one thread, which libvpx takes as it is, so until M4 VP9 exports ran on one core, a quarter as fast. They now run on 4 threads with row threading.

### The compress dialog (M4)

Decided at M4: the compress tool is one dialog. File → *Compress a video…* (Ctrl+M) picks the video, and the dialog that opens aims it at a size or a quality and says what the file will come out as before anything is written.

### Color step 2 moved out of swscale (M4)

swscale's own 16-bit RGB output came out 255/256 too dark for 8- and 10-bit sources alike (white at 65,280 of 65,535), while its YUV output is exact. So swscale stops at planar 16-bit YUV 4:4:4 and Rust does step 2 (`yuv_to_rgb`) and everything after it.

### Step 3 through tables (M4)

dusq runs step 3 on every pixel of a picture, where the reference functions cost about 360 ns a pixel. A 3D lookup table missed by up to 41 codes on saturated HDR colors. `SdrConverter` instead splits the math into tables for what acts on one channel and computes the little that needs the whole pixel, and stays within 0.02 of an 8-bit code of the reference.

### A packet without a duration (M4)

Encoders sometimes hand back a packet without a duration, and MP4 files written from such packets dropped their last frame. dusq gives such a packet one frame at the source's average rate.

### How long dusq takes (M4)

On grainy test clips through `h264_amf`: 1080p H.264 took 0.55 times the clip's duration, 4K H.264 at 2 threads 2 times, 4K 10-bit HEVC at 2 threads 4.5 times and 8K HEVC at 1 thread 5 times.

### Equivalence of the two paths (M4)

Every plane comes out between 56 and 67 dB, well over the 45 and 40 dB the test asks: untagged HD 67, the turned phone clip 61 to 65, the Display P3 photo 64 to 66, HLG 64 to 66, 4K to 480p 56 to 67. The test found two real differences, both fixed: the sliver of a bar ("Fitting against the sequence's shape") and the stills' chroma siting ("Stills keep their source's chroma siting").

### Target size on short clips (M4)

The plan leaves the container 3% of the file. On a short clip the container's share is larger, so the first pass comes out over and the second runs. A 60 s 1080p clip aimed at 10 MB came out at 9.98 MB in one pass.

## Memory discipline

### The compositor keeps its GPU memory (M1)

Allocating the compositor's textures and buffers per frame left 45 MB behind after playback, and freeing them once the preview went idle left more, not less: graphics drivers keep memory that is freed and allocated again at frame rate. So the compositor allocates once and reuses, and the after-playback budget counts that working set (REQUIREMENTS.md, "After playback stops").

## Project file and autosave

### Autosaves live in Dusk's own folder (M2)

An untitled project has no folder, a project's folder may be read-only or synced, and recovery has to find autosaves without knowing which project was open. So autosaves go to `%LOCALAPPDATA%\Dusk\autosave`, one session per running Dusk, rather than next to the project.

## Keyboard and settings

### Number fields report as they are typed (M5)

A bitrate or a width typed in the export dialog and not entered was lost when Export or Apply was clicked, since a click on a button leaves the keyboard in the field. The number fields of the export, compress and sequence dialogs now report each number as it is typed. The clip editor's and the properties panel's fields still wait for Enter: a speed taken as it is typed would cut the clip at the gap on the way to the number meant.

### The playhead past the last frame (M5)

Placing media puts it at the playhead. For placing to append, and for the last cut to be reachable with the cut keys, the playhead can stand one frame past the end, where the preview is black. End still goes to the last frame.

### Shift with a punctuation key (M6)

`Shift+/` in `shortcuts.txt` was read as `/`. A press of Shift and / arrives as `?`, so Shift with a punctuation key now stands for the character it types on a US layout.

### Quit's question over a dialog (M6)

The question closing asks showed under the missing media list, where Enter found a file instead of saving. Questions are now drawn over every dialog and take the keys first, and Quit is the one menu action that does not wait for an open dialog to close.

### F1 opens the shortcut list (M6)

F1 is where people look for help, so it opens the shortcut list beside `?`, and the About dialog moved to Shift+F1.

## Missing media

### Relinking (M6)

An id given to new media after an import was undone used to show the old file's frames, since the engine knew frames by media id alone. The engine now remembers which file each media's frames, decoder and thumbnail came from, and either preview names the file each time it asks for a frame, so the clip editor, still drafting from the project as it was before such an undo, cannot put another file's frames in the cache under that id.

## Error messages

### The M6 message pass (M6)

Every message a user can see was gone through. Twelve said what happened without what to do, and now say both: a file without sound, one FFmpeg cannot read, one with neither picture nor sound, a picture in a form Dusk cannot read (which still claimed stills were coming later), a clip reaching outside its file, a crop outside the picture, a damaged project file and one that breaks a timeline rule, an export already running, autosave being off, a recovery that failed and the clip editor failing to open. A media file that went away while Dusk ran was reported as a path that is not a file; it is now named with how to find it. Two messages had lost a line's `\` and showed a gap of spaces mid-sentence, which `dusk-app/tests/messages.rs` now looks for.

## Release

### The theme pass (M6)

Every view was checked against THEME.md with screenshots before and after. Every color, font size, weight, border and radius already came from the theme's tokens, and every string was in sentence case. Four things differed and were fixed:

- A disabled or muted clip faded whole, label included, so its `text-3` label was barely there; now only its fill is at 40%.
- Notices that an edit did more than was asked (cut at the gap, reached the source's edge) showed as plain text; both status lines gained a third kind, warnings in `signal`.
- Sequence settings' four picture sizes ran past the dialog's padding; the dialog is wider.
- Its frame rates sat 2 pixels in from the fields to make room for their focus ring; the ring is now drawn just outside them.

### The logo at every icon size (M6)

`scripts/make-logo.py` draws each icon size from the logo's geometry, so the small sizes are drawn on whole pixels rather than scaled down from a large picture.

### The installer's sizes (M6)

Measured on the installer CI builds and tries: the download is 46.5 MB, and Dusk installed takes 161.6 MB: the staged folder's 157.2 MB (`dusk.exe` 25.6 MB, `dusq.exe` 0.7 MB, the DLLs 130.3 MB, `licenses` 0.7 MB) and Inno Setup's uninstaller. That is 1.6 MB over the approximate 160 MB target, accepted for 0.1 rather than trading speed or panic handling for size (REQUIREMENTS.md, "Download"). Dusk's window appears 0.48–0.62 s after starting it from the staged folder. `scripts/ffmpeg-licenses.py` lists 88 libraries in the 8.1 build, and the Rust crates inside two of them, rav1e (82) and librsvg (175).

### cargo metadata for Windows alone (M6)

`cargo metadata` lists every platform's crates unless told one, and offline it needs them all downloaded, which CI's build had not done for Linux's. Both license scripts ask for Windows' packages alone (`--filter-platform`).

## FFmpeg (Windows)

### The ship set and the installer format (M0)

With the pinned 8.1 build the five DLLs take 130.3 MB installed (avcodec 90.9, avformat 22.7, swscale 12.9, avutil 3.0, swresample 0.7), 54.3 MB zipped (deflate), and 44.5 MB as one solid LZMA2 stream together with both executables. `dusk.exe` was 19.4 MB at M0 (1.5 MB of it bundled fonts) and `dusq.exe` 0.2 MB, before it had commands. The download target therefore needs LZMA, which MSI cannot use, so Inno Setup with solid LZMA2 was picked over MSI and NSIS. Most of the installed size is avcodec, which a slim custom FFmpeg build would shrink (ROADMAP.md).

## Slint specifics

### dusk-render creates the device (M0, M1)

Slint's own default instance loads Vulkan and DX12 together and showed the first window after about 3.0 s. DX12 alone took 2.5 s, with about 30 MB more private bytes; Vulkan alone 0.83 s. With `dusk-render` creating one Vulkan instance and device (DX12 only when Vulkan finds no adapter) and handing them to Slint, the first window appears after 0.71–0.85 s and the idle process holds about 185 MB (M1, fonts bundled).

### The preview stays a wgpu texture (M1)

On the Radeon RX 6750 XT the UI thread spends about 0.05 ms per preview frame turning the texture into an image, and a 1080p60 clip reaches the preview at 57–60 frames a second. No integrated GPU was at hand at M1, so the readback fallback stays pre-approved for one (ARCHITECTURE.md, Open questions 1).

### The surface reconfigure race (M2)

Under WARP, the compositor submitting while Slint waited for the queue to empty before reconfiguring a surface closed Dusk about one run in ten, since wgpu panics on that error by default. `dusk-render`'s device error handler now logs that one error and panics on any other.

## Known issues

### Two windows on AMD's Vulkan driver (M3)

At M3, memory grew when both windows redrew together on Vulkan. It is AMD's Vulkan driver (26.8.1 on a Radeon RX 6750 XT, Windows 11): each `vkQueuePresentKHR` that presents a different swapchain from the one presented before keeps about 0.19 MB of private memory for good.

- A program of plain Vulkan calls shows it (two windows, one queue, each frame an acquire, a clear, a submit and a present per swapchain: 37.6 MB more after 400 alternating presents), as do two plain wgpu windows and two Slint windows.
- One swapchain presenting at the same rate, or twice as often, stays flat, and so does presenting both swapchains in one `vkQueuePresentKHR` call.
- The Rust heap stays flat throughout, the present mode makes no difference, a longer frame latency loses more, and DX12 does not lose any.

Dusk cannot present both windows in one call (Slint presents each window by itself), and DX12 costs about 250 MB more at idle, so Dusk stays on Vulkan. In use the loss is about 0.4–1.5 MB each time both windows redraw together (REQUIREMENTS.md, "Clip editor"). To report to AMD with the plain Vulkan program.
