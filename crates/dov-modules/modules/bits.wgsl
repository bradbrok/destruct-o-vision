/* dov-module
format = 1
id = "dov.bits"
name = "Bits"
category = "glitch"
description = "Treats the picture as numbers: each channel becomes an 8-bit value, and Op combines A and B bit by bit. XOR two ramps for munching squares; Add, Sub and Mul wrap around past white instead of clipping. Rotate turns A's bits over before the Op, Flip inverts the bits it's set to, and Bits keeps only the highest few."
kernel = "pointwise"

[[input]]
id = "a"
name = "A"
kind = "rgb"

[[input]]
id = "b"
name = "B"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "op"
name = "Op"
choices = ["XOR", "AND", "OR", "Add", "Sub", "Mul"]

[[param]]
id = "rotate"
name = "Rotate"
range = [-7.0, 7.0]
default = 0.0
cv = "video"

[[param]]
id = "flip"
name = "Flip"
range = [0.0, 255.0]
default = 0.0
cv = "video"

[[param]]
id = "bits"
name = "Bits"
range = [1.0, 8.0]
default = 8.0
cv = "video"
*/

// 0..1 as a byte; past either end clips.
fn to_byte(v: vec3<f32>) -> vec3<u32> {
    return vec3<u32>(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + vec3<f32>(0.5));
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var a = to_byte(inp.a);
    let b = to_byte(inp.b);
    // Bits pushed off one end come back in at the other.
    let r = vec3<u32>(u32(((i32(round(p.rotate)) % 8) + 8) % 8));
    a = ((a << r) | (a >> (vec3<u32>(8u) - r))) & vec3<u32>(255u);
    var c: vec3<u32>;
    switch p.op {
        case 0u: {
            c = a ^ b;
        }
        case 1u: {
            c = a & b;
        }
        case 2u: {
            c = a | b;
        }
        case 3u: {
            c = a + b;
        }
        case 4u: {
            c = a - b;
        }
        default: {
            c = a * b;
        }
    }
    // Wrapping: only the low byte is kept.
    c = (c ^ vec3<u32>(u32(clamp(round(p.flip), 0.0, 255.0)))) & vec3<u32>(255u);
    let keep = u32(clamp(round(p.bits), 1.0, 8.0));
    c = c & vec3<u32>((0xffu << (8u - keep)) & 0xffu);
    var o: Outputs;
    o.out = vec3<f32>(c) / 255.0;
    return o;
}
