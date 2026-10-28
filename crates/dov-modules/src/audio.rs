//! Audio In: a resonant filter and an envelope follower, run at audio rate
//! over the samples captured since the last field.

use std::f32::consts::PI;
use std::sync::Arc;

use dov_io::{AudioCursor, AudioSource, AudioStatus};

use crate::{Behavior, FieldCtx, FieldIo};

// Params, in the order `library::AUDIO_IN` declares them. The device is
// the module's text: a name picked from the inputs connected.
const CHANNEL: usize = 0;
const FILTER: usize = 1;
const FREQ: usize = 2;
const Q: usize = 3;
const GAIN: usize = 4;
const ATTACK: usize = 5;
const RELEASE: usize = 6;
const THRESHOLD: usize = 7;
const RESPONSE: usize = 8;

/// Log response maps this many dB below full scale onto 0..1.
const LOG_RANGE_DB: f32 = 48.0;
/// The gate closes below this fraction of the threshold, so a level
/// hovering at the threshold doesn't chatter.
const HYSTERESIS: f32 = 0.8;
/// Without new audio for this long, the input counts as silent.
const STALL_SECONDS: f64 = 0.1;
/// The most audio read in one field, after the engine stalls.
const MAX_READ_SECONDS: f32 = 0.25;

#[derive(Default)]
pub(crate) struct AudioIn {
    source: Option<Arc<AudioSource>>,
    cursor: AudioCursor,
    detector: Detector,
    /// Seconds since audio last arrived.
    idle: f64,
    warning: Option<String>,
}

impl Behavior for AudioIn {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let device = io.params.text();
        if self.source.as_ref().is_none_or(|s| s.device() != device) {
            self.source = None;
            self.source = Some(AudioSource::open(&device));
            self.cursor = AudioCursor::default();
        }
        let Self {
            source: Some(source),
            cursor,
            detector,
            idle,
            warning,
        } = self
        else {
            return;
        };
        let channel = io.choice(CHANNEL);
        *warning = device_warning("Audio In", source, &[channel]);

        let rate = source.sample_rate() as f32;
        let settings = Settings::new(io, rate);
        let gain = 10f32.powf(io.param(GAIN) / 20.0);
        let mut peak = settings.response(detector.env);
        let mut opened = false;
        let mut run = |x: f32| {
            let (level, o) = detector.sample(&settings, x);
            peak = peak.max(level);
            opened |= o;
        };
        let max = (rate * MAX_READ_SECONDS) as usize;
        let read = source.read(cursor, max, |frame| run(pick(frame, channel) * gain));
        if read > 0 {
            *idle = 0.0;
        } else {
            // Audio arrives in blocks that can be longer than a field, so
            // wait a little before treating its absence as silence.
            *idle += ctx.elapsed_seconds();
            if *idle > STALL_SECONDS {
                let silence = (f64::from(rate) * ctx.elapsed_seconds()) as usize;
                (0..silence.min(max)).for_each(|_| run(0.0));
                peak = settings.response(detector.env);
            }
        }
        io.outputs[0] = peak;
        io.outputs[1] = f32::from(u8::from(detector.gate || opened));
        io.outputs[2] = f32::from(u8::from(opened));
    }

    fn warning(&self) -> Option<String> {
        self.warning.clone()
    }
}

/// Audio as it arrives, for modules that use the samples themselves (the
/// XY display, RF's sound carrier): from the device named in the module's
/// text, a field at a time.
#[derive(Default)]
pub(crate) struct AudioTap {
    source: Option<Arc<AudioSource>>,
    cursor: AudioCursor,
    warning: Option<String>,
}

impl AudioTap {
    /// Puts the audio that arrived since the last field into `out`, as
    /// `channels` picked from each frame (see `pick`), interleaved. Returns
    /// the device's sample rate.
    pub(crate) fn read(&mut self, module: &str, io: &FieldIo, channels: &[u32], out: &mut Vec<f32>) -> u32 {
        out.clear();
        let device = io.params.text();
        if self.source.as_ref().is_none_or(|s| s.device() != device) {
            self.source = None;
            self.source = Some(AudioSource::open(&device));
            self.cursor = AudioCursor::default();
        }
        let Some(source) = &self.source else {
            return 0;
        };
        self.warning = device_warning(module, source, channels);
        let rate = source.sample_rate();
        let max = (rate as f32 * MAX_READ_SECONDS) as usize;
        source.read(&mut self.cursor, max, |frame| {
            out.extend(channels.iter().map(|&c| pick(frame, c)));
        });
        rate
    }

    pub(crate) fn warning(&self) -> Option<String> {
        self.warning.clone()
    }
}

/// What's wrong, if anything, with `module`'s audio device and the
/// `channels` it listens to.
pub(crate) fn device_warning(module: &str, source: &AudioSource, channels: &[u32]) -> Option<String> {
    match source.status() {
        AudioStatus::Denied => Some(
            "Microphone access denied. Allow it in System Settings > Privacy & Security > Microphone.".into(),
        ),
        AudioStatus::Failed(e) => Some(format!("{module}: {e}")),
        AudioStatus::Running { name } => {
            let n = source.channels();
            let channel = channels.iter().find(|&&c| c as usize > n)?;
            let plural = if n == 1 { "" } else { "s" };
            Some(format!(
                "{module}: {name} has {n} input channel{plural}, so channel {channel} is silent"
            ))
        }
        AudioStatus::Starting => None,
    }
}

/// The selected channel of a frame: 0 mixes the first two, or takes a
/// mono input as it is; `n` is channel `n`, counted from 1.
pub(crate) fn pick(frame: &[f32], channel: u32) -> f32 {
    match (channel, frame) {
        (0, [a, b, ..]) => 0.5 * (a + b),
        (0, [a]) => *a,
        (0, []) => 0.0,
        (n, _) => frame.get(n as usize - 1).copied().unwrap_or(0.0),
    }
}

/// One field's worth of the module's settings, worked out for the device's rate.
struct Settings {
    filter: Filter,
    svf: SvfCoefs,
    /// Follower coefficients per sample.
    attack: f32,
    release: f32,
    log: bool,
    threshold: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    Full,
    Low,
    Band,
    High,
}

impl Settings {
    fn new(io: &FieldIo, rate: f32) -> Self {
        let per_sample = |ms: f32| 1.0 - (-1.0 / (ms.max(0.01) * 1e-3 * rate.max(1.0))).exp();
        Self {
            filter: match io.choice(FILTER) {
                0 => Filter::Full,
                1 => Filter::Low,
                2 => Filter::Band,
                _ => Filter::High,
            },
            svf: SvfCoefs::new(io.param(FREQ), io.param(Q), rate),
            attack: per_sample(io.param(ATTACK)),
            release: per_sample(io.param(RELEASE)),
            log: io.choice(RESPONSE) == 1,
            threshold: io.param(THRESHOLD),
        }
    }

    /// The envelope as the Env output reports it.
    fn response(&self, env: f32) -> f32 {
        if self.log {
            (1.0 + 20.0 * env.log10() / LOG_RANGE_DB).clamp(0.0, 1.0)
        } else {
            env
        }
    }
}

/// Filter, rectifier, envelope follower and gate, one sample at a time.
#[derive(Default)]
struct Detector {
    svf: Svf,
    env: f32,
    gate: bool,
}

impl Detector {
    /// Returns the level, as the Env output reports it, and whether the
    /// gate just opened.
    fn sample(&mut self, s: &Settings, x: f32) -> (f32, bool) {
        let (low, band, high) = self.svf.process(&s.svf, x);
        let y = match s.filter {
            Filter::Full => x,
            Filter::Low => low,
            // Scaled by 1/Q for unity gain at the center, whatever the Q.
            Filter::Band => band * s.svf.k,
            Filter::High => high,
        };
        let r = y.abs();
        let k = if r > self.env { s.attack } else { s.release };
        self.env += k * (r - self.env);
        let level = s.response(self.env);
        let opened = if self.gate {
            self.gate = level >= s.threshold * HYSTERESIS;
            false
        } else {
            self.gate = level > s.threshold;
            self.gate
        };
        (level, opened)
    }
}

/// Coefficients of a state-variable filter in its trapezoidal form
/// (Simper, "Linear Trap Integrated SVF"), which stays stable and in tune
/// right down to low cutoffs and up to high Q.
struct SvfCoefs {
    a1: f32,
    a2: f32,
    a3: f32,
    /// Damping: 1/Q.
    k: f32,
}

impl SvfCoefs {
    /// `rate` is 0 until the device opens; no audio is filtered then, but
    /// the coefficients must still come out finite.
    fn new(freq: f32, q: f32, rate: f32) -> Self {
        let rate = rate.max(1.0);
        // Below Nyquist, so the prewarp stays finite, and above 0, which CV
        // can reach. Not `clamp`, whose bounds would cross at low rates.
        let freq = freq.min(0.49 * rate).max(1e-3);
        let g = (PI * freq / rate).tan();
        let k = 1.0 / q.max(0.1);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

#[derive(Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Low-pass, band-pass and high-pass outputs for one input sample.
    fn process(&mut self, c: &SvfCoefs, v0: f32) -> (f32, f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, v0 - c.k * v1 - v2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find;

    const RATE: f32 = 48_000.0;

    fn settings(filter: Filter, freq: f32, q: f32) -> Settings {
        let per_sample = |ms: f32| 1.0 - (-1.0 / (ms * 1e-3 * RATE)).exp();
        Settings {
            filter,
            svf: SvfCoefs::new(freq, q, RATE),
            attack: per_sample(1.0),
            release: per_sample(100.0),
            log: false,
            threshold: 0.5,
        }
    }

    /// The envelope after half a second of a full-scale sine at `hz`.
    fn level(s: &Settings, hz: f32) -> f32 {
        let mut d = Detector::default();
        let n = (RATE * 0.5) as usize;
        let mut last = 0.0;
        for i in 0..n {
            last = d.sample(s, (2.0 * PI * hz * i as f32 / RATE).sin()).0;
        }
        last
    }

    #[test]
    fn params_match_the_module() {
        let desc = &find("dov.audio").unwrap().desc;
        for (i, id) in [
            (CHANNEL, "channel"),
            (FILTER, "filter"),
            (FREQ, "freq"),
            (Q, "q"),
            (GAIN, "gain"),
            (ATTACK, "attack"),
            (RELEASE, "release"),
            (THRESHOLD, "threshold"),
            (RESPONSE, "response"),
        ] {
            assert_eq!(desc.params[i].id, id);
        }
        assert_eq!(desc.text.unwrap().live, Some(dov_core::Live::AudioInputs));
        assert_eq!(desc.params[CHANNEL].live, Some(dov_core::Live::AudioChannels));
    }

    #[test]
    fn coefficients_stay_finite_before_the_device_opens() {
        for (freq, q, rate) in [
            (150.0, 0.707, 0.0),
            (16_000.0, 20.0, 8000.0),
            (f32::NAN, f32::NAN, 0.0),
        ] {
            let c = SvfCoefs::new(freq, q, rate);
            let all = [c.a1, c.a2, c.a3, c.k];
            assert!(all.iter().all(|v| v.is_finite()), "{freq} Hz at {rate}: {all:?}");
        }
    }

    #[test]
    fn full_band_follows_the_peak() {
        let l = level(&settings(Filter::Full, 1000.0, 0.7), 440.0);
        assert!((0.95..=1.0).contains(&l), "{l}");
    }

    #[test]
    fn low_pass_keeps_the_bass() {
        let s = settings(Filter::Low, 150.0, 0.707);
        assert!(level(&s, 50.0) > 0.95, "{}", level(&s, 50.0));
        // Two poles: about (150 / 5000)² at 5 kHz.
        assert!(level(&s, 5000.0) < 0.002, "{}", level(&s, 5000.0));
    }

    #[test]
    fn high_pass_keeps_the_treble() {
        let s = settings(Filter::High, 2000.0, 0.707);
        assert!(level(&s, 10_000.0) > 0.95, "{}", level(&s, 10_000.0));
        assert!(level(&s, 100.0) < 0.005, "{}", level(&s, 100.0));
    }

    #[test]
    fn band_pass_is_unity_at_the_center_and_narrows_with_q() {
        for q in [0.7, 4.0, 16.0] {
            let center = level(&settings(Filter::Band, 1000.0, q), 1000.0);
            assert!((0.95..=1.02).contains(&center), "Q {q}: {center}");
        }
        let octave_up = |q| level(&settings(Filter::Band, 1000.0, q), 2000.0);
        assert!(octave_up(16.0) < octave_up(4.0) && octave_up(4.0) < octave_up(0.7));
        assert!(octave_up(16.0) < 0.05, "{}", octave_up(16.0));
    }

    #[test]
    fn resonance_peaks_at_the_cutoff() {
        let s = settings(Filter::Low, 1000.0, 8.0);
        let peak = level(&s, 1000.0);
        assert!((7.5..8.5).contains(&peak), "Q 8 low-pass at cutoff: {peak}");
    }

    #[test]
    fn release_falls_by_1_over_e_in_the_release_time() {
        let s = settings(Filter::Full, 1000.0, 0.7);
        let mut d = Detector {
            env: 1.0,
            ..Default::default()
        };
        let mut level = 0.0;
        for _ in 0..(RATE * 0.1) as usize {
            level = d.sample(&s, 0.0).0;
        }
        assert!((level - (-1.0f32).exp()).abs() < 0.01, "{level}");
    }

    #[test]
    fn gate_opens_once_and_holds_through_ripple() {
        let s = settings(Filter::Full, 1000.0, 0.7);
        let mut d = Detector::default();
        let mut opened = 0;
        // A 50 Hz tone: the follower ripples, but stays above 0.8 × threshold.
        for i in 0..(RATE * 0.5) as usize {
            let x = 0.7 * (2.0 * PI * 50.0 * i as f32 / RATE).sin();
            opened += usize::from(d.sample(&s, x).1);
        }
        assert_eq!(opened, 1);
        assert!(d.gate);
        for _ in 0..(RATE * 0.5) as usize {
            d.sample(&s, 0.0);
        }
        assert!(!d.gate, "closes in silence");
    }

    #[test]
    fn log_response_spans_48_db() {
        let mut s = settings(Filter::Full, 1000.0, 0.7);
        s.log = true;
        assert_eq!(s.response(1.0), 1.0);
        assert!((s.response(10f32.powf(-24.0 / 20.0)) - 0.5).abs() < 1e-5);
        assert_eq!(s.response(1e-4), 0.0);
        assert_eq!(s.response(0.0), 0.0);
    }

    #[test]
    fn channels_are_picked_from_whatever_the_device_has() {
        let frame: Vec<f32> = (1..=34).map(|c| c as f32).collect();
        assert_eq!(pick(&frame, 0), 1.5, "1+2 mixes the first two");
        assert_eq!(pick(&frame, 34), 34.0, "the last of 34");
        assert_eq!(pick(&frame, 35), 0.0, "past the last is silent");
        assert_eq!(pick(&[0.2], 0), 0.2, "mono input is used as is");
        assert_eq!(pick(&[], 0), 0.0);
    }

    #[test]
    fn the_channel_switch_names_channels_past_its_fixed_options() {
        let p = &find("dov.audio").unwrap().desc.params[CHANNEL];
        let names: Vec<_> = [0, 1, 34].map(|i| p.option_name(i).unwrap()).into();
        assert_eq!(names, ["1+2", "1", "34"]);
    }
}
