/* dov-module
format = 1
id = "dov.blur"
name = "Blur"
category = "process"
description = "Box blur within the field, horizontally, vertically or both."
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
range = [0.0, 8.0]
default = 2.0

[[param]]
id = "dir"
name = "Dir"
choices = ["Both", "H", "V"]
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let r = clamp(i32(round(p.radius)), 0, 8);
    let rx = select(r, 0, p.dir == 2u);
    let rl = select(r, 0, p.dir == 1u);
    var sum = vec3<f32>(0.0);
    for (var dl = -rl; dl <= rl; dl = dl + 1) {
        let row = (i32(ctx.line) + dl) * ctx.stride + i32(u.info.parity);
        for (var dx = -rx; dx <= rx; dx = dx + 1) {
            sum = sum + fetch_sig(i32(ctx.x) + dx, row);
        }
    }
    var o: Outputs;
    o.out = sum / f32((2 * rx + 1) * (2 * rl + 1));
    return o;
}
