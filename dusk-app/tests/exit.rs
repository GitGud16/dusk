//! Starts the real `dusk` binary, so it needs a desktop session and a graphics adapter
//! (hardware, or WARP on CI runners).
#![cfg(windows)]

mod common;

use std::ffi::OsStr;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use common::{
    close, files_in, sample, saved_project_with_clip, start, state_dir, stderr_of,
    wait_for_clean_exit, wait_for_title, wait_for_window,
};

#[test]
fn closing_the_main_window_exits_cleanly() {
    let state = state_dir("empty");
    start_and_close(&[], &state, Duration::from_millis(500));
    // A clean exit leaves no autosave session behind.
    assert_eq!(files_in(&state.join("autosave")), 0);
    // The first start writes the shortcuts and settings files, here in the test's folder
    // rather than the user's own.
    let settings = state.join("settings");
    assert!(settings.join("shortcuts.txt").is_file());
    assert!(settings.join("settings.txt").is_file());
}

#[test]
fn closing_the_window_with_a_project_open_exits_cleanly() {
    let state = state_dir("project");
    let project = saved_project_with_clip(&state, "exit");
    // Long enough for the preview worker to decode and draw the first frame.
    start_and_close(&[project.as_os_str()], &state, Duration::from_millis(2000));
    assert_eq!(files_in(&state.join("autosave")), 0);
}

#[test]
fn closing_with_unsaved_changes_asks_first() {
    let state = state_dir("unsaved");
    // A clip from the command line is imported and placed: changes not yet saved.
    let mut dusk = start(&[sample().as_os_str()], &state);
    let window = wait_for_window(&mut dusk);
    // The title marks unsaved changes once the import is done, which takes longer on a
    // slow runner.
    wait_for_title(window, "no unsaved changes", |title| title.contains('*'));
    close(window);
    sleep(Duration::from_millis(1500));
    let still_running = dusk.try_wait().expect("poll dusk").is_none();
    let _ = dusk.kill();
    let status = dusk.wait();
    let said = stderr_of(&mut dusk);
    assert!(
        still_running,
        "dusk closed without asking about unsaved changes ({status:?}); it said: {said}"
    );
}

/// Starts `dusk` with `args`, waits for its window, lets it run for `linger` as a person
/// would, closes the window and checks that the process exits with code 0.
fn start_and_close(args: &[&OsStr], state: &Path, linger: Duration) {
    let mut dusk = start(args, state);
    let window = wait_for_window(&mut dusk);
    sleep(linger);
    close(window);
    wait_for_clean_exit(&mut dusk);
}
