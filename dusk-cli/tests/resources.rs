//! What Windows reads from `dusq.exe` itself: its version information, which Explorer's file
//! details show and a signature names (docs/RELEASE.md, "Logo and icon").
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

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// The version resource's string `name`, from its US English, Unicode table.
fn version_string(name: &str) -> Option<String> {
    let file = wide(OsStr::new(env!("CARGO_BIN_EXE_dusq")));
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
fn dusq_exe_names_its_product_and_version() {
    assert_eq!(version_string("ProductName").as_deref(), Some("Dusk"));
    assert_eq!(
        version_string("OriginalFilename").as_deref(),
        Some("dusq.exe")
    );
    assert_eq!(
        version_string("FileDescription").as_deref(),
        Some("Dusk command line")
    );
    assert_eq!(
        version_string("FileVersion").as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}
