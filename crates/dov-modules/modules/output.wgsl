/* dov-module
format = 1
id = "dov.output"
name = "Output"
category = "io"
description = "What reaches the output window, and how it's displayed there: clean, or as a picture tube with scan lines, phosphors and glow."
kernel = "pointwise"
screen = true

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"
hidden = true

[[param]]
id = "display"
name = "Display"
choices = ["Clean", "CRT"]

[[param]]
id = "mask_type"
name = "Mask"
choices = ["Grille", "Shadow", "Slot"]

[[param]]
id = "mask"
name = "Mask amt"
range = [0.0, 1.0]
default = 0.5

[[param]]
id = "scanlines"
name = "Lines"
range = [0.0, 1.0]
default = 0.8

[[param]]
id = "glow"
name = "Glow"
range = [0.0, 1.0]
default = 0.3

[[param]]
id = "curvature"
name = "Curve"
range = [0.0, 1.0]
default = 0.3
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = inp.sig;
    return o;
}
