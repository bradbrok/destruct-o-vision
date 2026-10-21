/* dov-module
format = 1
id = "dov.displace"
name = "Displace"
category = "space"
description = "Shifts the picture by video-rate signals, centered on 0.5. Per-line noise into H gives timebase wobble; a field-locked VCO bends the picture."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[input]]
id = "h"
name = "H shift"
kind = "mono"
default = 0.5

[[input]]
id = "v"
name = "V shift"
kind = "mono"
default = 0.5

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "h_amount"
name = "H amt"
range = [-0.5, 0.5]
default = 0.05
cv = "video"

[[param]]
id = "v_amount"
name = "V amt"
range = [-0.5, 0.5]
default = 0.0

[[param]]
id = "edge"
name = "Edge"
choices = ["Clamp", "Wrap", "Black"]
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let w = i32(u.info.width);
    let lines = i32(u.info.lines);
    var x = i32(ctx.x) - i32(round((inp.h - 0.5) * 2.0 * p.h_amount * f32(w)));
    var line = i32(ctx.line) - i32(round((inp.v - 0.5) * 2.0 * p.v_amount * f32(lines)));
    var o: Outputs;
    if (p.edge == 2u && (x < 0 || x >= w || line < 0 || line >= lines)) {
        o.out = vec3<f32>(0.0);
        return o;
    }
    if (p.edge == 1u) {
        x = ((x % w) + w) % w;
        line = ((line % lines) + lines) % lines;
    }
    o.out = fetch_sig(x, line * ctx.stride + i32(u.info.parity));
    return o;
}
