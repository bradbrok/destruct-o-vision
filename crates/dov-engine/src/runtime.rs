//! Executes a scheduled patch on the GPU, one field at a time.
//!
//! Module instances persist across re-patching (matched by id and type), so
//! oscillator phases, frame stores and TV deflection state survive edits.
//! Video passes write only the rows of the current field, so each video
//! texture holds a woven frame; composite passes write the current field's
//! rows of a whole-scan frame (blanking included).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use dov_core::{CompositeOp, Kernel, ModuleId, ParamBank, PortKind, RasterStd};
use dov_io::{CameraSource, CameraStatus, Frame};
use dov_modules::{
    Behavior, EXTRA_SLOTS, FieldCtx, FieldIo, ModuleType, Timeline, choice_value, param_value,
};

use crate::codegen::{
    self, FieldInfoGpu, INPUT_BINDING_BASE, MAX_PARAM_SLOTS, ModuleUniformsGpu, OUTPUT_BINDING_BASE,
    RING_BINDING, TEXT_BINDING, TEXT_SAMPLER_BINDING,
};
use crate::gpu::{GpuQueue, SIGNAL_FORMAT};
use crate::graph::{self, PatchSnapshot, Schedule, SnapshotModule, Source};
use crate::text::{self, Fonts};

/// Storage format of composite signals.
const COMPOSITE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Format of uploaded camera frames and pictures.
const CAMERA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// Scan lines a TV can draw per field (see tv_scan.wgsl).
const TV_MAX_BEAMS: u32 = 640;

/// A signal texture: a woven video frame at the raster's active size, or a
/// composite frame covering the whole scan.
pub(crate) struct SignalTex {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl SignalTex {
    fn new(device: &wgpu::Device, raster: &RasterStd, kind: PortKind, label: &str) -> Self {
        let (format, (width, height)) = match kind {
            PortKind::Composite => (COMPOSITE_FORMAT, composite_size(raster)),
            PortKind::Mono | PortKind::Rgb | PortKind::Control => (SIGNAL_FORMAT, raster.active_size()),
        };
        let mut usage = wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        if format == SIGNAL_FORMAT {
            // Frame stores are cleared to a constant with a render pass.
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
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
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}

/// Whole-scan composite frame size: samples per line × lines per frame.
fn composite_size(raster: &RasterStd) -> (u32, u32) {
    (raster.samples_per_line, raster.lines_per_frame)
}

/// GPU memory a single history ring may use, and its longest history.
const RING_BUDGET_BYTES: u64 = 400 << 20;
const RING_MAX_FIELDS: u32 = 120;

/// A module's history of past fields (see `RingSpec`).
struct Ring {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: u32,
    /// Fields stored so far; the newest is in slot `(writes - 1) % depth`.
    writes: u64,
}

impl Ring {
    /// Up to `fields` of history (the module's request), within the budget.
    fn new(device: &wgpu::Device, raster: &RasterStd, fields: u32) -> Self {
        let (width, height) = raster.active_size();
        let frame = u64::from(width) * u64::from(height) * 8;
        let wanted = u64::from(fields.clamp(2, RING_MAX_FIELDS));
        let depth = (RING_BUDGET_BYTES / frame).clamp(2, wanted) as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("history"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: depth,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SIGNAL_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            depth,
            writes: 0,
        }
    }
}

/// The average brightness of a video output, for cables into control inputs.
/// Measured on the GPU every field and read back without waiting, so a value
/// arrives a field or two after the picture it describes.
struct Meter {
    /// (mean R, G, B, mean luma), written by `meter.wgsl`.
    result: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    readbacks: Vec<Readback>,
    value: f32,
    /// Field the current value was measured on.
    measured: u64,
}

/// A staging buffer cycling through free → copied → mapping → ready.
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    field: u64,
}

const READBACK_FREE: u8 = 0;
const READBACK_COPIED: u8 = 1;
const READBACK_MAPPING: u8 = 2;
const READBACK_READY: u8 = 3;
const METER_READBACKS: usize = 3;

impl Meter {
    fn buffers(device: &wgpu::Device) -> (wgpu::Buffer, Vec<Readback>) {
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        let result = buffer(
            "meter",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readbacks = (0..METER_READBACKS)
            .map(|_| Readback {
                buffer: buffer(
                    "meter readback",
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
                state: Arc::new(AtomicU8::new(READBACK_FREE)),
                field: 0,
            })
            .collect();
        (result, readbacks)
    }

    /// Takes in every reading that has arrived; keeps the newest.
    fn collect(&mut self) {
        for rb in &mut self.readbacks {
            if rb.state.load(Ordering::Acquire) != READBACK_READY {
                continue;
            }
            let reading = rb
                .buffer
                .slice(..)
                .get_mapped_range()
                .map(|v| bytemuck::cast_slice::<u8, f32>(&v)[3]);
            rb.buffer.unmap();
            rb.state.store(READBACK_FREE, Ordering::Release);
            if let Ok(v) = reading
                && rb.field >= self.measured
            {
                self.value = if v.is_finite() { v } else { 0.0 };
                self.measured = rb.field;
            }
        }
    }
}

/// A text module's rendered text (see `text.rs`).
struct TextState {
    view: wgpu::TextureView,
    /// What was rendered: the bank's text version and the font.
    version: u64,
    font: usize,
    size: [f32; 2],
    rect: [f32; 4],
}

/// A 1×1 texture of zeros in `format`.
fn constant_texture(device: &wgpu::Device, format: wgpu::TextureFormat, label: &str) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// A camera module's device and its latest frame on the GPU.
struct CameraState {
    source: CameraSource,
    seen: u64,
    frame: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl CameraState {
    fn aspect(&self) -> Option<f32> {
        let size = self.frame.as_ref()?.0.size();
        Some(size.width as f32 / size.height.max(1) as f32)
    }
}

/// A picture module's current picture, as uploaded.
struct PictureState {
    frame: Arc<Frame>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// A TV's working state: separated sync, deflection, scan lines, decoded signal.
struct TvState {
    sync: wgpu::Buffer,
    scan: wgpu::Buffer,
    beams: wgpu::Buffer,
    decoded: wgpu::TextureView,
}

impl TvState {
    fn new(device: &wgpu::Device, raster: &RasterStd) -> Self {
        let buffer = |label, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                // COPY_SRC for `Runtime::debug_tv`.
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let (width, height) = composite_size(raster);
        let decoded = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tv decoded"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SIGNAL_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            sync: buffer("tv sync", u64::from(height) * 16),
            scan: buffer("tv scan", 32),
            beams: buffer("tv beams", u64::from(TV_MAX_BEAMS + 1) * 16),
            decoded: decoded.create_view(&Default::default()),
        }
    }
}

/// A resource a pass binds.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Uniforms,
    /// Module input `j`: whatever is patched, or black of the right kind.
    Input(usize),
    /// Module output `k`, written as a storage texture.
    Output(usize),
    FeedbackStore,
    /// The camera's latest frame, or a picture module's picture.
    Frame,
    LinearSampler,
    TvSync {
        write: bool,
    },
    TvScan,
    TvBeams {
        write: bool,
    },
    TvDecodedWrite,
    TvDecodedRead,
    Ring,
    TextSdf,
}

/// How many workgroups a pass needs.
#[derive(Clone, Copy, Debug)]
enum Dispatch {
    /// 16×16 tiles over the active picture of one field.
    Pixels,
    /// One invocation per picture line of the field.
    Lines,
    /// One invocation per composite row of the field.
    CompositeRows,
    Single,
    /// 16×16 tiles over (picture width × TV scan lines).
    Beams,
}

struct PassSpec {
    label: &'static str,
    source: String,
    slots: Vec<(u32, Slot)>,
    dispatch: Dispatch,
}

struct KernelPass {
    label: &'static str,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    slots: Vec<(u32, Slot)>,
    dispatch: Dispatch,
}

struct Instance {
    id: ModuleId,
    ty: &'static ModuleType,
    params: Arc<ParamBank>,
    behavior: Option<Box<dyn Behavior>>,
    input_defaults: Vec<f32>,
    input_kinds: Vec<PortKind>,
    /// Per output; `None` for control outputs.
    outputs: Vec<Option<SignalTex>>,
    /// Last field's output, kept only for outputs read through delayed cables.
    history: Vec<Option<SignalTex>>,
    /// Frame store for feedback modules.
    store: Option<SignalTex>,
    camera: Option<CameraState>,
    picture: Option<PictureState>,
    tv: Option<TvState>,
    ring: Option<Ring>,
    /// Per output: its brightness meter, when a cable takes it into a control input.
    meters: Vec<Option<Meter>>,
    text: Option<TextState>,
    control_out: Vec<f32>,
    extra: [[f32; 4]; EXTRA_SLOTS],
    uniforms: wgpu::Buffer,
    /// One per pass of the module's kernel.
    bind_groups: Vec<wgpu::BindGroup>,
}

impl Instance {
    fn new(device: &wgpu::Device, m: &SnapshotModule) -> Self {
        let desc = &m.ty.desc;
        let inputs = desc.inputs_full();
        Self {
            id: m.id,
            ty: m.ty,
            params: m.params.clone(),
            behavior: m.ty.behavior.map(|make| make()),
            input_defaults: inputs.iter().map(|p| p.default).collect(),
            input_kinds: inputs.iter().map(|p| p.kind).collect(),
            outputs: desc.outputs.iter().map(|_| None).collect(),
            history: desc.outputs.iter().map(|_| None).collect(),
            store: None,
            camera: None,
            picture: None,
            tv: None,
            ring: None,
            meters: desc.outputs.iter().map(|_| None).collect(),
            text: None,
            control_out: vec![0.0; desc.outputs.len()],
            extra: [[0.0; 4]; EXTRA_SLOTS],
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(desc.id),
                size: size_of::<ModuleUniformsGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            bind_groups: Vec::new(),
        }
    }

    /// Generated kernels apply CV per sample themselves; engine kernels get
    /// params with control-rate CV already folded in.
    fn folds_cv(&self) -> bool {
        !matches!(self.ty.desc.kernel, Kernel::Pointwise(_) | Kernel::LineScan(_))
    }
}

pub(crate) struct Runtime {
    raster: RasterStd,
    black: SignalTex,
    black_composite: SignalTex,
    /// Stands in for a camera frame that hasn't arrived yet.
    no_frame: wgpu::TextureView,
    /// Stands in for text that hasn't been rendered yet.
    no_text: wgpu::TextureView,
    fonts: Fonts,
    sampler: wgpu::Sampler,
    meter_layout: wgpu::BindGroupLayout,
    meter_pipeline: wgpu::ComputePipeline,
    /// Compiled passes per module type (see `type_key`).
    pipelines: HashMap<usize, Vec<KernelPass>>,
    instances: Vec<Instance>,
    schedule: Schedule,
    last_field: Option<u64>,
}

impl Runtime {
    pub fn new(device: &wgpu::Device, raster: RasterStd) -> Self {
        let (meter_layout, meter_pipeline) = meter_pipeline(device);
        Self {
            meter_layout,
            meter_pipeline,
            black: SignalTex::new(device, &raster, PortKind::Rgb, "black"),
            black_composite: SignalTex::new(device, &raster, PortKind::Composite, "no signal"),
            no_frame: camera_texture(device, 1, 1).1,
            no_text: constant_texture(device, wgpu::TextureFormat::R16Float, "no text"),
            fonts: Fonts::default(),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("camera"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            raster,
            pipelines: HashMap::new(),
            instances: Vec::new(),
            schedule: Schedule::default(),
            last_field: None,
        }
    }

    pub fn module_count(&self) -> usize {
        self.instances.len()
    }

    pub fn pass_count(&self) -> usize {
        self.schedule
            .gpu
            .iter()
            .filter_map(|&i| self.pipelines.get(&type_key(self.instances[i].ty)))
            .map(Vec::len)
            .sum()
    }

    /// Problems to show the user: patch errors and device trouble.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = self.schedule.warnings.clone();
        for cam in self.instances.iter().filter_map(|i| i.camera.as_ref()) {
            match cam.source.status() {
                CameraStatus::Denied => w.push(
                    "Camera access denied. Allow it in System Settings > Privacy & Security > Camera, \
                     then re-add the module."
                        .into(),
                ),
                CameraStatus::Failed(e) => w.push(format!("Camera: {e}")),
                CameraStatus::Starting | CameraStatus::Running { .. } => {}
            }
        }
        // Modules sharing a device report the same trouble; say it once.
        for warning in self
            .instances
            .iter()
            .filter_map(|i| i.behavior.as_ref()?.warning())
        {
            if !w.contains(&warning) {
                w.push(warning);
            }
        }
        w
    }

    /// Each module's note (see `Behavior::note`), for modules that have one.
    pub fn notes(&self) -> Vec<(ModuleId, String)> {
        self.instances
            .iter()
            .filter_map(|i| Some((i.id, i.behavior.as_ref()?.note()?)))
            .collect()
    }

    /// Where each movie module is in its movie.
    pub fn timelines(&self) -> Vec<(ModuleId, Timeline)> {
        self.instances
            .iter()
            .filter_map(|i| Some((i.id, i.behavior.as_ref()?.timeline()?)))
            .collect()
    }

    /// The latest control outputs of every module that has any.
    pub fn control_values(&self) -> Vec<(ModuleId, Vec<f32>)> {
        self.instances
            .iter()
            .filter(|i| i.ty.desc.outputs.iter().any(|p| p.kind == PortKind::Control))
            .map(|i| (i.id, i.control_out.clone()))
            .collect()
    }

    pub fn set_patch(&mut self, device: &wgpu::Device, snap: &PatchSnapshot) {
        let mut old: HashMap<ModuleId, Instance> = self.instances.drain(..).map(|i| (i.id, i)).collect();
        self.instances = snap
            .modules
            .iter()
            .map(|m| match old.remove(&m.id) {
                // The same type, not just the same id: a module file that
                // was edited and reloaded starts afresh.
                Some(mut inst) if std::ptr::eq(inst.ty, m.ty) => {
                    inst.params = m.params.clone();
                    inst
                }
                _ => Instance::new(device, m),
            })
            .collect();
        // Types no module uses any more, such as replaced module files.
        let used: HashSet<usize> = self.instances.iter().map(|i| type_key(i.ty)).collect();
        self.pipelines.retain(|key, _| used.contains(key));
        self.schedule = graph::schedule(snap);
        for w in &self.schedule.warnings {
            log::warn!("patch: {w}");
        }
        self.rebuild(device);
    }

    pub fn set_raster(&mut self, device: &wgpu::Device, raster: RasterStd) {
        self.raster = raster;
        self.black = SignalTex::new(device, &raster, PortKind::Rgb, "black");
        self.black_composite = SignalTex::new(device, &raster, PortKind::Composite, "no signal");
        for inst in &mut self.instances {
            inst.outputs.iter_mut().for_each(|t| *t = None);
            inst.history.iter_mut().for_each(|t| *t = None);
            inst.store = None;
            inst.tv = None;
            inst.ring = None;
            inst.meters.iter_mut().for_each(|m| *m = None);
        }
        self.last_field = None;
        self.rebuild(device);
    }

    /// The signal feeding the Output module, or black.
    pub fn output_view(&self) -> &wgpu::TextureView {
        self.schedule
            .output
            .and_then(|o| self.instances[o].outputs.first()?.as_ref())
            .map_or(&self.black.view, |t| &t.view)
    }

    /// Param values of the Output module, which set how the display shows it.
    pub fn output_params(&self) -> Option<Vec<f32>> {
        Some(self.instances[self.schedule.output?].params.snapshot())
    }

    /// Modules whose panel shows a live picture of their first output.
    pub fn screen_modules(&self) -> Vec<ModuleId> {
        self.instances
            .iter()
            .filter(|i| i.ty.desc.screen)
            .map(|i| i.id)
            .collect()
    }

    /// A module's video output, for probes and screens.
    pub fn port_view(&self, id: ModuleId, output: usize) -> Option<&wgpu::TextureView> {
        let inst = self.instances.iter().find(|i| i.id == id)?;
        if !inst.ty.desc.outputs.get(output)?.kind.is_video() {
            return None;
        }
        inst.outputs.get(output)?.as_ref().map(|t| &t.view)
    }

    fn rebuild(&mut self, device: &wgpu::Device) {
        let raster = self.raster;
        let delayed: HashSet<(usize, usize)> = self.schedule.delayed_outputs.iter().copied().collect();
        for (i, inst) in self.instances.iter_mut().enumerate() {
            let desc = &inst.ty.desc;
            for (k, port) in desc.outputs.iter().enumerate() {
                let textured = port.kind != PortKind::Control;
                if textured && inst.outputs[k].is_none() {
                    inst.outputs[k] = Some(SignalTex::new(device, &raster, port.kind, desc.id));
                }
                if !delayed.contains(&(i, k)) {
                    inst.history[k] = None;
                } else if inst.history[k].is_none() {
                    inst.history[k] = Some(SignalTex::new(device, &raster, port.kind, "history"));
                }
            }
            match desc.kernel {
                Kernel::Feedback if inst.store.is_none() => {
                    inst.store = Some(SignalTex::new(device, &raster, PortKind::Rgb, "store"));
                }
                Kernel::Composite(CompositeOp::Tv) if inst.tv.is_none() => {
                    inst.tv = Some(TvState::new(device, &raster));
                }
                _ => {}
            }
            if let Some(spec) = desc.ring
                && inst.ring.is_none()
            {
                inst.ring = Some(Ring::new(device, &raster, spec.fields));
            }
            self.pipelines
                .entry(type_key(inst.ty))
                .or_insert_with(|| build_passes(device, inst.ty));
        }
        for i in 0..self.instances.len() {
            let bind_groups = self.bind_groups(device, i);
            self.instances[i].bind_groups = bind_groups;
        }

        // Meters for video outputs patched into control inputs. Rebinding
        // keeps their readings; outputs no longer sampled lose theirs.
        let sampled: HashSet<(usize, usize)> = self.schedule.sampled_outputs.iter().copied().collect();
        for (i, inst) in self.instances.iter_mut().enumerate() {
            for (k, meter) in inst.meters.iter_mut().enumerate() {
                let Some(tex) = inst.outputs[k].as_ref().filter(|_| sampled.contains(&(i, k))) else {
                    *meter = None;
                    continue;
                };
                let (result, readbacks, value, measured) = match meter.take() {
                    Some(m) => (m.result, m.readbacks, m.value, m.measured),
                    None => {
                        let (result, readbacks) = Meter::buffers(device);
                        (result, readbacks, 0.0, 0)
                    }
                };
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("meter"),
                    layout: &self.meter_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&tex.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: result.as_entire_binding(),
                        },
                    ],
                });
                *meter = Some(Meter {
                    result,
                    bind_group,
                    readbacks,
                    value,
                    measured,
                });
            }
        }
    }

    /// Starts reading back the meters measured in the field just submitted.
    pub fn after_submit(&mut self) {
        let meters = self
            .instances
            .iter_mut()
            .flat_map(|i| i.meters.iter_mut().flatten());
        for rb in meters.flat_map(|m| m.readbacks.iter_mut()) {
            if rb.state.load(Ordering::Acquire) != READBACK_COPIED {
                continue;
            }
            rb.state.store(READBACK_MAPPING, Ordering::Release);
            let state = rb.state.clone();
            rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let next = if r.is_ok() { READBACK_READY } else { READBACK_FREE };
                state.store(next, Ordering::Release);
            });
        }
    }

    fn input_view(&self, i: usize, j: usize) -> &wgpu::TextureView {
        let black = if self.instances[i].input_kinds[j] == PortKind::Composite {
            &self.black_composite
        } else {
            &self.black
        };
        match self.schedule.inputs[i][j] {
            Source::Video {
                module,
                output,
                delayed,
            } => {
                let inst = &self.instances[module];
                let tex = if delayed {
                    &inst.history[output]
                } else {
                    &inst.outputs[output]
                };
                tex.as_ref().map_or(&black.view, |t| &t.view)
            }
            Source::Control { .. } | Source::Sampled { .. } | Source::Default => &black.view,
        }
    }

    /// Bind groups for each pass of module `i`; empty if anything is missing.
    fn bind_groups(&self, device: &wgpu::Device, i: usize) -> Vec<wgpu::BindGroup> {
        let inst = &self.instances[i];
        let Some(passes) = self.pipelines.get(&type_key(inst.ty)) else {
            return Vec::new();
        };
        let mut groups = Vec::with_capacity(passes.len());
        for pass in passes {
            let mut entries = Vec::with_capacity(pass.slots.len());
            for &(binding, slot) in &pass.slots {
                let tv = inst.tv.as_ref();
                let resource = match slot {
                    Slot::Uniforms => inst.uniforms.as_entire_binding(),
                    Slot::Input(j) => wgpu::BindingResource::TextureView(self.input_view(i, j)),
                    Slot::Output(k) => match inst.outputs.get(k).and_then(Option::as_ref) {
                        Some(t) => wgpu::BindingResource::TextureView(&t.view),
                        None => return Vec::new(),
                    },
                    Slot::FeedbackStore => match &inst.store {
                        Some(t) => wgpu::BindingResource::TextureView(&t.view),
                        None => return Vec::new(),
                    },
                    Slot::Frame => {
                        let camera = inst.camera.as_ref().and_then(|c| c.frame.as_ref()).map(|f| &f.1);
                        let picture = inst.picture.as_ref().map(|p| &p.view);
                        wgpu::BindingResource::TextureView(camera.or(picture).unwrap_or(&self.no_frame))
                    }
                    Slot::LinearSampler => wgpu::BindingResource::Sampler(&self.sampler),
                    Slot::TvSync { .. } => match tv {
                        Some(tv) => tv.sync.as_entire_binding(),
                        None => return Vec::new(),
                    },
                    Slot::TvScan => match tv {
                        Some(tv) => tv.scan.as_entire_binding(),
                        None => return Vec::new(),
                    },
                    Slot::TvBeams { .. } => match tv {
                        Some(tv) => tv.beams.as_entire_binding(),
                        None => return Vec::new(),
                    },
                    Slot::TvDecodedWrite | Slot::TvDecodedRead => match tv {
                        Some(tv) => wgpu::BindingResource::TextureView(&tv.decoded),
                        None => return Vec::new(),
                    },
                    Slot::Ring => match &inst.ring {
                        Some(r) => wgpu::BindingResource::TextureView(&r.view),
                        None => return Vec::new(),
                    },
                    Slot::TextSdf => wgpu::BindingResource::TextureView(
                        inst.text.as_ref().map_or(&self.no_text, |t| &t.view),
                    ),
                };
                entries.push(wgpu::BindGroupEntry { binding, resource });
            }
            groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(pass.label),
                layout: &pass.layout,
                entries: &entries,
            }));
        }
        groups
    }

    /// Per-input values the CPU can see: control signals and defaults.
    fn input_values(&self, i: usize) -> Vec<f32> {
        let inst = &self.instances[i];
        self.schedule.inputs[i]
            .iter()
            .enumerate()
            .map(|(j, src)| match *src {
                Source::Control { module, output } => self.instances[module].control_out[output],
                Source::Sampled { module, output } => self.instances[module].meters[output]
                    .as_ref()
                    .map_or(0.0, |m| m.value),
                Source::Default | Source::Video { .. } => inst.input_defaults[j],
            })
            .collect()
    }

    fn run_behavior(&mut self, i: usize, ctx: &FieldCtx) {
        let values = self.input_values(i);
        let inst = &mut self.instances[i];
        let Some(behavior) = inst.behavior.as_mut() else {
            return;
        };
        let slots = inst.params.snapshot();
        let mut io = FieldIo {
            desc: &inst.ty.desc,
            slots: &slots,
            params: &inst.params,
            inputs: &values,
            extra: &mut inst.extra,
            outputs: &mut inst.control_out,
        };
        behavior.field(ctx, &mut io);
    }

    fn uniforms(&self, i: usize, info: FieldInfoGpu, field: u64) -> ModuleUniformsGpu {
        let inst = &self.instances[i];
        let desc = &inst.ty.desc;
        let mut u = ModuleUniformsGpu {
            info,
            extra: inst.extra,
            ..Default::default()
        };
        let slots = inst.params.snapshot();
        let values = self.input_values(i);
        for (s, v) in slots.iter().enumerate().take(MAX_PARAM_SLOTS) {
            let v = if inst.folds_cv() && s < desc.params.len() {
                param_value(desc, &slots, &values, s)
            } else {
                *v
            };
            u.knobs[s / 4][s % 4] = v;
        }
        for (j, src) in self.schedule.inputs[i].iter().enumerate() {
            u.inputs[j] = match *src {
                Source::Video { .. } => [0.0, 0.0, 0.0, 1.0],
                Source::Control { .. } | Source::Sampled { .. } | Source::Default => {
                    let v = values[j];
                    [v, v, v, 0.0]
                }
            };
        }
        if let Some(t) = &inst.text {
            u.extra[6] = [t.size[0], t.size[1], 0.0, 0.0];
            u.extra[7] = t.rect;
        }
        if let Some(r) = &inst.ring {
            u.info.ring_depth = r.depth;
            u.info.ring_head = ((r.writes + u64::from(r.depth) - 1) % u64::from(r.depth)) as u32;
        }
        match desc.kernel {
            Kernel::Feedback => {
                u.extra = feedback_params(inst, &slots, &values);
            }
            Kernel::Camera => {
                if let Some(cam) = &inst.camera {
                    let fit = choice_value(desc, &slots, &values, 1);
                    let mirror = choice_value(desc, &slots, &values, 2) as f32;
                    let cam_aspect = cam.aspect().unwrap_or(16.0 / 9.0);
                    let scale = camera_scale(fit, self.raster.display_aspect, cam_aspect);
                    u.extra[0] = [scale[0], scale[1], mirror, 0.0];
                }
            }
            Kernel::Picture => {
                let choice = |id| {
                    desc.param_index(id)
                        .map_or(0, |k| choice_value(desc, &slots, &values, k))
                };
                let size = inst.picture.as_ref().map(|p| p.texture.size());
                let aspect = size.map_or(16.0 / 9.0, |s| s.width as f32 / s.height.max(1) as f32);
                let scale = camera_scale(choice("fit"), self.raster.display_aspect, aspect);
                u.extra[0] = [scale[0], scale[1], choice("mirror") as f32, 0.0];
            }
            Kernel::Composite(_) => {
                u.extra[4..8].copy_from_slice(&composite_timing(&self.raster, field));
            }
            Kernel::Pointwise(_) | Kernel::LineScan(_) | Kernel::Control => {}
        }
        u
    }

    /// Uploads a picture module's picture when its behavior has a new one.
    fn update_picture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: usize) {
        let inst = &mut self.instances[i];
        let wanted = inst.behavior.as_ref().and_then(|b| b.picture());
        let current = inst.picture.as_ref().map(|p| &p.frame);
        if wanted.as_ref().map(Arc::as_ptr) == current.map(Arc::as_ptr) {
            return;
        }
        let old = inst.picture.take();
        let mut rebind = true;
        if let Some(frame) = wanted {
            let (texture, view) = match old {
                Some(p) if p.texture.width() == frame.width && p.texture.height() == frame.height => {
                    rebind = false;
                    (p.texture, p.view)
                }
                _ => camera_texture(device, frame.width, frame.height),
            };
            queue.write_texture(
                texture.as_image_copy(),
                &frame.bgra,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                texture.size(),
            );
            inst.picture = Some(PictureState { frame, texture, view });
        }
        if rebind {
            self.instances[i].bind_groups = self.bind_groups(device, i);
        }
    }

    /// Renders a text module's text again when it or its font has changed.
    fn update_text(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: usize) {
        let inst = &self.instances[i];
        let desc = &inst.ty.desc;
        if desc.text.is_none() {
            return;
        }
        let font = desc
            .param_index("font")
            .map_or(0, |k| inst.params.get(k).round().max(0.0) as usize);
        let version = inst.params.text_version();
        if inst
            .text
            .as_ref()
            .is_some_and(|t| t.version == version && t.font == font)
        {
            return;
        }
        let field = text::render(&self.fonts.get(font), &inst.params.text());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("text"),
            size: wgpu::Extent3d {
                width: field.width,
                height: field.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texels: Vec<u16> = field
            .texels
            .iter()
            .map(|&d| half::f16::from_f32(d).to_bits())
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(field.width * 2),
                rows_per_image: Some(field.height),
            },
            texture.size(),
        );
        self.instances[i].text = Some(TextState {
            view: texture.create_view(&Default::default()),
            version,
            font,
            size: field.size,
            rect: field.rect,
        });
        self.instances[i].bind_groups = self.bind_groups(device, i);
    }

    /// Starts or switches camera devices, and uploads any new frame.
    fn update_camera(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: usize) {
        let values = self.input_values(i);
        let inst = &mut self.instances[i];
        let wanted = choice_value(&inst.ty.desc, &inst.params.snapshot(), &values, 0) as usize;
        if inst.camera.as_ref().is_none_or(|c| c.source.device != wanted) {
            // Drop the old device first so it's released before the new one opens.
            inst.camera = None;
            inst.camera = Some(CameraState {
                source: CameraSource::start(wanted),
                seen: 0,
                frame: None,
            });
        }
        let Some(cam) = inst.camera.as_mut() else { return };
        let mut resized = false;
        let CameraState { source, seen, frame } = cam;
        source.with_new_frame(seen, |f| {
            let fits = frame
                .as_ref()
                .is_some_and(|(t, _)| t.width() == f.width && t.height() == f.height);
            if !fits {
                *frame = Some(camera_texture(device, f.width, f.height));
                resized = true;
            }
            let Some((texture, _)) = frame.as_ref() else {
                return;
            };
            queue.write_texture(
                texture.as_image_copy(),
                &f.bgra,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(f.width * 4),
                    rows_per_image: Some(f.height),
                },
                texture.size(),
            );
        });
        if resized {
            let bind_groups = self.bind_groups(device, i);
            self.instances[i].bind_groups = bind_groups;
        }
    }

    /// Logs a TV's sync separator and deflection state for the field just
    /// submitted. Blocks on the GPU; for debugging only (`DOV_DEBUG_TV=1`).
    pub fn debug_tv(&self, device: &wgpu::Device, queue: &GpuQueue, field: u64) {
        let read = |src: &wgpu::Buffer| -> Vec<f32> {
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug readback"),
                size: src.size(),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(src, 0, &staging, 0, src.size());
            queue.submit([encoder.finish()]);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            let data = staging
                .slice(..)
                .get_mapped_range()
                .map(|v| bytemuck::cast_slice(&v).to_vec());
            data.unwrap_or_default()
        };
        let raster = self.raster;
        let spl = raster.samples_per_line as f32;
        // The composite signal feeding each TV, as (min, argmin) per half-line.
        for (i, inst) in self.instances.iter().enumerate() {
            if inst.tv.is_none() {
                continue;
            }
            let Source::Video { module, output, .. } = self.schedule.inputs[i][0] else {
                continue;
            };
            let Some(tex) = &self.instances[module].outputs[output] else {
                continue;
            };
            let size = tex.texture.size();
            let row_bytes = (size.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug composite"),
                size: u64::from(row_bytes * size.height),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                tex.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(size.height),
                    },
                },
                size,
            );
            queue.submit([encoder.finish()]);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            let Ok(view) = staging.slice(..).get_mapped_range() else {
                continue;
            };
            let row = |r: u32| -> Vec<f32> {
                let start = (r * row_bytes) as usize;
                bytemuck::cast_slice(&view[start..start + size.width as usize * 4]).to_vec()
            };
            let half = size.width as usize / 2;
            for r in [0u32, 3, 9, 263, 265, 266, 272] {
                let v = row(r);
                let stats = |a: &[f32]| {
                    let (k, m) =
                        a.iter().enumerate().fold(
                            (0, f32::MAX),
                            |(bk, bm), (k, &x)| if x < bm { (k, x) } else { (bk, bm) },
                        );
                    format!("min {m:+.2}@{k}")
                };
                log::info!(
                    "  composite row {r}: [{}] [{}] first {:?}",
                    stats(&v[..half]),
                    stats(&v[half..]),
                    v[..6]
                        .iter()
                        .map(|x| (x * 100.0).round() / 100.0)
                        .collect::<Vec<_>>()
                );
            }
        }
        for tv in self.instances.iter().filter_map(|i| i.tv.as_ref()) {
            let sync = read(&tv.sync);
            let scan = read(&tv.scan);
            let beams = read(&tv.beams);
            let parity = raster.field_parity(field);
            let first_row = composite_timing(&raster, field)[3][2] as usize;
            let rows = |r0: usize| -> String {
                (r0..r0 + 8)
                    .filter_map(|r| sync.get(r * 4..r * 4 + 4))
                    .map(|v| format!("[{:.0} {:.2} {:.2}]", v[0], v[1], v[2]))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            log::info!(
                "tv field {field} (parity {parity}): trig {:.2} lines, integ {:.2}, period {:.2}, {} beams",
                scan[2] / spl,
                scan[3],
                scan[1],
                beams[0]
            );
            log::info!("  sync rows {first_row}..: {}", rows(first_row));
            let active = first_row + raster.first_active_line as usize;
            let beam = beams.chunks(4).skip(1).take(beams[0] as usize).min_by(|a, b| {
                (a[0] / spl - active as f32)
                    .abs()
                    .total_cmp(&(b[0] / spl - active as f32).abs())
            });
            if let Some(b) = beam {
                log::info!("  beam at row {active}: t {:.2} lines, v {:.3}", b[0] / spl, b[1]);
            }
        }
    }

    /// Encodes one field: CPU behaviors, then every GPU pass in order, then
    /// the copies that carry signals into the next field.
    pub fn run_field(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        field: u64,
        offline: bool,
    ) {
        let advanced = self.last_field.map_or(0, |l| field.saturating_sub(l));
        self.last_field = Some(field);
        let _ = device.poll(wgpu::PollType::Poll);
        for meter in self
            .instances
            .iter_mut()
            .flat_map(|i| i.meters.iter_mut().flatten())
        {
            meter.collect();
        }
        let raster = self.raster;
        let ctx = FieldCtx {
            raster: &raster,
            field,
            advanced,
            offline,
        };
        for i in self.schedule.control.clone() {
            self.run_behavior(i, &ctx);
        }
        for i in self.schedule.gpu.clone() {
            self.run_behavior(i, &ctx);
            match self.instances[i].ty.desc.kernel {
                Kernel::Camera => self.update_camera(device, queue, i),
                Kernel::Picture => self.update_picture(device, queue, i),
                _ => {}
            }
            self.update_text(device, queue, i);
        }

        let info = field_info(&raster, field);
        let composite_rows = composite_timing(&raster, field)[3][3] as u32;
        for &i in &self.schedule.gpu {
            let inst = &self.instances[i];
            let Some(passes) = self.pipelines.get(&type_key(inst.ty)) else {
                continue;
            };
            if inst.bind_groups.len() != passes.len() {
                continue;
            }
            queue.write_buffer(
                &inst.uniforms,
                0,
                bytemuck::bytes_of(&self.uniforms(i, info, field)),
            );
            for (pass, bind_group) in passes.iter().zip(&inst.bind_groups) {
                let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(inst.ty.desc.id),
                    timestamp_writes: None,
                });
                cpass.set_pipeline(&pass.pipeline);
                cpass.set_bind_group(0, bind_group, &[]);
                let (x, y) = match pass.dispatch {
                    Dispatch::Pixels => (info.width.div_ceil(16), info.lines.div_ceil(16)),
                    Dispatch::Lines => (info.lines.div_ceil(64), 1),
                    Dispatch::CompositeRows => (composite_rows.div_ceil(64), 1),
                    Dispatch::Single => (1, 1),
                    Dispatch::Beams => (info.width.div_ceil(16), TV_MAX_BEAMS.div_ceil(16)),
                };
                cpass.dispatch_workgroups(x, y, 1);
            }
        }

        // Meters: measure this field, and copy the result out if a staging
        // buffer is free (otherwise this field's reading is skipped).
        for meter in self
            .instances
            .iter_mut()
            .flat_map(|i| i.meters.iter_mut().flatten())
        {
            {
                let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("meter"),
                    timestamp_writes: None,
                });
                cpass.set_pipeline(&self.meter_pipeline);
                cpass.set_bind_group(0, &meter.bind_group, &[]);
                cpass.dispatch_workgroups(1, 1, 1);
            }
            let free = meter
                .readbacks
                .iter_mut()
                .find(|rb| rb.state.load(Ordering::Acquire) == READBACK_FREE);
            if let Some(rb) = free {
                encoder.copy_buffer_to_buffer(&meter.result, 0, &rb.buffer, 0, 16);
                rb.field = field;
                rb.state.store(READBACK_COPIED, Ordering::Release);
            }
        }

        // Histories: store this field unless frozen.
        for &i in &self.schedule.gpu {
            let inst = &self.instances[i];
            let (Some(spec), Some(ring)) = (inst.ty.desc.ring, &inst.ring) else {
                continue;
            };
            let frozen = spec.freeze_param.is_some_and(|p| {
                param_value(&inst.ty.desc, &inst.params.snapshot(), &self.input_values(i), p) >= 0.5
            });
            let Some(src) = inst.outputs.get(spec.output).and_then(Option::as_ref) else {
                continue;
            };
            if frozen {
                continue;
            }
            let layer = (ring.writes % u64::from(ring.depth)) as u32;
            encoder.copy_texture_to_texture(
                src.texture.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &ring.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                src.texture.size(),
            );
            if let Some(ring) = self.instances[i].ring.as_mut() {
                ring.writes += 1;
            }
        }

        for &(m, o) in &self.schedule.delayed_outputs {
            let inst = &self.instances[m];
            if let (Some(src), Some(dst)) = (&inst.outputs[o], &inst.history[o]) {
                encoder.copy_texture_to_texture(
                    src.texture.as_image_copy(),
                    dst.texture.as_image_copy(),
                    src.texture.size(),
                );
            }
        }
        for &i in &self.schedule.gpu {
            let Some(store) = &self.instances[i].store else {
                continue;
            };
            match self.schedule.inputs[i][0] {
                Source::Video {
                    module,
                    output,
                    delayed,
                } => {
                    let src = &self.instances[module];
                    let tex = if delayed {
                        &src.history[output]
                    } else {
                        &src.outputs[output]
                    };
                    if let Some(tex) = tex {
                        encoder.copy_texture_to_texture(
                            tex.texture.as_image_copy(),
                            store.texture.as_image_copy(),
                            store.texture.size(),
                        );
                    }
                }
                Source::Control { .. } | Source::Sampled { .. } | Source::Default => {
                    let v = f64::from(self.uniforms(i, info, field).inputs[0][0]);
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("store fill"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &store.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: v,
                                    g: v,
                                    b: v,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                }
            }
        }
    }
}

fn field_info(raster: &RasterStd, field: u64) -> FieldInfoGpu {
    let (width, height) = raster.active_size();
    FieldInfoGpu {
        width,
        height,
        lines: raster.active_lines_per_field,
        row_stride: raster.fields_per_frame(),
        parity: raster.field_parity(field),
        field: field as u32,
        ring_depth: 0,
        ring_head: 0,
        aspect: raster.display_aspect,
        sample_rate: raster.sample_rate_hz as f32,
        line_rate: raster.line_rate_hz() as f32,
        field_rate: raster.field_rate_hz() as f32,
    }
}

/// Composite-domain timing for `field`, as `u.extra[4..8]` (see common.wgsl).
pub(crate) fn composite_timing(raster: &RasterStd, field: u64) -> [[f32; 4]; 4] {
    let lines = raster.lines_per_frame;
    let field2_row = if raster.interlaced {
        lines.div_ceil(2)
    } else {
        lines
    };
    let (row0, rows) = match raster.field_parity(field) {
        0 => (0, field2_row),
        _ => (field2_row, lines - field2_row),
    };
    let line_fract = raster.subcarrier_cycles_per_line().rem_euclid(1.0);
    let sample_inc = raster.subcarrier_hz / raster.sample_rate_hz;
    [
        [f32::from(u8::from(raster.interlaced)), 0.0, 0.0, 0.0],
        [
            line_fract as f32,
            sample_inc as f32,
            f32::from(u8::from(raster.pal)),
            0.0,
        ],
        [
            raster.active_start_sample as f32,
            raster.active_samples as f32,
            raster.first_active_line as f32,
            field2_row as f32,
        ],
        [
            raster.samples_per_line as f32,
            lines as f32,
            row0 as f32,
            rows as f32,
        ],
    ]
}

/// Frame store params for `feedback.wgsl`, which applies their CV per sample:
/// knob value and CV scale for zoom, rotate, x, y and gain, then the mirror.
fn feedback_params(inst: &Instance, slots: &[f32], inputs: &[f32]) -> [[f32; 4]; EXTRA_SLOTS] {
    let desc = &inst.ty.desc;
    let mut extra = inst.extra;
    for (k, e) in extra.iter_mut().enumerate().take(5) {
        let scale = desc
            .cv_slot(k)
            .map_or(0.0, |(_, atten)| slots[atten] * desc.params[k].span());
        *e = [slots[k], scale, 0.0, 0.0];
    }
    extra[5] = [choice_value(desc, slots, inputs, 5) as f32, 0.0, 0.0, 0.0];
    extra
}

fn camera_texture(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("camera frame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CAMERA_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// UV scale mapping the raster onto a camera frame: 0 fill (crop),
/// 1 fit (letterbox), 2 stretch.
fn camera_scale(fit: u32, raster_aspect: f32, camera_aspect: f32) -> [f32; 2] {
    let ratio = raster_aspect / camera_aspect;
    match fit {
        0 if ratio < 1.0 => [ratio, 1.0],
        0 => [1.0, 1.0 / ratio],
        1 if ratio < 1.0 => [1.0, 1.0 / ratio],
        1 => [ratio, 1.0],
        _ => [1.0, 1.0],
    }
}

/// The passes a module type runs, with their bindings.
fn pass_specs(ty: &ModuleType) -> Result<Vec<PassSpec>, String> {
    use Slot::*;
    let desc = &ty.desc;
    let outputs = |slots: &mut Vec<(u32, Slot)>| {
        slots.extend((0..desc.outputs.len()).map(|k| (OUTPUT_BINDING_BASE + k as u32, Output(k))));
    };
    let composite = |label, parts: &[&str], inputs: usize| {
        let mut slots = vec![(0, Uniforms)];
        slots.extend((0..inputs).map(|j| (INPUT_BINDING_BASE + j as u32, Input(j))));
        outputs(&mut slots);
        PassSpec {
            label,
            source: codegen::composite_shader(parts),
            slots,
            dispatch: Dispatch::CompositeRows,
        }
    };
    Ok(match desc.kernel {
        Kernel::Pointwise(_) | Kernel::LineScan(_) => {
            let mut slots = vec![(0, Uniforms)];
            slots.extend((0..desc.input_count()).map(|j| (INPUT_BINDING_BASE + j as u32, Input(j))));
            outputs(&mut slots);
            if desc.ring.is_some() {
                slots.push((RING_BINDING, Ring));
            }
            if desc.text.is_some() {
                slots.push((TEXT_BINDING, TextSdf));
                slots.push((TEXT_SAMPLER_BINDING, LinearSampler));
            }
            vec![PassSpec {
                label: desc.id,
                source: codegen::module_shader(desc)?,
                slots,
                dispatch: if matches!(desc.kernel, Kernel::LineScan(_)) {
                    Dispatch::Lines
                } else {
                    Dispatch::Pixels
                },
            }]
        }
        Kernel::Feedback => {
            // The store stands in for In (input 0); the CV inputs follow it.
            let mut slots = vec![(0, Uniforms), (INPUT_BINDING_BASE, FeedbackStore)];
            slots.extend((1..desc.input_count()).map(|j| (INPUT_BINDING_BASE + j as u32, Input(j))));
            outputs(&mut slots);
            vec![PassSpec {
                label: desc.id,
                source: codegen::feedback_shader(),
                slots,
                dispatch: Dispatch::Pixels,
            }]
        }
        Kernel::Camera | Kernel::Picture => vec![PassSpec {
            label: desc.id,
            source: codegen::camera_shader(),
            slots: vec![(0, Uniforms), (1, Frame), (2, LinearSampler), (16, Output(0))],
            dispatch: Dispatch::Pixels,
        }],
        Kernel::Composite(op) => {
            use codegen::composite as c;
            match op {
                CompositeOp::Encode => vec![composite("encoder", &[c::ENCODE], 1)],
                CompositeOp::Inject => vec![composite("inject", &[c::READ, c::INJECT], 2)],
                CompositeOp::Cable => vec![composite("cable", &[c::READ, c::CABLE], 1)],
                CompositeOp::Vcr => vec![composite("vcr", &[c::READ, c::VCR], 1)],
                CompositeOp::Tbc => vec![composite("tbc", &[c::READ, c::TBC], 1)],
                CompositeOp::Tv => vec![
                    PassSpec {
                        label: "tv sync",
                        source: codegen::composite_shader(&[c::READ, c::TV_COMMON, c::TV_SYNC]),
                        slots: vec![(0, Uniforms), (1, Input(0)), (20, TvSync { write: true })],
                        dispatch: Dispatch::CompositeRows,
                    },
                    PassSpec {
                        label: "tv scan",
                        source: codegen::composite_shader(&[c::TV_SCAN]),
                        slots: vec![
                            (0, Uniforms),
                            (20, TvSync { write: false }),
                            (21, TvScan),
                            (22, TvBeams { write: true }),
                        ],
                        dispatch: Dispatch::Single,
                    },
                    PassSpec {
                        label: "tv decode",
                        source: codegen::composite_shader(&[c::READ, c::TV_COMMON, c::TV_DECODE]),
                        slots: vec![(0, Uniforms), (1, Input(0)), (23, TvDecodedWrite)],
                        dispatch: Dispatch::CompositeRows,
                    },
                    PassSpec {
                        label: "tv draw",
                        source: codegen::composite_shader(&[c::TV_DRAW]),
                        slots: vec![
                            (0, Uniforms),
                            (22, TvBeams { write: false }),
                            (24, TvDecodedRead),
                            (16, Output(0)),
                        ],
                        dispatch: Dispatch::Beams,
                    },
                ],
            }
        }
        Kernel::Control => Vec::new(),
    })
}

fn meter_pipeline(device: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::ComputePipeline) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("meter"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("meter"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("meter"),
        source: wgpu::ShaderSource::Wgsl(codegen::METER_SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("meter"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (layout, pipeline)
}

fn layout_entry(binding: u32, slot: Slot, ty: &ModuleType) -> wgpu::BindGroupLayoutEntry {
    let texture = |filterable| wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    let storage_texture = |format| wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format,
        view_dimension: wgpu::TextureViewDimension::D2,
    };
    let storage_buffer = |read_only| wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let ty = match slot {
        Slot::Uniforms => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        Slot::Input(_) | Slot::FeedbackStore | Slot::TvDecodedRead => texture(false),
        Slot::Output(k) => storage_texture(match ty.desc.outputs[k].kind {
            PortKind::Composite => COMPOSITE_FORMAT,
            PortKind::Mono | PortKind::Rgb | PortKind::Control => SIGNAL_FORMAT,
        }),
        Slot::Frame | Slot::TextSdf => texture(true),
        Slot::LinearSampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        Slot::TvSync { write } | Slot::TvBeams { write } => storage_buffer(!write),
        Slot::TvScan => storage_buffer(false),
        Slot::TvDecodedWrite => storage_texture(SIGNAL_FORMAT),
        Slot::Ring => wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

/// Module types are compared by identity: a reloaded module file is a new
/// type with the same id.
fn type_key(ty: &'static ModuleType) -> usize {
    std::ptr::from_ref(ty) as usize
}

/// Compiles a module type's passes. A module that fails (which the module
/// format's checks should prevent) gets no passes, and its outputs stay
/// black rather than taking the app down.
fn build_passes(device: &wgpu::Device, ty: &'static ModuleType) -> Vec<KernelPass> {
    let specs = match pass_specs(ty) {
        Ok(specs) => specs,
        Err(e) => {
            log::error!("{e}");
            return Vec::new();
        }
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let passes: Vec<KernelPass> = specs
        .into_iter()
        .map(|spec| {
            let entries: Vec<_> = spec
                .slots
                .iter()
                .map(|&(binding, slot)| layout_entry(binding, slot, ty))
                .collect();
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(spec.label),
                entries: &entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(spec.label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(spec.label),
                source: wgpu::ShaderSource::Wgsl(spec.source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(spec.label),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            KernelPass {
                label: spec.label,
                layout,
                pipeline,
                slots: spec.slots,
                dispatch: spec.dispatch,
            }
        })
        .collect();
    // Native wgpu resolves error scopes immediately.
    if let Some(e) = pollster::block_on(scope.pop()) {
        log::error!("{}: shader failed to compile, so it won't run: {e}", ty.desc.id);
        return Vec::new();
    }
    passes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_fill_crops_and_fit_letterboxes() {
        // 16:9 camera into a 4:3 raster.
        let (raster, cam) = (4.0 / 3.0, 16.0 / 9.0);
        let fill = camera_scale(0, raster, cam);
        assert!((fill[0] - 0.75).abs() < 1e-6 && fill[1] == 1.0, "{fill:?}");
        let fit = camera_scale(1, raster, cam);
        assert!(fit[0] == 1.0 && (fit[1] - 4.0 / 3.0).abs() < 1e-6, "{fit:?}");
        assert_eq!(camera_scale(2, raster, cam), [1.0, 1.0]);
    }

    #[test]
    fn composite_fields_split_the_frame() {
        let ntsc = RasterStd::NTSC;
        let f1 = composite_timing(&ntsc, 0);
        let f2 = composite_timing(&ntsc, 1);
        assert_eq!((f1[3][2], f1[3][3]), (0.0, 263.0));
        assert_eq!((f2[3][2], f2[3][3]), (263.0, 262.0));
        assert_eq!(f1[1][0], 0.5, "NTSC subcarrier advances half a cycle per line");
        let hd = composite_timing(&RasterStd::HD1080P60, 0);
        assert_eq!((hd[3][2], hd[3][3]), (0.0, 1125.0));
        assert_eq!(hd[1][1], 0.0, "no subcarrier in HD");
    }

    #[test]
    fn every_pass_spec_builds_valid_wgsl() {
        for m in dov_modules::all() {
            for spec in pass_specs(m).unwrap() {
                let module = naga::front::wgsl::parse_str(&spec.source)
                    .unwrap_or_else(|e| panic!("{}: {}", spec.label, e.emit_to_string(&spec.source)));
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all(),
                )
                .validate(&module)
                .unwrap_or_else(|e| panic!("{}: {}", spec.label, e.emit_to_string(&spec.source)));
            }
        }
    }
}
