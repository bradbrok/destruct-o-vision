/* dov-module
format = 1
id = "dov.fields"
name = "Fields"
category = "analog"
description = "Interlace gone wrong, as it did in edit suites and cheap converters. Swap puts each frame's two fields in the wrong order, so motion steps back before it steps on. Drop throws the second field away and fills its lines from the first: half the detail, but no combing. Offset puts the second field's lines Lines out of place, for stair-stepped edges. Blend mixes each field with the one before it, ghosting motion like a cheap deinterlacer. On a progressive raster, frames take the place of fields."
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
id = "mode"
name = "Mode"
choices = ["Swap", "Drop", "Offset", "Blend"]
cv = "control"

[[param]]
id = "lines"
name = "Lines"
range = [-3.0, 3.0]
default = 1.0

[[param]]
id = "mix"
name = "Mix"
range = [0.0, 1.0]
default = 0.5
cv = "video"

[history]
output = "write"
fields = 3
*/

// The last field's picture at this place: on an interlaced raster, made
// from its own lines either side, since this row is the other field's.
fn last_field(x: i32, row: i32, stride: i32) -> vec3<f32> {
    if (stride == 2) {
        return 0.5 * (ring_at(x, row - 1, 1) + ring_at(x, row + 1, 1));
    }
    return ring_at(x, row, 1);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let x = i32(ctx.x);
    let row = i32(ctx.row);
    let s = ctx.stride;
    // The second field of a frame (or every other frame, progressive).
    let second = ctx.field % 2u == 1u;
    var out = inp.sig;
    switch p.mode {
        case 0u: {
            // Swap: second fields show the field two before, so the order
            // runs 0, -1, 2, 1, 4, 3…
            if (second) {
                out = ring_at(x, row, 2);
            }
        }
        case 1u: {
            if (second) {
                out = last_field(x, row, s);
            }
        }
        case 2u: {
            if (second) {
                out = fetch_sig(x, row + i32(round(p.lines)) * s);
            }
        }
        default: {
            out = mix(inp.sig, last_field(x, row, s), p.mix);
        }
    }
    var o: Outputs;
    o.out = out;
    o.write = inp.sig;
    return o;
}
