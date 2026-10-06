# Dusk

Free, open-source, native desktop video editor in Rust. Light on RAM and CPU, no accounts, no cloud, no ads. Its signature feature is in-place clip editing: click a clip, a pop-out window opens to edit just that clip, and the result is applied back to the project or exported as its own file.

Design docs live in `docs/`. Read the relevant one before changing the area it covers.

@docs/ARCHITECTURE.md
@docs/REQUIREMENTS.md

Also available (read when relevant, not every session):
- `docs/VISION.md` — why Dusk exists, scope guardrails, decisions so far
- `docs/FEATURES.md` — user stories, MVP vs later
- `docs/THEME.md` — colors (HSL), typography, spacing, timeline color rules
- `docs/ROADMAP.md` — milestones M0 to 0.1 with "done when" criteria

## Stack (decided, do not relitigate)

- Rust only. No C++ sources in this repo.
- UI: Slint **1.18.1**, pinned exactly (its feature names change between minor versions), with the **femtovg-wgpu** renderer: `default-features = false`, features `std`, `compat-1-18`, `backend-winit`, `renderer-femtovg-wgpu`, `unstable-wgpu-30`, `accessibility`, `unstable-winit-030` (its window-event hook receives files dropped from Explorer, which Slint does not pass on). Licensed under the Slint Royalty-free License 2.0 (the `AboutSlint` widget must stay in the About dialog). Video preview is a `slint::Image` made from a `wgpu::Texture`; this is the planned path, and the fallback in ARCHITECTURE.md (GPU readback of the composited RGBA texture into a `SharedPixelBuffer`) is pre-approved if M1 measurements fail. Do not open a third option.
- Media: FFmpeg through `ffmpeg-next`, linked dynamically to one exact prebuilt **BtbN LGPL shared** 8.1 build, pinned with its SHA-256 in `scripts/ffmpeg-pin.psd1` (see `docs/SETUP.md`); changing the pin is a documented decision, not a bump (newer BtbN builds raise the NVENC driver floor). Encoder limits (pixel formats, bit depth, max width/height, alignment, area, luma sample rate) come from the per-encoder table in `dusk-media`; never hardcode them elsewhere. All 0.1 exports are 8-bit SDR. `unsafe` FFmpeg calls live only in `dusk-media::ffi`; swscale is always configured explicitly through it with the legacy setup sequence (kernel, matrix, range, chroma siting), never left on defaults and never via `sws_scale_frame`. Plain BT.709 SDR sources skip the linearize/gamut step; everything else goes through it. Ship only avcodec, avformat, avutil, swscale, swresample. Decode in software in 0.1; the D3D11VA path waits for zero-copy decoding (measured at M1; see ARCHITECTURE.md, "Decoder pool"). Protocol whitelist `file` only.
- GPU: `wgpu` **30**, the version Slint's `unstable-wgpu-30` feature expects (`Cargo.lock` holds the exact release). Moving to another wgpu major means moving Slint's `unstable-wgpu-NN` feature with it, as one documented decision.
- Audio: `cpal` for output. Audio is the playback clock; video follows it.
- Threads: `crossbeam-channel` between UI and engine. Results reach the UI via `slint::invoke_from_event_loop`.
- License: MIT. **No GPL dependencies linked or shipped** (no `libx264` / `libx265`). Encoder order: H.264 `h264_nvenc` → `h264_qsv` → `h264_amf` → `libopenh264`; HEVC `hevc_nvenc` → `hevc_qsv` → `hevc_amf` → `libkvazaar`; AV1 `av1_nvenc` → `av1_qsv` → `av1_amf` → `libsvtav1`; WebM is VP9/AV1 only. GPL encoders are reachable only by piping frames to a user-supplied external `ffmpeg.exe`.
- Windows first. Keep platform-specific code behind a thin layer so Linux and macOS can follow.

## Workspace

```
dusk-core/    project model, commands, invariants, serialization   (no FFmpeg, wgpu, Slint, or I/O)
dusk-media/   FFmpeg: probe, decode, encode, mux                     (leaf)
dusk-render/  wgpu compositor; receives decoded frames as input      (leaf)
dusk-audio/   mixer, resampler, output device, playback clock        (leaf)
dusk-engine/  frame cache, decoder pool, scheduler, export pipeline  (worker threads)
dusk-app/     the `dusk.exe` binary: Slint windows, view models, undo stack, shortcuts, settings
dusk-cli/     the `dusq` binary: compress, extract-audio; builds dusk-engine without its `gpu` feature (no wgpu, no Slint)
docs/         design docs (above)
```

Dependencies point downward only: `dusk-app` / `dusk-cli` → `dusk-engine` → {`dusk-media`, `dusk-render`, `dusk-audio`} → `dusk-core` → `serde`. The three leaf crates never import each other.

## Commands

```
# local
cargo build                                   # debug build of the workspace
cargo run -p dusk-app                         # run the editor
cargo fmt --all                               # format (local only; CI runs the --check form)
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1   # release build, staged folder, installer (docs/SETUP.md, "Release")

# CI, and before every commit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                        # gpu feature on
cargo clippy -p dusk-engine --no-default-features --all-targets -- -D warnings
cargo test   -p dusk-engine --no-default-features
cargo build  -p dusk-cli --release            # dusq must be built alone: a workspace build unifies features and turns `gpu` on for it

# dusq must never link wgpu. Its own CI step, run under bash. Fails when wgpu is found and when
# cargo tree itself fails (a plain `cargo tree -i wgpu` exits non-zero when CLEAN, so don't use that).
deps=$(cargo tree -p dusk-cli -e normal,build --prefix none -f '{p}') || exit 1
if grep -q '^wgpu ' <<<"$deps"; then echo "dusq links wgpu"; exit 1; fi
```

CI runs the second and third blocks. The `gpu` feature is additive: the CPU transcode path (normalize, tone-map, rotate, convert) is always compiled and tested, and from M4 a test compares its pre-encode frames against the GPU path (per plane: PSNR ≥ 45 dB unscaled, ≥ 40 dB scaled, both paths on bicubic Catmull-Rom).

Before the first build, run `powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1` once. In every shell, dot-source `. .\scripts\dev-env.ps1`: it sets `FFMPEG_DIR`, puts the pinned FFmpeg DLLs first on PATH for that shell, and finds LLVM's `libclang` for `bindgen`. Prerequisites and details are in `docs/SETUP.md`; document any new setup step there.

## Hard rules

- **Never block the UI thread.** No decoding, encoding, file scanning, or waveform analysis on it. If a call can take more than a few milliseconds, it goes to a worker and reports back through a channel.
- **In the app, frames live in the cache only.** Decoded frames are copied into cache-owned buffers (NV12/P010 for video and timeline stills, planar 4:4:4 8-bit only for native-size still exports), each entry carrying its own color tags, and the decoder's frame is released; the LRU cache has a byte cap. Do not stash frames in view models, clips, or statics, and never keep references into a decoder's pool. (dusq's CPU path has no cache and uses a 4-frame bounded queue instead.) At most 2 video decoders open (1 for sources above 1080p), decoding only the topmost visible clip plus the next one to become visible; see the decoder budget in ARCHITECTURE.md.
- **Two time units, never mixed silently.** Timeline positions and lengths are `Frame` (integer frames at the sequence frame rate). Clip in/out points into source files are `MediaTime` (integer microseconds). Conversions go through the one conversion module in `dusk-core`. No floats for either.
- **Every edit is a `Command`** with `apply -> Result<(), Rejection>` and `revert`. No direct mutation of the project from the UI layer. The pop-out editor's Apply is one `ApplyClipSession` command over the whole link group.
- **Timeline invariants live in `dusk-core`:** clips on a track never overlap; video tracks hold only video clips; linked clips share trim, position, length and speed and are edited as a group; speed/trim that no longer fits cuts the clip at the gap; ripple delete shifts all unlocked tracks or is rejected. Commands that would break an invariant return a `Rejection`, which the UI shows; the UI never patches around them.
- **Source media is read-only.** Dusk never modifies or copies a user's media file.
- **Exports write to `name.ext.part` and rename on success.** Cancel and failure must leave no half-written output in the user's chosen location.
- **Every long job takes a cancellation token and reports progress.**
- **No network code.** No telemetry, no update checks, no analytics. A link in About that opens the browser is the only exception.
- **Measure memory** when touching decode, cache, render, or export: note idle, during-playback and after-playback private bytes in the PR description, against the budgets in REQUIREMENTS.md.

## Code style

- `cargo fmt` defaults, `clippy` clean with `-D warnings`.
- Errors: `thiserror` in library crates, `anyhow` only in the two binaries (`dusk-app`, `dusk-cli`). Error messages say what happened and what the user can do.
- No `unwrap()` outside tests, except on invariants with a comment explaining why they hold.
- Public items in every library crate (`dusk-core`, the three leaf crates, `dusk-engine`) get doc comments. Keep them short.
- Tests next to the code; `dusk-core` is the most heavily tested crate because it has no dependencies to mock.
- Commits: imperative mood, one logical change each, reference the milestone (`M1: add trim handles to timeline clip`).

## UI conventions (from THEME.md)

- Dark only, flat: no gradients, no shadows. Elevation is a background step plus a 1px border.
- Brand colors in order of importance: primary `#5003C0`, secondary `#AB03A9`, alert `#FF467A`, signal `#FFD51E`. Use `primary-bright` `#A468FD` for text, icons, borders and focus rings; the plain primary is a fill color. Text on alert and signal fills is `bg-0`, not white.
- Video clips are primary, audio clips are secondary, the playhead is alert, markers and warnings are signal.
- Inter at 11–13px for dense UI; JetBrains Mono for timecodes and numeric readouts (Slint can't enable tabular figures). Sentence case everywhere.
- Define colors and sizes once as Slint globals; never hardcode hex in components.
- Every user action gets a keyboard shortcut, registered in the central shortcut table so the shortcut list stays complete.

## When unsure

Prefer the lighter option: fewer dependencies, less memory, less UI. If a feature is not in `docs/FEATURES.md`, ask before building it. If a design choice contradicts `docs/ARCHITECTURE.md`, propose the change in the doc first.
