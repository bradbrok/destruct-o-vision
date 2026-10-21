/* dov-module
format = 1
id = "dov.combine"
name = "RGB Combine"
category = "color"
description = "Three mono signals into one RGB signal."
kernel = "pointwise"

[[input]]
id = "r"
name = "R"
kind = "mono"

[[input]]
id = "g"
name = "G"
kind = "mono"

[[input]]
id = "b"
name = "B"
kind = "mono"

[[output]]
id = "out"
name = "Out"
kind = "rgb"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = vec3<f32>(inp.r, inp.g, inp.b);
    return o;
}
