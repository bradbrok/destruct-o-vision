/* dov-module
format = 1
id = "dov.blend"
name = "Blend"
category = "process"
description = "Two-input mixer with Photoshop's blend modes: Top is laid over Base in the chosen mode and faded in by Mix, so in Normal Mix crossfades. Dissolve scatters Top's pixels by Mix. Mode takes video-rate CV too, so a ramp or shape can sweep through the modes across the picture."
kernel = "pointwise"

[[input]]
id = "base"
name = "Base"
kind = "rgb"

[[input]]
id = "top"
name = "Top"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "mode"
name = "Mode"
choices = [
    "Normal", "Dissolve",
    "Darken", "Multiply", "Color Burn", "Linear Burn", "Darker Color",
    "Lighten", "Screen", "Color Dodge", "Linear Dodge", "Lighter Color",
    "Overlay", "Soft Light", "Hard Light", "Vivid Light", "Linear Light", "Pin Light", "Hard Mix",
    "Difference", "Exclusion", "Subtract", "Divide",
    "Hue", "Saturation", "Color", "Luminosity",
]
default = "Normal"
cv = "video"

[[param]]
id = "mix"
name = "Mix"
range = [0.0, 1.0]
default = 0.5
cv = "video"
*/

// Luminosity as Photoshop weighs it for the color modes.
const LUM = vec3<f32>(0.3, 0.59, 0.11);
const TINY = 1e-6;

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, LUM);
}

// Brings a color with a channel outside 0..1 back in, keeping its luminosity.
fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let lo = min(min(c.r, c.g), c.b);
    let hi = max(max(c.r, c.g), c.b);
    var o = c;
    if (lo < 0.0) {
        o = l + (o - l) * l / max(l - lo, TINY);
    }
    if (hi > 1.0) {
        o = l + (o - l) * (1.0 - l) / max(hi - l, TINY);
    }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + (l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

// The same hue with saturation `s`: the lowest channel goes to 0, the
// highest to `s`.
fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let spread = sat(c);
    if (spread < TINY) {
        return vec3<f32>(0.0);
    }
    return (c - min(min(c.r, c.g), c.b)) * s / spread;
}

// A white base stays white; otherwise darken it by the inverse of b.
fn burn(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let r = 1.0 - min(vec3<f32>(1.0), (1.0 - a) / max(b, vec3<f32>(TINY)));
    return select(r, vec3<f32>(1.0), a >= vec3<f32>(1.0));
}

// A black base stays black; otherwise brighten it by b.
fn dodge(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let r = min(vec3<f32>(1.0), a / max(1.0 - b, vec3<f32>(TINY)));
    return select(r, vec3<f32>(0.0), a <= vec3<f32>(0.0));
}

fn hard_light(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return select(2.0 * a * b, 1.0 - 2.0 * (1.0 - a) * (1.0 - b), b >= vec3<f32>(0.5));
}

fn blend(a: vec3<f32>, b: vec3<f32>, mode: u32) -> vec3<f32> {
    let upper = b >= vec3<f32>(0.5);
    switch mode {
        // Darken
        case 2u: { return min(a, b); }
        case 3u: { return a * b; }
        case 4u: { return burn(a, b); }
        case 5u: { return a + b - 1.0; }
        case 6u: { return select(b, a, dot(a, vec3<f32>(1.0)) <= dot(b, vec3<f32>(1.0))); }
        // Lighten
        case 7u: { return max(a, b); }
        case 8u: { return 1.0 - (1.0 - a) * (1.0 - b); }
        case 9u: { return dodge(a, b); }
        case 10u: { return a + b; }
        case 11u: { return select(b, a, dot(a, vec3<f32>(1.0)) >= dot(b, vec3<f32>(1.0))); }
        // Contrast
        case 12u: { return hard_light(b, a); }
        case 13u: {
            let dark = 2.0 * a * b + a * a * (1.0 - 2.0 * b);
            let light = 2.0 * a * (1.0 - b) + sqrt(a) * (2.0 * b - 1.0);
            return select(dark, light, upper);
        }
        case 14u: { return hard_light(a, b); }
        case 15u: { return select(burn(a, 2.0 * b), dodge(a, 2.0 * b - 1.0), upper); }
        case 16u: { return a + 2.0 * b - 1.0; }
        case 17u: { return select(min(a, 2.0 * b), max(a, 2.0 * b - 1.0), upper); }
        case 18u: { return select(vec3<f32>(0.0), vec3<f32>(1.0), a + b >= vec3<f32>(1.0)); }
        // Inversion
        case 19u: { return abs(a - b); }
        case 20u: { return a + b - 2.0 * a * b; }
        case 21u: { return a - b; }
        case 22u: { return a / max(b, vec3<f32>(TINY)); }
        // Component
        case 23u: { return set_lum(set_sat(b, sat(a)), lum(a)); }
        case 24u: { return set_lum(set_sat(a, sat(b)), lum(a)); }
        case 25u: { return set_lum(b, lum(a)); }
        case 26u: { return set_lum(a, lum(b)); }
        // Normal and Dissolve show Top as it is.
        default: { return b; }
    }
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    var amount = p.mix;
    if (p.mode == 1u) {
        // Dissolve: each pixel is all Top or all Base, with Mix the odds.
        let roll = hash3(vec3<u32>(ctx.x, ctx.row, ctx.field));
        amount = select(0.0, 1.0, roll < p.mix);
    }
    // The modes are defined on 0..1, as in Photoshop.
    let a = clamp(inp.base, vec3<f32>(0.0), vec3<f32>(1.0));
    let b = clamp(inp.top, vec3<f32>(0.0), vec3<f32>(1.0));
    let blended = clamp(blend(a, b, p.mode), vec3<f32>(0.0), vec3<f32>(1.0));
    // At no Mix, Base passes untouched, headroom and all.
    o.out = mix(inp.base, blended, amount);
    return o;
}
