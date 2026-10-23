/* dov-module
format = 1
id = "dov.blur"
name = "Blur"
category = "process"
description = "Blurs within the field: across, down or both. Radius takes video-rate CV, so a ramp, shape or key blurs some parts of the picture and not others, and it glides smoothly between whole samples. Soft weighs nearer samples more, for a gentler blur than Box."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "radius"
name = "Radius"
range = [0.0, 12.0]
default = 2.0
cv = "video"

[[param]]
id = "dir"
name = "Dir"
choices = ["Both", "H", "V"]

[[param]]
id = "shape"
name = "Shape"
choices = ["Box", "Soft"]
*/

const MAX_RADIUS: f32 = 12.0;

// How much the sample `d` away counts, for radius `r`. A box takes every
// sample within it, and part of the next one out for a fractional radius.
fn weight(d: f32, r: f32, soft: bool) -> f32 {
    if (soft) {
        return max(0.0, 1.0 - d / (r + 1.0));
    }
    return clamp(r + 1.0 - d, 0.0, 1.0);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let r = clamp(p.radius, 0.0, MAX_RADIUS);
    let rx = select(r, 0.0, p.dir == 2u);
    let rl = select(r, 0.0, p.dir == 1u);
    let nx = i32(ceil(rx));
    let nl = i32(ceil(rl));
    let soft = p.shape == 1u;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var dl = -nl; dl <= nl; dl = dl + 1) {
        let wl = weight(abs(f32(dl)), rl, soft);
        let row = (i32(ctx.line) + dl) * ctx.stride + i32(u.info.parity);
        for (var dx = -nx; dx <= nx; dx = dx + 1) {
            let w = wl * weight(abs(f32(dx)), rx, soft);
            sum = sum + fetch_sig(i32(ctx.x) + dx, row) * w;
            total = total + w;
        }
    }
    var o: Outputs;
    o.out = sum / max(total, 1.0e-6);
    return o;
}
