//! Starts the real `dusk` binary, so it needs a desktop session and a graphics adapter
//! (hardware, or WARP on CI runners).
#![cfg(windows)]

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
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
    let mut title = [0u16; 16];
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
    if process_id == search.process_id
        && visible
        && String::from_utf16_lossy(&title[..length]) == "Dusk"
    {
        search.found = window;
        return 0;
    }
    1
}

/// The visible top-level window titled "Dusk" that belongs to `process_id`, if any.
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
    start_and_close(&[], Duration::from_millis(500));
}

#[test]
fn closing_the_window_with_a_clip_open_exits_cleanly() {
    let clip = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4");
    // Long enough for the preview worker to decode and draw the first frame.
    start_and_close(&[clip.as_os_str()], Duration::from_millis(2000));
}

/// Starts `dusk` with `args`, waits for its window, lets it run for `linger` as a person
/// would, closes the window and checks that the process exits with code 0.
fn start_and_close(args: &[&OsStr], linger: Duration) {
    let mut dusk = Command::new(env!("CARGO_BIN_EXE_dusk"))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start dusk");

    let deadline = Instant::now() + Duration::from_secs(60);
    let window = loop {
        if let Some(window) = main_window_of(dusk.id()) {
            break window;
        }
        if let Some(status) = dusk.try_wait().expect("poll dusk") {
            panic!(
                "dusk exited before showing its window ({status}); its stderr:\n{}",
                stderr_of(&mut dusk)
            );
        }
        assert!(Instant::now() < deadline, "no Dusk window within 60 s");
        sleep(Duration::from_millis(50));
    };
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

/// What the exited `child` wrote to stderr.
fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}
