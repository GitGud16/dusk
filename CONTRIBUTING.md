# Contributing to Dusk

Thank you for helping. Dusk is a small, light video editor, and it stays that way: few features, built in, fast, and measured. Before building something that is not in [docs/FEATURES.md](docs/FEATURES.md), open an issue to talk it over; [docs/VISION.md](docs/VISION.md) says what Dusk is not.

## Setting up

Windows 10 or 11, Rust, the Visual Studio C++ build tools, LLVM, and the one FFmpeg build Dusk is pinned to. [docs/SETUP.md](docs/SETUP.md) has every step; in short, once:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-ffmpeg.ps1
```

and in every shell you build from:

```powershell
. .\scripts\dev-env.ps1
cargo run -p dusk-app
```

## Before you open a pull request

CI runs these, and they must pass:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy -p dusk-engine --no-default-features --all-targets -- -D warnings
cargo test -p dusk-engine --no-default-features
cargo build -p dusk-cli --release
```

`dusq` (the command line, `dusk-cli`) must never link wgpu; CI checks that too. When you touch decoding, the frame cache, rendering or export, measure memory (idle, while playing, and 10 seconds after) and put the numbers in the pull request, against the budgets in [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md).

## How the code is laid out

Seven crates, with dependencies pointing down only: `dusk-app` (the editor) and `dusk-cli` (`dusq`) use `dusk-engine`, which uses `dusk-media` (FFmpeg), `dusk-render` (the GPU) and `dusk-audio`, which use `dusk-core` (the project model, with no I/O). [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains each, and is the place to propose a change of design before making it.

## Rules the code keeps

- The UI thread never waits: decoding, encoding, reading files and anything else that can take more than a few milliseconds runs on a worker and reports back.
- Decoded frames live in the frame cache only, never in view models or clips.
- Timeline positions are whole frames (`Frame`) and source times whole microseconds (`MediaTime`); they meet only in `dusk-core`'s conversion module. No floats for either.
- Every edit is a `Command` with `apply` and `revert`; the UI never changes the project directly. A command that would break a timeline rule returns a `Rejection`, which the UI shows as it is.
- Dusk never changes or copies a user's media file, writes exports to `name.ext.part` and renames them only once they are whole, and gives every long job a way to cancel and a progress report.
- No network code: no telemetry, no update checks. The About dialog's link to the releases page, which the system opens, is the one exception.
- No GPL dependency is linked or shipped.

## Style

- `cargo fmt` as it comes, and clippy clean with `-D warnings`.
- Errors use `thiserror` in the library crates and `anyhow` only in the two programs. Every message a user can see says what happened and what they can do.
- No `unwrap()` outside tests, except on an invariant, with a comment saying why it holds.
- Public items of the library crates have short doc comments, and tests sit next to the code they test.
- The UI follows [docs/THEME.md](docs/THEME.md): dark and flat, colors and sizes from the theme's tokens, sentence case everywhere. Every action has keys in the shortcut table (`dusk-app/src/shortcuts.rs`); after changing it, write [docs/SHORTCUTS.md](docs/SHORTCUTS.md) again by running `cargo test -p dusk-app reference` with `DUSK_WRITE_SHORTCUTS` set (`$env:DUSK_WRITE_SHORTCUTS = 1` in PowerShell).

## Commits

One logical change each, in the imperative, starting with the milestone it belongs to: `M6: draw the logo and build it into dusk.exe`.

## License

Dusk's own code is under the MIT License ([LICENSE](LICENSE)); by contributing you agree that your contribution is too.
