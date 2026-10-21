/* dov-module
format = 1
id = "dov.monitor"
name = "Monitor"
category = "io"
description = "A small screen in the rack showing whatever is patched in."
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
