//! The export pass (docs/ARCHITECTURE.md, "Decoder pool"): composited RGB frames become
//! limited-range BT.709 YUV 4:2:0 on the GPU, chroma downsampled with the same Catmull-Rom
//! and sited left, and are read back, so encoders never see RGB.

use std::sync::mpsc;

use dusk_core::{ColorMatrix, ColorRange, Picture, PictureLayout};
use wgpu::util::DeviceExt;

use crate::compositor::draw;
use crate::{Gpu, RenderError};

/// Converts composited frames to the YUV that encoders take.
pub struct ToYuv {
    gpu: Gpu,
    layout: wgpu::BindGroupLayout,
    luma: wgpu::RenderPipeline,
    chroma: wgpu::RenderPipeline,
}

impl ToYuv {
    /// Builds the pass on `gpu`.
    pub fn new(gpu: &Gpu) -> ToYuv {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("to yuv"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("to yuv"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("to yuv"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
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
        ToYuv {
            gpu: gpu.clone(),
            luma: pipeline("to_luma", wgpu::TextureFormat::R8Unorm),
            chroma: pipeline("to_chroma", wgpu::TextureFormat::Rg8Unorm),
            layout,
        }
    }

    /// Converts an `Rgba8Unorm` frame to an NV12 picture tagged limited-range BT.709, and
    /// reads it back. Blocks until the GPU is done; meant for export, not the UI thread.
    pub fn convert(&self, frame: &wgpu::Texture) -> Result<Picture, RenderError> {
        let device = &self.gpu.device;
        let (width, height) = (frame.width(), frame.height());
        let size: Vec<u8> = [width, height]
            .iter()
            .flat_map(|side| (*side as i32).to_le_bytes())
            .collect();
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &size,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = frame.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        let target = |(width, height), format| {
            device.create_texture(&wgpu::TextureDescriptor {
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let luma = target((width, height), wgpu::TextureFormat::R8Unorm);
        let chroma = target(
            (width.div_ceil(2), height.div_ceil(2)),
            wgpu::TextureFormat::Rg8Unorm,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        draw(&mut encoder, &self.luma, &bind_group, &luma);
        draw(&mut encoder, &self.chroma, &bind_group, &chroma);
        let luma_copy = Readback::new(&mut encoder, device, &luma, 1);
        let chroma_copy = Readback::new(&mut encoder, device, &chroma, 2);
        self.gpu.queue.submit([encoder.finish()]);
        let mapped = [luma_copy.map(), chroma_copy.map()];
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        for receiver in mapped {
            receiver
                .recv()
                .map_err(|e| RenderError::Readback(e.to_string()))?
                .map_err(|e| RenderError::Readback(e.to_string()))?;
        }
        Ok(Picture {
            width,
            height,
            layout: PictureLayout::Nv12,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            luma: luma_copy.rows()?,
            chroma: chroma_copy.rows()?,
        })
    }
}

/// A texture copied into a buffer that can be mapped for reading.
struct Readback {
    buffer: wgpu::Buffer,
    row: u32,
    padded: u32,
    height: u32,
}

impl Readback {
    /// Records a copy of `texture`, whose texels are `bytes` wide.
    fn new(
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        bytes: u32,
    ) -> Readback {
        let row = texture.width() * bytes;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let height = texture.height();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
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
        Readback {
            buffer,
            row,
            padded,
            height,
        }
    }

    /// Asks for the buffer to be mapped; the receiver hears when it is.
    fn map(&self) -> mpsc::Receiver<Result<(), wgpu::BufferAsyncError>> {
        let (sender, receiver) = mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        receiver
    }

    /// The mapped rows, packed.
    fn rows(&self) -> Result<Vec<u8>, RenderError> {
        let mut packed = Vec::with_capacity(self.row as usize * self.height as usize);
        {
            let mapped = self
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| RenderError::Readback(e.to_string()))?;
            for row in mapped.chunks(self.padded as usize) {
                packed.extend_from_slice(&row[..self.row as usize]);
            }
        }
        self.buffer.unmap();
        Ok(packed)
    }
}
