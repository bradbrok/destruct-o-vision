//! RF: the per-row timing of what the sound carrier and the other stations
//! do to a picture (`shaders/composite/rf.wgsl`).

use crate::audio::AudioTap;
use crate::{Behavior, FieldCtx, FieldIo};

// The audio channel param, in the order `library::RF` declares its params.
const CHANNEL: usize = 6;

#[derive(Default)]
pub(crate) struct Rf {
    tap: AudioTap,
    audio: Vec<f32>,
    /// Per row of this field: audio, sound beat phase, co-channel phase, 0.
    rows: Vec<f32>,
    /// Phases at the start of the next row, in cycles.
    beat: f64,
    cochannel: f64,
    /// How far the adjacent channel's scan has slipped against ours.
    slip_samples: f64,
    slip_lines: f64,
    held: f32,
}

impl Behavior for Rf {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let r = ctx.raster;
        let composite = r.composite();
        self.tap.read("RF", io, &[io.choice(CHANNEL)], &mut self.audio);

        // This field's rows of the composite frame (see `composite_timing`).
        let lines = r.lines_per_frame;
        let second = if r.interlaced { lines.div_ceil(2) } else { lines };
        let rows = match r.field_parity(ctx.field) {
            0 => second,
            _ => lines - second,
        } as usize;
        // The sound carrier sits 4.5 MHz above the picture (5.5 MHz in PAL),
        // so it beats with the chroma at the difference, ±25 kHz (50 kHz) of
        // FM with the audio; on other rasters, scaled to the picture.
        let (beat_hz, deviation_hz) = if r.pal {
            (1_066_381.25, 50e3)
        } else {
            (920_455.0, 25e3)
        };
        let per_sample = beat_hz / composite.picture_clock_hz;
        let deviation = deviation_hz / composite.picture_clock_hz;
        let spl = f64::from(r.samples_per_line);
        // Co-channel stations sit 10 kHz apart: 0.64 cycles a line in NTSC,
        // as many across the picture's height on any raster.
        let cochannel_per_line = 10_010.0 / 15_734.26 * 262.5 / r.lines_per_field();

        self.rows.clear();
        let n = self.audio.len();
        for i in 0..rows {
            if n > 0 {
                self.held = self.audio[i * n / rows];
            }
            let a = self.held.clamp(-1.0, 1.0);
            self.rows
                .extend([a, self.beat.fract() as f32, self.cochannel.fract() as f32, 0.0]);
            self.beat = (self.beat + spl * (per_sample + deviation * f64::from(a))).fract();
            self.cochannel = (self.cochannel + cochannel_per_line).fract();
        }
        // The adjacent channel's blanking crawls across in about two seconds
        // and rolls down in about three.
        let fields = ctx.advanced.max(1) as f64;
        let field_rate = r.field_rate_hz();
        self.slip_samples = (self.slip_samples + spl * fields / (2.0 * field_rate)).rem_euclid(spl);
        self.slip_lines =
            (self.slip_lines + f64::from(lines) * fields / (3.0 * field_rate)).rem_euclid(f64::from(lines));
        io.extra[0] = [
            self.slip_samples as f32,
            self.slip_lines.floor() as f32,
            per_sample as f32,
            0.0,
        ];
    }

    fn samples(&self) -> &[f32] {
        &self.rows
    }

    fn warning(&self) -> Option<String> {
        self.tap.warning()
    }
}
