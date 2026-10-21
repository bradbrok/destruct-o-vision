/* dov-module
format = 1
id = "dov.mirror"
name = "Mirror"
category = "space"
description = "Folds the picture onto itself: left onto right, top onto bottom, or both."
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
id = "mode"
name = "Mode"
choices = ["H", "V", "Quad"]

[[param]]
id = "flip"
name = "Keep"
choices = ["First", "Second"]
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let w = i32(u.info.width);
    let lines = i32(u.info.lines);
    var x = i32(ctx.x);
    var line = i32(ctx.line);
    let second = p.flip == 1u;
    if (p.mode != 1u && ((x >= w / 2) != second)) {
        x = w - 1 - x;
    }
    if (p.mode != 0u && ((line >= lines / 2) != second)) {
        line = lines - 1 - line;
    }
    var o: Outputs;
    o.out = fetch_sig(x, line * ctx.stride + i32(u.info.parity));
    return o;
}
