//! `dusq`, the Dusk command line: `dusq compress` and `dusq extract-audio`
//! (docs/ARCHITECTURE.md, "Compress tool paths"). It transcodes on the CPU, without a
//! graphics adapter, and writes beside the input unless told where.

mod args;
mod files;
mod platform;
mod progress;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::{Context, bail};
use dusk_engine::{
    EngineError, ExportFormat, ExportSettings, Progress, TranscodeSettings, Transcoded,
    extract_audio, transcode, transcode_to_size,
};

use args::{Command, Compress, Extract};
use platform::STOP;

/// Exit codes: done, failed, misused, and stopped with Ctrl+C.
const FAILED: u8 = 1;
const MISUSED: u8 = 2;
const STOPPED: u8 = 130;

fn main() -> ExitCode {
    // FFmpeg's own messages stay off the console, where dusq says what happens. Kvazaar and
    // SVT-AV1 still print their settings when they start: they write to the console
    // themselves, and the DLLs read SVT_LOG from an environment copied before dusq runs.
    dusk_engine::quiet_logs();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match args::parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("dusq: {message}");
            return ExitCode::from(MISUSED);
        }
    };
    let outcome = match command {
        Command::Help => {
            print!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("dusq {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Command::Compress(compress) => run_compress(&compress),
        Command::ExtractAudio(extract) => run_extract(&extract),
    };
    match outcome {
        Ok(Some(())) => ExitCode::SUCCESS,
        Ok(None) => {
            eprintln!("dusq: stopped; nothing was written");
            ExitCode::from(STOPPED)
        }
        Err(error) => {
            eprintln!("dusq: {error:#}");
            ExitCode::from(FAILED)
        }
    }
}

fn run_compress(compress: &Compress) -> anyhow::Result<Option<()>> {
    let extension = compress.container.extension();
    let output = output_for(
        &compress.input,
        compress.output.as_deref(),
        " compressed",
        extension,
        compress.overwrite,
    )?;
    let settings = TranscodeSettings {
        export: ExportSettings {
            format: ExportFormat::Video {
                container: compress.container,
                codec: compress.codec,
                audio: compress.container.audio_codecs()[0],
            },
            short_side: compress.short_side,
            quality: compress.quality,
            audio_bit_rate: Some(128_000),
        },
        fps: compress.fps,
        threads: compress.threads,
        ..TranscodeSettings::default()
    };
    eprintln!(
        "Compressing {} into {}",
        compress.input.display(),
        output.display()
    );
    let Some(mut target) = compress.size else {
        return run(&output, None, |report| {
            transcode(&compress.input, &output, &settings, &STOP, report)
        });
    };
    loop {
        let outcome = run(&output, Some(target), |report| {
            transcode_to_size(&compress.input, &output, target, &settings, &STOP, report)
        });
        let smallest = match &outcome {
            Err(error) => match error.downcast_ref::<EngineError>() {
                Some(EngineError::TooSmall { smallest }) => *smallest,
                _ => return outcome,
            },
            Ok(_) => return outcome,
        };
        let size = progress::size_up(smallest);
        if !ask(&format!(
            "The smallest this video can become is {size}. Make it that size instead?"
        )) {
            bail!(
                "the smallest this video can become is {size}; run again with --size {} or more",
                size.replace(' ', "")
            );
        }
        target = smallest;
    }
}

/// Asks a question on the console that a yes answers; no when nobody is there to answer.
fn ask(question: &str) -> bool {
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

fn run_extract(extract: &Extract) -> anyhow::Result<Option<()>> {
    let output = output_for(
        &extract.input,
        extract.output.as_deref(),
        "",
        extract.format.extension(),
        extract.overwrite,
    )?;
    eprintln!(
        "Taking the sound of {} into {}",
        extract.input.display(),
        output.display()
    );
    run(&output, None, |report| {
        extract_audio(&extract.input, &output, extract.format, &STOP, report)
    })
}

/// Where a job on `input` writes: `output` when given, which must not be the input and must
/// not exist unless `overwrite`; otherwise a free name beside the input.
fn output_for(
    input: &Path,
    output: Option<&Path>,
    suffix: &str,
    extension: &str,
    overwrite: bool,
) -> anyhow::Result<PathBuf> {
    if !input.is_file() {
        bail!(
            "{} is not a file that exists; check the path and try again",
            input.display()
        );
    }
    let Some(output) = output else {
        return Ok(files::beside(input, suffix, extension));
    };
    if files::same_file(input, output) {
        bail!(
            "{} is the file being read; dusq never writes over its input, so choose another name with -o",
            output.display()
        );
    }
    if output.exists() && !overwrite {
        bail!(
            "{} already exists; pass --overwrite to replace it, or choose another name with -o",
            output.display()
        );
    }
    Ok(output.to_path_buf())
}

/// Runs `job`, which writes `output` (aiming at `target` bytes when given), showing its
/// progress on a terminal and what it made at the end; `None` when stopped with Ctrl+C.
fn run(
    output: &Path,
    target: Option<u64>,
    job: impl FnOnce(&mut dyn FnMut(Progress)) -> Result<Option<Transcoded>, dusk_engine::EngineError>,
) -> anyhow::Result<Option<()>> {
    platform::stop_on_ctrl_c();
    let start = Instant::now();
    let terminal = std::io::stderr().is_terminal();
    let mut shown = 0usize;
    let mut second_pass = false;
    let mut report = |progress: Progress| {
        if progress.pass == 2 && !second_pass {
            second_pass = true;
            if terminal && shown > 0 {
                eprintln!();
                shown = 0;
            }
            eprintln!("It came out more than 3% over that size; compressing again, smaller.");
        }
        if terminal {
            let line = progress::line(progress, start.elapsed());
            // Over the last line, clearing what is left of it.
            let padding = shown.saturating_sub(line.len());
            eprint!("\r{line}{:padding$}", "");
            let _ = std::io::stderr().flush();
            shown = line.len();
        }
    };
    let done = job(&mut report);
    if terminal && shown > 0 {
        eprintln!();
    }
    let done = done.with_context(|| format!("{} was not written", output.display()))?;
    let Some(done) = done else {
        return Ok(None);
    };
    if STOP.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let made = match done.size {
        (0, 0) => format!("{}, {}", progress::size(done.bytes), done.encoder),
        (width, height) => format!(
            "{}, {width} × {height}, {}",
            progress::size(done.bytes),
            done.encoder
        ),
    };
    eprintln!(
        "Wrote {} ({made}) in {}.",
        done.path.display(),
        progress::clock(start.elapsed().as_secs())
    );
    if let Some(target) = target.filter(|target| done.bytes > *target) {
        let over = (done.bytes - target) as f64 * 100.0 / target as f64;
        eprintln!(
            "That is {over:.1}% over the {} asked for, as near as one more try could bring it.",
            progress::size(target)
        );
    }
    Ok(Some(()))
}
