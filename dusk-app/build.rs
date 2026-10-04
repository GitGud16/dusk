fn main() {
    // A .slint compile error is a build error; failing the build with Slint's own
    // diagnostics is the intended behavior here.
    slint_build::compile("ui/app.slint")
        .expect("ui/app.slint failed to compile (see the diagnostics above)");
}
