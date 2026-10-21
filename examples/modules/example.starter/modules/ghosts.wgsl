/* dov-module
format = 1
id = "example.ghosts"
name = "Ghosts"
category = "space"
tags = ["echo", "time"]
description = "Three fading echoes of the picture, each Spacing fields older than the last, drifting towards blue as they age."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

# What the history keeps. Hidden: it's not a jack, just storage.
[[output]]
id = "store"
kind = "rgb"
hidden = true

[[param]]
id = "spacing"
name = "Spacing"
range = [1.0, 40.0]
default = 6.0
unit = "fields"
cv = "control"

[[param]]
id = "decay"
name = "Decay"
range = [0.0, 1.0]
default = 0.6
cv = "video"

[[param]]
id = "tint"
name = "Tint"
range = [0.0, 1.0]
default = 0.4

[history]
output = "store"
*/

// `ring_at(x, row, age)` reads what `store` held `age` fields ago.
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let x = i32(ctx.x);
    let row = i32(ctx.row);
    let step = max(i32(round(p.spacing)), 1);
    var echoes = vec3<f32>(0.0);
    var gain = 1.0;
    for (var k = 1; k <= 3; k = k + 1) {
        gain = gain * p.decay;
        let tint = mix(vec3<f32>(1.0), vec3<f32>(0.4, 0.8, 1.3), p.tint * f32(k) / 3.0);
        echoes = max(echoes, ring_at(x, row, step * k) * tint * gain);
    }
    var o: Outputs;
    o.out = max(inp.sig, echoes);
    o.store = inp.sig;
    return o;
}
