/* dov-module
format = 1
id = "dov.filter"
name = "Filter"
category = "process"
description = "One-pole RC filter running along each line, like an analog circuit: smears and rings horizontally only."
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
id = "cutoff"
name = "Cutoff"
range = [0.01, 40.0]
default = 1.5
curve = "exp"
unit = "MHz"
cv = "video"

[[param]]
id = "mode"
name = "Mode"
choices = ["Low", "High"]
*/

struct State {
    y: vec3<f32>,
}

fn init_state() -> State {
    // Lines start from blanking (black), so a slow filter fades in from the left.
    return State(vec3<f32>(0.0));
}

fn process(ctx: Ctx, inp: Inputs, p: Params, s: ptr<function, State>) -> Outputs {
    let fc = max(p.cutoff, 0.001) * 1e6;
    let a = 1.0 - exp(-TAU * fc / u.info.sample_rate);
    (*s).y = (*s).y + a * (inp.sig - (*s).y);
    var o: Outputs;
    o.out = select((*s).y, inp.sig - (*s).y, p.mode == 1u);
    return o;
}
