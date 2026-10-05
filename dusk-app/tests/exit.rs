//! Starts the real `dusk` binary, so it needs a desktop session and a graphics adapter
//! (hardware, or WARP on CI runners).
#![cfg(windows)]

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(isize, isize) -> i32, param: isize) -> i32;
    fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    fn IsWindowVisible(window: isize) -> i32;
    fn GetWindowTextW(window: isize, text: *mut u16, capacity: i32) -> i32;
    fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
}

const WM_CLOSE: u32 = 0x0010;

struct Search {
    process_id: u32,
    found: isize,
}

unsafe extern "system" fn visit(window: isize, param: isize) -> i32 {
    // SAFETY: `param` is the `&mut Search` that `main_window_of` passes to `EnumWindows`,
    // which calls back synchronously while that borrow is alive.
    let search = unsafe { &mut *(param as *mut Search) };
    let mut process_id = 0;
    let mut title = [0u16; 128];
    // SAFETY: plain Win32 queries on a window handle EnumWindows just gave us, writing into
    // locals that outlive the calls.
    let (visible, length) = unsafe {
        GetWindowThreadProcessId(window, &mut process_id);
        (
            IsWindowVisible(window) != 0,
            GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32),
        )
    };
    let length = usize::try_from(length).unwrap_or(0);
    // "Untitled — Dusk", "edit — Dusk".
    if process_id == search.process_id
        && visible
        && String::from_utf16_lossy(&title[..length]).ends_with("Dusk")
    {
        search.found = window;
        return 0;
    }
    1
}

/// The title of `window`.
fn title_of(window: isize) -> String {
    let mut title = [0u16; 256];
    // SAFETY: reads at most `title.len()` characters into a local buffer.
    let length = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
    String::from_utf16_lossy(&title[..usize::try_from(length).unwrap_or(0)])
}

/// The visible top-level Dusk window that belongs to `process_id`, if any.
fn main_window_of(process_id: u32) -> Option<isize> {
    let mut search = Search {
        process_id,
        found: 0,
    };
    // SAFETY: `visit` matches the callback signature and only uses `search` during the call.
    unsafe { EnumWindows(visit, &mut search as *mut Search as isize) };
    (search.found != 0).then_some(search.found)
}

#[test]
fn closing_the_main_window_exits_cleanly() {
    let state = state_dir("empty");
    start_and_close(&[], &state, Duration::from_millis(500));
    // A clean exit leaves no autosave session behind.
    assert_eq!(files_in(&state.join("autosave")), 0);
}

#[test]
fn closing_the_window_with_a_project_open_exits_cleanly() {
    let state = state_dir("project");
    let project = saved_project_with_clip(&state);
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
    let deadline = Instant::now() + Duration::from_secs(60);
    while !title_of(window).contains('*') {
        assert!(Instant::now() < deadline, "no unsaved changes within 60 s");
        sleep(Duration::from_millis(50));
    }
    // SAFETY: posting a message to a window handle is safe even if the window is gone.
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
    sleep(Duration::from_millis(1500));
    let still_running = dusk.try_wait().expect("poll dusk").is_none();
    let _ = dusk.kill();
    let _ = dusk.wait();
    assert!(
        still_running,
        "dusk closed without asking about unsaved changes"
    );
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

/// A fresh folder for Dusk's own files, so tests never touch the user's.
fn state_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("dusk-exit-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("make a state folder");
    dir
}

fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

/// A project file in `dir` with the sample clip on the timeline.
fn saved_project_with_clip(dir: &Path) -> PathBuf {
    use dusk_core::{Frame, Project, import};
    let info = dusk_engine::media_info(&sample()).expect("probe the sample");
    let rate = info.frame_rate.expect("the sample has video");
    let mut project = Project::new(rate, (info.width, info.height));
    import(&project, sample(), info, Frame(0))
        .apply(&mut project)
        .expect("import the sample");
    let file = dir.join("exit.dusk");
    std::fs::write(&file, dusk_core::file::to_json(&project, &file)).expect("save");
    file
}

fn start(args: &[&OsStr], state: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_dusk"))
        .args(args)
        .env("DUSK_STATE_DIR", state)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start dusk")
}

/// Starts `dusk` with `args`, waits for its window, lets it run for `linger` as a person
/// would, closes the window and checks that the process exits with code 0.
fn start_and_close(args: &[&OsStr], state: &Path, linger: Duration) {
    let mut dusk = start(args, state);
    let window = wait_for_window(&mut dusk);
    sleep(linger);
    // SAFETY: posting a message to a window handle is safe even if the window is gone.
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = dusk.try_wait().expect("poll dusk") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = dusk.kill();
            panic!("dusk did not exit within 30 s of closing its window");
        }
        sleep(Duration::from_millis(50));
    };
    let stderr = stderr_of(&mut dusk);
    assert_eq!(status.code(), Some(0), "dusk's stderr:\n{stderr}");
}

/// Waits for `dusk` to show its window, which it returns.
fn wait_for_window(dusk: &mut Child) -> isize {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(window) = main_window_of(dusk.id()) {
            return window;
        }
        if let Some(status) = dusk.try_wait().expect("poll dusk") {
            panic!(
                "dusk exited before showing its window ({status}); its stderr:\n{}",
                stderr_of(dusk)
            );
        }
        assert!(Instant::now() < deadline, "no Dusk window within 60 s");
        sleep(Duration::from_millis(50));
    }
}

/// What the exited `child` wrote to stderr.
fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}
