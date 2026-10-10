use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // A .slint compile error is a build error; failing the build with Slint's own
    // diagnostics is the intended behavior here.
    slint_build::compile("ui/app.slint")
        .expect("ui/app.slint failed to compile (see the diagnostics above)");
    windows_resources();
}

/// Puts the icon and the version information into `dusk.exe`, as a resource file the
/// linker takes like an object file (docs/ARCHITECTURE.md, "Release"). The icon is
/// `assets/dusk.ico`, which `scripts/make-logo.py` draws.
fn windows_resources() {
    println!("cargo:rerun-if-changed=assets/dusk.ico");
    let target = (
        env::var("CARGO_CFG_TARGET_OS").unwrap_or_default(),
        env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default(),
    );
    if target != ("windows".to_owned(), "msvc".to_owned()) {
        return;
    }
    // Build scripts fail by panicking, with the reason in the build's output.
    let icon = fs::read("assets/dusk.ico")
        .expect("assets/dusk.ico is missing: run scripts/make-logo.py to draw it");
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("dusk.res");
    fs::write(&out, resource_file(&icon, &version)).expect("could not write dusk.res");
    println!("cargo:rustc-link-arg-bin=dusk={}", out.display());
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// US English, the language of the version strings.
const LANGUAGE: u16 = 0x0409;

/// A 32-bit resource file: an empty entry that marks the format, the icon's images, the
/// icon group that names them, and the version information.
fn resource_file(icon: &[u8], version: &str) -> Vec<u8> {
    let mut file = entry(0, 0, &[]);
    let images = icon_images(icon);
    // The group's header: reserved, type 1 (icon), the count.
    let mut group = [0u16, 1, images.len() as u16]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect::<Vec<u8>>();
    for (index, (directory, data)) in images.iter().enumerate() {
        let id = index as u16 + 1;
        file.extend(entry(RT_ICON, id, data));
        // An image's entry in the group is its entry in the ICO file, with its resource id
        // in place of its offset.
        group.extend_from_slice(&directory[..12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    file.extend(entry(RT_GROUP_ICON, 1, &group));
    file.extend(entry(RT_VERSION, 1, &version_info(version)));
    file
}

/// The images of an ICO file: each one's 16-byte directory entry and its data.
fn icon_images(icon: &[u8]) -> Vec<([u8; 16], &[u8])> {
    let bytes = |at: usize, count: usize| {
        icon.get(at..at + count)
            .expect("assets/dusk.ico is cut short: run scripts/make-logo.py again")
    };
    let word = |at: usize| usize::from(u16::from_le_bytes([bytes(at, 2)[0], bytes(at, 2)[1]]));
    let dword = |at: usize| {
        let four = bytes(at, 4);
        u32::from_le_bytes([four[0], four[1], four[2], four[3]]) as usize
    };
    assert_eq!(word(2), 1, "assets/dusk.ico is not an icon");
    (0..word(4))
        .map(|index| {
            let at = 6 + 16 * index;
            let mut directory = [0u8; 16];
            directory.copy_from_slice(bytes(at, 16));
            let (size, offset) = (dword(at + 8), dword(at + 12));
            (directory, bytes(offset, size))
        })
        .collect()
}

/// One resource: its header (type and name as numbers) and its data, padded to four bytes.
fn entry(kind: u16, name: u16, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend((data.len() as u32).to_le_bytes());
    // The header's size: these two fields, the type and the name, and 16 bytes after them.
    bytes.extend(32u32.to_le_bytes());
    for number in [kind, name] {
        bytes.extend(0xFFFFu16.to_le_bytes());
        bytes.extend(number.to_le_bytes());
    }
    // The data version, the memory flags (moveable, pure, discardable), the language, the
    // version and the characteristics.
    let language: u16 = if kind == 0 { 0 } else { LANGUAGE };
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(0x1030u16.to_le_bytes());
    bytes.extend(language.to_le_bytes());
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(0u32.to_le_bytes());
    bytes.extend_from_slice(data);
    pad(&mut bytes);
    bytes
}

fn pad(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A block of the version information: its key, its value (binary, or text counted in
/// UTF-16 units) and the blocks under it, each starting on four bytes.
fn block(key: &str, value: Option<Result<&[u8], &str>>, children: &[Vec<u8>]) -> Vec<u8> {
    let (value_length, kind, value) = match value {
        None => (0, 1u16, Vec::new()),
        Some(Ok(binary)) => (binary.len(), 0, binary.to_vec()),
        Some(Err(text)) => (text.encode_utf16().count() + 1, 1, utf16(text)),
    };
    let mut bytes = vec![0, 0];
    bytes.extend((value_length as u16).to_le_bytes());
    bytes.extend(kind.to_le_bytes());
    bytes.extend(utf16(key));
    pad(&mut bytes);
    bytes.extend(value);
    for child in children {
        pad(&mut bytes);
        bytes.extend_from_slice(child);
    }
    let length = (bytes.len() as u16).to_le_bytes();
    bytes[..2].copy_from_slice(&length);
    bytes
}

/// The version information Explorer and Task Manager show: what the file is, its version
/// and its license.
fn version_info(version: &str) -> Vec<u8> {
    // "0.1.0" or "0.1.0-rc.1": major, minor and patch, each a 16-bit number.
    let mut numbers = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let mut next = || numbers.next().unwrap_or(0);
    let (major, minor, patch) = (next(), next(), next());
    let high = (major << 16) | minor;
    let low = patch << 16;
    let fixed: Vec<u8> = [
        0xFEEF_04BD, // the signature
        0x0001_0000, // the structure's version
        high,
        low, // the file's version
        high,
        low,         // the product's version
        0x3F,        // which flags are meaningful
        0,           // none set: not a debug, patched or pre-release build
        0x0004_0004, // for 32- and 64-bit Windows
        1,           // an application
        0,
        0,
        0, // no subtype and no date
    ]
    .iter()
    .flat_map(|dword: &u32| dword.to_le_bytes())
    .collect();
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "The Dusk contributors"),
        ("FileDescription", "Dusk"),
        ("FileVersion", version),
        ("InternalName", "dusk"),
        (
            "LegalCopyright",
            "Copyright (c) 2026 The Dusk contributors, MIT License",
        ),
        ("OriginalFilename", "dusk.exe"),
        ("ProductName", "Dusk"),
        ("ProductVersion", version),
    ]
    .iter()
    .map(|(key, text)| block(key, Some(Err(text)), &[]))
    .collect();
    // US English in UTF-16, the table the strings are in.
    let table = block("040904B0", None, &strings);
    let translation = [0x09, 0x04, 0xB0, 0x04];
    block(
        "VS_VERSION_INFO",
        Some(Ok(&fixed)),
        &[
            block("StringFileInfo", None, &[table]),
            block(
                "VarFileInfo",
                None,
                &[block("Translation", Some(Ok(&translation)), &[])],
            ),
        ],
    )
}
