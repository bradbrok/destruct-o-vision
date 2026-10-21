//! Shared GPU helpers: signal formats and the letterboxing blitter.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use bytemuck::{Pod, Zeroable};

/// The device's queue, shared by the UI and engine threads.
///
/// wgpu's `Surface::configure` waits for the GPU to go idle, then fails
/// (fatally, by default) if another thread submitted work in the meantime.
/// So every submission and every surface configuration goes through here,
/// under one lock. Buffer and texture writes and `present` don't submit
/// work; they use [`GpuQueue::raw`].
#[derive(Clone, Debug)]
pub struct GpuQueue {
    queue: wgpu::Queue,
    lock: Arc<Mutex<()>>,
}

impl GpuQueue {
    pub fn new(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            lock: Arc::default(),
        }
    }

    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(&self, buffers: I) -> wgpu::SubmissionIndex {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.queue.submit(buffers)
    }

    pub fn configure(
        &self,
        surface: &wgpu::Surface,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        surface.configure(device, config);
    }

    /// For writes and presenting. Never submit through it.
    pub fn raw(&self) -> &wgpu::Queue {
        &self.queue
    }
}

/// Storage format of video-rate signals: float, with headroom beyond 0..1.
pub const SIGNAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Preview textures shown in the UI (egui samples them as gamma-encoded).
pub const PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Mirrors `struct Blit` in `blit.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct BlitGpu {
    pub scale: [f32; 2],
    /// 0 plain, 1 CRT.
    pub mode: f32,
    /// 0 aperture grille, 1 shadow mask, 2 slot mask.
    pub mask_type: f32,
    pub mask: f32,
    pub scanlines: f32,
    pub glow: f32,
    pub curvature: f32,
    pub src_size: [f32; 2],
    pub dst_size: [f32; 2],
}

impl BlitGpu {
    pub fn plain(scale: [f32; 2]) -> Self {
        Self {
            scale,
            ..Default::default()
        }
    }
}

/// A source texture plus its own uniforms, ready to blit to one target.
pub struct BlitBinding {
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Letterboxed fullscreen copy from a signal texture to any color target.
pub struct Blitter {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Blitter {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline_layout,
            module,
            sampler,
            pipelines: HashMap::new(),
        }
    }

    /// Builds the pipeline for `format` if it doesn't exist yet.
    pub fn prepare(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.pipelines.contains_key(&format) {
            return;
        }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(format, pipeline);
    }

    pub fn bind(&self, device: &wgpu::Device, src: &wgpu::TextureView) -> BlitBinding {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit uniforms"),
            size: size_of::<BlitGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        BlitBinding { uniforms, bind_group }
    }

    /// Panics if [`Self::prepare`] wasn't called for `format`.
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        binding: &BlitBinding,
        format: wgpu::TextureFormat,
        target: &wgpu::TextureView,
        uniforms: BlitGpu,
    ) {
        queue.write_buffer(&binding.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipelines[&format]);
        pass.set_bind_group(0, &binding.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Picture scale that letterboxes `aspect` into a `width` × `height` target.
pub fn letterbox(aspect: f32, width: u32, height: u32) -> [f32; 2] {
    let target = width as f32 / height.max(1) as f32;
    if target > aspect {
        [aspect / target, 1.0]
    } else {
        [1.0, target / aspect]
    }
}
