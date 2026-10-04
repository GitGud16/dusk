//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::anyhow;
use dusk_core::MediaTime;
use dusk_engine::{DECODER_IDLE, Gpu, Preview, PreviewEvent};

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

    // M1 opens one clip from the command line (docs/ROADMAP.md); the media bin comes in M2.
    let preview = match std::env::args_os().nth(1) {
        Some(path) => Some(open_preview(gpu, &window, PathBuf::from(path))?),
        None => {
            window.set_preview_message(
                "Open a clip by starting Dusk with it: dusk.exe clip.mp4".into(),
            );
            None
        }
    };
    let preview = Rc::new(preview);
    let show_first_frame = {
        let preview = Rc::clone(&preview);
        move |width: i32, height: i32| {
            let size = (u32::try_from(width), u32::try_from(height));
            if let (Some(preview), (Ok(width @ 1..), Ok(height @ 1..))) = (preview.as_ref(), size) {
                preview.show(MediaTime(0), (width, height));
            }
        }
    };
    window.on_preview_resized(show_first_frame.clone());
    window.show()?;
    // The preview has its size now that the window is shown.
    show_first_frame(
        window.get_preview_pixel_width(),
        window.get_preview_pixel_height(),
    );
    slint::run_event_loop()?;
    window.hide()?;
    Ok(())
}

/// Starts the preview worker for `path`; its frames and errors reach `window` on the UI
/// thread.
fn open_preview(gpu: &Gpu, window: &MainWindow, path: PathBuf) -> anyhow::Result<Preview> {
    let window = window.as_weak();
    let preview = Preview::open(gpu, path, DECODER_IDLE, move |event| {
        let window = window.clone();
        // This runs on the worker; the UI is only touched on its own thread.
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window.upgrade() {
                show_preview_event(&window, event);
            }
        });
    })?;
    Ok(preview)
}

fn show_preview_event(window: &MainWindow, event: PreviewEvent) {
    match event {
        PreviewEvent::Frame { texture, .. } => match slint::Image::try_from(texture) {
            Ok(image) => {
                window.set_preview_image(image);
                window.set_preview_message("".into());
            }
            Err(error) => window.set_preview_message(error.to_string().into()),
        },
        PreviewEvent::Nothing { .. } => window.set_preview_image(slint::Image::default()),
        PreviewEvent::Error(error) => {
            window.set_preview_image(slint::Image::default());
            window.set_preview_message(error.to_string().into());
        }
    }
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
