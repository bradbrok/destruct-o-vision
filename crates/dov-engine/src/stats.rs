//! Telemetry published by the engine for the UI: pacing, warnings and
//! control signal levels.

use std::collections::VecDeque;
use std::time::Instant;

use dov_core::{ModuleId, RasterStd};
use dov_modules::Timeline;

#[derive(Clone, Copy, Debug, Default)]
pub struct IntervalSummary {
    pub mean_ms: f32,
    pub min_ms: f32,
    pub max_ms: f32,
    /// Standard deviation.
    pub jitter_ms: f32,
}

/// Rolling window of intervals between successive ticks.
#[derive(Debug)]
pub struct IntervalTracker {
    last: Option<Instant>,
    samples: VecDeque<f32>,
    capacity: usize,
}

impl IntervalTracker {
    pub fn new(capacity: usize) -> Self {
        Self {
            last: None,
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if let Some(last) = self.last {
            self.push((now - last).as_secs_f32() * 1000.0);
        }
        self.last = Some(now);
    }

    /// Records a duration directly instead of an interval between ticks.
    pub fn push(&mut self, ms: f32) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    /// Forget history, e.g. when the pacing source changes.
    pub fn reset(&mut self) {
        self.last = None;
        self.samples.clear();
    }

    pub fn samples(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().copied()
    }

    pub fn summary(&self) -> IntervalSummary {
        if self.samples.is_empty() {
            return IntervalSummary::default();
        }
        let n = self.samples.len() as f32;
        let mean = self.samples.iter().sum::<f32>() / n;
        let var = self.samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n;
        IntervalSummary {
            mean_ms: mean,
            min_ms: self.samples.iter().copied().fold(f32::INFINITY, f32::min),
            max_ms: self.samples.iter().copied().fold(0.0, f32::max),
            jitter_ms: var.sqrt(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EngineStats {
    pub raster: RasterStd,
    pub field: u64,
    pub fields_rendered: u64,
    /// Field indices skipped because the engine woke up late.
    pub fields_dropped: u64,
    /// Presents that showed the same field again (display faster than f_V).
    pub fields_repeated: u64,
    pub presents: u64,
    pub presenting: bool,
    pub field_interval: IntervalSummary,
    pub present_interval: IntervalSummary,
    /// CPU time spent encoding and submitting GPU work for the last iteration.
    pub encode_ms: f32,
    /// Time blocked waiting for an output drawable.
    pub acquire_wait: IntervalSummary,
    pub acquire_occluded: u64,
    pub acquire_timeout: u64,
    /// The flywheel's estimate of the output display's refresh rate.
    pub display_hz: f64,
    pub modules: usize,
    /// GPU passes per field.
    pub passes: usize,
    /// Problems found while scheduling the patch, and device trouble.
    pub warnings: Vec<String>,
    /// Each module's control outputs, as of the last field.
    pub controls: Vec<(ModuleId, Vec<f32>)>,
    /// Notes from modules about what they're doing, such as a movie's loop.
    pub notes: Vec<(ModuleId, String)>,
    /// Where each movie module is in its movie.
    pub timelines: Vec<(ModuleId, Timeline)>,
    pub recent_present_ms: Vec<f32>,
}

impl Default for EngineStats {
    fn default() -> Self {
        Self {
            raster: RasterStd::NTSC,
            field: 0,
            fields_rendered: 0,
            fields_dropped: 0,
            fields_repeated: 0,
            presents: 0,
            presenting: false,
            field_interval: IntervalSummary::default(),
            present_interval: IntervalSummary::default(),
            encode_ms: 0.0,
            acquire_wait: IntervalSummary::default(),
            acquire_occluded: 0,
            acquire_timeout: 0,
            display_hz: 0.0,
            modules: 0,
            passes: 0,
            warnings: Vec::new(),
            controls: Vec::new(),
            notes: Vec::new(),
            timelines: Vec::new(),
            recent_present_ms: Vec::new(),
        }
    }
}
