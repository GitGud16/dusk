//! Turning decoded pictures into RGBA frames on the GPU (docs/ARCHITECTURE.md, "Decoder pool",
//! color and scaling steps 1, 2 and 4): each plane is resampled to the output size with a
//! Catmull-Rom kernel, chroma at its sited position, then YUV becomes RGB with the picture's
//! matrix and range, fitted into the frame between black bars.

use std::sync::mpsc;

use dusk_core::{ColorMatrix, ColorRange, Picture, PictureLayout};
use wgpu::util::DeviceExt;

use crate::Gpu;

/// Why a picture could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The picture or the requested frame has no pixels, or is larger than the GPU allows.
    #[error(
        "a {width}x{height} frame cannot be drawn on this GPU (the limit is {limit} pixels a \
         side); pick a smaller size"
    )]
    Size {
        /// The width that was refused.
        width: u32,
        /// The height that was refused.
        height: u32,
        /// The largest side the device accepts.
        limit: u32,
    },
    /// The picture's planes hold fewer or more bytes than its size and layout need.
    #[error("the decoded picture is malformed; reopen the file and try again")]
    Malformed,
    /// The GPU could not hand a rendered frame back.
    #[error(
        "the GPU did not return the rendered frame ({0}); update the graphics driver and try again"
    )]
    Readback(String),
}

/// Draws decoded pictures into RGBA textures on the shared device.
pub struct Compositor {
    gpu: Gpu,
    resample_float_layout: wgpu::BindGroupLayout,
    resample_uint_layout: wgpu::BindGroupLayout,
    convert_layout: wgpu::BindGroupLayout,
    /// Resampling from float textures, into a luma and into a chroma intermediate.
    resample_float: [wgpu::RenderPipeline; 2],
    /// Resampling from 16-bit integer (P010) planes, into a luma and into a chroma intermediate.
    resample_uint: [wgpu::RenderPipeline; 2],
    convert: wgpu::RenderPipeline,
}

/// Indices into the pipeline pairs, and the intermediate format each one writes.
const LUMA: usize = 0;
const CHROMA: usize = 1;
const INTERMEDIATE: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::R16Float,
    wgpu::TextureFormat::Rg16Float,
];

impl Compositor {
    /// Builds the shaders and pipelines on `gpu`.
    pub fn new(gpu: &Gpu) -> Compositor {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // The shader reads every texture with textureLoad, so nothing needs filtering.
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let resample_float_layout = layout("resample float", &[uniform(0), texture(1, float)]);
        let resample_uint_layout = layout(
            "resample uint",
            &[uniform(0), texture(2, wgpu::TextureSampleType::Uint)],
        );
        let convert_layout = layout(
            "convert",
            &[uniform(3), texture(4, float), texture(5, float)],
        );

        let pipeline = |bind_group_layout: &wgpu::BindGroupLayout, entry, format| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(bind_group_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let resample_float =
            INTERMEDIATE.map(|format| pipeline(&resample_float_layout, "resample_float", format));
        let resample_uint =
            INTERMEDIATE.map(|format| pipeline(&resample_uint_layout, "resample_uint", format));
        let convert = pipeline(&convert_layout, "to_rgb", wgpu::TextureFormat::Rgba8Unorm);
        Compositor {
            gpu: gpu.clone(),
            resample_float_layout,
            resample_uint_layout,
            convert_layout,
            resample_float,
            resample_uint,
            convert,
        }
    }

    /// Draws `picture` fitted into a frame of `size` (width, height): aspect ratio kept,
    /// centered, black where it does not reach. The result is an `Rgba8Unorm` texture that
    /// Slint can show and [`Compositor::read_rgba`] can read back.
    pub fn render(
        &self,
        picture: &Picture,
        size: (u32, u32),
    ) -> Result<wgpu::Texture, RenderError> {
        let limit = self.gpu.device.limits().max_texture_dimension_2d;
        for (width, height) in [(picture.width, picture.height), size] {
            if width == 0 || height == 0 || width > limit || height > limit {
                return Err(RenderError::Size {
                    width,
                    height,
                    limit,
                });
            }
        }
        let (chroma_width, chroma_height) = picture.chroma_size();
        let bytes = picture.layout.bytes_per_sample();
        let (width, height) = (picture.width as usize, picture.height as usize);
        let chroma_samples = 2 * chroma_width as usize * chroma_height as usize;
        if picture.luma.len() != width * height * bytes
            || picture.chroma.len() != chroma_samples * bytes
        {
            return Err(RenderError::Malformed);
        }

        let rect = fit((picture.width, picture.height), size);
        let ten_bit = picture.layout == PictureLayout::P010;
        let (luma_format, chroma_format) = if ten_bit {
            (wgpu::TextureFormat::R16Uint, wgpu::TextureFormat::Rg16Uint)
        } else {
            (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
        };
        let luma = self.upload(&picture.luma, (picture.width, picture.height), luma_format);
        let chroma = self.upload(
            &picture.chroma,
            (chroma_width, chroma_height),
            chroma_format,
        );
        // Each plane is resampled across into an intermediate as tall as the plane; the final
        // pass resamples both down and converts to RGB.
        let intermediate =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let luma_across = self.texture(
            (rect.width, picture.height),
            INTERMEDIATE[LUMA],
            intermediate,
        );
        let chroma_across = self.texture(
            (rect.width, chroma_height),
            INTERMEDIATE[CHROMA],
            intermediate,
        );
        let frame = self.texture(
            size,
            wgpu::TextureFormat::Rgba8Unorm,
            intermediate | wgpu::TextureUsages::COPY_SRC,
        );

        // Source texels per output texel. 4:2:0 chroma is sited left (MPEG-2): horizontally
        // on the even luma columns, a quarter chroma texel right of where centered chroma
        // would be, and vertically halfway between two luma rows.
        let across = picture.width as f32 / rect.width as f32;
        let down = picture.height as f32 / rect.height as f32;
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let passes = [
            (
                &luma,
                &luma_across,
                LUMA,
                Pass::new(picture.width, across, 0.0),
            ),
            (
                &chroma,
                &chroma_across,
                CHROMA,
                Pass::new(chroma_width, across / 2.0, 0.25),
            ),
        ];
        for (source, target, plane, pass) in passes {
            let (pipeline, layout, binding) = if ten_bit {
                (&self.resample_uint[plane], &self.resample_uint_layout, 2)
            } else {
                (&self.resample_float[plane], &self.resample_float_layout, 1)
            };
            let uniforms = self.uniforms(&pass.bytes());
            let bind_group = self.bind_group(layout, &uniforms, 0, &[(binding, source)]);
            draw(&mut encoder, pipeline, &bind_group, target);
        }
        let bits = if ten_bit { 10 } else { 8 };
        let convert = Convert {
            rect,
            to_rgb: yuv_to_rgb(picture.matrix, picture.range, bits),
            luma_down: Pass::new(picture.height, down, 0.0),
            chroma_down: Pass::new(chroma_height, down / 2.0, 0.0),
        };
        let uniforms = self.uniforms(&convert.bytes());
        let bind_group = self.bind_group(
            &self.convert_layout,
            &uniforms,
            3,
            &[(4, &luma_across), (5, &chroma_across)],
        );
        draw(&mut encoder, &self.convert, &bind_group, &frame);
        self.gpu.queue.submit([encoder.finish()]);
        Ok(frame)
    }

    /// Reads an `Rgba8Unorm` texture back into memory, four bytes a pixel, rows packed.
    /// Blocks until the GPU is done; meant for export and tests, not for the UI thread.
    pub fn read_rgba(&self, texture: &wgpu::Texture) -> Result<Vec<u8>, RenderError> {
        let (width, height) = (texture.width(), texture.height());
        let row = width * 4;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        self.gpu.queue.submit([encoder.finish()]);

        let (sender, receiver) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        receiver
            .recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut rgba = Vec::with_capacity(row as usize * height as usize);
        {
            let mapped = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| RenderError::Readback(e.to_string()))?;
            for padded_row in mapped.chunks(padded as usize) {
                rgba.extend_from_slice(&padded_row[..row as usize]);
            }
        }
        buffer.unmap();
        Ok(rgba)
    }

    fn texture(
        &self,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    /// A texture holding one plane of a picture, rows packed.
    fn upload(&self, data: &[u8], size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
        let texture = self.texture(
            size,
            format,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let bytes_per_texel = format.block_copy_size(None).unwrap_or(1);
        self.gpu.queue.write_texture(
            texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * bytes_per_texel),
                rows_per_image: Some(size.1),
            },
            texture.size(),
        );
        texture
    }

    fn uniforms(&self, bytes: &[u8]) -> wgpu::Buffer {
        self.gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    fn bind_group(
        &self,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        uniform_binding: u32,
        textures: &[(u32, &wgpu::Texture)],
    ) -> wgpu::BindGroup {
        let views: Vec<_> = textures
            .iter()
            .map(|(binding, texture)| (*binding, texture.create_view(&Default::default())))
            .collect();
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: uniform_binding,
            resource: uniforms.as_entire_binding(),
        }];
        entries.extend(views.iter().map(|(binding, view)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: wgpu::BindingResource::TextureView(view),
        }));
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &entries,
            })
    }
}

/// Runs `pipeline` over the whole of `target`.
fn draw(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target: &wgpu::Texture,
) {
    let view = target.create_view(&Default::default());
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

/// One direction of resampling: how many texels the source has along it, how many source
/// texels one output texel covers, and the source offset (chroma siting).
#[derive(Clone, Copy)]
struct Pass {
    source_texels: u32,
    step: f32,
    offset: f32,
}

impl Pass {
    fn new(source_texels: u32, step: f32, offset: f32) -> Pass {
        Pass {
            source_texels,
            step,
            offset,
        }
    }

    /// Kernel scale: a downscale widens the kernel so it does not alias.
    fn scale(&self) -> f32 {
        self.step.max(1.0)
    }

    /// The shader's `Resample` uniform.
    fn bytes(&self) -> Vec<u8> {
        [
            &(self.source_texels as i32).to_le_bytes()[..],
            &self.step.to_le_bytes(),
            &self.offset.to_le_bytes(),
            &self.scale().to_le_bytes(),
        ]
        .concat()
    }
}

/// The final pass, as the shader's `Convert` uniform.
struct Convert {
    rect: Rect,
    to_rgb: [[f32; 4]; 3],
    luma_down: Pass,
    chroma_down: Pass,
}

impl Convert {
    fn bytes(&self) -> Vec<u8> {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.rect;
        let corners = [x, y, x + width, y + height].map(|v| (v as i32).to_le_bytes());
        let rows = self.to_rgb.iter().flatten().map(|v| v.to_le_bytes());
        let (luma, chroma) = (self.luma_down, self.chroma_down);
        let counts = [luma.source_texels, chroma.source_texels].map(|v| (v as i32).to_le_bytes());
        let down = [
            luma.step,
            luma.offset,
            luma.scale(),
            chroma.step,
            chroma.offset,
            chroma.scale(),
        ]
        .map(f32::to_le_bytes);
        corners
            .into_iter()
            .chain(rows)
            .chain(counts)
            .chain(down)
            .flatten()
            .collect()
    }
}

/// Where a picture lands in the frame, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Where a picture of size `picture` lands in a frame of size `frame`: aspect ratio kept,
/// centered, at least one pixel each way.
fn fit(picture: (u32, u32), frame: (u32, u32)) -> Rect {
    let (picture_width, picture_height) = (u64::from(picture.0), u64::from(picture.1));
    let (frame_width, frame_height) = (u64::from(frame.0), u64::from(frame.1));
    let (width, height) = if picture_width * frame_height >= frame_width * picture_height {
        let height = (picture_height * frame_width + picture_width / 2) / picture_width;
        (frame_width, height.clamp(1, frame_height))
    } else {
        let width = (picture_width * frame_height + picture_height / 2) / picture_height;
        (width.clamp(1, frame_width), frame_height)
    };
    // Every value is at most the frame's size, which is a u32.
    Rect {
        x: ((frame_width - width) / 2) as u32,
        y: ((frame_height - height) / 2) as u32,
        width: width as u32,
        height: height as u32,
    }
}

/// The rows that turn (Y, U, V, 1), in codes of a `bits`-deep picture, into R', G' and B'
/// from 0 to 1.
fn yuv_to_rgb(matrix: ColorMatrix, range: ColorRange, bits: u32) -> [[f32; 4]; 3] {
    let (kr, kb) = match matrix {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    let max = f64::from((1u32 << bits) - 1);
    let k = f64::from(1u32 << (bits - 8));
    // Y' = (Y - y_zero) / y_span; Cb and Cr = (U or V - c_zero) / c_span.
    let (y_zero, y_span, c_zero, c_span) = match range {
        ColorRange::Limited => (16.0 * k, 219.0 * k, 128.0 * k, 224.0 * k),
        ColorRange::Full => (0.0, max, f64::from(1u32 << (bits - 1)), max),
    };
    let rows: [[f64; 3]; 3] = [
        [1.0, 0.0, 2.0 * (1.0 - kr)],
        [
            1.0,
            -2.0 * kb * (1.0 - kb) / kg,
            -2.0 * kr * (1.0 - kr) / kg,
        ],
        [1.0, 2.0 * (1.0 - kb), 0.0],
    ];
    rows.map(|[y, cb, cr]| {
        let (y, cb, cr) = (y / y_span, cb / c_span, cr / c_span);
        let constant = -y * y_zero - (cb + cr) * c_zero;
        [y as f32, cb as f32, cr as f32, constant as f32]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_picture_with_the_frames_shape_fills_it() {
        assert_eq!(fit((1920, 1080), (960, 540)), rect(0, 0, 960, 540));
    }

    #[test]
    fn a_wider_picture_gets_bars_above_and_below() {
        assert_eq!(fit((64, 32), (64, 64)), rect(0, 16, 64, 32));
        assert_eq!(fit((2560, 1080), (1920, 1080)), rect(0, 135, 1920, 810));
    }

    #[test]
    fn a_taller_picture_gets_bars_left_and_right() {
        assert_eq!(fit((1080, 1920), (1920, 1080)), rect(656, 0, 608, 1080));
    }

    #[test]
    fn a_tiny_picture_still_covers_a_pixel() {
        assert_eq!(fit((10_000, 1), (100, 100)).height, 1);
    }

    fn apply(rows: [[f32; 4]; 3], yuv: [f32; 3]) -> [f32; 3] {
        rows.map(|[a, b, c, d]| a * yuv[0] + b * yuv[1] + c * yuv[2] + d)
    }

    fn near(actual: [f32; 3], expected: [f32; 3], tolerance: f32) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() <= tolerance)
    }

    #[test]
    fn limited_range_maps_16_and_235_to_black_and_white() {
        let rows = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        assert!(near(apply(rows, [16.0, 128.0, 128.0]), [0.0; 3], 1e-6));
        assert!(near(apply(rows, [235.0, 128.0, 128.0]), [1.0; 3], 1e-6));
    }

    #[test]
    fn the_textbook_reds_come_out_red() {
        let bt601 = yuv_to_rgb(ColorMatrix::Bt601, ColorRange::Limited, 8);
        let bt709 = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        assert!(near(
            apply(bt601, [81.0, 90.0, 240.0]),
            [1.0, 0.0, 0.0],
            0.01
        ));
        assert!(near(
            apply(bt709, [63.0, 102.0, 240.0]),
            [1.0, 0.0, 0.0],
            0.01
        ));
    }

    #[test]
    fn ten_bit_codes_are_four_times_eight_bit_ones() {
        let eight = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        let ten = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 10);
        let rgb = apply(eight, [63.0, 102.0, 240.0]);
        assert!(near(apply(ten, [252.0, 408.0, 960.0]), rgb, 1e-5));
    }

    #[test]
    fn full_range_spans_every_code() {
        let rows = yuv_to_rgb(ColorMatrix::Bt601, ColorRange::Full, 8);
        assert!(near(apply(rows, [0.0, 128.0, 128.0]), [0.0; 3], 1e-6));
        assert!(near(apply(rows, [255.0, 128.0, 128.0]), [1.0; 3], 1e-6));
    }
}
