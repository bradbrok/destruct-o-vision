//! Display flywheel: turns jittery drawable-acquire times into a steady
//! prediction of when each presented frame reaches the screen.
//!
//! Like a TV's horizontal flywheel sync, it is a small phase-locked loop: it
//! rejects per-frame jitter but follows slow drift. Choosing fields from the
//! raw acquire time instead makes the engine skip and repeat fields whenever
//! the display and field clocks are near a boundary.

use std::collections::VecDeque;

/// Fraction of each phase error applied immediately.
const PHASE_GAIN: f64 = 0.05;
/// Fraction of each phase error folded into the period estimate.
const FREQ_GAIN: f64 = 0.002;
/// Re-estimate the period when the median interval disagrees by this much.
const REESTIMATE_TOLERANCE: f64 = 0.1;
const HISTORY: usize = 32;

#[derive(Debug)]
pub struct Flywheel {
    period: f64,
    /// Predicted time of the next present, in seconds.
    next: Option<f64>,
    last_measured: Option<f64>,
    intervals: VecDeque<f64>,
}

impl Flywheel {
    pub fn new(period_hint: f64) -> Self {
        Self {
            period: period_hint,
            next: None,
            last_measured: None,
            intervals: VecDeque::with_capacity(HISTORY),
        }
    }

    pub fn period(&self) -> f64 {
        self.period
    }

    /// Restart from scratch, e.g. after the output moved to another display.
    pub fn reset(&mut self, period_hint: f64) {
        *self = Self::new(period_hint);
    }

    /// Feeds the time (seconds) a frame was acquired; returns the smoothed
    /// time that frame will be presented, on the same timeline.
    pub fn track(&mut self, t: f64) -> f64 {
        if let Some(last) = self.last_measured.replace(t) {
            if self.intervals.len() == HISTORY {
                self.intervals.pop_front();
            }
            self.intervals.push_back(t - last);
            self.reestimate_period(t);
        }
        let Some(predicted) = self.next else {
            self.next = Some(t + self.period);
            return t;
        };
        // Whole periods (missed vsyncs, stalls) are not phase error.
        let skipped = ((t - predicted) / self.period).round();
        if skipped.abs() > 8.0 {
            self.next = Some(t + self.period);
            return t;
        }
        let aligned = predicted + skipped * self.period;
        let error = t - aligned;
        let estimate = aligned + PHASE_GAIN * error;
        self.period += FREQ_GAIN * error;
        self.next = Some(estimate + self.period);
        estimate
    }

    /// Catches a wrong hint or a display change the loop can't pull in.
    fn reestimate_period(&mut self, t: f64) {
        if self.intervals.len() < HISTORY / 2 {
            return;
        }
        let mut sorted: Vec<f64> = self.intervals.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        if ((median - self.period) / self.period).abs() > REESTIMATE_TOLERANCE {
            log::info!(
                "flywheel: display period {:.3} ms -> {:.3} ms",
                self.period * 1e3,
                median * 1e3
            );
            self.period = median;
            self.next = Some(t + median);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise in [-1, 1].
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
    }

    /// Simulates presenting at `display_hz` with acquire jitter, choosing
    /// fields of `field_hz` from either the raw or the smoothed times.
    /// Returns (dropped, repeated) over `seconds`.
    fn simulate(display_hz: f64, hint_hz: f64, field_hz: f64, jitter: f64, smooth: bool) -> (u64, u64) {
        let mut noise = Noise(0x2545_f491_4f6c_dd1d);
        let mut flywheel = Flywheel::new(1.0 / hint_hz);
        let (mut dropped, mut repeated) = (0, 0);
        let mut last: Option<u64> = None;
        let frames = (display_hz * 20.0) as u64;
        for n in 0..frames {
            let vsync = 1.0 + n as f64 / display_hz;
            let measured = vsync + jitter * noise.next();
            let t = if smooth {
                flywheel.track(measured)
            } else {
                measured
            };
            let field = (t * field_hz) as u64;
            // Skip the first two seconds while the loop locks.
            if n as f64 > display_hz * 2.0 {
                match last {
                    Some(l) if field <= l => repeated += 1,
                    Some(l) => dropped += field - l - 1,
                    None => {}
                }
            }
            last = Some(last.map_or(field, |l| l.max(field)));
        }
        (dropped, repeated)
    }

    const NTSC_FIELD_HZ: f64 = 59.940_059_94;

    #[test]
    fn raw_acquire_times_skip_and_repeat_fields() {
        let (dropped, repeated) = simulate(60.0, 60.0, NTSC_FIELD_HZ, 0.003, false);
        assert!(
            dropped > 10 && repeated > 10,
            "dropped {dropped}, repeated {repeated}"
        );
    }

    #[test]
    fn flywheel_removes_jitter_at_60hz() {
        // 18 s of 60 Hz vs 59.94 Hz fields: about one unavoidable repeat.
        let (dropped, repeated) = simulate(60.0, 60.0, NTSC_FIELD_HZ, 0.003, true);
        assert_eq!(dropped, 0);
        assert!(repeated <= 2, "repeated {repeated}");
    }

    #[test]
    fn flywheel_recovers_from_a_wrong_hint_at_144hz() {
        // Every field should be shown 2 or 3 times; never dropped.
        let (dropped, repeated) = simulate(144.0, 60.0, NTSC_FIELD_HZ, 0.002, true);
        assert_eq!(dropped, 0);
        let fields = 18.0 * NTSC_FIELD_HZ;
        let expected_repeats = 18.0 * 144.0 - fields;
        assert!(
            (repeated as f64 - expected_repeats).abs() < 10.0,
            "repeated {repeated}"
        );
    }

    #[test]
    fn missed_vsync_does_not_shift_phase() {
        let period = 1.0 / 60.0;
        let mut flywheel = Flywheel::new(period);
        for n in 0..120 {
            flywheel.track(n as f64 * period);
        }
        // Frame 120 is late by a whole vsync.
        let t = flywheel.track(121.0 * period);
        assert!((t - 121.0 * period).abs() < 1e-4);
        assert!((flywheel.period() - period).abs() < 1e-6);
    }
}
