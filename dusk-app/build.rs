use std::env;
use std::fs;
use std::path::PathBuf;

include!("../build-support/windows-resources.rs");

fn main() {
    // A .slint compile error is a build error; failing the build with Slint's own
    // diagnostics is the intended behavior here.
    slint_build::compile("ui/app.slint")
        .expect("ui/app.slint failed to compile (see the diagnostics above)");
    windows_resources();
}

/// Puts the icon and the version information into `dusk.exe` (docs/RELEASE.md, "Logo and
/// icon"). The icon is `assets/dusk.ico`, which `scripts/make-logo.py` draws.
fn windows_resources() {
    println!("cargo:rerun-if-changed=assets/dusk.ico");
    println!("cargo:rerun-if-changed=../build-support/windows-resources.rs");
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
    let program = Program {
        name: "dusk",
        description: "Dusk",
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("dusk.res");
    fs::write(&out, resource_file(Some(&icon), &program, &version))
        .expect("could not write dusk.res");
    println!("cargo:rustc-link-arg-bin=dusk={}", out.display());
}
