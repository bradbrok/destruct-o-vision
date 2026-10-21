/* dov-module
format = 1
id = "dov.chromakey"
name = "Chroma Key"
category = "process"
description = "Keys on color: where Key (Fg if unpatched) is near the chosen hue and colorful enough, Bg shows through. Spill takes leftover key color out of Fg's edges. Matte is the foreground's opacity."
kernel = "pointwise"

[[input]]
id = "fg"
name = "Fg"
kind = "rgb"

[[input]]
id = "bg"
name = "Bg"
kind = "rgb"

[[input]]
id = "key"
name = "Key"
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
id = "hue"
name = "Hue"
range = [0.0, 1.0]
default = 0.333
cv = "video"

[[param]]
id = "width"
name = "Width"
range = [0.0, 0.5]
default = 0.08
cv = "video"

[[param]]
id = "soft"
name = "Soft"
range = [0.0, 0.3]
default = 0.04

[[param]]
id = "min_sat"
name = "Min sat"
range = [0.0, 1.0]
default = 0.15

[[param]]
id = "spill"
name = "Spill"
range = [0.0, 1.0]
default = 0.5

[[param]]
id = "invert"
name = "Invert"
choices = ["Off", "On"]
*/

// Hue on a color wheel (0 red, 1/3 green, 2/3 blue) and chroma (max - min).
fn hue_chroma(c: vec3<f32>) -> vec2<f32> {
    let chroma = max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
    if (chroma < 1e-5) {
        // Grays have no hue, and atan2(0, 0) can be NaN.
        return vec2<f32>(0.0, 0.0);
    }
    let a = 2.0 * c.r - c.g - c.b;
    let b = 1.7320508 * (c.g - c.b);
    return vec2<f32>(fract(atan2(b, a) / TAU + 1.0), chroma);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let k = select(inp.fg, inp.key, patched_key());
    let hc = hue_chroma(k);
    let dh = abs(fract(hc.x - p.hue + 0.5) - 0.5);
    let soft = max(p.soft, 1e-4);
    var keyed = (1.0 - smoothstep(p.width, p.width + soft, dh)) * smoothstep(p.min_sat, p.min_sat + soft, hc.y);
    if (p.invert == 1u) {
        keyed = 1.0 - keyed;
    }
    // Spill: desaturate what's left of the foreground near the key hue.
    let near = (1.0 - smoothstep(p.width, p.width + 0.25, dh)) * smoothstep(0.0, p.min_sat + soft, hc.y);
    let fg = mix(inp.fg, vec3<f32>(luma(inp.fg)), p.spill * near * (1.0 - keyed));
    var o: Outputs;
    o.out = mix(fg, inp.bg, keyed);
    o.matte = 1.0 - keyed;
    return o;
}
