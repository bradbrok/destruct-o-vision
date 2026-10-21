/* dov-module
format = 1
id = "dov.colorize"
name = "Colorize"
category = "color"
description = "Maps luma to a hue sweep through YIQ chroma. Black stays black."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "mono"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "hue"
name = "Hue"
range = [0.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "spread"
name = "Spread"
range = [0.0, 4.0]
default = 1.0

[[param]]
id = "sat"
name = "Sat"
range = [0.0, 2.0]
default = 0.8
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let angle = TAU * (p.hue + inp.sig * p.spread);
    // Chroma fades out near black, so black stays black.
    let chroma = 0.35 * p.sat * smoothstep(0.0, 0.15, inp.sig);
    var o: Outputs;
    o.out = yiq_to_rgb(inp.sig, chroma * cos(angle), chroma * sin(angle));
    return o;
}
