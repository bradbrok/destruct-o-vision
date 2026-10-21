/* dov-module
format = 1
id = "dov.split"
name = "RGB Split"
category = "color"
description = "One RGB signal into R, G, B and luma."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "r"
name = "R"
kind = "mono"

[[output]]
id = "g"
name = "G"
kind = "mono"

[[output]]
id = "b"
name = "B"
kind = "mono"

[[output]]
id = "y"
name = "Luma"
kind = "mono"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.r = inp.sig.r;
    o.g = inp.sig.g;
    o.b = inp.sig.b;
    o.y = luma(inp.sig);
    return o;
}
