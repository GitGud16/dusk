//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod document;
mod editing;
mod files;
mod history;
mod platform;
mod recovery;
mod shortcuts;
mod speed;
mod stats;
mod timeline;

use std::path::PathBuf;

use anyhow::anyhow;
use dusk_engine::{Engine, EngineOptions, Gpu};
use slint::{CloseRequestResponse, ComponentHandle};

use crate::app::{App, with_app};
use crate::document::AUTOSAVE_EVERY;
use crate::files::Worker;
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
    let files = Worker::start().map_err(|e| anyhow!("Dusk could not start a thread: {e}."))?;
    app::install(App::new(&window, engine, files));
    connect(&window);
    window.show()?;
    // The preview and the timeline have their sizes now that the window is shown.
    with_app(|app| {
        app.preview_resized(
            window.get_preview_pixel_width(),
            window.get_preview_pixel_height(),
        );
        app.timeline_resized(window.get_timeline_width());
    });
    // Files named on the command line: a project to open, or media to import and place.
    let named: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    with_app(|app| app.start(named));
    let autosave = slint::Timer::default();
    autosave.start(slint::TimerMode::Repeated, AUTOSAVE_EVERY, || {
        with_app(App::autosave);
    });
    slint::run_event_loop()?;
    drop(autosave);
    window.hide()?;
    with_app(App::shut_down);
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
    window.on_select_clip(|clip| {
        with_app(|app| app.select_clip(clip));
    });
    window.on_trim(|clip, start, frame| {
        with_app(|app| app.trim(clip, start, frame));
    });
    window.on_snap_move(|clip, frame, row| {
        with_app(|app| app.snap_move(clip, frame, row)).unwrap_or(-1)
    });
    window.on_move_clip(|clip, frame, row| {
        with_app(|app| app.move_clip(clip, frame, row));
    });
    window.on_toggle_lock(|track| {
        with_app(|app| app.toggle_track_by_id(track, true));
    });
    window.on_toggle_mute(|track| {
        with_app(|app| app.toggle_track_by_id(track, false));
    });
    window.on_scroll_by(|frames| {
        with_app(|app| app.scroll_by(frames));
    });
    window.on_zoom_by(|factor, frame| {
        with_app(|app| app.zoom_by(factor, dusk_core::Frame(frame.into())));
    });
    window.on_select_media(|media| {
        with_app(|app| app.select_media(media));
    });
    window.on_place_media_at_playhead(|media| {
        with_app(|app| app.place_media_at_playhead(media));
    });
    window.on_snap_place(|media, row, frame| {
        with_app(|app| app.snap_place(media, row, frame)).unwrap_or(-1)
    });
    window.on_place_media(|media, row, frame| {
        with_app(|app| app.place_media(media, row, frame));
    });
    window.on_media_item(|media| with_app(|app| app.media_view(media)).unwrap_or_default());
    window.on_set_clip_enabled(|enabled| {
        with_app(|app| app.set_clip_enabled(enabled));
    });
    window.on_set_clip_fill(|fill| {
        with_app(|app| app.set_clip_fill(fill));
    });
    window.on_set_clip_length(|frames| {
        with_app(|app| app.set_clip_length(frames));
    });
    window.on_set_clip_volume(|decibels| {
        with_app(|app| app.set_clip_volume(decibels));
    });
    window.on_set_clip_fades(|fade_in, fade_out| {
        with_app(|app| app.set_clip_fades(fade_in, fade_out));
    });
    window.on_unlink_clip(|| {
        with_app(App::unlink);
    });
    window.on_play_pause(|| {
        with_app(App::play_pause);
    });
    window.on_prompt_answered(|index| {
        with_app(|app| app.answer(usize::try_from(index).unwrap_or(usize::MAX)));
    });
    window.on_sequence_settings_done(|apply, rate, width, height| {
        with_app(|app| app.sequence_settings_done(apply, rate, width, height));
    });
    window.on_action(|name| {
        if let Some(action) = shortcuts::action_named(&name) {
            with_app(|app| app.act(action));
        }
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
        with_app(|app| app.key(&text, ctrl, shift, alt)).unwrap_or(false)
    });
    // Closing the window asks about unsaved changes first.
    window.window().on_close_requested(|| {
        if with_app(App::may_close).unwrap_or(true) {
            CloseRequestResponse::HideWindow
        } else {
            CloseRequestResponse::KeepWindowShown
        }
    });
    let weak = window.as_weak();
    platform::watch_window(
        window.window(),
        |paths| {
            with_app(|app| app.import_dropped(paths));
        },
        move |hovering| {
            if let Some(window) = weak.upgrade() {
                window.set_files_hovering(hovering);
            }
        },
    );
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
