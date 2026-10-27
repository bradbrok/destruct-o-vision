//! Oscillator phase in raster time.
//!
//! A 5 MHz oscillator runs ~83k cycles per field, far more than f32 can
//! resolve. The CPU keeps each oscillator's phase in f64 and hands the GPU
//! only small fractional terms, so the shader evaluates
//!
//! ```text
//! phase = fract(base + fract(line * inc_line) + sample * inc_sample)
//! ```
//!
//! with every operand below ~1000 and an error around 1e-4 cycles.

use crate::raster::RasterStd;

/// What resets an oscillator's phase, like the lock switch on hardware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SyncReset {
    /// Free-running: phase is continuous across lines and fields.
    #[default]
    Free = 0,
    /// Reset at every H sync: vertical patterns stand still at any frequency.
    Line = 1,
    /// Reset at every V sync: horizontal patterns stand still at any frequency.
    Field = 2,
}

impl SyncReset {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Line,
            2 => Self::Field,
            _ => Self::Free,
        }
    }
}

/// Continuous f64 phase accumulator, advanced once per field.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseAcc {
    cycles: f64,
}

impl PhaseAcc {
    /// Phase in cycles, in [0, 1).
    pub fn get(&self) -> f64 {
        self.cycles
    }

    pub fn advance(&mut self, freq_hz: f64, seconds: f64) {
        self.cycles = (self.cycles + freq_hz * seconds).rem_euclid(1.0);
    }
}

/// Per-field phase terms consumed by the GPU.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RasterPhase {
    /// Phase at the first active sample of the field's first active line.
    pub base: f32,
    /// Fractional phase advance per line.
    pub inc_line: f32,
    /// Fractional phase advance per sample.
    pub inc_sample: f32,
}

impl RasterPhase {
    /// `phase_at_field_start` is the accumulated phase for [`SyncReset::Free`]
    /// and the user's phase offset for the reset modes.
    pub fn compute(
        std: &RasterStd,
        field: u64,
        freq_hz: f64,
        phase_at_field_start: f64,
        reset: SyncReset,
    ) -> Self {
        let t_line = std.line_period();
        let t_sample = std.sample_period();
        let to_first_sample = f64::from(std.active_start_sample) * t_sample;
        let inc_sample = fract(freq_hz * t_sample) as f32;
        match reset {
            SyncReset::Line => Self {
                base: fract(phase_at_field_start + freq_hz * to_first_sample) as f32,
                inc_line: 0.0,
                inc_sample,
            },
            SyncReset::Free | SyncReset::Field => {
                let t0 = std.first_active_line_offset(field) * t_line + to_first_sample;
                Self {
                    base: fract(phase_at_field_start + freq_hz * t0) as f32,
                    inc_line: fract(freq_hz * t_line) as f32,
                    inc_sample,
                }
            }
        }
    }

    /// The shader's computation, in f32, for tests and CPU reference paths.
    pub fn eval_f32(&self, line: u32, sample: u32) -> f32 {
        let l = (line as f32 * self.inc_line).fract();
        (self.base + l + sample as f32 * self.inc_sample).rem_euclid(1.0)
    }
}

fn fract(x: f64) -> f64 {
    x.rem_euclid(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance between two phases on the unit circle, in cycles.
    fn wrap_err(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(1.0);
        d.min(1.0 - d)
    }

    #[test]
    fn integer_multiples_of_line_rate_stand_still() {
        let std = RasterStd::NTSC;
        let freq = 16.0 * std.line_rate_hz();
        let mut acc = PhaseAcc::default();
        let first = RasterPhase::compute(&std, 0, freq, acc.get(), SyncReset::Free);
        for field in 1..=4 {
            acc.advance(freq, std.field_period());
            let p = RasterPhase::compute(&std, field, freq, acc.get(), SyncReset::Free);
            // Same phase at the same screen position every frame (fields of equal parity).
            if field % 2 == 0 {
                assert!(wrap_err(p.base as f64, first.base as f64) < 1e-6);
            }
        }
    }

    #[test]
    fn accumulator_matches_closed_form_after_ten_minutes() {
        let std = RasterStd::NTSC;
        let freq = 16.0 * std.line_rate_hz() + 3.0;
        let fields = (600.0 * std.field_rate_hz()) as u64;
        let mut acc = PhaseAcc::default();
        for _ in 0..fields {
            acc.advance(freq, std.field_period());
        }
        let exact = (freq * fields as f64 * std.field_period()).rem_euclid(1.0);
        assert!(
            wrap_err(acc.get(), exact) < 1e-6,
            "drift {}",
            wrap_err(acc.get(), exact)
        );
    }

    #[test]
    fn gpu_phase_math_stays_within_budget() {
        // Compare the f32 shader evaluation against f64 raster time across a
        // whole field, for frequencies up to Nyquist, late into a long run.
        for std in [RasterStd::NTSC, RasterStd::HD1080P60] {
            let field = 36_001;
            for freq in [
                3.0,
                std.field_rate_hz() * 7.0,
                std.line_rate_hz() * 16.0 + 3.0,
                std.sample_rate_hz * 0.37,
            ] {
                let acc = 0.123_456_789;
                let p = RasterPhase::compute(&std, field, freq, acc, SyncReset::Free);
                let t0 = std.first_active_line_offset(field) * std.line_period()
                    + f64::from(std.active_start_sample) * std.sample_period();
                let mut worst: f64 = 0.0;
                for line in (0..std.active_lines_per_field).step_by(7) {
                    for x in (0..std.active_samples).step_by(13) {
                        let t = t0 + f64::from(line) * std.line_period() + f64::from(x) * std.sample_period();
                        let exact = acc + freq * t;
                        worst = worst.max(wrap_err(p.eval_f32(line, x) as f64, exact));
                    }
                }
                assert!(
                    worst < 2e-4,
                    "{} @ {freq} Hz: worst error {worst} cycles",
                    std.name()
                );
            }
        }
    }
}
