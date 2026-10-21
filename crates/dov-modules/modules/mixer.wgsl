/* dov-module
format = 1
id = "dov.mixer"
name = "Mixer"
category = "process"
description = "Four-channel mixer. Negative levels invert. Every level takes video-rate CV, so each channel is a VCA: patch a shape, ramp or key into a level to wipe between channels."
kernel = "pointwise"

[[input]]
id = "a"
name = "A"
kind = "rgb"

[[input]]
id = "b"
name = "B"
kind = "rgb"

[[input]]
id = "c"
name = "C"
kind = "rgb"

[[input]]
id = "d"
name = "D"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "la"
name = "A"
range = [-1.0, 1.0]
default = 1.0
cv = "video"

[[param]]
id = "lb"
name = "B"
range = [-1.0, 1.0]
default = 1.0
cv = "video"

[[param]]
id = "lc"
name = "C"
range = [-1.0, 1.0]
default = 1.0
cv = "video"

[[param]]
id = "ld"
name = "D"
range = [-1.0, 1.0]
default = 1.0
cv = "video"

[[param]]
id = "master"
name = "Master"
range = [0.0, 2.0]
default = 1.0
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = (inp.a * p.la + inp.b * p.lb + inp.c * p.lc + inp.d * p.ld) * p.master;
    return o;
}
