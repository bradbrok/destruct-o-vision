/* dov-module
format = 1
id = "dov.shape"
name = "Shape"
category = "source"
description = "Circle, diamond, square or ring. Patch oscillators into the position CVs to bend it line by line."
kernel = "pointwise"

[[output]]
id = "out"
name = "Out"
kind = "mono"

[[param]]
id = "shape"
name = "Shape"
choices = ["Circle", "Diamond", "Square", "Ring"]

[[param]]
id = "size"
name = "Size"
range = [0.0, 1.0]
default = 0.3
cv = "video"

[[param]]
id = "soft"
name = "Soft"
range = [0.0, 0.5]
default = 0.02

[[param]]
id = "x"
name = "X"
range = [0.0, 1.0]
default = 0.5
cv = "video"

[[param]]
id = "y"
name = "Y"
range = [0.0, 1.0]
default = 0.5
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let d0 = (ctx.uv - vec2<f32>(p.x, p.y)) * vec2<f32>(ctx.aspect, 1.0);
    let soft = max(p.soft, 1e-4);
    var d: f32;
    var edge = p.size;
    switch p.shape {
        case 0u: { d = length(d0); }
        case 1u: { d = abs(d0.x) + abs(d0.y); }
        case 2u: { d = max(abs(d0.x), abs(d0.y)); }
        default: {
            d = abs(length(d0) - p.size);
            edge = p.size * 0.15;
        }
    }
    var o: Outputs;
    o.out = 1.0 - smoothstep(edge - soft, edge + soft, d);
    return o;
}
