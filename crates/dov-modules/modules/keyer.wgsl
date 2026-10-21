/* dov-module
format = 1
id = "dov.keyer"
name = "Keyer"
category = "process"
description = "Luma key: where Key is above the threshold show Fg, else Bg."
kernel = "pointwise"

[[input]]
id = "key"
name = "Key"
kind = "mono"

[[input]]
id = "fg"
name = "Fg"
kind = "rgb"
default = 1.0

[[input]]
id = "bg"
name = "Bg"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[output]]
id = "matte"
name = "Matte"
kind = "mono"

[[param]]
id = "threshold"
name = "Thresh"
range = [0.0, 1.0]
default = 0.5
cv = "video"

[[param]]
id = "soft"
name = "Soft"
range = [0.0, 0.5]
default = 0.02

[[param]]
id = "invert"
name = "Invert"
choices = ["Off", "On"]
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let soft = max(p.soft, 1e-4);
    var m = smoothstep(p.threshold - soft, p.threshold + soft, inp.key);
    if (p.invert == 1u) {
        m = 1.0 - m;
    }
    var o: Outputs;
    o.out = mix(inp.bg, inp.fg, m);
    o.matte = m;
    return o;
}
