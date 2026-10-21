/* dov-module
format = 1
id = "dov.vca"
name = "VCA"
category = "process"
description = "Multiplies a signal by a video-rate control (ring modulation), plus offset."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[input]]
id = "cv"
name = "CV"
kind = "mono"
default = 1.0

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "gain"
name = "Gain"
range = [-2.0, 2.0]
default = 1.0

[[param]]
id = "offset"
name = "Offset"
range = [-1.0, 1.0]
default = 0.0
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = inp.sig * inp.cv * p.gain + vec3<f32>(p.offset);
    return o;
}
