/* dov-module
format = 1
id = "dov.posterize"
name = "Posterize"
category = "process"
description = "Quantizes each channel to a few levels."
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
id = "levels"
name = "Levels"
range = [2.0, 32.0]
default = 4.0
curve = "exp"
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let levels = max(round(p.levels), 2.0);
    var o: Outputs;
    o.out = floor(clamp(inp.sig, vec3<f32>(0.0), vec3<f32>(0.9999)) * levels) / (levels - 1.0);
    return o;
}
