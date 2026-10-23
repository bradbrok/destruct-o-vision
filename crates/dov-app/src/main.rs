//! Destruct-O-Vision: the rack window (egui, main thread) and the output window
//! (presented by the engine thread).

mod art;
mod cli;
mod demo;
mod dialog;
mod modules;
mod movie_editor;
mod panel;
mod rack;
mod rack_view;
mod ui;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dov_core::{ModuleId, Patch, RasterStd};
use dov_engine::capture::Capture;
use dov_engine::gpu::GpuQueue;
use dov_engine::{Command, EngineEvent, EngineHandle, IntervalTracker, Previews, TempoSync};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::rack::Rack;
use crate::rack_view::ProbeView;
use crate::ui::{AppUi, PATCH_EXT, UiAction};

/// How often a hidden rack window checks whether it became visible.
const OCCLUDED_PROBE: Duration = Duration::from_millis(250);
/// Patch files bigger than this aren't opened.
const MAX_PATCH_BYTES: u64 = 16 << 20;
/// How long a status message stays in the status line.
const STATUS_TIME: Duration = Duration::from_secs(4);

const USAGE: &str = "\
usage: destructovision [options]
  --patch FILE           open a patch (default: the demo patch)
  --demo NAME            start from a built-in patch: showcase (the default) | feedback | analog |
                         time | lab | camera | audio | empty,
                         or tv[:stage,...], e.g. tv:vcr,cable (Encoder -> stages -> TV)
  --set TYPE:PARAM=VAL   set a param on every module of a type, e.g. dov.vcr:tracking=0.5
  --raster NAME          ntsc | pal | 720p | 1080p (default ntsc)
  --sync NAME            where the tempo comes from: internal (default) | link | midi
  --ui-load MS           simulated UI work per frame, for pacing tests
  --snapshot-after SECS  save the output and the UI to captures/ after SECS
  --quit-after SECS      log pacing stats and quit after SECS
  --record DIR           render the output as numbered PNGs in DIR (every field, offline
                         and smooth, as shown on the display), then quit
  --record-seconds SECS  how much to record (default 10)
  --movie SECS           record a movie of the output as it plays, as Output > Record movie
                         does, for SECS, then quit
  --modules DIR          load user modules from DIR (default: ./modules, or
                         ~/Documents/Destruct-O-Vision/modules for the app bundle)

Module authors: destructovision module check | new | glue (run it for details)";

/// Command-line options, mostly for automated checks.
#[derive(Default)]
struct Options {
    patch: Option<PathBuf>,
    demo: Option<String>,
    sets: Vec<(String, String, f32)>,
    raster: Option<RasterStd>,
    sync: Option<TempoSync>,
    ui_load_ms: f32,
    snapshot_after: Option<f32>,
    quit_after: Option<f32>,
    modules: Option<PathBuf>,
    record: Option<PathBuf>,
    record_seconds: f32,
    movie: Option<f32>,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut opts = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("{arg} needs a value"));
            let mut secs =
                || -> Result<f32, String> { value()?.parse().map_err(|_| format!("{arg} expects a number")) };
            match arg.as_str() {
                "--patch" => opts.patch = Some(PathBuf::from(value()?)),
                "--demo" => opts.demo = Some(value()?),
                "--set" => {
                    let v = value()?;
                    let parsed = v.split_once(':').and_then(|(ty, rest)| {
                        let (param, val) = rest.split_once('=')?;
                        Some((ty.to_string(), param.to_string(), val.parse().ok()?))
                    });
                    opts.sets
                        .push(parsed.ok_or(format!("--set expects TYPE:PARAM=VALUE, got '{v}'"))?);
                }
                "--raster" => {
                    opts.raster = Some(match value()?.as_str() {
                        "ntsc" => RasterStd::NTSC,
                        "pal" => RasterStd::PAL,
                        "720p" => RasterStd::HD720P60,
                        "1080p" => RasterStd::HD1080P60,
                        other => return Err(format!("unknown raster '{other}'")),
                    })
                }
                "--sync" => {
                    opts.sync = Some(match value()?.as_str() {
                        "internal" => TempoSync::Internal,
                        "link" => TempoSync::Link,
                        "midi" => TempoSync::MidiClock,
                        other => return Err(format!("unknown sync '{other}'")),
                    })
                }
                "--ui-load" => opts.ui_load_ms = secs()?,
                "--snapshot-after" => opts.snapshot_after = Some(secs()?),
                "--quit-after" => opts.quit_after = Some(secs()?),
                "--modules" => opts.modules = Some(PathBuf::from(value()?)),
                "--record" => opts.record = Some(PathBuf::from(value()?)),
                "--record-seconds" => opts.record_seconds = secs()?,
                "--movie" => opts.movie = Some(secs()?),
                "-h" | "--help" => return Err(String::new()),
                other => return Err(format!("unknown option '{other}'")),
            }
        }
        Ok(opts)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "module") {
        std::process::exit(cli::module(&args[1..], &ui::data_dir().join("modules")));
    }
    let options = match Options::parse() {
        Ok(options) => options,
        Err(err) => {
            if !err.is_empty() {
                eprintln!("error: {err}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut App { options, state: None })?;
    Ok(())
}

struct App {
    options: Options,
    state: Option<AppState>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match AppState::new(event_loop, &self.options) {
            Ok(state) => self.state = Some(state),
            Err(err) => {
                log::error!("startup failed: {err}");
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else { return };
        if state.rack_window.occluded {
            // Vsync doesn't pace a hidden window. Probe slowly instead of
            // spinning; a successful redraw clears the occluded state.
            state.run_scripted(event_loop, &self.options);
            let next_probe = state.rack_window.last_probe + OCCLUDED_PROBE;
            if Instant::now() >= next_probe {
                state.rack_window.last_probe = Instant::now();
                state.rack_window.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(next_probe.max(Instant::now())));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        if id == state.rack_window.window.id() {
            state.rack_event(event_loop, event, &self.options);
        } else if id == state.output.window.id() {
            state.output_event(event);
        }
    }
}

struct RackWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    occluded: bool,
    last_probe: Instant,
}

struct OutputWindow {
    window: Arc<Window>,
    shown: bool,
    occluded: bool,
    refresh_hz: Option<f64>,
}

impl OutputWindow {
    fn visible(&self) -> bool {
        self.shown && !self.occluded
    }

    /// Refresh rate of the window's display, if it changed since last asked.
    fn refresh_changed(&mut self) -> Option<f64> {
        let hz = self
            .window
            .current_monitor()?
            .refresh_rate_millihertz()
            .map(|mhz| f64::from(mhz) / 1000.0)?;
        (self.refresh_hz != Some(hz)).then(|| {
            self.refresh_hz = Some(hz);
            hz
        })
    }
}

/// UI texture ids for the engine's previews.
struct PreviewIds {
    output: egui::TextureId,
    probe: egui::TextureId,
    aspect: f32,
}

struct AppState {
    device: wgpu::Device,
    queue: GpuQueue,
    rack_window: RackWindow,
    output: OutputWindow,
    engine: EngineHandle,
    previews: PreviewIds,
    rack: Rack,
    ui: AppUi,
    probe: Option<(ModuleId, usize)>,
    /// UI textures for in-panel screens.
    screens: HashMap<ModuleId, egui::TextureId>,
    status: Option<(String, Instant)>,
    ui_frames: IntervalTracker,
    started: Instant,
    snapshot_taken: bool,
    quitting: bool,
    /// Where to save the next rendered UI frame.
    ui_snapshot: Option<PathBuf>,
    ui_captures: Vec<Capture>,
    /// The user's modules folder, watched for changes.
    modules: modules::UserModules,
    /// A `--record` run has finished: quit.
    recorded: bool,
    /// The movie being recorded, and since when.
    movie: Option<(PathBuf, Instant)>,
    /// A `--movie` run: whether it has started and been stopped.
    scripted_movie: (bool, bool),
    /// Where the tempo comes from (the engine follows), and the tempo last
    /// sent it.
    sync: TempoSync,
    sent_tempo: Option<f64>,
}

impl AppState {
    fn new(event_loop: &ActiveEventLoop, options: &Options) -> Result<Self, Box<dyn std::error::Error>> {
        // Before any patch opens, so its modules are there.
        let user_modules = modules::UserModules::new(
            options
                .modules
                .clone()
                .unwrap_or_else(|| ui::data_dir().join("modules")),
        );
        let rack_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Destruct-O-Vision")
                    .with_inner_size(LogicalSize::new(1440.0, 900.0)),
            )?,
        );
        let output_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Destruct-O-Vision · Output")
                    .with_inner_size(LogicalSize::new(960.0, 720.0)),
            )?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let rack_surface = instance.create_surface(rack_window.clone())?;
        let output_surface = instance.create_surface(output_window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&rack_surface),
            ..Default::default()
        }))?;
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("dov"),
            ..Default::default()
        }))?;
        let queue = GpuQueue::new(queue);

        let rack_config = surface_config(&rack_surface, &adapter, rack_window.inner_size());
        queue.configure(&rack_surface, &device, &rack_config);
        let output_config = surface_config(&output_surface, &adapter, output_window.inner_size());

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx,
            egui::ViewportId::ROOT,
            &rack_window,
            Some(rack_window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let mut renderer = egui_wgpu::Renderer::new(&device, rack_config.format, Default::default());

        let raster = options.raster.unwrap_or(RasterStd::NTSC);
        let (engine, previews) = EngineHandle::spawn(device.clone(), queue.clone(), raster);
        engine.send(Command::SetOutput {
            surface: output_surface,
            config: output_config,
        });
        let previews = PreviewIds {
            output: renderer.register_native_texture(
                &device,
                &previews.output.view,
                wgpu::FilterMode::Linear,
            ),
            probe: renderer.register_native_texture(&device, &previews.probe.view, wgpu::FilterMode::Linear),
            aspect: previews.output.aspect,
        };

        let mut state = Self {
            device,
            queue,
            rack_window: RackWindow {
                window: rack_window,
                surface: rack_surface,
                config: rack_config,
                egui: egui_state,
                renderer,
                occluded: false,
                last_probe: Instant::now(),
            },
            output: OutputWindow {
                window: output_window,
                shown: true,
                occluded: false,
                refresh_hz: None,
            },
            engine,
            previews,
            rack: match options.demo.as_deref() {
                Some("analog") => demo::analog(),
                Some("time") => demo::time(),
                Some("lab") => demo::lab(),
                Some(d) if d == "tv" || d.starts_with("tv:") => {
                    let stages: Vec<String> = d
                        .strip_prefix("tv:")
                        .map(|s| s.split(',').map(|k| format!("dov.{k}")).collect())
                        .unwrap_or_default();
                    demo::tv(&stages.iter().map(String::as_str).collect::<Vec<_>>())
                }
                Some("camera") => demo::camera(),
                Some("audio") => demo::audio(),
                Some("empty") => demo::empty(),
                Some("feedback") => demo::demo(),
                _ => demo::showcase(),
            },
            ui: AppUi {
                load_ms: options.ui_load_ms,
                ..Default::default()
            },
            probe: None,
            screens: HashMap::new(),
            status: None,
            ui_frames: IntervalTracker::new(120),
            started: Instant::now(),
            snapshot_taken: false,
            quitting: false,
            ui_snapshot: None,
            movie: None,
            scripted_movie: (false, false),
            sync: TempoSync::Internal,
            sent_tempo: None,
            ui_captures: Vec::new(),
            modules: user_modules,
            recorded: false,
        };
        if let Some(path) = &options.patch {
            state.open(path);
        }
        for (ty, param, value) in &options.sets {
            let mut found = false;
            for m in state.rack.modules.iter().filter(|m| m.ty.desc.id == ty) {
                if let Some(slot) = m.ty.desc.param_index(param) {
                    m.params.set(slot, *value);
                    found = true;
                }
            }
            if !found {
                log::warn!("--set: no module of type {ty} with param {param}");
            }
        }
        if let Some(hz) = state.output.refresh_changed() {
            log::info!("output display: {hz:.2} Hz");
            state.engine.send(Command::DisplayRefresh(hz));
        }
        state.sync_engine();
        if let Some(sync) = options.sync {
            state.apply(UiAction::SetSync(sync));
        }
        if let Some(dir) = &options.record {
            let seconds = if options.record_seconds > 0.0 {
                options.record_seconds
            } else {
                10.0
            };
            state.engine.send(Command::Record {
                dir: dir.clone(),
                seconds: f64::from(seconds),
            });
        }
        Ok(state)
    }

    fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::info!("{message}");
        self.status = Some((message, Instant::now()));
    }

    /// Puts reloaded module types into the rack, and says what happened.
    fn modules_changed(&mut self) {
        for w in self.rack.reload_types() {
            log::warn!("{w}");
        }
        self.rack.reflow();
        let errors = self.modules.errors.len();
        let message = match errors {
            0 => format!(
                "Modules reloaded: {} from {}",
                self.modules.count(),
                self.modules.dir.display()
            ),
            n => format!("Modules reloaded, {n} with errors: see the Modules menu"),
        };
        self.set_status(message);
    }

    /// Sends the patch to the engine if its structure changed.
    fn sync_engine(&mut self) {
        if self.rack.take_dirty() {
            self.engine.send(Command::SetPatch(self.rack.snapshot()));
        }
        // A patch's own tempo, when it runs on its own: opening one in a
        // Link session doesn't change everyone's tempo.
        let tempo = self.rack.tempo();
        if self.sync == TempoSync::Internal && self.sent_tempo != Some(tempo) {
            self.engine.send(Command::SetTempo(tempo));
            self.sent_tempo = Some(tempo);
        }
    }

    fn open(&mut self, path: &Path) {
        // Patches get shared, so they're read like any other untrusted file.
        let loaded = dov_format::read_text_file(path, MAX_PATCH_BYTES)
            .and_then(|text| Patch::from_ron(&text).map_err(|e| e.to_string()));
        match loaded {
            Ok(patch) => {
                let (mut rack, warnings) = Rack::from_patch(&patch);
                for w in &warnings {
                    log::warn!("{}: {w}", path.display());
                }
                rack.path = Some(path.to_path_buf());
                self.rack = rack;
                if let Some(std) = patch
                    .raster
                    .and_then(|n| RasterStd::ALL.into_iter().find(|s| s.name == n))
                {
                    self.engine.send(Command::SetRaster(std));
                }
                self.ui.rack_view.fit();
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(err) => self.set_status(format!("Couldn't open {}: {err}", path.display())),
        }
    }

    fn save(&mut self, path: PathBuf) {
        let mut patch = self.rack.to_patch();
        patch.raster = Some(self.engine.stats().raster.name.to_string());
        let written = patch.to_ron().map_err(|e| e.to_string()).and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, text).map_err(|e| e.to_string())
        });
        match written {
            Ok(()) => {
                self.rack.path = Some(path.clone());
                self.rack.modified = false;
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(err) => self.set_status(format!("Couldn't save {}: {err}", path.display())),
        }
    }

    fn rack_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent, options: &Options) {
        let response = self
            .rack_window
            .egui
            .on_window_event(&self.rack_window.window, &event);
        // egui asks to repaint after every RedrawRequested; honoring that
        // would loop forever. The redraw itself schedules the next one.
        let wants_repaint = response.repaint && !matches!(event, WindowEvent::RedrawRequested);
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop),
            WindowEvent::Resized(size) => {
                self.rack_window.config.width = size.width.max(1);
                self.rack_window.config.height = size.height.max(1);
                self.queue
                    .configure(&self.rack_window.surface, &self.device, &self.rack_window.config);
                self.rack_window.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.run_scripted(event_loop, options);
                self.redraw_rack();
            }
            WindowEvent::Occluded(occluded) => {
                self.rack_window.occluded = occluded;
                if !occluded {
                    self.rack_window.window.request_redraw();
                }
            }
            _ => {}
        }
        if wants_repaint && !self.rack_window.occluded {
            self.rack_window.window.request_redraw();
        }
    }

    /// Timed actions requested on the command line.
    fn run_scripted(&mut self, event_loop: &ActiveEventLoop, options: &Options) {
        let elapsed = self.started.elapsed().as_secs_f32();
        if !self.snapshot_taken && options.snapshot_after.is_some_and(|t| elapsed >= t) {
            self.snapshot_taken = true;
            self.apply(UiAction::Snapshot);
        }
        if let Some(seconds) = options.movie {
            match self.scripted_movie {
                (false, _) => {
                    self.scripted_movie.0 = true;
                    self.apply(UiAction::StartMovie(dov_io::Codec::H264));
                }
                (true, false) if elapsed >= seconds => {
                    self.scripted_movie.1 = true;
                    self.apply(UiAction::StopMovie);
                }
                _ => {}
            }
        }
        if self.recorded || options.quit_after.is_some_and(|t| elapsed >= t) {
            self.quit(event_loop);
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if std::mem::replace(&mut self.quitting, true) {
            return;
        }
        let s = self.engine.stats();
        let f = s.field_interval;
        let p = s.present_interval;
        let u = self.ui_frames.summary();
        log::info!(
            "pacing: {} | {} modules, {} passes | fields {} rendered, {} dropped, {} repeated | \
             field {:.2}±{:.2} ms | present {:.2}±{:.2} ms [{:.2}–{:.2}] | acquire wait {:.2}±{:.2} ms, \
             {} occluded | ui frame {:.2} ms [{:.2}–{:.2}] | ui load {} ms | tempo {:.2} BPM, beat {:.2} \
             ({:?}, {} peers, clock {})",
            s.raster.name,
            s.modules,
            s.passes,
            s.fields_rendered,
            s.fields_dropped,
            s.fields_repeated,
            f.mean_ms,
            f.jitter_ms,
            p.mean_ms,
            p.jitter_ms,
            p.min_ms,
            p.max_ms,
            s.acquire_wait.mean_ms,
            s.acquire_wait.jitter_ms,
            s.acquire_occluded,
            u.mean_ms,
            u.min_ms,
            u.max_ms,
            self.ui.load_ms,
            s.tempo.bpm,
            s.tempo.beats,
            s.tempo.sync,
            s.tempo.peers,
            if s.tempo.receiving { "in" } else { "none" },
        );
        self.engine.shutdown();
        event_loop.exit();
    }

    fn output_event(&mut self, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) && let Some(hz) = self.output.refresh_changed()
        {
            log::info!("output display: {hz:.2} Hz");
            self.engine.send(Command::DisplayRefresh(hz));
        }
        match event {
            WindowEvent::Resized(size) => self.engine.send(Command::ResizeOutput {
                width: size.width,
                height: size.height,
            }),
            WindowEvent::CloseRequested => {
                self.output.shown = false;
                self.output.window.set_visible(false);
                self.sync_output_visibility();
            }
            WindowEvent::Occluded(occluded) => {
                self.output.occluded = occluded;
                self.sync_output_visibility();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => self.output.window.set_fullscreen(None),
                Key::Character(c) if c.eq_ignore_ascii_case("f") => {
                    let window = &self.output.window;
                    let fullscreen = match window.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(window.current_monitor())),
                    };
                    window.set_fullscreen(fullscreen);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn sync_output_visibility(&self) {
        self.engine.send(Command::OutputVisible(self.output.visible()));
    }

    fn apply(&mut self, action: UiAction) {
        match action {
            UiAction::SetRaster(std) => self.engine.send(Command::SetRaster(std)),
            UiAction::ShowOutput => {
                self.output.shown = true;
                self.output.window.set_visible(true);
                self.output.window.focus_window();
                self.sync_output_visibility();
            }
            UiAction::Fullscreen(index) => {
                let monitor = self.output.window.available_monitors().nth(index);
                self.output
                    .window
                    .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
            }
            UiAction::Windowed => self.output.window.set_fullscreen(None),
            UiAction::Snapshot => {
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let dir = ui::data_dir().join("captures");
                self.engine
                    .send(Command::Capture(dir.join(format!("dov-{ms}.png"))));
                self.ui_snapshot = Some(dir.join(format!("dov-{ms}-ui.png")));
            }
            UiAction::StartMovie(codec) => {
                if self.movie.is_none() {
                    let ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    let path = ui::data_dir()
                        .join("captures")
                        .join(format!("dov-{ms}.{}", codec.extension()));
                    self.engine.send(Command::StartMovie {
                        path: path.clone(),
                        codec,
                    });
                    self.set_status(format!("Recording to {}", path.display()));
                    self.movie = Some((path, Instant::now()));
                }
            }
            UiAction::SetTempo(bpm) => {
                self.rack.set_tempo(bpm);
                self.engine.send(Command::SetTempo(bpm));
                self.sent_tempo = Some(bpm);
            }
            UiAction::SetSync(sync) => {
                self.engine.send(Command::SetSync(sync));
                // Back on its own, the patch keeps the tempo it was synced to.
                if sync == TempoSync::Internal && self.sync != TempoSync::Internal {
                    let bpm = (self.engine.stats().tempo.bpm * 10.0).round() / 10.0;
                    if bpm > 0.0 {
                        self.rack.set_tempo(bpm);
                    }
                }
                self.sync = sync;
                let message = match sync {
                    TempoSync::Internal => "Tempo: this patch's own",
                    TempoSync::Link => "Tempo: Ableton Link. Turn Link on in Live's settings to join",
                    TempoSync::MidiClock => {
                        "Tempo: MIDI clock from any device. In Live, turn Sync on for an output"
                    }
                };
                self.set_status(message);
            }
            UiAction::StopMovie => {
                if self.movie.is_some() {
                    self.engine.send(Command::StopMovie);
                    self.set_status("Finishing the movie…");
                }
            }
            UiAction::NewPatch => {
                self.rack = demo::empty();
                self.ui.rack_view.fit();
            }
            UiAction::LoadShowcase => {
                self.rack = demo::showcase();
                self.ui.rack_view.fit();
            }
            UiAction::LoadDemo => {
                self.rack = demo::demo();
                self.ui.rack_view.fit();
            }
            UiAction::LoadTimeDemo => {
                self.rack = demo::time();
                self.ui.rack_view.fit();
            }
            UiAction::OpenModulesFolder => {
                let dir = self.modules.dir.clone();
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| reveal(&dir)) {
                    self.set_status(format!("Couldn't open {}: {e}", dir.display()));
                }
            }
            UiAction::ReloadModules => {
                self.modules.reload();
                self.modules_changed();
            }
            UiAction::NewModule => {
                let dir = self.modules.dir.clone();
                let free = (1..)
                    .map(|n| match n {
                        1 => "my-module".to_string(),
                        n => format!("my-module-{n}"),
                    })
                    .find(|name| !dir.join(format!("{name}.wgsl")).exists())
                    .unwrap_or_default();
                let path = dir.join(format!("{free}.wgsl"));
                let written = std::fs::create_dir_all(&dir)
                    .and_then(|()| std::fs::write(&path, modules::template(&format!("local.{free}"), false)));
                match written {
                    Ok(()) => {
                        let _ = reveal(&path);
                        self.modules.reload();
                        self.modules_changed();
                        self.set_status(format!(
                            "Created {}: edit it and save, and the module reloads (Add > Processors)",
                            path.display()
                        ));
                    }
                    Err(e) => self.set_status(format!("Couldn't create {}: {e}", path.display())),
                }
            }
            UiAction::LoadLabDemo => {
                self.rack = demo::lab();
                self.ui.rack_view.fit();
            }
            UiAction::LoadAnalogDemo => {
                self.rack = demo::analog();
                self.ui.rack_view.fit();
            }
            UiAction::LoadCameraDemo => {
                self.rack = demo::camera();
                self.ui.rack_view.fit();
            }
            UiAction::LoadAudioDemo => {
                self.rack = demo::audio();
                self.ui.rack_view.fit();
            }
            UiAction::Open(path) => self.open(&path),
            UiAction::Save => match self.rack.path.clone() {
                Some(path) => self.save(path),
                None => self.ui.save_as = Some(ui::patch_name(&self.rack)),
            },
            UiAction::SaveAs(name) => {
                let file = Path::new(&name).with_extension(PATCH_EXT);
                self.save(ui::patch_dir().join(file));
            }
            UiAction::ChooseMovie(id) => self.choose_movie(id),
        }
    }

    /// Asks for a movie file for the Movie module `id`.
    fn choose_movie(&mut self, id: ModuleId) {
        let Some(params) = self.rack.module(id).map(|m| m.params.clone()) else {
            return;
        };
        let current = params.text();
        if let Some(path) = dialog::choose_movie(&current).filter(|p| *p != current) {
            self.rack.push_undo(self.rack.to_patch());
            params.set_text(&path);
        }
    }

    fn redraw_rack(&mut self) {
        let frame_start = Instant::now();
        self.ui_frames.tick(frame_start);
        if !self.ui_captures.is_empty() {
            let _ = self.device.poll(wgpu::PollType::Poll);
            self.ui_captures.retain(|c| !c.try_finish());
        }

        while let Some(event) = self.engine.try_event() {
            match event {
                EngineEvent::MovieSaved(saved) => {
                    self.movie = None;
                    match saved {
                        Ok(w) => {
                            let dropped = match w.dropped {
                                0 => String::new(),
                                n => format!(", {n} dropped keeping up"),
                            };
                            self.set_status(format!(
                                "Saved {} ({} frames{dropped})",
                                w.path.display(),
                                w.frames
                            ));
                        }
                        Err(e) => self.set_status(format!("Couldn't record the movie: {e}")),
                    }
                    if self.scripted_movie.1 {
                        self.recorded = true;
                    }
                }
                EngineEvent::Recorded(dir, frames) => {
                    self.set_status(format!("Recorded {frames} frames to {}", dir.display()));
                    self.recorded = true;
                }
                EngineEvent::Previews(Previews { output, probe }) => {
                    let renderer = &mut self.rack_window.renderer;
                    renderer.update_egui_texture_from_wgpu_texture(
                        &self.device,
                        &output.view,
                        wgpu::FilterMode::Linear,
                        self.previews.output,
                    );
                    renderer.update_egui_texture_from_wgpu_texture(
                        &self.device,
                        &probe.view,
                        wgpu::FilterMode::Linear,
                        self.previews.probe,
                    );
                    self.previews.aspect = output.aspect;
                }
                EngineEvent::Screens(list) => {
                    let renderer = &mut self.rack_window.renderer;
                    let keep: Vec<ModuleId> = list.iter().map(|(id, _)| *id).collect();
                    self.screens.retain(|id, texture| {
                        let k = keep.contains(id);
                        if !k {
                            renderer.free_texture(texture);
                        }
                        k
                    });
                    for (id, preview) in list {
                        match self.screens.get(&id) {
                            Some(&texture) => renderer.update_egui_texture_from_wgpu_texture(
                                &self.device,
                                &preview.view,
                                wgpu::FilterMode::Linear,
                                texture,
                            ),
                            None => {
                                let texture = renderer.register_native_texture(
                                    &self.device,
                                    &preview.view,
                                    wgpu::FilterMode::Linear,
                                );
                                self.screens.insert(id, texture);
                            }
                        }
                    }
                }
            }
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > STATUS_TIME)
        {
            self.status = None;
        }

        if self.modules.poll() {
            self.modules_changed();
        }
        let monitors: Vec<String> = self
            .output
            .window
            .available_monitors()
            .enumerate()
            .map(|(i, m)| m.name().unwrap_or_else(|| format!("Display {}", i + 1)))
            .collect();
        let mut stats = self.engine.stats();
        let controls = std::mem::take(&mut stats.controls).into_iter().collect();
        let notes = std::mem::take(&mut stats.notes).into_iter().collect();
        let timelines = std::mem::take(&mut stats.timelines).into_iter().collect();
        let frame = ui::FrameData {
            stats,
            output_preview: self.previews.output,
            probe: ProbeView {
                texture: self.previews.probe,
                aspect: self.previews.aspect,
                screens: self.screens.clone(),
                controls,
                notes,
                timelines,
            },
            probing: self.probe.is_some(),
            monitors: &monitors,
            output_shown: self.output.shown,
            ui_frames: self.ui_frames.summary(),
            status: self.status.as_ref().map(|(s, _)| s.as_str()),
            movie: self.movie.as_ref().map(|(_, since)| since.elapsed()),
            modules: ui::ModulesView {
                dir: &self.modules.dir,
                count: self.modules.count(),
                errors: &self.modules.errors,
            },
        };
        let mut actions = Vec::new();
        let ctx = self.rack_window.egui.egui_ctx().clone();
        let raw_input = self.rack_window.egui.take_egui_input(&self.rack_window.window);
        let (ui_state, rack) = (&mut self.ui, &mut self.rack);
        let mut probe = None;
        let output = ctx.run_ui(raw_input, |ui| {
            probe = ui_state.show(ui, rack, &frame, &mut actions)
        });
        drop(frame);
        for action in actions {
            self.apply(action);
        }
        self.sync_engine();
        if probe != self.probe {
            self.probe = probe;
            self.engine.send(Command::Probe(probe));
        }
        let title = format!(
            "Destruct-O-Vision — {}{}",
            ui::patch_name(&self.rack),
            if self.rack.modified { " •" } else { "" }
        );
        if self.rack_window.window.title() != title {
            self.rack_window.window.set_title(&title);
        }

        // Simulated UI cost, to check that output pacing doesn't depend on it.
        let load = Duration::from_secs_f32(self.ui.load_ms / 1000.0);
        if let Some(remaining) = load.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(remaining);
        }

        let egui::FullOutput {
            platform_output,
            mut textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = output;
        self.rack_window
            .egui
            .handle_platform_output(&self.rack_window.window, platform_output);
        let paint_jobs = ctx.tessellate(shapes, pixels_per_point);
        for (id, deltas) in textures_delta.set.drain() {
            for delta in &deltas {
                self.rack_window
                    .renderer
                    .update_texture(&self.device, self.queue.raw(), id, delta);
            }
        }
        self.paint_rack(&paint_jobs, pixels_per_point);
        if let Some(path) = self.ui_snapshot.take() {
            self.capture_ui(&paint_jobs, pixels_per_point, path);
        }
        for id in textures_delta.free.drain() {
            self.rack_window.renderer.free_texture(&id);
        }

        // Previews are live, so keep redrawing at display rate while visible.
        if !self.rack_window.occluded {
            self.rack_window.window.request_redraw();
        }
    }

    /// Renders the UI offscreen and saves it, whether or not the window is visible.
    fn capture_ui(&mut self, paint_jobs: &[egui::ClippedPrimitive], pixels_per_point: f32, path: PathBuf) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui capture"),
            size: wgpu::Extent3d {
                width: self.rack_window.config.width,
                height: self.rack_window.config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.rack_window.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let extra = self.encode_egui(&mut encoder, &view, paint_jobs, pixels_per_point);
        let capture = Capture::encode(&self.device, &mut encoder, &texture, path);
        self.queue.submit(extra.into_iter().chain([encoder.finish()]));
        capture.map();
        self.ui_captures.push(capture);
    }

    fn encode_egui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        paint_jobs: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Vec<wgpu::CommandBuffer> {
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.rack_window.config.width, self.rack_window.config.height],
            pixels_per_point,
        };
        let extra = self.rack_window.renderer.update_buffers(
            &self.device,
            self.queue.raw(),
            encoder,
            paint_jobs,
            &screen,
        );
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        self.rack_window.renderer.render(&mut pass, paint_jobs, &screen);
        extra
    }

    fn paint_rack(&mut self, paint_jobs: &[egui::ClippedPrimitive], pixels_per_point: f32) {
        let surface_texture = match self.rack_window.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Occluded => {
                // winit only reports occlusion changes, so a window that
                // starts hidden is only detected here.
                self.rack_window.occluded = true;
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.queue
                    .configure(&self.rack_window.surface, &self.device, &self.rack_window.config);
                return;
            }
            _ => return,
        };
        self.rack_window.occluded = false;
        let view = surface_texture.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let extra = self.encode_egui(&mut encoder, &view, paint_jobs, pixels_per_point);
        self.queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.queue.raw().present(surface_texture);
    }
}

fn surface_config(
    surface: &wgpu::Surface,
    adapter: &wgpu::Adapter,
    size: PhysicalSize<u32>,
) -> wgpu::SurfaceConfiguration {
    let caps = surface.get_capabilities(adapter);
    // Non-sRGB target: signals are already gamma-encoded, like analog video.
    let format = egui_wgpu::preferred_framebuffer_format(&caps.formats).unwrap_or(caps.formats[0]);
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Auto,
    }
}

/// Shows a file or folder in the system file browser.
fn reveal(path: &Path) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        if path.is_file() {
            c.arg("-R");
        }
        c
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(path).spawn().map(drop)
}
