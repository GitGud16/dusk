//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::anyhow;
use dusk_engine::Gpu;

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
    let _gpu: &'static Gpu = Box::leak(Box::new(select_renderer()?));
    let window = MainWindow::new()?;
    window.run()?;
    Ok(())
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
