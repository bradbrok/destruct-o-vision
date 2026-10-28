//! The VCR's transport: where the tape is, field by field.
//!
//! The deck tapes what comes in while the tape runs (the engine keeps a
//! field per slot of the module's history ring, frozen while the tape is
//! stopped or spooling) and plays back from it. Its place on the tape is an
//! age: fields back from the newest one taped, 0 for what's coming in now.

use crate::behaviors::FieldIo;
use crate::{Behavior, FieldCtx};

pub(crate) static FORMATS: [&str; 6] = ["VHS", "S-VHS", "Betamax", "U-matic", "Video8", "Hi8"];
pub(crate) static TRANSPORT: [&str; 4] = ["Play", "Pause", "Search fwd", "Search rev"];
/// The transport choice's param index. Anything but Play stops taping.
pub(crate) const TRANSPORT_PARAM: usize = 2;
const SEARCH: usize = 3;
const TRACKING: usize = 4;

const PLAY: u32 = 0;
const PAUSE: u32 = 1;
const FORWARD: u32 = 2;

/// Noise bar half-way into the gap between tracks: where Tracking puts the
/// paused picture's bar, from near the bottom at its default down to the
/// middle with it all the way off.
fn pause_bar(tracking: f32) -> f32 {
    (0.88 + (0.95 - tracking) * 1.4).rem_euclid(1.0)
}

#[derive(Default)]
pub(crate) struct Deck {
    /// Fields back on the tape that the heads are playing.
    age: f64,
    /// Fields taped so far, up to what the ring holds.
    taped: u64,
    /// The newest field taped.
    newest: Option<u64>,
    transport: Option<u32>,
    /// 1 as the transport changes, falling to 0 as the servo locks.
    relock: f32,
    bar_phase: f64,
    /// For the paused picture's jitter.
    seed: u32,
}

impl Behavior for Deck {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let transport = io.choice(TRANSPORT_PARAM);
        let fields = ctx.advanced.max(1) as f64;
        let depth = f64::from(io.history);
        // What can be played: as much as the tape holds and has had taped.
        let longest = depth.min(self.taped as f64);
        let speed = f64::from(io.param(SEARCH).clamp(1.0, 30.0));
        let tracking = io.param(TRACKING).clamp(0.0, 1.0);

        if self.transport != Some(transport) {
            if self.transport.is_some() {
                self.relock = 1.0;
            }
            self.transport = Some(transport);
        } else {
            self.relock *= 0.88f32.powf(fields as f32);
        }
        match transport {
            PLAY => {}
            PAUSE => self.age = self.age.max(1.0),
            FORWARD => self.age = (self.age - speed * fields).max(0.0),
            _ => self.age += speed * fields,
        }
        // Stopped, the heads can only play what's on the tape.
        let floor: f64 = if transport == PLAY || transport == FORWARD {
            0.0
        } else {
            1.0
        };
        self.age = self.age.clamp(floor.min(longest), longest);

        // Noise bars: one where a paused head crosses between tracks, and
        // spooling, one per track it crosses (speed − 1 forward, speed + 1
        // back), bands of picture from further along the tape between them.
        let (bars, step) = match transport {
            PLAY => (0.0, 0.0),
            PAUSE => (1.0, 0.0),
            FORWARD => ((speed - 1.0).round().max(1.0), -1.0),
            _ => ((speed + 1.0).round(), 1.0),
        };
        let step = step * speed / (bars + 1.0);
        let phase = if transport == PAUSE {
            // The bar stands still where Tracking puts it.
            f64::from(pause_bar(tracking))
        } else {
            // Spooling, the bars drift, faster the worse the tracking.
            self.bar_phase += (0.004 + f64::from(1.0 - tracking) * 0.03) * fields;
            self.bar_phase.rem_euclid(1.0)
        };

        // The tape's field number at the top of the picture, and which field
        // of a frame the newest field taped was.
        let newest = self.newest.unwrap_or(ctx.field);
        let tape_field = if self.age < 0.5 {
            ctx.field
        } else {
            newest.saturating_sub(self.age.round() as u64 - 1)
        };
        let newest_parity = ctx.raster.field_parity(newest);

        // The picture jumps: hunting as the servo relocks, and a paused
        // field's line of jitter now and then.
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let random = f64::from(self.seed >> 8) / f64::from(1u32 << 24);
        let lines = ctx.raster.lines_per_field();
        let mut jump = f64::from(self.relock).powi(2) * lines * 0.12 * (random - 0.35);
        if transport == PAUSE && random < 0.25 {
            jump += 1.0;
        }

        io.extra[0] = [self.age as f32, step as f32, bars as f32, phase as f32];
        io.extra[1] = [
            match transport {
                PLAY => 0.0,
                PAUSE => 1.0,
                _ => 2.0,
            },
            self.relock,
            (tape_field % (1 << 20)) as f32,
            newest_parity as f32,
        ];
        io.extra[2] = [jump.round() as f32, 0.0, 0.0, 0.0];

        // The engine tapes this field after it's played, unless stopped.
        if transport == PLAY && io.history > 0 {
            self.newest = Some(ctx.field);
            self.taped = (self.taped + 1).min(u64::from(io.history));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paused_bar_follows_tracking() {
        assert!((pause_bar(0.95) - 0.88).abs() < 1e-6);
        let bars: Vec<f32> = [1.0, 0.5, 0.0].into_iter().map(pause_bar).collect();
        assert!(bars.iter().all(|b| (0.0..1.0).contains(b)), "{bars:?}");
    }
}
