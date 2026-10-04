//! `dusk`, the Dusk editor.

// Release builds are GUI-subsystem executables (no console window); debug builds keep the
// console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::anyhow;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    select_renderer()?;
    let window = MainWindow::new()?;
    window.run()?;
    Ok(())
}

/// Selects Slint's FemtoVG renderer on wgpu 30 with WebGPU baseline limits instead of
/// Slint's WebGL2-level default (docs/ARCHITECTURE.md, "Slint specifics").
fn select_renderer() -> anyhow::Result<()> {
    let mut settings = slint::wgpu_30::WGPUSettings::default();
    settings.device_required_limits = wgpu::Limits::default();
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(settings))
        .select()
        .map_err(|e| {
            anyhow!(
                "Dusk could not start its GPU renderer ({e}). It needs a graphics driver with \
                 Direct3D 12 or Vulkan; update the graphics driver and try again."
            )
        })
}
