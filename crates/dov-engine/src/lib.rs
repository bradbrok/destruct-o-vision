//! The real-time engine. It runs on its own thread, paced by the raster
//! standard's field clock and the output display, so UI work never stalls
//! the picture.

pub mod capture;
pub mod codegen;
mod engine;
mod flywheel;
pub mod gpu;
pub mod graph;
mod runtime;
pub mod stats;
mod text;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use dov_core::{ModuleId, RasterStd};

pub use graph::{PatchSnapshot, SnapshotCable, SnapshotModule};
pub use stats::{EngineStats, IntervalSummary, IntervalTracker};

/// Messages from the UI thread to the engine.
pub enum Command {
    /// Hand over the output window's surface; the engine configures and presents it.
    SetOutput {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    ResizeOutput {
        width: u32,
        height: u32,
    },
    /// Refresh rate of the display the output window is on.
    DisplayRefresh(f64),
    /// Whether the output window is shown and not occluded.
    OutputVisible(bool),
    SetRaster(RasterStd),
    /// Replace the running patch. Module state carries over by id.
    SetPatch(PatchSnapshot),
    /// Mirror one module output into the probe preview.
    Probe(Option<(ModuleId, usize)>),
    /// Save the next woven frame (at storage resolution) as a PNG.
    Capture(PathBuf),
    /// Render `seconds` of fields as fast as possible, one after another
    /// whatever the clock says, saving each as it appears on the display to
    /// `dir/frame-NNNNN.png`. For demo videos: nothing is dropped or repeated.
    Record {
        dir: PathBuf,
        seconds: f64,
    },
    Shutdown,
}

/// Messages from the engine to the UI thread.
pub enum EngineEvent {
    /// Preview textures were replaced (the raster changed); re-register them.
    Previews(Previews),
    /// The set of in-panel screens changed; register these, drop the rest.
    Screens(Vec<(ModuleId, Preview)>),
    /// A recording finished: this many frames were saved to the folder.
    Recorded(PathBuf, u64),
}

/// A texture the UI can display, updated by the engine every field.
#[derive(Clone)]
pub struct Preview {
    pub view: wgpu::TextureView,
    pub size: [u32; 2],
    pub aspect: f32,
}

#[derive(Clone)]
pub struct Previews {
    /// What the Output module receives.
    pub output: Preview,
    /// The output selected with [`Command::Probe`].
    pub probe: Preview,
}

pub struct EngineHandle {
    stats: Arc<Mutex<EngineStats>>,
    commands: Sender<Command>,
    events: Receiver<EngineEvent>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Starts the engine thread. Returns the handle and the initial previews.
    pub fn spawn(device: wgpu::Device, queue: gpu::GpuQueue, raster: RasterStd) -> (Self, Previews) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stats = Arc::new(Mutex::new(EngineStats::default()));
        let (engine, previews) =
            engine::Engine::new(device, queue, raster, command_rx, event_tx, stats.clone());
        let thread = std::thread::Builder::new()
            .name("dov-engine".into())
            .spawn(move || engine.run())
            .expect("failed to spawn engine thread");
        let handle = Self {
            stats,
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
        };
        (handle, previews)
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            log::warn!("engine is not running");
        }
    }

    pub fn try_event(&self) -> Option<EngineEvent> {
        self.events.try_recv().ok()
    }

    pub fn stats(&self) -> EngineStats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Stops the engine thread and waits for it. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.commands.send(Command::Shutdown);
            if thread.join().is_err() {
                log::error!("engine thread panicked");
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}
