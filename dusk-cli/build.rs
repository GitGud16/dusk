use std::env;
use std::fs;
use std::path::PathBuf;

include!("../build-support/windows-resources.rs");

/// Puts the version information into `dusq.exe`, so Explorer names it and a signature can
/// name its product (docs/RELEASE.md, "Logo and icon"). A command-line tool, it keeps
/// Windows' plain icon.
fn main() {
    println!("cargo:rerun-if-changed=../build-support/windows-resources.rs");
    let target = (
        env::var("CARGO_CFG_TARGET_OS").unwrap_or_default(),
        env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default(),
    );
    if target != ("windows".to_owned(), "msvc".to_owned()) {
        return;
    }
    // Build scripts fail by panicking, with the reason in the build's output.
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let program = Program {
        name: "dusq",
        description: "Dusk command line",
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("dusq.res");
    fs::write(&out, resource_file(None, &program, &version)).expect("could not write dusq.res");
    println!("cargo:rustc-link-arg-bin=dusq={}", out.display());
}
