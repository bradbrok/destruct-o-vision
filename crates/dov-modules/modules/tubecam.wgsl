/* dov-module
format = 1
id = "dov.tubecam"
name = "Tube Camera"
category = "analog"
description = "A camera's pickup tube, for a picture on its way to the Encoder. Light charges the tube's target and the scanning beam reads the charge off, but never all of it: what's left lags behind anything that moves, more in the dark (Lag). The beam can only carry so much (Beam), so highlights leave the target overcharged: comet tails behind bright things that move, and Bloom spills the excess into its surroundings. Burn dulls the target wherever it stayed bright, leaving a dark ghost when the picture changes. Gain is the light reaching the tube."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[output]]
id = "charge"
name = "Charge"
kind = "rgb"
hidden = true

[[param]]
id = "lag"
name = "Lag"
range = [0.0, 1.0]
default = 0.4
cv = "video"

[[param]]
id = "beam"
name = "Beam"
range = [0.2, 2.0]
default = 1.0
curve = "exp"
cv = "video"

[[param]]
id = "bloom"
name = "Bloom"
range = [0.0, 1.0]
default = 0.3

[[param]]
id = "burn"
name = "Burn"
range = [0.0, 1.0]
default = 0.0

[[param]]
id = "gain"
name = "Gain"
range = [0.0, 3.0]
default = 1.0
cv = "video"

[history]
output = "charge"
fields = 32
*/

// The target's charge `age` fields ago: what light had put there before
// the beam read it.
fn charge_at(x: i32, row: i32, age: i32) -> vec3<f32> {
    if (u.info.ring_depth == 0u) {
        return vec3<f32>(0.0);
    }
    return max(ring_at(x, row, age), vec3<f32>(0.0));
}

// What the beam reads off `charge`: a share of it, smaller in the dark,
// where the target is slow, and never more than the beam can carry.
fn readout(charge: vec3<f32>, lag: f32, beam: f32) -> vec3<f32> {
    let share = 1.0 - clamp(lag, 0.0, 1.0) * 0.85 * (1.0 - 0.6 * clamp(luma(charge), 0.0, 1.0));
    return min(charge * share, vec3<f32>(max(beam, 0.05)));
}

// What the beam left behind at (x, row) when it last read it.
fn left_at(x: i32, row: i32, lag: f32, beam: f32) -> vec3<f32> {
    let c = charge_at(x, row, 1);
    return c - readout(c, lag, beam);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let x = i32(ctx.x);
    let row = i32(ctx.row);
    let light = max(inp.sig * p.gain, vec3<f32>(0.0));
    // The charge now: what the beam left last time, and the new light. A
    // still picture settles where the beam reads off just what comes in.
    let charge = min(left_at(x, row, p.lag, p.beam) + light, vec3<f32>(6.0));
    var read = readout(charge, p.lag, p.beam);

    // Bloom: charge piled up past what the beam can carry spreads into its
    // neighbors and is read off with them.
    if (p.bloom > 0.0) {
        let limit = vec3<f32>(max(p.beam, 0.05) * 0.5);
        let s = ctx.stride;
        let around = max(left_at(x - 2, row, p.lag, p.beam) - limit, vec3<f32>(0.0))
            + max(left_at(x + 2, row, p.lag, p.beam) - limit, vec3<f32>(0.0))
            + max(left_at(x, row - s, p.lag, p.beam) - limit, vec3<f32>(0.0))
            + max(left_at(x, row + s, p.lag, p.beam) - limit, vec3<f32>(0.0));
        read = read + p.bloom * 0.3 * around;
    }

    // Burn: wherever the target has held a lot of charge for the last half
    // second or so, it has dulled.
    if (p.burn > 0.0) {
        let old = (charge_at(x, row, 8) + charge_at(x, row, 16) + charge_at(x, row, 24) + charge_at(x, row, 32)) * 0.25;
        read = read * (1.0 - clamp(p.burn * luma(old), 0.0, 0.85));
    }

    var o: Outputs;
    o.out = read;
    o.charge = charge;
    return o;
}
