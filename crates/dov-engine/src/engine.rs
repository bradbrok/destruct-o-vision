//! The engine thread: owns the field clock, runs the patch, presents output.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dov_core::{ModuleId, RasterStd};

use crate::capture::Capture;
use crate::flywheel::Flywheel;
use crate::gpu::{self, BlitBinding, Blitter, GpuQueue};
use crate::runtime::Runtime;
use crate::stats::{EngineStats, IntervalTracker};
use crate::{Command, EngineEvent, Preview, Previews};

const STATS_WINDOW: usize = 240;
const DEFAULT_DISPLAY_HZ: f64 = 60.0;

/// Maps wall-clock time onto field indices of the current raster standard.
struct FieldClock {
    start: Instant,
    period: f64,
}

impl FieldClock {
    fn new(std: &RasterStd) -> Self {
        Self {
            start: Instant::now(),
            period: std.field_period(),
        }
    }

    fn field_at(&self, t: Instant) -> u64 {
        ((t - self.start).as_secs_f64() / self.period) as u64
    }

    fn time_of(&self, field: u64) -> Instant {
        self.start + Duration::from_secs_f64(field as f64 * self.period)
    }
}

/// An `Rgba8Unorm` copy of a signal that the UI can display.
struct PreviewTex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Size of the in-panel screens.
const SCREEN_SIZE: [u32; 2] = [240, 180];

impl PreviewTex {
    fn new(device: &wgpu::Device, std: &RasterStd, label: &str) -> Self {
        let (width, height) = std.active_size();
        Self::sized(device, width, height, label)
    }

    fn sized(device: &wgpu::Device, width: u32, height: u32, label: &str) -> Self {
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
            format: gpu::PREVIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }

    fn preview(&self, std: &RasterStd) -> Preview {
        let (w, h) = std.active_size();
        Preview {
            view: self.view.clone(),
            size: [w, h],
            aspect: std.display_aspect,
        }
    }
}

/// Blit sources; rebuilt whenever the signals behind them change.
/// A module's in-panel screen.
struct Screen {
    id: ModuleId,
    preview: PreviewTex,
    binding: BlitBinding,
}

struct Bindings {
    output: BlitBinding,
    preview: BlitBinding,
    probe: Option<BlitBinding>,
}

struct Output {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    visible: bool,
    needs_configure: bool,
}

/// A recording in progress (see `Command::Record`).
struct Recording {
    dir: PathBuf,
    total: u64,
    done: u64,
}

pub(crate) struct Engine {
    device: wgpu::Device,
    queue: GpuQueue,
    commands: Receiver<Command>,
    events: Sender<EngineEvent>,
    stats: Arc<Mutex<EngineStats>>,

    raster: RasterStd,
    clock: FieldClock,
    /// Timeline origin for the flywheel.
    epoch: Instant,
    flywheel: Flywheel,
    display_hz_hint: f64,
    /// Last rendered field index.
    field: Option<u64>,

    runtime: Runtime,
    blitter: Blitter,
    output_preview: PreviewTex,
    probe_preview: PreviewTex,
    bindings: Bindings,
    probe: Option<(ModuleId, usize)>,
    screens: Vec<Screen>,
    output: Option<Output>,
    capture_requests: Vec<PathBuf>,
    captures: Vec<Capture>,
    recording: Option<Recording>,

    fields_rendered: u64,
    fields_dropped: u64,
    fields_repeated: u64,
    presents: u64,
    encode_ms: f32,
    presented: bool,
    acquire_waits: IntervalTracker,
    acquire_occluded: u64,
    acquire_timeout: u64,
    field_intervals: IntervalTracker,
    present_intervals: IntervalTracker,
    debug_tv: bool,
}

impl Engine {
    pub(crate) fn new(
        device: wgpu::Device,
        queue: GpuQueue,
        raster: RasterStd,
        commands: Receiver<Command>,
        events: Sender<EngineEvent>,
        stats: Arc<Mutex<EngineStats>>,
    ) -> (Self, Previews) {
        let mut blitter = Blitter::new(&device);
        blitter.prepare(&device, gpu::PREVIEW_FORMAT);
        let runtime = Runtime::new(&device, raster);
        let output_preview = PreviewTex::new(&device, &raster, "output preview");
        let probe_preview = PreviewTex::new(&device, &raster, "probe preview");
        let bindings = bind(&device, &runtime, &blitter, None);
        let previews = Previews {
            output: output_preview.preview(&raster),
            probe: probe_preview.preview(&raster),
        };
        let engine = Self {
            clock: FieldClock::new(&raster),
            epoch: Instant::now(),
            flywheel: Flywheel::new(1.0 / DEFAULT_DISPLAY_HZ),
            display_hz_hint: DEFAULT_DISPLAY_HZ,
            device,
            queue,
            commands,
            events,
            stats,
            raster,
            field: None,
            runtime,
            blitter,
            output_preview,
            probe_preview,
            bindings,
            probe: None,
            screens: Vec::new(),
            output: None,
            capture_requests: Vec::new(),
            captures: Vec::new(),
            recording: None,
            fields_rendered: 0,
            fields_dropped: 0,
            fields_repeated: 0,
            presents: 0,
            encode_ms: 0.0,
            presented: false,
            acquire_waits: IntervalTracker::new(STATS_WINDOW),
            acquire_occluded: 0,
            acquire_timeout: 0,
            field_intervals: IntervalTracker::new(STATS_WINDOW),
            present_intervals: IntervalTracker::new(STATS_WINDOW),
            debug_tv: std::env::var_os("DOV_DEBUG_TV").is_some(),
        };
        (engine, previews)
    }

    pub(crate) fn run(mut self) {
        raise_thread_priority();
        log::info!("engine running: {}", self.raster.name);
        while self.handle_commands() {
            if self.recording.is_some() {
                self.record_field();
                continue;
            }
            // Blocks until the display can take a frame when presenting with vsync.
            let frame = self.acquire_output();
            let now = Instant::now();
            // Presenting: pick the field for when this frame will actually be
            // on screen, from the flywheel rather than the jittery acquire time.
            let shown_at = match frame {
                Some(_) => {
                    let t = self.flywheel.track((now - self.epoch).as_secs_f64());
                    self.epoch + Duration::from_secs_f64(t.max(0.0))
                }
                None => now,
            };
            let target = self.clock.field_at(shown_at);

            let encode_start = Instant::now();
            let mut encoder = self.device.create_command_encoder(&Default::default());
            match self.field {
                Some(last) if target <= last => {
                    self.fields_repeated += 1;
                    log::debug!("field {last} repeated");
                }
                last => {
                    let advanced = last.map_or(0, |l| target - l);
                    if advanced > 1 {
                        log::debug!("fields {}..{target} dropped", target - advanced + 1);
                    }
                    self.fields_dropped += advanced.saturating_sub(1);
                    self.encode_field(&mut encoder, target);
                    self.field = Some(target);
                    self.fields_rendered += 1;
                    self.field_intervals.tick(now);
                }
            }
            let frame = frame.inspect(|frame| self.encode_output(&mut encoder, frame));
            let mut new_captures = Vec::new();
            for path in std::mem::take(&mut self.capture_requests) {
                let display = path.with_file_name(format!(
                    "{}-display.png",
                    path.file_stem()
                        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
                ));
                new_captures.push(self.encode_display_capture(&mut encoder, display));
                new_captures.push(Capture::encode(
                    &self.device,
                    &mut encoder,
                    &self.output_preview.texture,
                    path,
                ));
            }
            self.queue.submit([encoder.finish()]);
            self.runtime.after_submit();
            self.encode_ms = encode_start.elapsed().as_secs_f32() * 1000.0;
            if self.debug_tv && (120..124).contains(&self.fields_rendered) {
                self.runtime.debug_tv(&self.device, &self.queue, target);
            }
            for capture in &new_captures {
                capture.map();
            }
            self.captures.extend(new_captures);
            if !self.captures.is_empty() {
                let _ = self.device.poll(wgpu::PollType::Poll);
                self.captures.retain(|c| !c.try_finish());
            }

            self.presented = frame.is_some();
            match frame {
                Some(frame) => {
                    self.queue.raw().present(frame);
                    self.presents += 1;
                    self.present_intervals.tick(Instant::now());
                }
                None => {
                    // Not presenting: pace ourselves on the field clock instead.
                    self.present_intervals.reset();
                    self.flywheel.reset(1.0 / self.display_hz_hint);
                    let next = self.clock.time_of(target + 1);
                    std::thread::sleep(next.saturating_duration_since(Instant::now()));
                }
            }
            self.publish_stats();
        }
        log::info!("engine stopped");
    }

    /// Renders the next field of a recording and saves it, without
    /// presenting: every field is rendered exactly once, so the frames make
    /// a smooth video however long each takes.
    fn record_field(&mut self) {
        let Some(mut rec) = self.recording.take() else {
            return;
        };
        let target = self.field.map_or(0, |f| f + 1);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_field(&mut encoder, target);
        self.field = Some(target);
        self.fields_rendered += 1;
        let path = rec.dir.join(format!("frame-{:05}.png", rec.done));
        let capture = self.encode_display_capture(&mut encoder, path);
        self.queue.submit([encoder.finish()]);
        self.runtime.after_submit();
        if let Err(e) = capture.finish_now(&self.device) {
            log::error!("recording frame {}: {e}", rec.done);
        }
        rec.done += 1;
        if rec.done < rec.total {
            self.recording = Some(rec);
        } else {
            log::info!("recorded {} frames to {}", rec.done, rec.dir.display());
            let _ = self.events.send(EngineEvent::Recorded(rec.dir, rec.done));
        }
        self.publish_stats();
    }

    /// Returns false on shutdown.
    fn handle_commands(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::SetOutput { surface, config } => {
                    self.blitter.prepare(&self.device, config.format);
                    self.output = Some(Output {
                        surface,
                        config,
                        visible: true,
                        needs_configure: true,
                    });
                }
                Command::ResizeOutput { width, height } => {
                    if let Some(out) = &mut self.output {
                        out.config.width = width.max(1);
                        out.config.height = height.max(1);
                        out.needs_configure = true;
                    }
                }
                Command::DisplayRefresh(hz) if hz > 0.0 => {
                    self.display_hz_hint = hz;
                    self.flywheel.reset(1.0 / hz);
                }
                Command::DisplayRefresh(_) => {}
                Command::OutputVisible(visible) => {
                    if let Some(out) = &mut self.output {
                        out.visible = visible;
                    }
                }
                Command::SetRaster(std) => self.set_raster(std),
                Command::SetPatch(snapshot) => {
                    let start = Instant::now();
                    self.runtime.set_patch(&self.device, &snapshot);
                    self.bindings = bind(&self.device, &self.runtime, &self.blitter, self.probe);
                    self.rebuild_screens();
                    log::debug!(
                        "patch: {} modules, {} passes, built in {:.2} ms",
                        self.runtime.module_count(),
                        self.runtime.pass_count(),
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                }
                Command::Probe(probe) => {
                    self.probe = probe;
                    self.bindings.probe = bind_probe(&self.device, &self.runtime, &self.blitter, probe);
                }
                Command::Capture(path) => self.capture_requests.push(path),
                Command::Record { dir, seconds } => {
                    let total = (seconds * self.raster.field_rate_hz()).round().max(1.0) as u64;
                    log::info!("recording {total} fields to {}", dir.display());
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        log::error!("recording to {}: {e}", dir.display());
                    } else {
                        self.recording = Some(Recording { dir, total, done: 0 });
                    }
                }
                Command::Shutdown => return false,
            }
        }
        true
    }

    fn set_raster(&mut self, std: RasterStd) {
        if std == self.raster {
            return;
        }
        self.raster = std;
        self.clock = FieldClock::new(&std);
        self.field = None;
        self.runtime.set_raster(&self.device, std);
        self.output_preview = PreviewTex::new(&self.device, &std, "output preview");
        self.probe_preview = PreviewTex::new(&self.device, &std, "probe preview");
        self.bindings = bind(&self.device, &self.runtime, &self.blitter, self.probe);
        self.rebuild_screens();
        self.field_intervals.reset();
        let previews = Previews {
            output: self.output_preview.preview(&std),
            probe: self.probe_preview.preview(&std),
        };
        // The UI may have gone away during shutdown; nothing to do then.
        let _ = self.events.send(EngineEvent::Previews(previews));
        log::info!("raster: {}", std.name);
    }

    fn acquire_output(&mut self) -> Option<wgpu::SurfaceTexture> {
        let out = self.output.as_mut()?;
        if !out.visible {
            return None;
        }
        if out.needs_configure {
            self.queue.configure(&out.surface, &self.device, &out.config);
            out.needs_configure = false;
        }
        let wait_start = Instant::now();
        let acquired = out.surface.get_current_texture();
        self.acquire_waits
            .push(wait_start.elapsed().as_secs_f32() * 1000.0);
        match acquired {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                out.needs_configure = true;
                Some(frame)
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.acquire_timeout += 1;
                None
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.acquire_occluded += 1;
                log::debug!("output occluded");
                None
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                out.needs_configure = true;
                None
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("output surface lost; output disabled");
                self.output = None;
                None
            }
        }
    }

    fn encode_field(&mut self, encoder: &mut wgpu::CommandEncoder, field: u64) {
        self.runtime
            .run_field(&self.device, self.queue.raw(), encoder, field);
        let b = &self.bindings;
        self.blitter.encode(
            self.queue.raw(),
            encoder,
            &b.preview,
            gpu::PREVIEW_FORMAT,
            &self.output_preview.view,
            gpu::BlitGpu::plain([1.0, 1.0]),
        );
        if let Some(probe) = &b.probe {
            self.blitter.encode(
                self.queue.raw(),
                encoder,
                probe,
                gpu::PREVIEW_FORMAT,
                &self.probe_preview.view,
                gpu::BlitGpu::plain([1.0, 1.0]),
            );
        }
        let [w, h] = SCREEN_SIZE;
        let scale = gpu::letterbox(self.raster.display_aspect, w, h);
        for s in &self.screens {
            self.blitter.encode(
                self.queue.raw(),
                encoder,
                &s.binding,
                gpu::PREVIEW_FORMAT,
                &s.preview.view,
                gpu::BlitGpu::plain(scale),
            );
        }
    }

    /// Keeps one screen per module that has one, reusing textures the UI
    /// already registered, and tells the UI when the set changes.
    fn rebuild_screens(&mut self) {
        let mut old: std::collections::HashMap<ModuleId, PreviewTex> =
            self.screens.drain(..).map(|s| (s.id, s.preview)).collect();
        let mut changed = false;
        for id in self.runtime.screen_modules() {
            let Some(view) = self.runtime.port_view(id, 0) else {
                continue;
            };
            let preview = old.remove(&id).unwrap_or_else(|| {
                changed = true;
                let [w, h] = SCREEN_SIZE;
                PreviewTex::sized(&self.device, w, h, "screen")
            });
            let binding = self.blitter.bind(&self.device, view);
            self.screens.push(Screen { id, preview, binding });
        }
        if changed || !old.is_empty() {
            let [w, h] = SCREEN_SIZE;
            let list = self
                .screens
                .iter()
                .map(|s| {
                    let preview = Preview {
                        view: s.preview.view.clone(),
                        size: [w, h],
                        aspect: w as f32 / h as f32,
                    };
                    (s.id, preview)
                })
                .collect();
            let _ = self.events.send(EngineEvent::Screens(list));
        }
    }

    fn encode_output(&self, encoder: &mut wgpu::CommandEncoder, frame: &wgpu::SurfaceTexture) {
        let Some(out) = &self.output else { return };
        let view = frame.texture.create_view(&Default::default());
        let uniforms = self.display_uniforms(out.config.width, out.config.height);
        self.blitter.encode(
            self.queue.raw(),
            encoder,
            &self.bindings.output,
            out.config.format,
            &view,
            uniforms,
        );
    }

    /// How the output window draws the picture, from the Output module's params.
    fn display_uniforms(&self, width: u32, height: u32) -> gpu::BlitGpu {
        let scale = gpu::letterbox(self.raster.display_aspect, width, height);
        let Some(p) = self
            .runtime
            .output_params()
            .filter(|p| p.len() >= 6 && p[0] >= 0.5)
        else {
            return gpu::BlitGpu::plain(scale);
        };
        let (w, h) = self.raster.active_size();
        gpu::BlitGpu {
            scale,
            mode: 1.0,
            mask_type: p[1].round(),
            mask: p[2],
            scanlines: p[3],
            glow: p[4],
            curvature: p[5],
            src_size: [w as f32, h as f32],
            dst_size: [width as f32, height as f32],
        }
    }

    /// Renders what the output window shows, at 1080 lines, for a capture.
    fn encode_display_capture(&self, encoder: &mut wgpu::CommandEncoder, path: PathBuf) -> Capture {
        let height = 1080;
        let width = (height as f32 * self.raster.display_aspect).round() as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu::PREVIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = self.blitter.bind(&self.device, self.runtime.output_view());
        let uniforms = self.display_uniforms(width, height);
        self.blitter.encode(
            self.queue.raw(),
            encoder,
            &binding,
            gpu::PREVIEW_FORMAT,
            &view,
            uniforms,
        );
        Capture::encode(&self.device, encoder, &texture, path)
    }

    fn publish_stats(&self) {
        // Never block the engine on the UI; skip this update if it's reading.
        let Ok(mut s) = self.stats.try_lock() else { return };
        s.raster = self.raster;
        s.field = self.field.unwrap_or(0);
        s.fields_rendered = self.fields_rendered;
        s.fields_dropped = self.fields_dropped;
        s.fields_repeated = self.fields_repeated;
        s.presents = self.presents;
        s.presenting = self.presented;
        s.field_interval = self.field_intervals.summary();
        s.present_interval = self.present_intervals.summary();
        s.encode_ms = self.encode_ms;
        s.acquire_wait = self.acquire_waits.summary();
        s.acquire_occluded = self.acquire_occluded;
        s.acquire_timeout = self.acquire_timeout;
        s.display_hz = 1.0 / self.flywheel.period();
        s.modules = self.runtime.module_count();
        s.passes = self.runtime.pass_count();
        s.warnings = self.runtime.warnings();
        s.controls = self.runtime.control_values();
        s.recent_present_ms.clear();
        s.recent_present_ms.extend(self.present_intervals.samples());
    }
}

fn bind(
    device: &wgpu::Device,
    runtime: &Runtime,
    blitter: &Blitter,
    probe: Option<(ModuleId, usize)>,
) -> Bindings {
    let output = runtime.output_view();
    Bindings {
        output: blitter.bind(device, output),
        preview: blitter.bind(device, output),
        probe: bind_probe(device, runtime, blitter, probe),
    }
}

fn bind_probe(
    device: &wgpu::Device,
    runtime: &Runtime,
    blitter: &Blitter,
    probe: Option<(ModuleId, usize)>,
) -> Option<BlitBinding> {
    let (id, port) = probe?;
    Some(blitter.bind(device, runtime.port_view(id, port)?))
}

/// Ask the OS to treat the engine like the UI thread, not background work,
/// so its timers aren't coalesced or deprioritized.
fn raise_thread_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the QoS class of the calling thread.
    unsafe {
        let err = libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
        if err != 0 {
            log::warn!("could not raise engine thread QoS: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_clock_round_trips() {
        let clock = FieldClock::new(&RasterStd::NTSC);
        for field in [0, 1, 59, 60, 3_600, 216_000] {
            // Half a field in, to stay clear of rounding at the boundary.
            let t = clock.time_of(field) + Duration::from_secs_f64(clock.period * 0.5);
            assert_eq!(clock.field_at(t), field);
        }
    }
}
