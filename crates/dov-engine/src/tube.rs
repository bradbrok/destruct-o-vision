//! The CRT module's picture tube, drawn at the output window's resolution
//! and refresh rate in four passes (see `shaders/tube`):
//!
//! 1. Phosphors, at the signal's size: each refresh, the beam sweeps the
//!    rows whose time has come and lays down light that fades at the
//!    phosphor's rate. Light a refresh is too short to show carries over,
//!    so the average is right on any display, and on a fast one the
//!    picture is drawn top to bottom as on a real tube.
//! 2. Spot: each line's light spread across by the beam spot, which grows
//!    with beam current.
//! 3. Halation: a blur pyramid of the light.
//! 4. Screen, at the target's size: the lines spread down by the spot,
//!    through the phosphor mask and the curved glass.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use dov_core::RasterStd;
use dov_modules::crt;

use crate::runtime::CrtState;

/// Peak over average of each mask at full strength (grille, shadow, slot,
/// none): one over the share of the face one color's phosphors cover.
const MASK_GAIN: [f32; 4] = [3.75, 4.6875, 4.6875, 1.0];
/// Light-to-signal exponent of the tube.
const GAMMA: f32 = 2.4;
/// The video amplifier's bandwidth, as a spread in samples.
const BANDWIDTH: f32 = 0.55;
/// Glow pyramid depth.
const GLOW_LEVELS: usize = 6;

/// Phosphor decay times to 1/e in fields, per channel: P22 color phosphors
/// (the red one lingers a little), then the white, green and amber of
/// monochrome tubes.
const DECAY: [[f32; 3]; 4] = [[0.06, 0.02, 0.02], [0.05; 3], [3.0; 3], [1.0; 3]];
/// Monochrome phosphor colors in linear light.
const PHOSPHOR: [[f32; 3]; 4] = [
    [1.0, 1.0, 1.0],
    [0.92, 0.96, 1.0],
    [0.10, 1.0, 0.22],
    [1.0, 0.50, 0.06],
];

/// Mirrors `struct Tube` in `shaders/tube/common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct TubeGpu {
    src: [f32; 4],
    time: [f32; 4],
    scan: [f32; 4],
    decay: [f32; 4],
    drive: [f32; 4],
    phosphor: [f32; 4],
    spot: [f32; 4],
    mask: [f32; 4],
    geom: [f32; 4],
    glass: [f32; 4],
    events: [f32; 4],
    magnet: [f32; 4],
}

/// Mirrors `struct View` in `shaders/tube/draw.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct ViewGpu {
    /// Fraction of the target the tube face covers, per axis.
    pub scale: [f32; 2],
    /// Target size in pixels.
    pub size: [f32; 2],
}

/// How the beam has moved since the last refresh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Refresh {
    /// Whole fields at a time: this many new ones.
    Hold { fields: u32 },
    /// The beam swept from `from` to `to`, in fields from the start of the
    /// current field (`from` is negative when it began in an earlier one).
    Beam { from: f32, to: f32 },
}

/// The display may run off a whole multiple of the field rate by this much
/// for the beam to draw each field over its refreshes. Further off, the
/// rows' turns come round unevenly and the picture flickers at the beat.
const BEAM_TOLERANCE: f64 = 0.01;

/// How the beam moved for a refresh shown at `shown` on the field clock
/// (the last at `last_shown`), with `fresh` new fields rendered for it and
/// the current one `target`. Displays at two or more times the field rate
/// (or a whole multiple near it) show the beam sweeping; others, or `hold`,
/// show whole fields, and `None` when no new one has come.
pub(crate) fn refresh(
    hold: bool,
    refreshes_per_field: f64,
    shown: f64,
    last_shown: Option<f64>,
    target: u64,
    fresh: u64,
) -> Option<Refresh> {
    let whole = refreshes_per_field.round();
    let beam = whole >= 2.0 && (refreshes_per_field - whole).abs() <= BEAM_TOLERANCE * whole;
    if hold || !beam {
        return (fresh > 0).then(|| Refresh::Hold {
            fields: fresh.min(4) as u32,
        });
    }
    let from = last_shown.map_or(shown - 1.0 / refreshes_per_field, |last| last.max(shown - 2.0));
    let start = target as f64;
    Some(Refresh::Beam {
        from: (from - start) as f32,
        to: (shown - start) as f32,
    })
}

/// The tube's settings for one refresh, from the CRT module's params and
/// state.
pub(crate) fn settings(crt: &CrtState, raster: &RasterStd, refresh: Refresh, field: u64) -> TubeGpu {
    let p = &crt.params;
    let phosphor = p[crt::PHOSPHOR].round() as usize;
    let mono = phosphor > 0;
    let kind = if mono {
        3
    } else {
        p[crt::MASK_TYPE].round() as usize
    };
    // The mask puts the light through a fraction of the face, so its
    // phosphors shine brighter than the average; the guns are turned down to
    // leave the display room for them. (The same when the window is too
    // small to draw the mask, so the picture doesn't change with its size.)
    let peak = 1.0 / (1.0 + p[crt::MASK] * (MASK_GAIN[kind] - 1.0));
    let white = if mono {
        [1.0; 3]
    } else {
        white_balance(p[crt::WHITE])
    };
    let drive = p[crt::CONTRAST] * peak;
    // Turned up past that, the tube gets brighter and the mask clips.
    let limit = peak * p[crt::CONTRAST].max(1.0);
    let persist = 10f32.powf(4.0 * p[crt::PERSIST]);
    let decay = DECAY[phosphor].map(|d| d * persist);

    // Past halfway, Bloom opens right up: highlights swell into fat,
    // overlapping blobs and the raster breathes hard.
    let bloom = p[crt::BLOOM];
    let past = (bloom - 0.5).max(0.0);
    let (width, rows) = raster.active_size();
    let stride = raster.fields_per_frame() as f32;
    let parity = raster.field_parity(field) as f32;
    let (time, after) = match refresh {
        Refresh::Hold { fields } => ([0.0, fields.max(1) as f32, parity, 1.0], 0.0),
        Refresh::Beam { from, to } => ([from, to, parity, 0.0], to),
    };
    // Seconds into the field, for the power and degauss animations.
    let into = after * raster.field_period() as f32;
    let aspect = raster.display_aspect;
    TubeGpu {
        src: [width as f32, rows as f32, stride, raster.lines_per_field() as f32],
        time,
        scan: [raster.first_active_line as f32, peak, limit, GAMMA],
        decay: [decay[0], decay[1], decay[2], f32::from(u8::from(mono))],
        drive: [
            drive * white[0],
            drive * white[1],
            drive * white[2],
            p[crt::BRIGHT],
        ],
        phosphor: [
            PHOSPHOR[phosphor][0],
            PHOSPHOR[phosphor][1],
            PHOSPHOR[phosphor][2],
            0.0,
        ],
        spot: [
            0.2 * 10f32.powf(p[crt::SPOT]),
            1.5 * bloom + 30.0 * past * past,
            BANDWIDTH,
            width as f32 / rows as f32 / aspect,
        ],
        mask: [
            kind as f32,
            if mono { 0.0 } else { p[crt::MASK] },
            p[crt::PITCH],
            0.0,
        ],
        geom: [
            p[crt::CURVATURE],
            1.0 + p[crt::OVERSCAN] / 100.0,
            p[crt::CONVERGE],
            aspect,
        ],
        glass: [
            0.35 * p[crt::GLOW],
            p[crt::ROOM],
            0.04 * bloom + 0.3 * past * past,
            0.0,
        ],
        events: [
            crt.events[0] + into,
            crt.events[1],
            crt.events[2] + into,
            crt.events[3],
        ],
        magnet: [p[crt::MAGNET], 0.0, 0.0, 0.0],
    }
}

/// Gun gains that set white to a color temperature on the daylight locus,
/// equal at D65; the strongest is 1.
fn white_balance(kelvin: f32) -> [f32; 3] {
    let rgb = |k: f32| {
        let k = f64::from(k.clamp(4000.0, 25000.0));
        let x = if k <= 7000.0 {
            -4.6070e9 / k.powi(3) + 2.9678e6 / k.powi(2) + 0.09911e3 / k + 0.244063
        } else {
            -2.0064e9 / k.powi(3) + 1.9018e6 / k.powi(2) + 0.24748e3 / k + 0.237040
        };
        let y = -3.0 * x * x + 2.870 * x - 0.275;
        let (cx, cz) = (x / y, (1.0 - x - y) / y);
        [
            3.2406 * cx - 1.5372 - 0.4986 * cz,
            -0.9689 * cx + 1.8758 + 0.0415 * cz,
            0.0557 * cx - 0.2040 + 1.0570 * cz,
        ]
    };
    let d65 = rgb(6504.0);
    let c = rgb(kelvin);
    let gains = [c[0] / d65[0], c[1] / d65[1], c[2] / d65[2]];
    let top = gains.iter().copied().fold(f64::MIN, f64::max);
    gains.map(|g| (g / top) as f32)
}

/// A texture the tube's passes write and read.
struct Tex {
    view: wgpu::TextureView,
}

impl Tex {
    fn new(
        device: &wgpu::Device,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&Default::default()),
        }
    }
}

const LIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Stored energy decays slowly with long persistence; half floats would
/// round it to a standstill.
const CHARGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Everything sized to the signal, and the bind groups over it.
struct Frames {
    size: (u32, u32),
    /// Stored phosphor energy: `charge[latest]` is current.
    charge: [Tex; 2],
    latest: usize,
    light: Tex,
    current: Tex,
    beam: Tex,
    sigma: Tex,
    /// Glow pyramid sizes, from half the signal's down.
    levels: Vec<(u32, u32)>,
    down: Vec<Tex>,
    /// `up[i]` is level i's glow, gathered from the wider levels below; the
    /// bottom level is its own.
    up: Vec<Tex>,
    yoke: wgpu::TextureView,
    /// `phosphor[i]` reads `charge[i]` and writes the other.
    phosphor: [wgpu::BindGroup; 2],
    spot: wgpu::BindGroup,
    downs: Vec<wgpu::BindGroup>,
    /// Bottom level first.
    ups: Vec<wgpu::BindGroup>,
    /// The screen pass for the output window.
    window: Option<wgpu::BindGroup>,
}

pub(crate) struct Tube {
    phosphor: wgpu::ComputePipeline,
    spot: wgpu::ComputePipeline,
    down: wgpu::ComputePipeline,
    up: wgpu::ComputePipeline,
    draw_layout: wgpu::BindGroupLayout,
    draw_pipeline_layout: wgpu::PipelineLayout,
    draw_module: wgpu::ShaderModule,
    draws: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// The output window's view (captures bring their own).
    window_view: wgpu::Buffer,
    frames: Option<Frames>,
}

fn compute(device: &wgpu::Device, module: &wgpu::ShaderModule, entry: &str) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn uniform_buffer(device: &wgpu::Device, size: usize, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    entries: &[wgpu::BindingResource],
) -> wgpu::BindGroup {
    let entries: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(i, resource)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tube"),
        layout,
        entries: &entries,
    })
}

/// Bind group entries by binding number; `None` leaves a gap.
fn bind_sparse(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    entries: &[(u32, wgpu::BindingResource)],
) -> wgpu::BindGroup {
    let entries: Vec<_> = entries
        .iter()
        .map(|(binding, resource)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tube"),
        layout,
        entries: &entries,
    })
}

impl Tube {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let common = include_str!("shaders/tube/common.wgsl");
        let passes = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tube"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{common}\n{}", include_str!("shaders/tube/phosphor.wgsl")).into(),
            ),
        });
        let glow = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tube glow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tube/glow.wgsl").into()),
        });
        let draw_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tube screen"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{common}\n{}", include_str!("shaders/tube/draw.wgsl")).into(),
            ),
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
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tube screen"),
            entries: &[
                uniform(0),
                uniform(1),
                texture(2),
                texture(3),
                texture(4),
                texture(5),
                texture(6),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let draw_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tube screen"),
            bind_group_layouts: &[Some(&draw_layout)],
            immediate_size: 0,
        });
        Self {
            phosphor: compute(device, &passes, "phosphor"),
            spot: compute(device, &passes, "spot"),
            down: compute(device, &glow, "down"),
            up: compute(device, &glow, "up"),
            draw_layout,
            draw_pipeline_layout,
            draw_module,
            draws: HashMap::new(),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("tube"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            uniforms: uniform_buffer(device, size_of::<TubeGpu>(), "tube"),
            window_view: uniform_buffer(device, size_of::<ViewGpu>(), "tube window"),
            frames: None,
        }
    }

    /// Builds the screen pass for `format` if it doesn't exist yet.
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.draws.contains_key(&format) {
            return;
        }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tube screen"),
            layout: Some(&self.draw_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.draw_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.draw_module,
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
        self.draws.insert(format, pipeline);
    }

    /// Points the tube at a signal and its yoke offsets. The phosphors keep
    /// their glow unless the raster changed size.
    pub(crate) fn bind(
        &mut self,
        device: &wgpu::Device,
        raster: &RasterStd,
        signal: &wgpu::TextureView,
        yoke: &wgpu::TextureView,
    ) {
        let size = raster.active_size();
        let (charge, latest, light, current, beam, sigma, levels, down, up) = match self.frames.take() {
            Some(f) if f.size == size => (
                f.charge, f.latest, f.light, f.current, f.beam, f.sigma, f.levels, f.down, f.up,
            ),
            _ => {
                let wide = (size.0 * 2, size.1);
                let mut levels = Vec::new();
                let mut level = size;
                while levels.len() < GLOW_LEVELS && level.0.min(level.1) > 8 {
                    level = (level.0.div_ceil(2), level.1.div_ceil(2));
                    levels.push(level);
                }
                let down = levels
                    .iter()
                    .map(|&l| Tex::new(device, l, LIGHT_FORMAT, "tube glow"))
                    .collect::<Vec<_>>();
                let up = levels[..levels.len() - 1]
                    .iter()
                    .map(|&l| Tex::new(device, l, LIGHT_FORMAT, "tube glow"))
                    .collect::<Vec<_>>();
                (
                    [
                        Tex::new(device, size, CHARGE_FORMAT, "tube charge"),
                        Tex::new(device, size, CHARGE_FORMAT, "tube charge"),
                    ],
                    0,
                    Tex::new(device, size, LIGHT_FORMAT, "tube light"),
                    Tex::new(device, size, LIGHT_FORMAT, "tube current"),
                    Tex::new(device, wide, LIGHT_FORMAT, "tube beam"),
                    Tex::new(device, wide, LIGHT_FORMAT, "tube spot"),
                    levels,
                    down,
                    up,
                )
            }
        };
        use wgpu::BindingResource::{Sampler, TextureView};
        let uniforms = self.uniforms.as_entire_binding();
        let phosphor = |from: usize| {
            bind_sparse(
                device,
                &self.phosphor.get_bind_group_layout(0),
                &[
                    (0, uniforms.clone()),
                    (1, TextureView(signal)),
                    (2, TextureView(&charge[from].view)),
                    (3, TextureView(&charge[1 - from].view)),
                    (4, TextureView(&light.view)),
                    (5, TextureView(&current.view)),
                ],
            )
        };
        let phosphor = [phosphor(0), phosphor(1)];
        let spot = bind_sparse(
            device,
            &self.spot.get_bind_group_layout(0),
            &[
                (0, uniforms.clone()),
                (6, TextureView(&light.view)),
                (7, TextureView(&current.view)),
                (8, TextureView(&beam.view)),
                (9, TextureView(&sigma.view)),
            ],
        );
        // Down from the light, level by level.
        let downs = (0..down.len())
            .map(|i| {
                let src = if i == 0 { &light.view } else { &down[i - 1].view };
                bind_sparse(
                    device,
                    &self.down.get_bind_group_layout(0),
                    &[
                        (0, TextureView(src)),
                        (1, Sampler(&self.sampler)),
                        (2, TextureView(&down[i].view)),
                    ],
                )
            })
            .collect();
        let ups = (0..up.len())
            .rev()
            .map(|i| {
                let wider = up.get(i + 1).map_or(&down[down.len() - 1].view, |u| &u.view);
                bind_sparse(
                    device,
                    &self.up.get_bind_group_layout(0),
                    &[
                        (0, TextureView(&down[i].view)),
                        (1, Sampler(&self.sampler)),
                        (2, TextureView(&up[i].view)),
                        (3, TextureView(wider)),
                    ],
                )
            })
            .collect();
        let mut frames = Frames {
            size,
            charge,
            latest,
            light,
            current,
            beam,
            sigma,
            levels,
            down,
            up,
            yoke: yoke.clone(),
            phosphor,
            spot,
            downs,
            ups,
            window: None,
        };
        frames.window = Some(self.screen_group(device, &frames, &self.window_view));
        self.frames = Some(frames);
    }

    fn screen_group(&self, device: &wgpu::Device, f: &Frames, view: &wgpu::Buffer) -> wgpu::BindGroup {
        use wgpu::BindingResource::{Sampler, TextureView};
        let glow = f.up.first().unwrap_or(&f.down[0]);
        bind(
            device,
            &self.draw_layout,
            &[
                self.uniforms.as_entire_binding(),
                view.as_entire_binding(),
                TextureView(&f.beam.view),
                TextureView(&f.sigma.view),
                TextureView(&glow.view),
                TextureView(&f.down[f.down.len() - 1].view),
                TextureView(&f.yoke),
                Sampler(&self.sampler),
            ],
        )
    }

    /// Sets this refresh's settings, for both advancing and drawing.
    pub(crate) fn update(&self, queue: &wgpu::Queue, settings: &TubeGpu) {
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(settings));
    }

    /// Moves the beam and phosphors on by one refresh (see [`Refresh`]).
    pub(crate) fn advance(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(f) = &mut self.frames else { return };
        let groups = |(w, h): (u32, u32), (gw, gh): (u32, u32)| (w.div_ceil(gw), h.div_ceil(gh));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("tube"),
            timestamp_writes: None,
        });
        let (x, y) = groups(f.size, (16, 8));
        pass.set_pipeline(&self.phosphor);
        pass.set_bind_group(0, &f.phosphor[f.latest], &[]);
        pass.dispatch_workgroups(x, y, 1);
        f.latest = 1 - f.latest;

        let (x, y) = groups((f.size.0 * 2, f.size.1), (16, 8));
        pass.set_pipeline(&self.spot);
        pass.set_bind_group(0, &f.spot, &[]);
        pass.dispatch_workgroups(x, y, 1);

        pass.set_pipeline(&self.down);
        for (group, &level) in f.downs.iter().zip(&f.levels) {
            let (x, y) = groups(level, (8, 8));
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
        pass.set_pipeline(&self.up);
        let up_levels = &f.levels[..f.ups.len()];
        for (group, &level) in f.ups.iter().zip(up_levels.iter().rev()) {
            let (x, y) = groups(level, (8, 8));
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
    }

    /// Draws the screen into `target`. The output window's view is kept;
    /// any other target (a capture) gets its own.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        target: &wgpu::TextureView,
        view: ViewGpu,
        window: bool,
    ) {
        let Some(f) = &self.frames else { return };
        let own;
        let group = if let (true, Some(group)) = (window, &f.window) {
            queue.write_buffer(&self.window_view, 0, bytemuck::bytes_of(&view));
            group
        } else {
            let buffer = uniform_buffer(device, size_of::<ViewGpu>(), "tube capture");
            queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&view));
            own = self.screen_group(device, f, &buffer);
            &own
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tube screen"),
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
        pass.set_pipeline(&self.draws[&format]);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use dov_core::RasterStd;

    use super::*;

    fn validate(label: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
    }

    #[test]
    fn its_shaders_are_valid_wgsl() {
        let common = include_str!("shaders/tube/common.wgsl");
        validate(
            "phosphor",
            &format!("{common}\n{}", include_str!("shaders/tube/phosphor.wgsl")),
        );
        validate("glow", include_str!("shaders/tube/glow.wgsl"));
        validate(
            "draw",
            &format!("{common}\n{}", include_str!("shaders/tube/draw.wgsl")),
        );
    }

    #[test]
    fn white_is_neutral_at_d65_and_bluer_when_hotter() {
        let d65 = white_balance(6504.0);
        assert!(d65.iter().all(|g| (g - 1.0).abs() < 1e-4), "{d65:?}");
        let cool = white_balance(9300.0);
        assert!(cool[2] == 1.0 && cool[0] < 0.85, "{cool:?}");
        let warm = white_balance(5000.0);
        assert!(warm[0] == 1.0 && warm[2] < 0.8, "{warm:?}");
    }

    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    /// A CRT at its defaults, but with `set` params changed.
    fn crt_state(set: &[(usize, f32)]) -> CrtState {
        let desc = &dov_modules::find(crt::ID).unwrap().desc;
        let mut params: Vec<f32> = desc.params.iter().map(|p| p.default_value()).collect();
        for &(i, v) in set {
            params[i] = v;
        }
        CrtState {
            params,
            events: [1.0e4, 1.0, 1.0e4, 0.0],
        }
    }

    /// Light the phosphors give off over `refreshes` refreshes of a flat
    /// gray signal, averaged over time, for one row near the middle, plus
    /// the light the signal asks for.
    fn average_light(
        raster: RasterStd,
        gray: f32,
        per_refresh: Option<f64>,
        refreshes: usize,
    ) -> Option<([f32; 2], f32)> {
        let (device, queue) = gpu()?;
        let (width, rows) = raster.active_size();
        let signal = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height: rows,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texel: Vec<u8> = (0..4)
            .flat_map(|_| half::f16::from_f32(gray).to_le_bytes())
            .collect();
        let data: Vec<u8> = texel
            .iter()
            .copied()
            .cycle()
            .take(texel.len() * (width * rows) as usize)
            .collect();
        queue.write_texture(
            signal.as_image_copy(),
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 8),
                rows_per_image: None,
            },
            signal.size(),
        );
        let signal = signal.create_view(&Default::default());
        let mut tube = Tube::new(&device);
        tube.bind(&device, &raster, &signal, &signal);
        let crt = crt_state(&[]);

        let row = rows / 2;
        let bytes_per_row = (width * 8).next_multiple_of(256);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(bytes_per_row) * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut sums = [0.0f64; 2];
        let mut want = 0.0;
        let mut pos = 10.0f64;
        for _ in 0..refreshes {
            let last = pos;
            let refresh = match per_refresh {
                Some(step) => {
                    pos += step;
                    let start = pos.floor();
                    Refresh::Beam {
                        from: (last - start) as f32,
                        to: (pos - start) as f32,
                    }
                }
                None => {
                    pos += 1.0;
                    Refresh::Hold { fields: 1 }
                }
            };
            let settings = settings(&crt, &raster, refresh, pos.floor() as u64);
            want = settings.drive[1] * gray.powf(GAMMA);
            tube.update(&queue, &settings);
            let mut encoder = device.create_command_encoder(&Default::default());
            tube.advance(&mut encoder);
            let light = tube.frames.as_ref().unwrap().light.view.texture();
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: light,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: 2,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            {
                let bytes = readback.slice(..).get_mapped_range().unwrap();
                for (r, sum) in sums.iter_mut().enumerate() {
                    // Green of the middle sample.
                    let at = r * bytes_per_row as usize + (width as usize / 2) * 8 + 2;
                    *sum += f64::from(half::f16::from_le_bytes([bytes[at], bytes[at + 1]]).to_f32());
                }
            }
            readback.unmap();
        }
        Some((sums.map(|s| (s / refreshes as f64) as f32), want))
    }

    fn assert_averages_to(raster: RasterStd, gray: f32, per_refresh: Option<f64>) {
        // Whole fields, so the average isn't off by a partial one.
        let refreshes = per_refresh.map_or(120, |step| (60.0 / step).round() as usize);
        let Some((light, want)) = average_light(raster, gray, per_refresh, refreshes) else {
            eprintln!("no GPU; skipped");
            return;
        };
        for l in light {
            assert!(
                (l - want).abs() < want * 0.03 + 1e-3,
                "{} at {gray}: {light:?}, want {want}",
                raster.name
            );
        }
    }

    #[test]
    fn the_beam_shows_each_row_as_bright_as_asked_on_average() {
        // A 144 Hz display: 0.42 fields a refresh. Both of a pair of rows,
        // one from each field when interlaced, bright enough to carry over
        // to later refreshes and not.
        for raster in [RasterStd::NTSC, RasterStd::HD720P60] {
            for gray in [0.3, 0.7, 1.0] {
                assert_averages_to(raster, gray, Some(60.0 / 144.0));
            }
        }
    }

    #[test]
    fn whole_fields_show_each_row_as_bright_as_asked_on_average() {
        for raster in [RasterStd::NTSC, RasterStd::HD720P60] {
            for gray in [0.3, 1.0] {
                assert_averages_to(raster, gray, None);
            }
        }
    }

    /// How long a refresh takes at 5K, for a feel of the cost:
    /// `cargo test -p dov-engine --release tube_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tube_cost() {
        let Some((device, queue)) = gpu() else { return };
        let raster = RasterStd::NTSC;
        let (width, rows) = raster.active_size();
        let signal = device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width,
                    height: rows,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let (w, h) = (3840, 2880);
        let target = device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut tube = Tube::new(&device);
        tube.prepare(&device, format);
        tube.bind(&device, &raster, &signal, &signal);
        for (name, set, draw) in [
            ("grille", vec![], true),
            ("shadow", vec![(crt::MASK_TYPE, 1.0)], true),
            ("slot", vec![(crt::MASK_TYPE, 2.0)], true),
            ("no mask", vec![(crt::MASK, 0.0)], true),
            ("full bloom", vec![(crt::BLOOM, 1.0)], true),
            ("phosphors only", vec![], false),
        ] {
            let crt = crt_state(&set);
            let mut run = |n: usize| {
                let start = std::time::Instant::now();
                for i in 0..n {
                    let refresh = Refresh::Beam {
                        from: 0.0,
                        to: 60.0 / 144.0,
                    };
                    tube.update(&queue, &settings(&crt, &raster, refresh, i as u64));
                    let mut encoder = device.create_command_encoder(&Default::default());
                    tube.advance(&mut encoder);
                    let view = ViewGpu {
                        scale: [1.0, 1.0],
                        size: [w as f32, h as f32],
                    };
                    if draw {
                        tube.draw(&device, &queue, &mut encoder, format, &target, view, true);
                    }
                    queue.submit([encoder.finish()]);
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                }
                start.elapsed().as_secs_f64() * 1000.0 / n as f64
            };
            run(5);
            println!("{name}: {:.2} ms a refresh at {w}x{h}", run(50));
        }
    }

    #[test]
    fn the_beam_sweeps_only_at_whole_multiples_of_the_field_rate() {
        let ntsc = 59.94;
        let per_field = |hz: f64| hz / ntsc;
        // 120 Hz: half a field a refresh, from where the last one left off.
        let r = refresh(false, per_field(120.0), 100.75, Some(100.25), 100, 0);
        assert_eq!(r, Some(Refresh::Beam { from: 0.25, to: 0.75 }));
        assert!(matches!(
            refresh(false, per_field(240.0), 7.5, None, 7, 1),
            Some(Refresh::Beam { .. })
        ));
        // 60 and 144 Hz, or held: whole fields, and nothing new on a repeat.
        for hz in [60.0, 144.0, 165.0] {
            assert_eq!(
                refresh(false, per_field(hz), 100.5, Some(100.1), 100, 1),
                Some(Refresh::Hold { fields: 1 })
            );
            assert_eq!(refresh(false, per_field(hz), 100.5, Some(100.1), 100, 0), None);
        }
        assert_eq!(
            refresh(true, per_field(120.0), 100.5, Some(100.1), 100, 2),
            Some(Refresh::Hold { fields: 2 })
        );
        // A stall doesn't sweep more than two fields' worth.
        let r = refresh(false, per_field(120.0), 100.5, Some(90.0), 100, 10);
        assert_eq!(r, Some(Refresh::Beam { from: -1.5, to: 0.5 }));
    }

    #[test]
    fn its_uniforms_match_the_shader() {
        let common = include_str!("shaders/tube/common.wgsl");
        let module = naga::front::wgsl::parse_str(common).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Tube"))
            .unwrap();
        assert_eq!(ty.inner.size(module.to_ctx()) as usize, size_of::<TubeGpu>());
    }
}
