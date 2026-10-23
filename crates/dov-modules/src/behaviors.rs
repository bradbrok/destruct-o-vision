//! CPU behaviors: per-field work that the GPU can't do well, like f64 phase
//! accumulation and control-rate signal generation.

use dov_core::{ModuleDesc, ParamBank, ParamKind, PhaseAcc, PortKind, RasterPhase, SyncReset};

use crate::{Behavior, EXTRA_SLOTS, FieldCtx};

/// What a behavior sees and produces for one field.
pub struct FieldIo<'a> {
    pub desc: &'a ModuleDesc,
    /// Param slots: params followed by attenuverters.
    pub slots: &'a [f32],
    /// The live params behind `slots`, for behaviors that turn their own
    /// knobs (MIDI learn).
    pub params: &'a ParamBank,
    /// One value per input: the upstream control value when a control
    /// source is patched, otherwise the port default. Video-rate signals
    /// aren't visible here.
    pub inputs: &'a [f32],
    pub extra: &'a mut [[f32; 4]; EXTRA_SLOTS],
    /// Control-rate outputs, indexed like `desc.outputs`.
    pub outputs: &'a mut [f32],
}

impl FieldIo<'_> {
    /// A param's value including control-rate CV. Video-rate CV is applied
    /// per sample on the GPU and isn't included.
    pub fn param(&self, i: usize) -> f32 {
        param_value(self.desc, self.slots, self.inputs, i)
    }

    pub fn choice(&self, i: usize) -> u32 {
        choice_value(self.desc, self.slots, self.inputs, i)
    }

    /// Turns a knob or flips a switch as if the user had: the panel shows
    /// it and saved patches keep it. `slots` still holds the old value.
    pub fn set_param(&self, i: usize, v: f32) {
        self.params.set(i, v);
    }
}

/// See [`FieldIo::param`].
pub fn param_value(desc: &ModuleDesc, slots: &[f32], inputs: &[f32], i: usize) -> f32 {
    let knob = slots[i];
    let p = &desc.params[i];
    match (p.cv, desc.cv_slot(i)) {
        (Some(PortKind::Control), Some((input, atten))) => knob + inputs[input] * slots[atten] * p.span(),
        _ => knob,
    }
}

/// See [`FieldIo::choice`].
pub fn choice_value(desc: &ModuleDesc, slots: &[f32], inputs: &[f32], i: usize) -> u32 {
    let p = &desc.params[i];
    let n = match p.kind {
        // A live list runs past the fixed options; its length isn't known here.
        ParamKind::Choice { options, .. } if p.live.is_none() => options.len() as u32,
        ParamKind::Choice { .. } | ParamKind::Knob { .. } => u32::MAX,
    };
    (param_value(desc, slots, inputs, i).round().max(0.0) as u32).min(n.saturating_sub(1))
}

const VCO_HZ: usize = 7;
const VCO_BPM: usize = 8;
const VCO_DIV: usize = 9;

/// Raster-locked oscillator: keeps phase in f64 and hands the shader the
/// fractional per-line and per-sample increments (see `dov_core::phase`).
#[derive(Default)]
pub(crate) struct Vco {
    acc: PhaseAcc,
}

impl Behavior for Vco {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let raster = ctx.raster;
        let detune = f64::from(io.param(2));
        let multiple = f64::from(io.param(1));
        let freq = match io.choice(0) {
            0 => multiple * raster.line_rate_hz() + detune,
            1 => multiple * raster.field_rate_hz() + detune,
            2 => f64::from(io.param(VCO_HZ)) + detune,
            _ => beat_hz(io.param(VCO_BPM), io.choice(VCO_DIV)) + detune,
        };
        if io.choice(0) == 3 {
            // On the engine's beat grid, like a tempo-synced LFO.
            self.acc = PhaseAcc::default();
            self.acc.advance(freq, ctx.seconds());
        } else {
            self.acc.advance(freq, ctx.elapsed_seconds());
        }
        let reset = SyncReset::from_u32(io.choice(3));
        let start = match reset {
            SyncReset::Free => self.acc.get(),
            SyncReset::Line | SyncReset::Field => 0.0,
        };
        let p = RasterPhase::compute(raster, ctx.field, freq, start, reset);
        io.extra[0] = [p.base, p.inc_line, p.inc_sample, 0.0];
    }
}

/// Composite encoder: keeps the (possibly detuned) subcarrier phase in f64,
/// as the phase at row 0 of the current frame.
#[derive(Default)]
pub(crate) struct Encoder {
    phase: f64,
    frame: Option<u64>,
}

impl Behavior for Encoder {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let r = ctx.raster;
        if r.subcarrier_hz <= 0.0 {
            io.extra[0] = [0.0; 4];
            return;
        }
        let fsc = r.subcarrier_hz + f64::from(io.param(5));
        let frame = ctx.field / u64::from(r.fields_per_frame());
        let frame_period = f64::from(r.lines_per_frame) * r.line_period();
        if let Some(last) = self.frame {
            let frames = frame.saturating_sub(last) as f64;
            self.phase = (self.phase + fsc * frames * frame_period).rem_euclid(1.0);
        }
        self.frame = Some(frame);
        let per_line = (fsc / r.line_rate_hz()).rem_euclid(1.0);
        io.extra[0] = [
            self.phase as f32,
            per_line as f32,
            (fsc / r.sample_rate_hz) as f32,
            0.0,
        ];
    }
}

/// Cable: phases of the stray carrier and the mains hum, at row 0 of the frame.
#[derive(Default)]
pub(crate) struct Cable {
    interference: f64,
    hum: f64,
    frame: Option<u64>,
}

impl Behavior for Cable {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let r = ctx.raster;
        let f_int = f64::from(io.param(6)) * 1e6;
        let mains = if r.pal { 50.0 } else { 60.0 };
        let frame = ctx.field / u64::from(r.fields_per_frame());
        let frame_period = f64::from(r.lines_per_frame) * r.line_period();
        if let Some(last) = self.frame {
            let dt = frame.saturating_sub(last) as f64 * frame_period;
            self.interference = (self.interference + f_int * dt).rem_euclid(1.0);
            self.hum = (self.hum + mains * dt).rem_euclid(1.0);
        }
        self.frame = Some(frame);
        io.extra[0] = [
            self.interference as f32,
            (f_int / r.line_rate_hz()).rem_euclid(1.0) as f32,
            (f_int / r.sample_rate_hz) as f32,
            self.hum as f32,
        ];
    }
}

/// Control-rate LFO. In BPM mode its phase comes from the engine's clock,
/// so every LFO at the same tempo stays on the same beat grid.
#[derive(Default)]
pub(crate) struct Lfo {
    /// Cycles run so far (Hz mode accumulates; BPM mode recomputes).
    cycles: f64,
    held: [f32; 2],
    seed: u64,
    started: bool,
}

impl Lfo {
    fn next_random(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.seed >> 40) as f32 / (1u64 << 24) as f32
    }
}

impl Behavior for Lfo {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let before = self.cycles;
        self.cycles = match io.choice(LFO_MODE) {
            0 => before + f64::from(io.param(0).max(0.0)) * ctx.elapsed_seconds(),
            _ => ctx.seconds() * beat_hz(io.param(LFO_BPM), io.choice(LFO_DIV)),
        };
        let total = self.cycles + f64::from(io.param(LFO_PHASE));
        let new_cycle = !self.started || total.floor() != (before + f64::from(io.param(LFO_PHASE))).floor();
        if new_cycle {
            self.held = [self.held[1], self.next_random()];
            if !self.started {
                self.held[0] = self.held[1];
                self.started = true;
            }
        }
        let ph = total.rem_euclid(1.0) as f32;
        let tau = std::f32::consts::TAU;
        let unipolar = match io.choice(1) {
            0 => 0.5 - 0.5 * (tau * ph).cos(),
            1 => 1.0 - (2.0 * ph - 1.0).abs(),
            2 => ph,
            3 => (ph < 0.5) as u32 as f32,
            4 => self.held[1],
            // Drift: glides from the last random value to the next.
            5 => {
                self.held[0] + (self.held[1] - self.held[0]) * (0.5 - 0.5 * (std::f32::consts::PI * ph).cos())
            }
            _ => 1.0 - ph,
        };
        io.outputs[0] = unipolar * io.param(2) + io.param(3);
        io.outputs[1] = f32::from(u8::from(new_cycle));
    }
}

const LFO_MODE: usize = 4;
const LFO_BPM: usize = 5;
const LFO_DIV: usize = 6;
const LFO_PHASE: usize = 7;

/// Note values for tempo-synced rates, as cycles per beat (a beat is a quarter note).
pub(crate) const DIVISION_CYCLES: [f64; 12] = [
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
    3.0 / 4.0,
    3.0 / 2.0,
    3.0,
    6.0,
];

/// Frequency of one cycle per `division` at `bpm`.
pub(crate) fn beat_hz(bpm: f32, division: u32) -> f64 {
    let cycles = DIVISION_CYCLES.get(division as usize).copied().unwrap_or(1.0);
    f64::from(bpm.max(0.0)) / 60.0 * cycles
}

/// Spin for the 3D modules: accumulates the `spin_x` and `spin_y` params
/// (degrees per second) into rotation angles, so changing the speed never
/// makes the object jump. Hands the shader the angles in `extra[0].xyz`.
#[derive(Default)]
pub(crate) struct Spin {
    angles: [f64; 2],
}

impl Behavior for Spin {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let dt = ctx.elapsed_seconds();
        for (angle, id) in self.angles.iter_mut().zip(["spin_x", "spin_y"]) {
            let speed = io.desc.param_index(id).map_or(0.0, |k| io.param(k));
            *angle = (*angle + f64::from(speed) * dt).rem_euclid(360.0);
        }
        io.extra[0] = [self.angles[0] as f32, self.angles[1] as f32, 0.0, 0.0];
    }
}

/// Knobs as control outputs: performance macros, offsets.
#[derive(Default)]
pub(crate) struct Macros;

impl Behavior for Macros {
    fn field(&mut self, _ctx: &FieldCtx, io: &mut FieldIo) {
        for i in 0..io.outputs.len() {
            io.outputs[i] = io.param(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use dov_core::RasterStd;

    use super::*;
    use crate::find;

    /// Runs a module's behavior for `fields` fields starting at `first`,
    /// returning its control outputs (and VCO phase terms) for each field.
    fn run(kind: &str, slots: &[(&str, f32)], first: u64, fields: u64) -> Vec<(Vec<f32>, [f32; 4])> {
        let ty = find(kind).unwrap();
        let desc = &ty.desc;
        let mut values = desc.default_slots();
        for (id, v) in slots {
            values[desc.param_index(id).unwrap()] = *v;
        }
        let inputs: Vec<f32> = desc.inputs_full().iter().map(|p| p.default).collect();
        let params = ParamBank::new(&values);
        let mut behavior = (ty.behavior.unwrap())();
        let raster = RasterStd::NTSC;
        let mut out = Vec::new();
        for field in first..first + fields {
            let mut extra = [[0.0; 4]; EXTRA_SLOTS];
            let mut outputs = vec![0.0; desc.outputs.len()];
            let ctx = FieldCtx {
                raster: &raster,
                field,
                advanced: u64::from(field > first),
                offline: false,
            };
            let mut io = FieldIo {
                desc,
                slots: &values,
                params: &params,
                inputs: &inputs,
                extra: &mut extra,
                outputs: &mut outputs,
            };
            behavior.field(&ctx, &mut io);
            out.push((outputs, extra[0]));
        }
        out
    }

    #[test]
    fn bpm_lfos_share_the_beat_grid() {
        let slots = [("mode", 1.0), ("bpm", 128.0), ("div", 4.0), ("shape", 2.0)];
        // One LFO started at field 0, another added 1000 fields later.
        let early = run("dov.lfo", &slots, 0, 1200);
        let late = run("dov.lfo", &slots, 1000, 200);
        for (a, b) in early[1001..].iter().zip(&late[1..]) {
            assert!((a.0[0] - b.0[0]).abs() < 1e-5, "{} vs {}", a.0[0], b.0[0]);
        }
    }

    #[test]
    fn lfo_trig_fires_once_per_cycle() {
        // 1 Hz for 10.5 s of NTSC fields: cycles start at 1 s … 10 s, plus the first field.
        let fields = (10.5 * RasterStd::NTSC.field_rate_hz()) as u64;
        let out = run("dov.lfo", &[("rate", 1.0)], 0, fields);
        let trigs = out.iter().filter(|(o, _)| o[1] > 0.5).count();
        assert_eq!(trigs, 11);
    }

    #[test]
    fn note_values_scale_the_tempo() {
        assert_eq!(beat_hz(120.0, 4), 2.0, "quarter notes at 120 BPM");
        assert_eq!(beat_hz(120.0, 2), 0.5, "one per bar");
        assert_eq!(beat_hz(120.0, 9), 3.0, "quarter-note triplets");
    }

    #[test]
    fn vco_runs_at_the_typed_frequency() {
        let raster = RasterStd::NTSC;
        // Hz mode: the per-sample phase step is exactly f / sample rate.
        let out = run("dov.vco", &[("range", 2.0), ("hz", 1_000_000.0)], 0, 2);
        let expected = (1_000_000.0 / raster.sample_rate_hz) as f32;
        assert!((out[1].1[2] - expected).abs() < 1e-6);
        // BPM mode: the phase at a field start comes from the beat grid.
        let field = 900;
        let out = run(
            "dov.vco",
            &[("range", 3.0), ("bpm", 90.0), ("div", 4.0)],
            field,
            1,
        );
        let t = field as f64 * raster.field_period()
            + raster.first_active_line_offset(field) * raster.line_period()
            + f64::from(raster.active_start_sample) * raster.sample_period();
        let phase = (1.5 * t).rem_euclid(1.0) as f32;
        let d = (out[0].1[0] - phase).rem_euclid(1.0);
        assert!(d.min(1.0 - d) < 1e-4, "{} vs {phase}", out[0].1[0]);
    }
}
