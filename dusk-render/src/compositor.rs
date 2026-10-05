//! Turning decoded pictures into RGBA frames on the GPU (docs/ARCHITECTURE.md, "Decoder pool",
//! color and scaling steps 1, 2 and 4): each plane is resampled to the output size with a
//! Catmull-Rom kernel, chroma at its sited position, then YUV becomes RGB with the picture's
//! matrix and range, fitted into the frame between black bars.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use dusk_core::{ColorMatrix, ColorRange, Picture, PictureLayout};
use wgpu::util::DeviceExt;

use crate::Gpu;
use crate::placement::{FrameRect, Placement, mapping};

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
    /// The GPU did not take a decoded picture.
    #[error(
        "the GPU did not take the decoded picture ({0}); update the graphics driver and try again"
    )]
    Upload(String),
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
    pool: Pool,
}

/// Textures and upload buffers kept from one frame to the next, so that playing allocates no
/// GPU memory per frame. Graphics drivers keep memory that is freed and allocated again at
/// that pace: at M1 fresh textures and upload buffers for every frame left 25 to 45 MB more
/// after playback than before.
struct Pool {
    /// Two upload buffers used in turn: a frame's planes are written into one while the GPU
    /// may still be copying out of the other.
    uploads: RefCell<[Option<Upload>; 2]>,
    /// The upload buffer the next frame uses.
    next_upload: RefCell<usize>,
    /// Upload and intermediate textures not in use.
    spare: RefCell<Vec<wgpu::Texture>>,
    /// The frames handed out last, oldest first; each is drawn into again once
    /// [`FRAMES`] newer ones exist.
    frames: RefCell<VecDeque<wgpu::Texture>>,
}

/// Spare textures kept for the next frame: two uploads and two intermediates make a frame.
const SPARE: usize = 8;

/// A buffer the CPU writes a picture into for the GPU to copy from.
struct Upload {
    buffer: wgpu::Buffer,
    /// Set once the buffer is mapped again after the GPU has copied out of it.
    ready: Arc<AtomicBool>,
}
/// Frames in rotation. A frame handed out stays as it was until two more have been drawn;
/// the preview only ever shows the newest, and since the compositor and the UI share one
/// queue, a frame drawn again is never shown half-drawn.
const FRAMES: usize = 3;

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
            pool: Pool {
                uploads: RefCell::default(),
                next_upload: RefCell::default(),
                spare: RefCell::default(),
                frames: RefCell::default(),
            },
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
        self.render_placed(picture, &Placement::default(), size)
    }

    /// Draws `picture` in a frame of `size` as `placement` says: turned upright, cropped,
    /// turned and mirrored as its clip says, and fitted between bars or filling the frame.
    pub fn render_placed(
        &self,
        picture: &Picture,
        placement: &Placement,
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

        let map = mapping((picture.width, picture.height), placement, size);
        let ten_bit = picture.layout == PictureLayout::P010;
        let (luma_format, chroma_format) = if ten_bit {
            (wgpu::TextureFormat::R16Uint, wgpu::TextureFormat::Rg16Uint)
        } else {
            (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
        };
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let planes = [
            (&picture.luma, (picture.width, picture.height), luma_format),
            (
                &picture.chroma,
                (chroma_width, chroma_height),
                chroma_format,
            ),
        ];
        let ([luma, chroma], upload) = self.upload(&mut encoder, planes)?;
        // Each plane is resampled along the source axis that output x follows, into an
        // intermediate as long as the plane is along the other axis; the final pass
        // resamples both along that other axis and converts to RGB.
        let (luma_rows, chroma_rows) = if map.transposed {
            (picture.width, chroma_width)
        } else {
            (picture.height, chroma_height)
        };
        let intermediate =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let luma_across = self.spare(
            (map.rect.width, luma_rows),
            INTERMEDIATE[LUMA],
            intermediate,
        );
        let chroma_across = self.spare(
            (map.rect.width, chroma_rows),
            INTERMEDIATE[CHROMA],
            intermediate,
        );
        let frame = self.frame(size);

        // The shown part of the source along each pass's axis.
        let (x_min, x_max, y_min, y_max) = map.window;
        let (across_window, down_window) = if map.transposed {
            ((y_min, y_max), (x_min, x_max))
        } else {
            ((x_min, x_max), (y_min, y_max))
        };
        // 4:2:0 chroma has half as many texels each way. It is sited left (MPEG-2): along
        // the source's x a chroma texel sits on the even luma columns, a quarter of a chroma
        // texel right of centered; along its y it sits halfway between two luma rows.
        let half = |(min, max): (u32, u32)| (min / 2, max.div_ceil(2));
        let siting = |along_x: bool| if along_x { 0.25 } else { 0.0 };
        let passes = [
            (
                &luma,
                &luma_across,
                LUMA,
                Pass::new(across_window, map.x_step, map.x_origin, map.transposed),
            ),
            (
                &chroma,
                &chroma_across,
                CHROMA,
                Pass::new(
                    half(across_window),
                    map.x_step / 2.0,
                    map.x_origin / 2.0 + siting(!map.transposed),
                    map.transposed,
                ),
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
            rect: map.rect,
            to_rgb: yuv_to_rgb(picture.matrix, picture.range, bits),
            luma_down: Pass::new(down_window, map.y_step, map.y_origin, false),
            chroma_down: Pass::new(
                half(down_window),
                map.y_step / 2.0,
                map.y_origin / 2.0 + siting(map.transposed),
                false,
            ),
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
        self.remap(upload);
        // Copies into these for the next frame are ordered after this submission.
        self.give_back([luma, chroma, luma_across, chroma_across]);
        Ok(frame)
    }

    /// A black frame of `size`, for where no clip is visible.
    pub fn blank(&self, size: (u32, u32)) -> Result<wgpu::Texture, RenderError> {
        let limit = self.gpu.device.limits().max_texture_dimension_2d;
        let (width, height) = size;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(RenderError::Size {
                width,
                height,
                limit,
            });
        }
        let frame = self.frame(size);
        // A pass that only clears: its load operation paints the frame black.
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let view = frame.create_view(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

    /// A spare texture of this kind, or a new one.
    fn spare(
        &self,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        let mut spare = self.pool.spare.borrow_mut();
        let found = spare.iter().position(|texture| {
            (texture.width(), texture.height()) == size
                && texture.format() == format
                && texture.usage() == usage
        });
        match found {
            Some(index) => spare.swap_remove(index),
            None => self.texture(size, format, usage),
        }
    }

    /// Keeps `textures` for the next frame; the oldest spares go when there are too many.
    fn give_back(&self, textures: [wgpu::Texture; 4]) {
        let mut spare = self.pool.spare.borrow_mut();
        spare.extend(textures);
        let excess = spare.len().saturating_sub(SPARE);
        spare.drain(..excess);
    }

    /// The texture the next frame of `size` is drawn into: the oldest of the frames in
    /// rotation once there are [`FRAMES`] of that size.
    fn frame(&self, size: (u32, u32)) -> wgpu::Texture {
        let mut frames = self.pool.frames.borrow_mut();
        frames.retain(|frame| (frame.width(), frame.height()) == size);
        let frame = if frames.len() >= FRAMES {
            frames.pop_front()
        } else {
            None
        };
        let frame = frame.unwrap_or_else(|| {
            self.texture(
                size,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
            )
        });
        frames.push_back(frame.clone());
        frame
    }

    /// Textures holding the planes of a picture, each given as its packed rows, its size and
    /// its texture format, copied in through the next upload buffer as part of `encoder`.
    /// Returns which upload buffer was used, for [`remap`](Self::remap) after submitting.
    fn upload(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        planes: [(&Vec<u8>, (u32, u32), wgpu::TextureFormat); 2],
    ) -> Result<([wgpu::Texture; 2], usize), RenderError> {
        // Where each plane goes in the buffer: rows padded to the copy alignment, planes one
        // after the other (a padded plane's length keeps the next one aligned too).
        let layouts = planes.map(|(_, (width, height), format)| {
            let row = width * format.block_copy_size(None).unwrap_or(1);
            let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            (row as usize, padded, u64::from(padded) * u64::from(height))
        });
        let total = layouts[0].2 + layouts[1].2;
        let (index, buffer) = self.upload_buffer(total)?;
        {
            let mut mapped = buffer
                .slice(..total)
                .get_mapped_range_mut()
                .map_err(|e| RenderError::Upload(e.to_string()))?;
            let mut offset = 0;
            for ((data, _, _), (row, padded, length)) in planes.iter().zip(layouts) {
                for (index, source) in data.chunks(row).enumerate() {
                    let at = offset + index * padded as usize;
                    mapped.slice(at..at + row).copy_from_slice(source);
                }
                offset += length as usize;
            }
        }
        buffer.unmap();
        let mut offset = 0;
        let textures = [0, 1].map(|plane| {
            let (_, size, format) = planes[plane];
            let (_, padded, length) = layouts[plane];
            let texture = self.spare(
                size,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(size.1),
                    },
                },
                texture.as_image_copy(),
                texture.size(),
            );
            offset += length;
            texture
        });
        Ok((textures, index))
    }

    /// The next upload buffer, mapped for writing and at least `size` bytes long. It waits
    /// for the GPU to finish copying out of it if it has not yet; a larger picture than
    /// before gets a larger buffer.
    fn upload_buffer(&self, size: u64) -> Result<(usize, wgpu::Buffer), RenderError> {
        let index = {
            let mut next = self.pool.next_upload.borrow_mut();
            let index = *next;
            *next = (index + 1) % 2;
            index
        };
        let mut uploads = self.pool.uploads.borrow_mut();
        if let Some(upload) = &uploads[index]
            && upload.buffer.size() >= size
        {
            // Mapping callbacks run when the device is polled.
            if !upload.ready.load(Ordering::Acquire) {
                self.gpu
                    .device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .map_err(|e| RenderError::Upload(e.to_string()))?;
            }
            if upload.ready.load(Ordering::Acquire) {
                return Ok((index, upload.buffer.clone()));
            }
        }
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upload"),
            size,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        uploads[index] = Some(Upload {
            buffer: buffer.clone(),
            ready: Arc::new(AtomicBool::new(true)),
        });
        Ok((index, buffer))
    }

    /// Maps upload buffer `index` for writing again, once the GPU has copied out of it.
    fn remap(&self, index: usize) {
        let uploads = self.pool.uploads.borrow();
        let Some(upload) = &uploads[index] else {
            return;
        };
        upload.ready.store(false, Ordering::Release);
        let ready = Arc::clone(&upload.ready);
        upload
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Write, move |result| {
                ready.store(result.is_ok(), Ordering::Release);
            });
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
pub(crate) fn draw(
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

/// One direction of resampling: the source texels it may read, how many source texels one
/// output texel covers (negative where mirrored), where the output's edge falls in the source
/// (crop and chroma siting), and whether it reads down the source's columns.
#[derive(Clone, Copy)]
struct Pass {
    window: (u32, u32),
    step: f32,
    offset: f32,
    transposed: bool,
}

impl Pass {
    fn new(window: (u32, u32), step: f32, offset: f32, transposed: bool) -> Pass {
        Pass {
            window,
            step,
            offset,
            transposed,
        }
    }

    /// Kernel scale: a downscale widens the kernel so it does not alias.
    fn scale(&self) -> f32 {
        self.step.abs().max(1.0)
    }

    /// The window's ends as the shader takes them.
    fn ends(&self) -> [[u8; 4]; 2] {
        [self.window.0, self.window.1].map(|end| (end as i32).to_le_bytes())
    }

    /// The shader's `Resample` uniform.
    fn bytes(&self) -> Vec<u8> {
        let [min, max] = self.ends();
        [
            &min[..],
            &max,
            &self.step.to_le_bytes(),
            &self.offset.to_le_bytes(),
            &self.scale().to_le_bytes(),
            &i32::from(self.transposed).to_le_bytes(),
        ]
        .concat()
    }
}

/// The final pass, as the shader's `Convert` uniform.
struct Convert {
    rect: FrameRect,
    to_rgb: [[f32; 4]; 3],
    luma_down: Pass,
    chroma_down: Pass,
}

impl Convert {
    fn bytes(&self) -> Vec<u8> {
        let FrameRect {
            x,
            y,
            width,
            height,
        } = self.rect;
        let corners = [x, y, x + width, y + height].map(|v| (v as i32).to_le_bytes());
        let rows = self.to_rgb.iter().flatten().map(|v| v.to_le_bytes());
        let (luma, chroma) = (self.luma_down, self.chroma_down);
        let windows = [luma.ends(), chroma.ends()].into_iter().flatten();
        let down = [
            luma.step,
            luma.offset,
            luma.scale(),
            chroma.step,
            chroma.offset,
            chroma.scale(),
        ]
        .map(f32::to_le_bytes);
        // WGSL rounds the struct up to a multiple of 16 bytes.
        let padding = [[0; 4]; 2];
        corners
            .into_iter()
            .chain(rows)
            .chain(windows)
            .chain(down)
            .chain(padding)
            .flatten()
            .collect()
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
