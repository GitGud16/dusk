//! What the tests that start the real `dusk` binary share: starting it with folders of its
//! own, finding its window, and posting messages to it as Windows would.
// Each test file uses only some of these.
#![allow(dead_code)]

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
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
    fn GetMenu(window: isize) -> isize;
    fn GetMenuItemCount(menu: isize) -> i32;
    fn GetSubMenu(menu: isize, position: i32) -> isize;
    fn GetMenuItemID(menu: isize, position: i32) -> u32;
    fn GetMenuStringW(menu: isize, item: u32, text: *mut u16, capacity: i32, flags: u32) -> i32;
}

const WM_CLOSE: u32 = 0x0010;
const WM_COMMAND: u32 = 0x0111;
const MF_BYPOSITION: u32 = 0x0400;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;

/// Virtual-key codes of the named keys the tests press.
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_DELETE: u32 = 0x2E;
pub const VK_F1: u32 = 0x70;

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
pub fn title_of(window: isize) -> String {
    let mut title = [0u16; 256];
    // SAFETY: reads at most `title.len()` characters into a local buffer.
    let length = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
    String::from_utf16_lossy(&title[..usize::try_from(length).unwrap_or(0)])
}

/// Waits up to 60 s for `window`'s title to pass `test`, a slow runner being slow.
pub fn wait_for_title(window: isize, what: &str, test: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !test(&title_of(window)) {
        assert!(
            Instant::now() < deadline,
            "{what} within 60 s; the title is \"{}\"",
            title_of(window)
        );
        sleep(Duration::from_millis(50));
    }
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

/// Asks `window` to close, as its close button does.
pub fn close(window: isize) {
    // SAFETY: posting a message to a window handle is safe even if the window is gone.
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
}

/// Chooses the menu item of `window` whose text, without its keys, is `item`, as a click on
/// it does.
pub fn menu(window: isize, item: &str) {
    fn find(menu: isize, item: &str) -> Option<u32> {
        // SAFETY: plain Win32 queries on a menu handle, writing into a local buffer.
        let count = unsafe { GetMenuItemCount(menu) };
        for position in 0..count {
            let sub = unsafe { GetSubMenu(menu, position) };
            if sub != 0 {
                if let Some(id) = find(sub, item) {
                    return Some(id);
                }
                continue;
            }
            let mut text = [0u16; 256];
            // SAFETY: as above; at most `text.len()` characters are written.
            let length = unsafe {
                GetMenuStringW(
                    menu,
                    position as u32,
                    text.as_mut_ptr(),
                    text.len() as i32,
                    MF_BYPOSITION,
                )
            };
            let text = String::from_utf16_lossy(&text[..usize::try_from(length).unwrap_or(0)]);
            let name = text.split('\t').next().unwrap_or("").replace('&', "");
            if name == item {
                // SAFETY: as above.
                return Some(unsafe { GetMenuItemID(menu, position) });
            }
        }
        None
    }
    // SAFETY: a plain Win32 query on the window handle.
    let bar = unsafe { GetMenu(window) };
    let id = find(bar, item).unwrap_or_else(|| panic!("no menu item {item:?}"));
    // SAFETY: posting a message to a window handle is safe even if the window is gone.
    unsafe { PostMessageW(window, WM_COMMAND, id as usize, 0) };
    sleep(Duration::from_millis(100));
}

/// Presses and lets go of the key `vk` in `window`, typing `typed` as a letter key does.
/// Posted keys reach Dusk as typed ones do, but they cannot hold Shift, Ctrl or Alt down.
pub fn press(window: isize, vk: u32, typed: Option<char>) {
    // SAFETY: a pure lookup in the keyboard layout.
    let scan = unsafe { MapVirtualKeyW(vk, 0) };
    // The arrows, Home, End and Delete sit apart from the number pad's keys.
    let extended = if (0x21..=0x2E).contains(&vk) {
        0x0100_0000
    } else {
        0
    };
    let down = 1 | (isize::try_from(scan).unwrap_or(0) << 16) | extended;
    let up = down | 0xC000_0000;
    // SAFETY: posting messages to a window handle is safe even if the window is gone.
    unsafe {
        PostMessageW(window, WM_KEYDOWN, vk as usize, down);
        if let Some(typed) = typed {
            PostMessageW(window, WM_CHAR, typed as usize, down);
        }
    }
    sleep(Duration::from_millis(30));
    // SAFETY: as above.
    unsafe { PostMessageW(window, WM_KEYUP, vk as usize, up) };
    sleep(Duration::from_millis(30));
}

/// Presses the letter or digit key that types `letter`.
pub fn type_key(window: isize, letter: char) {
    press(window, u32::from(letter.to_ascii_uppercase()), Some(letter));
}

pub fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

/// A fresh folder for Dusk's own files, so tests never touch the user's.
pub fn state_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("dusk-app-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("make a state folder");
    dir
}

pub fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

/// A project file `<name>.dusk` in `dir` with the sample clip on the timeline at frame 0.
pub fn saved_project_with_clip(dir: &Path, name: &str) -> PathBuf {
    use dusk_core::{Frame, Project, import};
    let info = dusk_engine::media_info(&sample()).expect("probe the sample");
    let rate = info.frame_rate.expect("the sample has video");
    let mut project = Project::new(rate, (info.width, info.height));
    import(&project, sample(), info, Frame(0))
        .apply(&mut project)
        .expect("import the sample");
    let file = dir.join(format!("{name}.dusk"));
    std::fs::write(&file, dusk_core::file::to_json(&project, &file)).expect("save");
    file
}

/// Starts `dusk` with `args`, its autosaves and settings in `state`.
pub fn start(args: &[&OsStr], state: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_dusk"))
        .args(args)
        .env("DUSK_STATE_DIR", state)
        .env("DUSK_SETTINGS_DIR", state.join("settings"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start dusk")
}

/// Waits up to 30 s for `dusk` to exit, and checks that it exited with code 0.
pub fn wait_for_clean_exit(dusk: &mut Child) {
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
    let stderr = stderr_of(dusk);
    assert_eq!(status.code(), Some(0), "dusk's stderr:\n{stderr}");
}

/// Waits for `dusk` to show its window, which it returns.
pub fn wait_for_window(dusk: &mut Child) -> isize {
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
pub fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}
