//! The `dusq` binary as people run it: its exit codes, what it writes and what it refuses.
//! Files are written under the target directory, never beside the test data.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata")
        .join(name)
}

/// A fresh, empty folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dusq")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn dusq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dusq"))
        .args(args)
        .output()
        .expect("dusq runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn compress_writes_beside_the_video_by_default() {
    let dir = folder("beside");
    let video = dir.join("beach.mp4");
    std::fs::copy(testdata("sample-h264-aac.mp4"), &video).unwrap();
    let run = dusq(&["compress", video.to_str().unwrap(), "--quality", "small"]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert!(dir.join("beach compressed.mp4").is_file());
    assert!(stderr(&run).contains("Wrote"), "{}", stderr(&run));
    // A second run steps around the first one's file.
    let again = dusq(&["compress", video.to_str().unwrap(), "--quality", "small"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(dir.join("beach compressed 2.mp4").is_file());
}

#[test]
fn extract_audio_writes_the_sound_alone() {
    let dir = folder("sound");
    let output = dir.join("tone.wav");
    let run = dusq(&[
        "extract-audio",
        testdata("sample-h264-aac.mp4").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(run.status.success(), "{}", stderr(&run));
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
}

#[test]
fn an_existing_file_is_kept_unless_told_to_replace_it() {
    let dir = folder("existing");
    let output = dir.join("taken.mp4");
    std::fs::write(&output, b"keep me").unwrap();
    let input = testdata("sample-h264-aac.mp4");
    let args = [
        "compress",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ];
    let refused = dusq(&args);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr(&refused).contains("--overwrite"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"keep me");
    let replaced = dusq(&[&args[..], &["--overwrite"]].concat());
    assert!(replaced.status.success(), "{}", stderr(&replaced));
    assert_ne!(std::fs::read(&output).unwrap(), b"keep me");
}

#[test]
fn the_input_is_never_written_over() {
    let dir = folder("input");
    let video = dir.join("clip.mp4");
    std::fs::copy(testdata("sample-h264-aac.mp4"), &video).unwrap();
    let before = std::fs::read(&video).unwrap();
    let path = video.to_str().unwrap();
    let refused = dusq(&["compress", path, "-o", path, "--overwrite"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr(&refused).contains("never writes over"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(std::fs::read(&video).unwrap(), before);
}

#[test]
fn a_missing_file_is_reported() {
    let run = dusq(&["compress", "no such file.mp4"]);
    assert_eq!(run.status.code(), Some(1));
    assert!(
        stderr(&run).contains("not a file that exists"),
        "{}",
        stderr(&run)
    );
}

#[test]
fn misuse_shows_how_to_use_it() {
    let run = dusq(&["squash", "clip.mp4"]);
    assert_eq!(run.status.code(), Some(2));
    assert!(stderr(&run).contains("--help"), "{}", stderr(&run));
    let help = dusq(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("dusq compress"));
}

#[test]
fn compress_aims_at_a_size() {
    let dir = folder("size");
    let output = dir.join("small.mp4");
    let input = testdata("sample-h264-aac.mp4");
    let run = dusq(&[
        "compress",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--size",
        "100KB",
    ]);
    assert!(run.status.success(), "{}", stderr(&run));
    let bytes = std::fs::metadata(&output).unwrap().len();
    assert!((40_000..=150_000).contains(&bytes), "{bytes} bytes");
}

#[test]
fn a_size_too_small_says_the_smallest() {
    let dir = folder("too-small");
    let output = dir.join("tiny.mp4");
    let input = testdata("sample-h264-aac.mp4");
    let run = dusq(&[
        "compress",
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--size",
        "10KB",
    ]);
    assert_eq!(run.status.code(), Some(1));
    let said = stderr(&run);
    assert!(
        said.contains("smallest") && said.contains("--size 0.1MB"),
        "{said}"
    );
    assert!(!output.exists());
}
