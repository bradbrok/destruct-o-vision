//! Vector: audio drawn by the beam of an XY display (`Kernel::Vector`).

use std::collections::VecDeque;

use crate::audio::AudioTap;
use crate::{Behavior, FieldCtx, FieldIo};

pub(crate) static MODES: [&str; 2] = ["XY", "Scope"];

// Params, in the order `library::VECTOR` declares them.
const MODE: usize = 0;
const X_CHANNEL: usize = 1;
const Y_CHANNEL: usize = 2;
const SIZE: usize = 3;
const X: usize = 4;
const Y: usize = 5;
const TIME: usize = 10;

/// Points the engine draws through in a field, at most (`VECTOR_MAX_POINTS`).
const MAX_POINTS: usize = 16384;
/// The audio Scope mode keeps to find a trigger in and draw from.
const SCOPE_SECONDS: f32 = 0.5;

#[derive(Default)]
pub(crate) struct Vector {
    tap: AudioTap,
    frames: Vec<f32>,
    /// Scope mode: the latest audio on Y, newest last.
    recent: VecDeque<f32>,
    /// x, y pairs for the beam to go through.
    points: Vec<f32>,
    /// Where the beam was at the end of the last field, in XY mode.
    last: Option<[f32; 2]>,
}

impl Behavior for Vector {
    fn field(&mut self, _ctx: &FieldCtx, io: &mut FieldIo) {
        let channels = [io.choice(X_CHANNEL), io.choice(Y_CHANNEL)];
        let rate = self.tap.read("Vector", io, &channels, &mut self.frames);
        let size = io.param(SIZE);
        let (x0, y0) = (io.param(X), io.param(Y));
        self.points.clear();
        let xy = io.choice(MODE) == 0;
        if xy {
            // XY: one channel across, the other up and down, the beam going
            // on from where it was at the end of the last field.
            if let Some(last) = self.last {
                self.points.extend(last);
            }
            for f in self.frames.as_chunks::<2>().0.iter().take(MAX_POINTS - 2) {
                self.points.extend([f[0] * size + x0, f[1] * size + y0]);
            }
            if self.frames.is_empty() {
                // No audio: the beam rests where X and Y put it.
                self.points.extend([x0, y0]);
            }
        } else {
            // Scope: Y against time, from where it last rose through zero
            // with a whole sweep after it, so a steady wave stands still.
            let keep = (rate as f32 * SCOPE_SECONDS) as usize;
            self.recent
                .extend(self.frames.as_chunks::<2>().0.iter().map(|f| f[1]));
            while self.recent.len() > keep {
                self.recent.pop_front();
            }
            let sweep = ((io.param(TIME) * 1e-3 * rate as f32) as usize).max(2);
            if self.recent.len() > sweep {
                let last = self.recent.len() - sweep;
                let start = (1..=last)
                    .rev()
                    .find(|&i| self.recent[i - 1] < 0.0 && self.recent[i] >= 0.0)
                    .unwrap_or(last);
                let step = sweep.div_ceil(MAX_POINTS - 1).max(1);
                for k in (0..sweep).step_by(step) {
                    let x = -1.0 + 2.0 * k as f32 / sweep as f32;
                    self.points.extend([x + x0, self.recent[start + k] * size + y0]);
                }
            }
        }
        if self.points.len() < 4 {
            // No sweep yet, or the beam resting: a dot.
            self.points.extend([x0, y0, x0, y0]);
        }
        let n = self.points.len();
        self.last = xy.then(|| [self.points[n - 2], self.points[n - 1]]);
        io.extra[0] = [(self.points.len() / 2) as f32, 0.0, 0.0, 0.0];
    }

    fn samples(&self) -> &[f32] {
        &self.points
    }

    fn warning(&self) -> Option<String> {
        self.tap.warning()
    }
}
