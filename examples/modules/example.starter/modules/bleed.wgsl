/* dov-module
format = 1
id = "example.bleed"
name = "Bleed"
category = "process"
tags = ["vhs", "smear"]
description = "Bright parts of each line bleed to the right, like an overloaded tape: the longer the Length, the longer the trail."
kernel = "linescan"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "length"
name = "Length"
range = [0.0, 1.0]
default = 0.6
cv = "video"

[[param]]
id = "threshold"
name = "Thresh"
range = [0.0, 1.0]
default = 0.6
cv = "video"
*/

// A line scan runs left to right, carrying `State` from sample to sample
// like a capacitor: it charges at once on bright picture and drains after.
struct State {
    glow: vec3<f32>,
}

fn init_state() -> State {
    return State(vec3<f32>(0.0));
}

fn process(ctx: Ctx, inp: Inputs, p: Params, s: ptr<function, State>) -> Outputs {
    let hot = max(inp.sig - vec3<f32>(p.threshold), vec3<f32>(0.0));
    let keep = mix(0.5, 0.995, clamp(p.length, 0.0, 1.0));
    (*s).glow = max((*s).glow * keep, hot);
    var o: Outputs;
    o.out = inp.sig + (*s).glow;
    return o;
}
