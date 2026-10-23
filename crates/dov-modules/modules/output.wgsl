/* dov-module
format = 1
id = "dov.output"
name = "Output"
category = "io"
description = "What reaches the output window, shown clean. For a picture tube, use CRT instead."
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
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = inp.sig;
    return o;
}
