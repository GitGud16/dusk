//! The wgpu device that Dusk and Slint render with.

/// The wgpu objects Dusk renders with. Slint renders with the same device and queue, so a
/// texture the compositor produces can be shown in the preview as it is.
#[derive(Clone, Debug)]
pub struct Gpu {
    /// The instance the adapter came from; Slint creates the window surfaces with it.
    pub instance: wgpu::Instance,
    /// The graphics adapter in use.
    pub adapter: wgpu::Adapter,
    /// The device that every GPU resource of Dusk and Slint lives on.
    pub device: wgpu::Device,
    /// The queue of that device.
    pub queue: wgpu::Queue,
}

/// Why no GPU device could be created.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// No backend produced a usable adapter and device.
    #[error(
        "no graphics adapter could be used ({}). Dusk needs a graphics driver with Vulkan or \
         Direct3D 12; update the graphics driver and try again",
        .attempts.join("; ")
    )]
    NoDevice {
        /// One entry per backend tried, saying what went wrong.
        attempts: Vec<String>,
    },
}

impl Gpu {
    /// Creates the device. On Windows it tries Vulkan first, then Direct3D 12; the
    /// `WGPU_BACKEND` environment variable overrides the order. A software adapter, such as
    /// WARP, is used only when no backend offers a hardware one.
    pub fn new() -> Result<Gpu, GpuError> {
        let order = backend_order(wgpu::Backends::from_env());
        first_hardware(&order, try_backend).map_err(|attempts| GpuError::NoDevice { attempts })
    }
}

/// What trying one backend produced.
enum Attempt<T> {
    /// A hardware adapter and its device.
    Hardware(T),
    /// A software adapter (WARP, a CPU Vulkan driver) and its device.
    Software(T),
    /// Nothing usable, and why.
    Failed(String),
}

/// The backends to try, in order.
fn backend_order(env_override: Option<wgpu::Backends>) -> Vec<wgpu::Backends> {
    match env_override {
        Some(backends) => vec![backends],
        // Vulkan alone reaches the first window far sooner than DX12, or than both together
        // (measured at M0; see docs/DECISIONS.md, "Slint specifics").
        None if cfg!(windows) => vec![wgpu::Backends::VULKAN, wgpu::Backends::DX12],
        None => vec![wgpu::Backends::VULKAN],
    }
}

/// Tries the backends in `order` and returns the first hardware result, falling back to the
/// first software one. If nothing works, returns one message per backend.
fn first_hardware<T>(
    order: &[wgpu::Backends],
    mut attempt: impl FnMut(wgpu::Backends) -> Attempt<T>,
) -> Result<T, Vec<String>> {
    let mut software = None;
    let mut failures = Vec::new();
    for &backends in order {
        match attempt(backends) {
            Attempt::Hardware(found) => return Ok(found),
            Attempt::Software(found) => {
                software.get_or_insert(found);
            }
            Attempt::Failed(why) => failures.push(format!("{}: {why}", backend_name(backends))),
        }
    }
    software.ok_or(failures)
}

/// Creates an instance limited to `backends`, then an adapter and a device on it.
fn try_backend(backends: wgpu::Backends) -> Attempt<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });
    let adapter = match ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::from_env().unwrap_or_default(),
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    })) {
        Some(Ok(adapter)) => adapter,
        Some(Err(e)) => return Attempt::Failed(e.to_string()),
        None => return Attempt::Failed("the adapter request did not complete".to_owned()),
    };
    let info = adapter.get_info();
    let (device, queue) = match ready(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("dusk"),
        // The WebGPU baseline, which every D3D12 GPU meets, and no optional features, so
        // device creation cannot fail on a GPU Dusk supports (docs/ARCHITECTURE.md).
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    })) {
        Some(Ok(pair)) => pair,
        Some(Err(e)) => return Attempt::Failed(format!("{}: {e}", info.name)),
        None => return Attempt::Failed("the device request did not complete".to_owned()),
    };
    // wgpu panics on errors nobody captures. One is expected, because Slint and the
    // compositor share this device: Slint reconfigures a window's surface (on a resize, or
    // when it went stale) after waiting for the queue to empty, and if the compositor on the
    // video thread submits meanwhile, wgpu reports that the GPU did not come idle. The surface
    // stays as it was and Slint configures it again on the next frame, so that one is logged;
    // anything else is a bug and still panics.
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        if raced_surface_configure(&error) {
            eprintln!("Dusk: {error}");
        } else {
            panic!("wgpu error: {error}");
        }
    }));
    let gpu = Gpu {
        instance,
        adapter,
        device,
        queue,
    };
    if info.device_type == wgpu::DeviceType::Cpu {
        Attempt::Software(gpu)
    } else {
        Attempt::Hardware(gpu)
    }
}

/// Whether `error` is a surface reconfigure that another thread's submission raced, which
/// wgpu reports as the GPU not coming idle (wgpu-core's `ConfigureSurfaceError::GpuWaitTimeout`,
/// recognized by its message, since wgpu does not pass its type on).
fn raced_surface_configure(error: &wgpu::Error) -> bool {
    let text = error.to_string();
    text.contains("Surface::configure") && text.contains("Failed to wait for GPU to come idle")
}

/// Polls a future once. wgpu's native backends answer adapter and device requests at once,
/// so there is never anything to wait for.
fn ready<F: Future>(future: F) -> Option<F::Output> {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match std::pin::pin!(future).poll(&mut context) {
        std::task::Poll::Ready(output) => Some(output),
        std::task::Poll::Pending => None,
    }
}

fn backend_name(backends: wgpu::Backends) -> String {
    if backends == wgpu::Backends::VULKAN {
        "Vulkan".to_owned()
    } else if backends == wgpu::Backends::DX12 {
        "Direct3D 12".to_owned()
    } else {
        format!("{backends:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_reconfigure_raced_by_the_compositor_is_not_fatal() {
        let error = |description: &str| wgpu::Error::Validation {
            source: Box::new(std::fmt::Error),
            description: description.to_owned(),
        };
        let raced = error(
            "Validation Error

Caused by:
  In Surface::configure
    Failed to wait for GPU to come idle before reconfiguring the Surface
",
        );
        assert!(raced_surface_configure(&raced));
        let other = error(
            "Validation Error

Caused by:
  In Queue::submit
    Buffer is destroyed
",
        );
        assert!(!raced_surface_configure(&other));
    }
    use wgpu::Backends;

    #[test]
    #[cfg(windows)]
    fn tries_vulkan_then_direct3d_12_on_windows() {
        assert_eq!(backend_order(None), [Backends::VULKAN, Backends::DX12]);
    }

    #[test]
    fn an_override_is_the_only_backend_tried() {
        assert_eq!(backend_order(Some(Backends::DX12)), [Backends::DX12]);
    }

    #[test]
    fn stops_at_the_first_backend_with_hardware() {
        let mut tried = Vec::new();
        let found = first_hardware(&[Backends::VULKAN, Backends::DX12], |backend| {
            tried.push(backend);
            Attempt::Hardware(backend)
        });
        assert_eq!(found, Ok(Backends::VULKAN));
        assert_eq!(tried, [Backends::VULKAN]);
    }

    #[test]
    fn prefers_hardware_on_a_later_backend_to_software_on_an_earlier_one() {
        let found = first_hardware(&[Backends::VULKAN, Backends::DX12], |backend| {
            if backend == Backends::VULKAN {
                Attempt::Software(backend)
            } else {
                Attempt::Hardware(backend)
            }
        });
        assert_eq!(found, Ok(Backends::DX12));
    }

    #[test]
    fn uses_software_when_no_backend_has_hardware() {
        let found = first_hardware(&[Backends::VULKAN, Backends::DX12], |backend| {
            if backend == Backends::VULKAN {
                Attempt::Failed("no adapter".into())
            } else {
                Attempt::Software(backend)
            }
        });
        assert_eq!(found, Ok(Backends::DX12));
    }

    #[test]
    fn reports_every_backend_when_none_works() {
        let found: Result<Backends, _> =
            first_hardware(&[Backends::VULKAN, Backends::DX12], |_| {
                Attempt::Failed("no adapter".into())
            });
        assert_eq!(
            found,
            Err(vec![
                "Vulkan: no adapter".to_owned(),
                "Direct3D 12: no adapter".to_owned()
            ])
        );
    }
}
