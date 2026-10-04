//! The only place in Dusk where `unsafe` FFmpeg access is allowed (CLAUDE.md). Each function
//! wraps one raw read or call that `ffmpeg-next` does not expose and states the invariant it
//! relies on.

use ffmpeg_next::codec::Parameters;

/// The size and audio format fields of a stream's codec parameters, which `ffmpeg-next`
/// does not expose without opening a decoder.
pub(crate) struct CodecFields {
    pub width: i32,
    pub height: i32,
    pub sample_rate: i32,
    pub channels: i32,
}

/// Reads [`CodecFields`] from `parameters` without opening a decoder.
pub(crate) fn codec_fields(parameters: &Parameters) -> CodecFields {
    // SAFETY: `as_ptr` returns the AVCodecParameters that `parameters` points to. It is
    // non-null and initialized for as long as `parameters` (and the stream it borrows from)
    // is alive, which the borrow guarantees; only plain integer fields are read, and nothing
    // is written.
    unsafe {
        let raw = &*parameters.as_ptr();
        CodecFields {
            width: raw.width,
            height: raw.height,
            sample_rate: raw.sample_rate,
            channels: raw.ch_layout.nb_channels,
        }
    }
}

/// A D3D11VA hardware device. The decoder that uses it holds its own reference, so this one
/// may be dropped once it is attached.
#[cfg(windows)]
pub(crate) struct HwDevice(*mut ffmpeg_next::ffi::AVBufferRef);

#[cfg(windows)]
impl HwDevice {
    /// Opens the default D3D11VA device, or `None` when the machine has no GPU video decoder.
    pub(crate) fn d3d11va() -> Option<HwDevice> {
        use ffmpeg_next::ffi::{AVHWDeviceType, av_hwdevice_ctx_create};
        let mut device = std::ptr::null_mut();
        // SAFETY: on success av_hwdevice_ctx_create stores a new reference in `device`, which
        // this struct then owns; on failure it leaves `device` null. A null device name and
        // null options select the default adapter.
        let result = unsafe {
            av_hwdevice_ctx_create(
                &mut device,
                AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        (result >= 0 && !device.is_null()).then_some(HwDevice(device))
    }
}

#[cfg(windows)]
impl Drop for HwDevice {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the reference this struct owns; av_buffer_unref releases it.
        unsafe { ffmpeg_next::ffi::av_buffer_unref(&mut self.0) };
    }
}

/// Whether `codec` can decode on a D3D11VA device.
#[cfg(windows)]
pub(crate) fn supports_d3d11va(codec: &ffmpeg_next::Codec) -> bool {
    use ffmpeg_next::ffi::{
        AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AVHWDeviceType, avcodec_get_hw_config,
    };
    (0..)
        .map_while(|index| {
            // SAFETY: avcodec_get_hw_config returns a pointer to a static config, or null
            // after the last one; `codec` points to a registered codec.
            unsafe { avcodec_get_hw_config(codec.as_ptr(), index).as_ref() }
        })
        .any(|config| {
            config.device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
                && config.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
        })
}

/// Makes the decoder that `context` will open decode on `device` whenever FFmpeg offers
/// D3D11 surfaces for the stream; otherwise it decodes in software.
#[cfg(windows)]
pub(crate) fn use_hw_device(context: &mut ffmpeg_next::codec::Context, device: &HwDevice) {
    // SAFETY: the context is not opened yet, so these fields may be set. av_buffer_ref adds a
    // reference that the context owns and frees with itself; `pick_d3d11` has the signature
    // FFmpeg expects for `get_format`.
    unsafe {
        let raw = context.as_mut_ptr();
        (*raw).hw_device_ctx = ffmpeg_next::ffi::av_buffer_ref(device.0);
        (*raw).get_format = Some(pick_d3d11);
    }
}

/// FFmpeg's pixel format negotiation: D3D11 surfaces when offered, else FFmpeg's own pick,
/// which is a software format.
#[cfg(windows)]
unsafe extern "C" fn pick_d3d11(
    context: *mut ffmpeg_next::ffi::AVCodecContext,
    formats: *const ffmpeg_next::ffi::AVPixelFormat,
) -> ffmpeg_next::ffi::AVPixelFormat {
    use ffmpeg_next::ffi::{AVPixelFormat, avcodec_default_get_format};
    // SAFETY: FFmpeg passes a list that ends with AV_PIX_FMT_NONE and stays valid for the
    // duration of the call.
    unsafe {
        let mut format = formats;
        while *format != AVPixelFormat::AV_PIX_FMT_NONE {
            if *format == AVPixelFormat::AV_PIX_FMT_D3D11 {
                return *format;
            }
            format = format.add(1);
        }
        avcodec_default_get_format(context, formats)
    }
}

/// Copies a hardware frame's picture into system memory, as NV12 or P010.
pub(crate) fn transfer_to_system(
    hardware: &ffmpeg_next::frame::Video,
) -> Result<ffmpeg_next::frame::Video, ffmpeg_next::Error> {
    let mut system = ffmpeg_next::frame::Video::empty();
    // SAFETY: both frames are valid AVFrames; av_hwframe_transfer_data allocates the
    // destination's buffers in the format the hardware frame maps to.
    let result = unsafe {
        ffmpeg_next::ffi::av_hwframe_transfer_data(system.as_mut_ptr(), hardware.as_ptr(), 0)
    };
    if result < 0 {
        Err(ffmpeg_next::Error::from(result))
    } else {
        Ok(system)
    }
}

/// The container's start time in microseconds, or 0 when the file does not state one.
pub(crate) fn start_time(input: &ffmpeg_next::format::context::Input) -> i64 {
    // SAFETY: reads one integer field of the open format context `input` points to.
    let start = unsafe { (*input.as_ptr()).start_time };
    // AV_NOPTS_VALUE, "no time", is i64::MIN.
    if start == i64::MIN { 0 } else { start }
}
