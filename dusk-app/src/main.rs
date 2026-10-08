//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod files;
mod history;
mod shortcuts;
mod stats;
mod timeline;

use std::path::PathBuf;

use anyhow::anyhow;
use dusk_engine::{Engine, EngineOptions, Gpu};

use crate::app::{App, with_app};
use crate::shortcuts::SHORTCUTS;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    // Dusk picks the adapter itself and prefers hardware (dusk_render::Gpu). Without any
    // hardware adapter (a VM, a CI runner) it gets WARP, Windows' software rasterizer, and
    // Slint renders on such a CPU adapter only when this variable is set.
    // SAFETY: nothing else runs yet, so no other thread can read the environment meanwhile.
    unsafe { std::env::set_var("SLINT_WGPU_CPU", "1") };

    // The GPU objects live for the whole process, on purpose. Slint keeps its own references
    // in thread-local state that is destroyed at exit after wgpu's thread-locals, and
    // destroying the device there panics (exit code 2170 instead of 0). With this reference
    // never dropped, that teardown only lowers a reference count.
    let gpu: &'static Gpu = Box::leak(Box::new(select_renderer()?));
    let window = MainWindow::new()?;
    let engine = Engine::new(gpu, EngineOptions::default(), app::engine_events())?;
    app::install(App::new(&window, engine));
    connect(&window);

    // M1 opens one clip from the command line (docs/ROADMAP.md); the media bin comes in M2.
    match std::env::args_os().nth(1) {
        Some(path) => {
            with_app(|app| app.open(PathBuf::from(path)));
        }
        None => window
            .set_preview_message("Open a clip by starting Dusk with it: dusk.exe clip.mp4".into()),
    }
    window.show()?;
    // The preview and the timeline have their sizes now that the window is shown.
    with_app(|app| {
        app.preview_resized(
            window.get_preview_pixel_width(),
            window.get_preview_pixel_height(),
        );
        app.timeline_resized(window.get_timeline_width());
    });
    slint::run_event_loop()?;
    window.hide()?;
    app::uninstall();
    Ok(())
}

/// Hands the window's callbacks to the editor.
fn connect(window: &MainWindow) {
    window.on_preview_resized(|width, height| {
        with_app(|app| app.preview_resized(width, height));
    });
    window.on_timeline_resized(|width| {
        with_app(|app| app.timeline_resized(width));
    });
    window.on_scrub(|frame| {
        with_app(|app| app.scrub(frame));
    });
    window.on_scrub_end(|frame| {
        with_app(|app| app.scrub_end(frame));
    });
    window.on_trim(|clip, start, frame| {
        with_app(|app| app.trim(clip, start, frame));
    });
    window.on_play_pause(|| {
        with_app(App::play_pause);
    });
    window.on_export(|| {
        with_app(App::export);
    });
    window.on_cancel_export(|| {
        with_app(App::cancel_export);
    });
    let list: Vec<ShortcutView> = SHORTCUTS
        .iter()
        .map(|shortcut| ShortcutView {
            keys: shortcut.keys().into(),
            description: shortcut.description.into(),
        })
        .collect();
    window.set_shortcuts(std::rc::Rc::new(slint::VecModel::from(list)).into());
    window.on_key(|text, ctrl, shift, alt| {
        let Some(action) = shortcuts::action_for(&text, ctrl, shift, alt) else {
            return false;
        };
        with_app(|app| app.act(action));
        true
    });
}

/// Creates Dusk's GPU device and has Slint's FemtoVG renderer use it, so the compositor and
/// the UI share one device and queue (docs/ARCHITECTURE.md, "Slint specifics").
fn select_renderer() -> anyhow::Result<Gpu> {
    let gpu = Gpu::new().map_err(|e| anyhow!("Dusk could not start its GPU renderer: {e}."))?;
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance: gpu.instance.clone(),
            adapter: gpu.adapter.clone(),
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
        })
        .select()
        .map_err(|e| {
            anyhow!(
                "Dusk could not start its GPU renderer ({e}); update the graphics driver and try again."
            )
        })?;
    Ok(gpu)
}
