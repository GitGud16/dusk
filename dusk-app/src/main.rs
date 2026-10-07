//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod app;
mod clip_editor;
mod compress_choices;
mod compress_dialog;
mod document;
mod draft;
mod editing;
mod export_choices;
mod export_dialog;
mod files;
mod history;
mod keymap;
mod missing;
mod navigation;
mod platform;
mod recovery;
mod settings;
mod settings_dialog;
mod shortcut_dialog;
mod shortcuts;
mod speed;
mod stats;
mod thumbnails;
mod timeline;

use std::path::PathBuf;

use anyhow::anyhow;
use dusk_engine::{Engine, EngineOptions, Gpu};
use slint::{CloseRequestResponse, ComponentHandle};

use crate::app::{App, with_app};
use crate::document::AUTOSAVE_EVERY;
use crate::files::Worker;

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
    window.set_about_version(env!("CARGO_PKG_VERSION").into());
    window.set_releases_url(about::RELEASES.into());
    // The user's settings and keys, before the engine and the windows use them
    // (docs/ARCHITECTURE.md, "Keyboard and settings"); the files are small, so reading them
    // here keeps no window waiting.
    let settings_dir = platform::settings_dir();
    let (user_settings, settings_problems) = settings::read_settings(settings_dir.as_deref());
    let (keymap, keymap_problems) = settings::read_keymap(settings_dir.as_deref());
    let options = EngineOptions {
        cache_cap: user_settings.cache_bytes(),
        ..EngineOptions::default()
    };
    let engine = Engine::new(gpu, options, app::engine_events())?;
    let files = Worker::start().map_err(|e| anyhow!("Dusk could not start a thread: {e}."))?;
    let mut editor = App::new(&window, engine, files);
    editor.keymap = keymap;
    editor.keymap_problem = settings::problems_message(&keymap_problems).unwrap_or_default();
    window.set_shortcut_problem(editor.keymap_problem.clone().into());
    editor.settings_dir.clone_from(&settings_dir);
    let program = user_settings.ffmpeg.clone();
    editor.settings = user_settings;
    app::install(editor);
    // The first time, a shortcuts file that names every action with its default keys and a
    // settings file with every setting, ready to change.
    if let Some(dir) = settings_dir {
        let shortcuts = !dir.join(settings::SHORTCUTS_FILE).exists();
        let settings = !dir.join(settings::SETTINGS_FILE).exists();
        with_app(|app| {
            app.files.run(move || {
                if shortcuts
                    && let Err(error) = settings::write_keymap(&dir, &keymap::Keymap::default())
                {
                    eprintln!("Dusk could not write its shortcuts file: {error}");
                }
                if settings
                    && let Err(error) =
                        settings::write_settings(&dir, &settings::Settings::default())
                {
                    eprintln!("Dusk could not write its settings file: {error}");
                }
            });
        });
    }
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
    let problems: Vec<String> = settings_problems
        .into_iter()
        .chain(keymap_problems)
        .collect();
    if let Some(message) = settings::problems_message(&problems) {
        for problem in &problems {
            eprintln!("{problem}");
        }
        with_app(|app| app.fail(&message));
    }
    // The user's own ffmpeg, if the settings keep one, is checked on a worker.
    if let Some(program) = program {
        with_app(|app| app.check_program(program, false));
    }
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
    window.on_open_clip(|clip| {
        with_app(|app| app.open_clip_id(clip));
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
    window.on_export_changed(|what, value| {
        with_app(|app| app.export_changed(&what, value));
    });
    window.on_export_done(|export| {
        with_app(|app| app.export_done(export));
    });
    window.on_compress_changed(|what, value| {
        with_app(|app| app.compress_changed(&what, value));
    });
    window.on_compress_done(|go| {
        with_app(|app| app.compress_done(go));
    });
    window.on_dialogs_closed(|| {
        // After the closing call that changed the window has let go of the app.
        let _ = slint::invoke_from_event_loop(|| {
            with_app(App::missing_list_waited);
        });
    });
    window.on_prompt_answered(|index| {
        with_app(|app| app.answer(usize::try_from(index).unwrap_or(usize::MAX)));
    });
    window.on_sequence_settings_done(|apply, rate, width, height| {
        with_app(|app| app.sequence_settings_done(apply, rate, width, height));
    });
    window.on_action(|name| {
        if let Some(action) = shortcuts::Action::named(&name) {
            with_app(|app| app.act(action));
        }
    });
    window.on_shortcut_search(|text| {
        with_app(|app| app.shortcut_search(&text));
    });
    window.on_shortcut_pick(|row| {
        with_app(|app| app.shortcut_pick(row));
    });
    window.on_shortcut_change(|| {
        with_app(App::shortcut_change);
    });
    window.on_shortcut_remove(|| {
        with_app(App::shortcut_remove);
    });
    window.on_shortcut_restore(|| {
        with_app(App::shortcut_restore);
    });
    window.on_shortcut_restore_all(|| {
        with_app(App::shortcut_restore_all);
    });
    window.on_shortcut_close(|| {
        with_app(App::shortcut_close);
    });
    window.on_missing_pick(|row| {
        with_app(|app| app.missing_pick(row));
    });
    window.on_missing_find(|| {
        with_app(App::missing_find);
    });
    window.on_missing_close(|| {
        with_app(App::missing_close);
    });
    window.on_about_close(|| {
        with_app(App::about_close);
    });
    window.on_about_show_licenses(|| {
        with_app(|app| app.about_licenses());
    });
    window.on_about_link_failed(|| {
        with_app(|app| app.about_link_failed());
    });
    window.on_settings_changed(|what, value| {
        with_app(|app| app.settings_changed(&what, value));
    });
    window.on_settings_close(|| {
        with_app(App::settings_close);
    });
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
