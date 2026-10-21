/* dov-module
format = 1
id = "dov.edge"
name = "Edge"
category = "process"
description = "Gradient magnitude of luma: outlines."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "mono"

[[output]]
id = "out"
name = "Out"
kind = "mono"

[[param]]
id = "gain"
name = "Gain"
range = [0.0, 16.0]
default = 4.0
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let x = i32(ctx.x);
    let row = i32(ctx.row);
    let gx = fetch_sig(x + 1, row) - fetch_sig(x - 1, row);
    let gy = fetch_sig(x, row + ctx.stride) - fetch_sig(x, row - ctx.stride);
    var o: Outputs;
    o.out = length(vec2<f32>(gx, gy)) * p.gain;
    return o;
}
