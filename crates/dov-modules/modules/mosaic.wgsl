/* dov-module
format = 1
id = "dov.mosaic"
name = "Mosaic"
category = "space"
description = "Sample-and-hold in two dimensions: big digital blocks."
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
id = "sx"
name = "Width"
range = [1.0, 128.0]
default = 16.0
curve = "exp"
unit = "px"
cv = "video"

[[param]]
id = "sy"
name = "Height"
range = [1.0, 64.0]
default = 8.0
curve = "exp"
unit = "lines"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let sx = max(i32(p.sx), 1);
    let sy = max(i32(p.sy), 1);
    let x = (i32(ctx.x) / sx) * sx + sx / 2;
    let line = (i32(ctx.line) / sy) * sy + sy / 2;
    var o: Outputs;
    o.out = fetch_sig(x, line * ctx.stride + i32(u.info.parity));
    return o;
}
