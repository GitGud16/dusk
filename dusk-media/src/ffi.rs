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

/// A stream's display matrix: how its frames are turned and mirrored for display, when the
/// file says (FFmpeg keeps it with the codec parameters since 6.1).
pub(crate) fn display_matrix(parameters: &Parameters) -> Option<[i32; 9]> {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_packet_side_data_get};
    // SAFETY: as in `codec_fields`, the parameters are alive and initialized for the borrow.
    // FFmpeg keeps `coded_side_data` and `nb_coded_side_data` consistent, and
    // av_packet_side_data_get only reads them. A display matrix is nine i32 values: the
    // entry's size is checked first, and it is read unaligned because its bytes come from a
    // byte buffer. Nothing is written.
    unsafe {
        let raw = &*parameters.as_ptr();
        let entry = av_packet_side_data_get(
            raw.coded_side_data,
            raw.nb_coded_side_data,
            AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if entry.is_null()
            || (*entry).data.is_null()
            || (*entry).size < std::mem::size_of::<[i32; 9]>()
        {
            return None;
        }
        Some(std::ptr::read_unaligned((*entry).data.cast::<[i32; 9]>()))
    }
}

/// A picture stored as a grid of tiles (HEIC, AVIF), as FFmpeg's demuxer describes it.
pub(crate) struct TileGrid {
    /// Each tile: its stream's index in the file, and where its top-left corner goes on the
    /// canvas the tiles make up.
    pub tiles: Vec<(usize, i32, i32)>,
    /// The picture's window on that canvas: left, top, width and height.
    pub window: (i32, i32, u32, u32),
    /// How the picture is turned and mirrored for display, when the file says (`irot`,
    /// `imir`).
    pub display_matrix: Option<[i32; 9]>,
    /// The picture's color profile, when the grid has one (`colr`), as iPhones' do.
    pub icc_profile: Option<Vec<u8>>,
}

/// The first tile grid of the file `input` holds, if it holds one.
pub(crate) fn tile_grid(input: &ffmpeg_next::format::context::Input) -> Option<TileGrid> {
    use ffmpeg_next::ffi::{
        AVPacketSideDataType, AVStreamGroupParamsType, av_packet_side_data_get,
    };
    // SAFETY: the format context is alive for the borrow, and everything below is only read.
    // FFmpeg keeps `nb_stream_groups` groups behind `stream_groups`, each with `nb_streams`
    // streams. A group's type is read as the integer it is stored as, so no enum value is
    // made from memory unchecked; a tile grid group's params point to its grid, whose
    // `offsets` hold `nb_tiles` entries indexing the group's streams (each index checked
    // against `nb_streams`), and whose coded side data av_packet_side_data_get reads as for
    // streams in `display_matrix`.
    unsafe {
        let context = &*input.as_ptr();
        let groups = slice(context.stream_groups, context.nb_stream_groups as usize);
        for &group in groups {
            let Some(group) = group.as_ref() else {
                continue;
            };
            let kind = std::ptr::addr_of!(group.type_).cast::<i32>().read();
            if kind != AVStreamGroupParamsType::AV_STREAM_GROUP_PARAMS_TILE_GRID as i32 {
                continue;
            }
            let Some(grid) = group.params.tile_grid.as_ref() else {
                continue;
            };
            let streams = slice(group.streams, group.nb_streams as usize);
            let mut tiles = Vec::new();
            for offset in slice(grid.offsets, grid.nb_tiles as usize) {
                let stream = streams.get(offset.idx as usize)?.as_ref()?;
                let index = usize::try_from(stream.index).ok()?;
                tiles.push((index, offset.horizontal, offset.vertical));
            }
            let entry = av_packet_side_data_get(
                grid.coded_side_data,
                grid.nb_coded_side_data,
                AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
            );
            let display_matrix = entry
                .as_ref()
                .filter(|entry| {
                    !entry.data.is_null() && entry.size >= std::mem::size_of::<[i32; 9]>()
                })
                .map(|entry| std::ptr::read_unaligned(entry.data.cast::<[i32; 9]>()));
            let profile = av_packet_side_data_get(
                grid.coded_side_data,
                grid.nb_coded_side_data,
                AVPacketSideDataType::AV_PKT_DATA_ICC_PROFILE,
            );
            let icc_profile = profile
                .as_ref()
                .filter(|entry| !entry.data.is_null() && entry.size > 0)
                .map(|entry| std::slice::from_raw_parts(entry.data, entry.size).to_vec());
            let width = u32::try_from(grid.width).ok()?;
            let height = u32::try_from(grid.height).ok()?;
            return Some(TileGrid {
                tiles,
                window: (grid.horizontal_offset, grid.vertical_offset, width, height),
                display_matrix,
                icc_profile,
            });
        }
        None
    }
}

/// The `count` items at `items`, or none when it is null.
///
/// # Safety
///
/// When not null, `items` must point to `count` initialized items that outlive `'a`.
unsafe fn slice<'a, T>(items: *const T, count: usize) -> &'a [T] {
    if items.is_null() || count == 0 {
        &[]
    } else {
        // SAFETY: the caller's promise.
        unsafe { std::slice::from_raw_parts(items, count) }
    }
}

/// Whether an ICC profile describes RGB, the only kind FFmpeg's ICC support takes (it refuses
/// gray and CMYK profiles): its header's color space field, bytes 16 to 20.
pub(crate) fn is_rgb_profile(profile: &[u8]) -> bool {
    profile.get(16..20) == Some(b"RGB ".as_slice())
}

/// Hands `profile` to the decoder with `packet`, as FFmpeg's demuxers hand a picture's ICC
/// profile on, so that a decoder with ICC support on tags the frame with its colors.
pub(crate) fn attach_icc_profile(packet: &mut ffmpeg_next::Packet, profile: &[u8]) {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_packet_new_side_data};
    use ffmpeg_next::packet::Mut;
    // SAFETY: the packet is alive and ours for the borrow. av_packet_new_side_data allocates
    // `profile.len()` bytes owned by the packet, or returns null; exactly that many bytes are
    // copied into them.
    unsafe {
        let data = av_packet_new_side_data(
            packet.as_mut_ptr(),
            AVPacketSideDataType::AV_PKT_DATA_ICC_PROFILE,
            profile.len(),
        );
        if !data.is_null() {
            std::ptr::copy_nonoverlapping(profile.as_ptr(), data, profile.len());
        }
    }
}

/// Bytes per sample of frames of `format`: 1 up to 8 bits, 2 above.
pub(crate) fn sample_bytes(format: ffmpeg_next::format::Pixel) -> usize {
    // SAFETY: av_pix_fmt_desc_get returns a static descriptor, or null for an unknown format.
    unsafe {
        ffmpeg_next::ffi::av_pix_fmt_desc_get(format.into())
            .as_ref()
            .map_or(1, |desc| if desc.comp[0].depth > 8 { 2 } else { 1 })
    }
}

/// Gives `to` the chroma siting of `from`, which ffmpeg-next has no setter for.
pub(crate) fn copy_chroma_location(
    from: &ffmpeg_next::frame::Video,
    to: &mut ffmpeg_next::frame::Video,
) {
    // SAFETY: both AVFrames are alive for the borrows; one field is copied from a frame
    // FFmpeg filled in to another, the same enum type with a value FFmpeg wrote.
    unsafe {
        (*to.as_mut_ptr()).chroma_location = (*from.as_ptr()).chroma_location;
    }
}

/// A decoded frame's display matrix: how to turn and mirror it for display, as decoders of
/// still images report a photo's EXIF orientation.
pub(crate) fn frame_display_matrix(frame: &ffmpeg_next::frame::Video) -> Option<[i32; 9]> {
    use ffmpeg_next::ffi::{AVFrameSideDataType, av_frame_get_side_data};
    // SAFETY: `as_ptr` is the AVFrame `frame` owns, alive for the borrow. av_frame_get_side_data
    // only reads the frame's side data list. The entry's size is checked before its nine i32
    // values are read, unaligned because they sit in a byte buffer. Nothing is written.
    unsafe {
        let entry = av_frame_get_side_data(
            frame.as_ptr(),
            AVFrameSideDataType::AV_FRAME_DATA_DISPLAYMATRIX,
        );
        if entry.is_null()
            || (*entry).data.is_null()
            || (*entry).size < std::mem::size_of::<[i32; 9]>()
        {
            return None;
        }
        Some(std::ptr::read_unaligned((*entry).data.cast::<[i32; 9]>()))
    }
}

/// FFmpeg's `AVContentLightMetadata` (libavutil/mastering_display_metadata.h), which
/// ffmpeg-sys-next does not bind; a public, stable layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct ContentLight {
    max_cll: u32,
    max_fall: u32,
}

/// FFmpeg's `AVRational`, as laid out in the structs below.
#[repr(C)]
#[derive(Clone, Copy)]
struct Fraction {
    num: i32,
    den: i32,
}

/// FFmpeg's `AVMasteringDisplayMetadata`, likewise unbound.
#[repr(C)]
#[derive(Clone, Copy)]
struct MasteringDisplay {
    display_primaries: [[Fraction; 2]; 3],
    white_point: [Fraction; 2],
    min_luminance: Fraction,
    max_luminance: Fraction,
    has_primaries: i32,
    has_luminance: i32,
}

/// An HDR frame's light levels in nits: MaxCLL from its content light level metadata, and
/// its mastering display's maximum luminance, where it carries them.
pub(crate) fn light_levels(frame: &ffmpeg_next::frame::Video) -> (Option<f64>, Option<f64>) {
    use ffmpeg_next::ffi::{AVFrameSideDataType, av_frame_get_side_data};
    /// The side data of `kind`, if the frame has at least `size` bytes of it.
    ///
    /// # Safety
    ///
    /// `frame` must be a valid frame.
    unsafe fn data(
        frame: *const ffmpeg_next::ffi::AVFrame,
        kind: AVFrameSideDataType,
        size: usize,
    ) -> Option<*const u8> {
        // SAFETY: the caller's promise; the entry is only read.
        unsafe {
            let entry = av_frame_get_side_data(frame, kind);
            (!entry.is_null() && !(*entry).data.is_null() && (*entry).size >= size)
                .then(|| (*entry).data.cast_const())
        }
    }
    // SAFETY: the frame is alive for the borrow. Each entry is checked to hold the struct
    // read from it, which is read unaligned because it sits in a byte buffer.
    unsafe {
        let frame = frame.as_ptr();
        let max_cll = data(
            frame,
            AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
            std::mem::size_of::<ContentLight>(),
        )
        .map(|data| f64::from(std::ptr::read_unaligned(data.cast::<ContentLight>()).max_cll));
        let mastering = data(
            frame,
            AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
            std::mem::size_of::<MasteringDisplay>(),
        )
        .and_then(|data| {
            let display = std::ptr::read_unaligned(data.cast::<MasteringDisplay>());
            let max = display.max_luminance;
            (display.has_luminance != 0 && max.den != 0)
                .then(|| f64::from(max.num) / f64::from(max.den))
        });
        (max_cll, mastering)
    }
}

/// Whether a decoded frame carries an ICC profile for RGB, the only kind FFmpeg's ICC
/// support takes (it refuses gray and CMYK profiles).
pub(crate) fn has_rgb_icc_profile(frame: &ffmpeg_next::frame::Video) -> bool {
    use ffmpeg_next::ffi::{AVFrameSideDataType, av_frame_get_side_data};
    // SAFETY: as in `frame_display_matrix`; the entry's data holds `size` bytes, borrowed
    // as a slice for the frame's lifetime and only read.
    unsafe {
        let entry = av_frame_get_side_data(
            frame.as_ptr(),
            AVFrameSideDataType::AV_FRAME_DATA_ICC_PROFILE,
        );
        if entry.is_null() || (*entry).data.is_null() {
            return false;
        }
        is_rgb_profile(std::slice::from_raw_parts((*entry).data, (*entry).size))
    }
}

/// Bits per sample of the pictures a video stream decodes to, from the pixel format its codec
/// parameters name; 8 when they name none.
pub(crate) fn bit_depth(parameters: &Parameters) -> u8 {
    use ffmpeg_next::ffi::{av_pix_fmt_desc_get_id, av_pix_fmt_desc_next};
    // SAFETY: as in `codec_fields`, one integer field of the parameters is read. The pixel
    // format descriptors are FFmpeg's static table, walked with av_pix_fmt_desc_next until it
    // returns null; comparing each one's id with the stored integer avoids making an enum
    // value out of an integer nobody checked.
    unsafe {
        let format = (*parameters.as_ptr()).format;
        let mut descriptor = av_pix_fmt_desc_next(std::ptr::null());
        while let Some(found) = descriptor.as_ref() {
            if av_pix_fmt_desc_get_id(descriptor) as i32 == format {
                return u8::try_from(found.comp[0].depth).unwrap_or(8);
            }
            descriptor = av_pix_fmt_desc_next(descriptor);
        }
        8
    }
}

/// How many bytes a frame of `format` at `width` by `height` takes, rows unpadded.
pub(crate) fn frame_bytes(format: ffmpeg_next::format::Pixel, width: u32, height: u32) -> u64 {
    let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) else {
        return u64::MAX;
    };
    // SAFETY: a pure function of its arguments; it reads nothing behind pointers.
    let bytes =
        unsafe { ffmpeg_next::ffi::av_image_get_buffer_size(format.into(), width, height, 1) };
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

/// The profile FFmpeg found decoding with `decoder`, such as a progressive JPEG's.
pub(crate) fn decoder_profile(decoder: &ffmpeg_next::decoder::Video) -> i32 {
    // SAFETY: the decoder's AVCodecContext is alive for the borrow; one integer is read.
    unsafe { (*decoder.as_ptr()).profile }
}

/// Whether frames of `format` hold RGB rather than YUV or gray.
pub(crate) fn is_rgb(format: ffmpeg_next::format::Pixel) -> bool {
    pixel_flags(format) & ffmpeg_next::ffi::AV_PIX_FMT_FLAG_RGB as u64 != 0
}

/// Whether frames of `format` have chroma at a lower resolution than luma.
pub(crate) fn is_subsampled(format: ffmpeg_next::format::Pixel) -> bool {
    // SAFETY: av_pix_fmt_desc_get returns a static descriptor, or null for an unknown format.
    unsafe {
        ffmpeg_next::ffi::av_pix_fmt_desc_get(format.into())
            .as_ref()
            .is_some_and(|desc| desc.log2_chroma_w > 0 || desc.log2_chroma_h > 0)
    }
}

fn pixel_flags(format: ffmpeg_next::format::Pixel) -> u64 {
    // SAFETY: av_pix_fmt_desc_get returns a static descriptor, or null for an unknown format.
    unsafe {
        ffmpeg_next::ffi::av_pix_fmt_desc_get(format.into())
            .as_ref()
            .map_or(0, |desc| desc.flags)
    }
}

/// How swscale reads a frame's colors and writes the picture's.
pub(crate) struct ScaleColors {
    /// The source's YUV matrix (an `SWS_CS_` value) and whether it is full range; ignored
    /// for RGB sources.
    pub source_matrix: i32,
    pub source_full_range: bool,
    /// Where the source's subsampled chroma sits, in 256ths of a luma pixel: (across, down).
    /// `None` for sources without subsampled chroma.
    pub source_chroma: Option<(i32, i32)>,
    /// The picture's YUV matrix and range.
    pub matrix: i32,
    pub full_range: bool,
}

/// Scales `frame` to an NV12 picture of `size`, returned as its luma plane and its plane of
/// interleaved U and V, rows packed. swscale is set up the legacy way, the only one where
/// every override takes effect in FFmpeg 8.1 (docs/ARCHITECTURE.md, "FFmpeg (Windows)"):
/// Catmull-Rom (bicubic with B = 0 and C = 0.5) and full chroma interpolation, the given
/// matrices and ranges, the source's chroma siting, and the picture's chroma sited left as
/// the compositor reads it.
pub(crate) fn scale_to_nv12(
    frame: &ffmpeg_next::frame::Video,
    size: (u32, u32),
    colors: &ScaleColors,
) -> Result<(Vec<u8>, Vec<u8>), ffmpeg_next::Error> {
    use ffmpeg_next::ffi::{
        AVPixelFormat, SwsFlags, av_opt_set_double, av_opt_set_int, sws_alloc_context,
        sws_freeContext, sws_getCoefficients, sws_init_context, sws_scale,
        sws_setColorspaceDetails,
    };
    use std::ffi::{CStr, c_void};

    /// Frees the context however scaling ends.
    struct Context(*mut ffmpeg_next::ffi::SwsContext);
    impl Drop for Context {
        fn drop(&mut self) {
            // SAFETY: the context was allocated by sws_alloc_context and is freed once.
            unsafe { sws_freeContext(self.0) };
        }
    }

    let failed = |code: i32| ffmpeg_next::Error::from(code);
    let side = |value: u32| i32::try_from(value).map_err(|_| failed(-22));
    let (width, height) = (side(size.0)?, side(size.1)?);
    let (source_width, source_height) = (side(frame.width())?, side(frame.height())?);
    if width == 0 || height == 0 || source_width == 0 || source_height == 0 {
        return Err(failed(-22));
    }
    let chroma_width = size.0.div_ceil(2) as usize;
    let chroma_height = size.1.div_ceil(2) as usize;
    let mut luma = vec![0u8; size.0 as usize * size.1 as usize];
    let mut chroma = vec![0u8; 2 * chroma_width * chroma_height];
    let flags = SwsFlags::SWS_BICUBIC as i64
        | SwsFlags::SWS_FULL_CHR_H_INT as i64
        | SwsFlags::SWS_FULL_CHR_H_INP as i64
        | SwsFlags::SWS_ACCURATE_RND as i64;
    let source_format: AVPixelFormat = frame.format().into();
    let mut integers: Vec<(&CStr, i64)> = vec![
        (c"srcw", i64::from(source_width)),
        (c"srch", i64::from(source_height)),
        (c"src_format", source_format as i64),
        (c"dstw", i64::from(width)),
        (c"dsth", i64::from(height)),
        (c"dst_format", AVPixelFormat::AV_PIX_FMT_NV12 as i64),
        (c"sws_flags", flags),
        (c"src_range", i64::from(colors.source_full_range)),
        (c"dst_range", i64::from(colors.full_range)),
        // Left-sited 4:2:0: on the even columns, between two rows.
        (c"dst_h_chr_pos", 0),
        (c"dst_v_chr_pos", 128),
    ];
    if let Some((across, down)) = colors.source_chroma {
        integers.push((c"src_h_chr_pos", i64::from(across)));
        integers.push((c"src_v_chr_pos", i64::from(down)));
    }
    // SAFETY: the context is allocated here, checked, and freed by `Context` on every path.
    // Options are set by name before sws_init_context, as the legacy sequence requires. The
    // source pointers and strides are the frame's own planes, alive for the borrow and as
    // tall as `source_height`; the destination planes are the vectors above, sized for
    // NV12 at `size` with the strides passed.
    unsafe {
        let context = Context(sws_alloc_context());
        if context.0.is_null() {
            return Err(failed(-12));
        }
        let object = context.0.cast::<c_void>();
        for (name, value) in integers {
            let result = av_opt_set_int(object, name.as_ptr(), value, 0);
            if result < 0 {
                return Err(failed(result));
            }
        }
        for (name, value) in [(c"param0", 0.0), (c"param1", 0.5)] {
            let result = av_opt_set_double(object, name.as_ptr(), value, 0);
            if result < 0 {
                return Err(failed(result));
            }
        }
        let result = sws_init_context(context.0, std::ptr::null_mut(), std::ptr::null_mut());
        if result < 0 {
            return Err(failed(result));
        }
        let result = sws_setColorspaceDetails(
            context.0,
            sws_getCoefficients(colors.source_matrix),
            i32::from(colors.source_full_range),
            sws_getCoefficients(colors.matrix),
            i32::from(colors.full_range),
            0,
            1 << 16,
            1 << 16,
        );
        if result < 0 {
            return Err(failed(result));
        }
        let raw = &*frame.as_ptr();
        let source: [*const u8; 4] = raw.data[..4]
            .iter()
            .map(|plane| plane.cast_const())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or([std::ptr::null(); 4]);
        let source_strides: [i32; 4] = raw.linesize[..4].try_into().unwrap_or([0; 4]);
        let destination = [
            luma.as_mut_ptr(),
            chroma.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let destination_strides = [width, side(2 * chroma_width as u32)?, 0, 0];
        let rows = sws_scale(
            context.0,
            source.as_ptr(),
            source_strides.as_ptr(),
            0,
            source_height,
            destination.as_ptr(),
            destination_strides.as_ptr(),
        );
        if rows < 0 {
            return Err(failed(rows));
        }
    }
    Ok((luma, chroma))
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
