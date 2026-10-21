/* dov-module
format = 1
id = "dov.ramps"
name = "Ramps"
category = "source"
description = "Raster-locked ramps: horizontal, vertical, radial and angular."
kernel = "pointwise"

[[output]]
id = "h"
name = "H"
kind = "mono"

[[output]]
id = "v"
name = "V"
kind = "mono"

[[output]]
id = "radial"
name = "Radial"
kind = "mono"

[[output]]
id = "angle"
name = "Angle"
kind = "mono"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.h = ctx.uv.x;
    o.v = ctx.uv.y;
    let d = (ctx.uv - vec2<f32>(0.5)) * vec2<f32>(ctx.aspect, 1.0);
    o.radial = length(d) / length(vec2<f32>(ctx.aspect * 0.5, 0.5));
    o.angle = atan2(d.y, d.x) / TAU + 0.5;
    return o;
}
