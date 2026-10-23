//! The CRT module's per-field side: the power switch, the degauss button and
//! the magnetism its shadow mask picks up. The picture tube itself is drawn
//! by the engine, at the output window's resolution and refresh rate.

use crate::{Behavior, FieldCtx, FieldIo};

pub const ID: &str = "dov.crt";

// Param indices, in the order the module declares them.
pub const POWER: usize = 0;
pub const DEGAUSS: usize = 1;
pub const MASK_TYPE: usize = 2;
pub const PHOSPHOR: usize = 3;
pub const SCAN: usize = 4;
pub const BRIGHT: usize = 5;
pub const CONTRAST: usize = 6;
pub const WHITE: usize = 7;
pub const MASK: usize = 8;
pub const PITCH: usize = 9;
pub const SPOT: usize = 10;
pub const BLOOM: usize = 11;
pub const GLOW: usize = 12;
pub const PERSIST: usize = 13;
pub const CONVERGE: usize = 14;
pub const CURVATURE: usize = 15;
pub const OVERSCAN: usize = 16;
pub const ROOM: usize = 17;
pub const MAGNET: usize = 18;

/// How long the degaussing coil's field takes to die away, in seconds.
pub const DEGAUSS_SECONDS: f32 = 2.0;
/// How much of the strongest magnet it has met the mask keeps afterward.
const REMANENCE: f32 = 0.6;
/// How fast degaussing clears the mask's magnetism, in seconds.
const CLEARING: f64 = 0.35;
/// Stands in for "long ago": no power-up or degauss in progress.
const LONG_AGO: f64 = 1.0e4;

/// Hands the engine, in `extra[0]`: seconds since the power last switched,
/// the power (0 or 1), seconds since degaussing began, and the mask's
/// magnetism (-1..1).
pub struct Tube {
    on: Option<bool>,
    since_power: f64,
    since_degauss: f64,
    pushed: bool,
    magnetism: f32,
}

impl Default for Tube {
    fn default() -> Self {
        Self {
            on: None,
            since_power: LONG_AGO,
            since_degauss: LONG_AGO,
            pushed: false,
            magnetism: 0.0,
        }
    }
}

impl Behavior for Tube {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let dt = ctx.elapsed_seconds();
        self.since_power = (self.since_power + dt).min(LONG_AGO);
        self.since_degauss = (self.since_degauss + dt).min(LONG_AGO);

        // A patch opens with the set already on; only switching it warms up.
        let on = io.choice(POWER) == 1;
        if self.on.is_some_and(|was| was != on) {
            self.since_power = 0.0;
            // Sets degauss themselves every time they're switched on.
            if on {
                self.since_degauss = 0.0;
            }
        }
        self.on = Some(on);

        // The button springs back; a gate at its jack presses it once.
        let pushed = io.choice(DEGAUSS) == 1;
        if pushed && !self.pushed && on {
            self.since_degauss = 0.0;
        }
        self.pushed = pushed;
        if io.slots[DEGAUSS] >= 0.5 {
            io.set_param(DEGAUSS, 0.0);
        }

        let magnet = io.param(MAGNET).clamp(-1.0, 1.0) * REMANENCE;
        if magnet.abs() > self.magnetism.abs() {
            self.magnetism = magnet;
        }
        if self.since_degauss < f64::from(DEGAUSS_SECONDS) {
            self.magnetism *= (-dt / CLEARING).exp() as f32;
        }

        io.extra[0] = [
            self.since_power as f32,
            f32::from(u8::from(on)),
            self.since_degauss as f32,
            self.magnetism,
        ];
    }
}

#[cfg(test)]
mod tests {
    use dov_core::{ParamBank, RasterStd};

    use super::*;
    use crate::{EXTRA_SLOTS, find};

    /// Drives a CRT for `fields` fields; `set` changes params before a
    /// field. Returns `extra[0]` after each one.
    fn run(fields: u64, set: impl Fn(u64, &ParamBank)) -> Vec<[f32; 4]> {
        let ty = find(ID).unwrap();
        let desc = &ty.desc;
        let params = ParamBank::new(&desc.default_slots());
        let inputs: Vec<f32> = desc.inputs_full().iter().map(|p| p.default).collect();
        let mut behavior = (ty.behavior.unwrap())();
        let raster = RasterStd::NTSC;
        (0..fields)
            .map(|field| {
                set(field, &params);
                let slots = params.snapshot();
                let mut extra = [[0.0; 4]; EXTRA_SLOTS];
                let mut outputs = vec![0.0; desc.outputs.len()];
                let ctx = FieldCtx {
                    raster: &raster,
                    field,
                    advanced: u64::from(field > 0),
                    offline: false,
                    beats: 0.0,
                    bpm: 120.0,
                };
                let mut io = FieldIo {
                    desc,
                    slots: &slots,
                    params: &params,
                    inputs: &inputs,
                    extra: &mut extra,
                    outputs: &mut outputs,
                };
                behavior.field(&ctx, &mut io);
                extra[0]
            })
            .collect()
    }

    #[test]
    fn params_are_where_the_indices_say() {
        let desc = &find(ID).unwrap().desc;
        for (i, id) in [
            (POWER, "power"),
            (DEGAUSS, "degauss"),
            (MASK_TYPE, "mask_type"),
            (PHOSPHOR, "phosphor"),
            (SCAN, "scan"),
            (BRIGHT, "bright"),
            (CONTRAST, "contrast"),
            (WHITE, "white"),
            (MASK, "mask"),
            (PITCH, "pitch"),
            (SPOT, "spot"),
            (BLOOM, "bloom"),
            (GLOW, "glow"),
            (PERSIST, "persist"),
            (CONVERGE, "converge"),
            (CURVATURE, "curvature"),
            (OVERSCAN, "overscan"),
            (ROOM, "room"),
            (MAGNET, "magnet"),
        ] {
            assert_eq!(desc.params[i].id, id);
        }
    }

    #[test]
    fn opening_a_patch_doesnt_warm_up_or_degauss() {
        let state = run(3, |_, _| {});
        assert!(state[2][0] > 100.0 && state[2][2] > 100.0, "{:?}", state[2]);
        assert_eq!(state[2][1], 1.0);
    }

    #[test]
    fn switching_on_warms_up_and_degausses() {
        let state = run(20, |field, p| {
            p.set(POWER, if (3..10).contains(&field) { 0.0 } else { 1.0 })
        });
        assert_eq!(state[5][1], 0.0);
        assert!(state[5][0] < 0.1, "switched off at field 3: {:?}", state[5]);
        assert_eq!(state[19][1], 1.0);
        let field = RasterStd::NTSC.field_period() as f32;
        assert!((state[19][0] - 9.0 * field).abs() < 1e-4);
        assert!((state[19][2] - 9.0 * field).abs() < 1e-4);
    }

    #[test]
    fn the_degauss_button_springs_back_and_clears_the_mask() {
        let fields = 4 * 60;
        let state = run(fields, |field, p| {
            match field {
                // A magnet held to the screen, then taken away.
                0..60 => p.set(MAGNET, 1.0),
                60 => p.set(MAGNET, 0.0),
                120 => p.set(DEGAUSS, 1.0),
                _ => {}
            }
            if field == 121 {
                assert_eq!(p.snapshot()[DEGAUSS], 0.0, "the button sprang back");
            }
        });
        assert!(
            (state[100][3] - REMANENCE).abs() < 1e-6,
            "the mask stays magnetized"
        );
        assert!(state[121][2] < 0.05, "degaussing: {:?}", state[121]);
        assert!(state[fields as usize - 1][3].abs() < 0.01, "{:?}", state.last());
    }
}
