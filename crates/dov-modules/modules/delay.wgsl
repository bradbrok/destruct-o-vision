/* dov-module
format = 1
id = "dov.delay"
name = "Delay"
category = "space"
description = "Field delay with feedback, like a video echo pedal. Patch a signal into Time CV and each pixel looks back a different distance: slit-scan and time displacement. Freeze stops recording and holds what's there."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[output]]
id = "write"
name = "Write"
kind = "rgb"
hidden = true

[[param]]
id = "time"
name = "Time"
range = [0.0, 120.0]
default = 12.0
unit = "fields"
cv = "video"

[[param]]
id = "feedback"
name = "Feedback"
range = [0.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "mix"
name = "Mix"
range = [0.0, 1.0]
default = 1.0
cv = "video"

[[param]]
id = "freeze"
name = "Freeze"
choices = ["Off", "On"]
cv = "control"

[history]
output = "write"
freeze = "freeze"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    // Fractional times blend neighbouring fields, so modulated delays glide.
    let t = max(p.time, 0.0);
    let a0 = i32(floor(t));
    let x = i32(ctx.x);
    let row = i32(ctx.row);
    var early = inp.sig;
    if (a0 >= 1) {
        early = ring_at(x, row, a0);
    }
    let delayed = mix(early, ring_at(x, row, a0 + 1), fract(t));
    var o: Outputs;
    o.out = mix(inp.sig, delayed, p.mix);
    // What gets recorded: the input plus the echo, so repeats decay by
    // Feedback. Under one field there is nothing to echo yet.
    o.write = inp.sig + p.feedback * smoothstep(0.5, 1.0, t) * delayed;
    return o;
}
