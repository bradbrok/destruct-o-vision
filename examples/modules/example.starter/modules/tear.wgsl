/* dov-module
format = 1
id = "example.tear"
name = "Line Tear"
category = "space"
tags = ["glitch", "vhs"]
description = "Shoves bands of lines sideways by random amounts, like a VCR losing its grip. New tears every few fields."
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
id = "amount"
name = "Amount"
range = [0.0, 0.5]
default = 0.08
cv = "video"

[[param]]
id = "band"
name = "Band"
range = [1.0, 64.0]
default = 6.0
curve = "exp"
unit = "lines"

[[param]]
id = "hold"
name = "Hold"
range = [1.0, 60.0]
default = 4.0
curve = "exp"
unit = "fields"

[[param]]
id = "chance"
name = "Chance"
range = [0.0, 1.0]
default = 0.3
cv = "control"
*/

// Each band of lines draws its own random number, renewed every `hold`
// fields. Bands that draw under `chance` are shifted by up to `amount` of
// the picture's width. `fetch_sig` reads the input anywhere in the frame.
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let band = u32(max(p.band, 1.0));
    let epoch = ctx.field / u32(max(p.hold, 1.0));
    let roll = hash3(vec3<u32>(ctx.line / band, epoch, 7u));
    var shift = 0.0;
    if (roll < p.chance) {
        // Reuse the draw for the size and direction of the shift.
        shift = (roll / max(p.chance, 1e-3) * 2.0 - 1.0) * p.amount;
    }
    let dx = i32(round(shift * f32(u.info.width)));
    var o: Outputs;
    o.out = fetch_sig(i32(ctx.x) - dx, i32(ctx.row));
    return o;
}
