/* dov-module
format = 1
id = "dov.noise"
name = "Noise"
category = "source"
description = "Random per sample (snow), per line, per block, or per field."
kernel = "pointwise"

[[output]]
id = "out"
name = "Out"
kind = "mono"

[[param]]
id = "mode"
name = "Mode"
choices = ["Snow", "Lines", "Blocks", "Field"]

[[param]]
id = "size"
name = "Block"
range = [1.0, 64.0]
default = 8.0
curve = "exp"
unit = "px"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var seed: vec3<u32>;
    switch p.mode {
        case 0u: { seed = vec3<u32>(ctx.x, ctx.row, ctx.field); }
        case 1u: { seed = vec3<u32>(7u, ctx.row, ctx.field); }
        case 2u: {
            let s = max(u32(p.size), 1u);
            seed = vec3<u32>(ctx.x / s, ctx.row / s, ctx.field);
        }
        default: { seed = vec3<u32>(7u, 11u, ctx.field); }
    }
    var o: Outputs;
    o.out = hash3(seed);
    return o;
}
