//! What Windows reads from `dusk.exe` itself: its icon, which Explorer, the taskbar and the
//! installer's shortcuts show, and its version information, which Explorer's file details and
//! Task Manager show (docs/ARCHITECTURE.md, "Release").
#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(file: *const u16, handle: u32, len: u32, data: *mut u8) -> i32;
    fn VerQueryValueW(
        block: *const u8,
        sub: *const u16,
        value: *mut *const u8,
        len: *mut u32,
    ) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ExtractIconExW(
        file: *const u16,
        index: i32,
        large: *mut isize,
        small: *mut isize,
        count: u32,
    ) -> u32;
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

fn exe() -> Vec<u16> {
    wide(OsStr::new(env!("CARGO_BIN_EXE_dusk")))
}

/// The version resource's string `name`, from its US English, Unicode table.
fn version_string(name: &str) -> Option<String> {
    let file = exe();
    let mut handle = 0;
    // SAFETY: `file` is a NUL-terminated wide string that outlives the call.
    let size = unsafe { GetFileVersionInfoSizeW(file.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` holds the `size` bytes the call may write.
    if unsafe { GetFileVersionInfoW(file.as_ptr(), 0, size, data.as_mut_ptr()) } == 0 {
        return None;
    }
    let sub = wide(OsStr::new(&format!("\\StringFileInfo\\040904B0\\{name}")));
    let (mut value, mut len) = (std::ptr::null(), 0u32);
    // SAFETY: `data` is the block the system filled; the value points into it and is read
    // while it lives.
    let found = unsafe { VerQueryValueW(data.as_ptr(), sub.as_ptr(), &mut value, &mut len) };
    if found == 0 || len == 0 {
        return None;
    }
    // SAFETY: a string value is `len` UTF-16 units, the last of them the NUL.
    let units = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), len as usize) };
    Some(
        String::from_utf16_lossy(units)
            .trim_end_matches('\0')
            .to_owned(),
    )
}

#[test]
fn dusk_exe_has_an_icon() {
    let file = exe();
    // SAFETY: index -1 asks only for the number of icons; nothing is written.
    let icons = unsafe {
        ExtractIconExW(
            file.as_ptr(),
            -1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert!(icons >= 1, "dusk.exe has no icon");
}

#[test]
fn dusk_exe_says_what_it_is_and_its_version() {
    assert_eq!(version_string("FileDescription").as_deref(), Some("Dusk"));
    assert_eq!(version_string("ProductName").as_deref(), Some("Dusk"));
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(version_string("ProductVersion").as_deref(), Some(version));
    assert_eq!(version_string("FileVersion").as_deref(), Some(version));
    assert_eq!(
        version_string("OriginalFilename").as_deref(),
        Some("dusk.exe")
    );
}
