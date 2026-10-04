//! Needs a graphics adapter. CI runners have no GPU, but Windows always has WARP, the
//! Direct3D 12 software rasterizer, which is the last resort `Gpu::new` falls back to.

use dusk_render::Gpu;

#[test]
fn creates_a_device_with_the_webgpu_baseline_limits() {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    // Slint's default settings would ask only for WebGL2-level limits (2048 here).
    assert_eq!(
        gpu.device.limits().max_texture_dimension_2d,
        wgpu::Limits::default().max_texture_dimension_2d
    );
}

#[test]
#[cfg(windows)]
fn uses_vulkan_or_direct3d_12_on_windows() {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let backend = gpu.adapter.get_info().backend;
    assert!(
        matches!(backend, wgpu::Backend::Vulkan | wgpu::Backend::Dx12),
        "unexpected backend {backend:?}"
    );
}
