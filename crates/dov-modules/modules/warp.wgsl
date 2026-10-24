/* dov-module
format = 1
id = "dov.warp"
name = "Warp"
category = "space"
description = "Bends the picture around a point. Swirl twists it, Bulge swells it like a lens (or pinches it, turned the other way), Ripple sends rings out from the center, and Wave ripples the whole picture like water. Radius is how far the bend reaches; X and Y move its center. Patch a saw into Phase to keep the ripples moving."
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
id = "mode"
name = "Mode"
choices = ["Swirl", "Bulge", "Ripple", "Wave"]

[[param]]
id = "amount"
name = "Amount"
range = [-1.0, 1.0]
default = 0.5
cv = "video"

[[param]]
id = "radius"
name = "Radius"
range = [0.05, 2.0]
default = 0.5
curve = "exp"
cv = "video"

[[param]]
id = "x"
name = "X"
range = [-1.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "y"
name = "Y"
range = [-1.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "freq"
name = "Freq"
range = [1.0, 32.0]
default = 6.0
curve = "exp"
cv = "video"

[[param]]
id = "phase"
name = "Phase"
range = [0.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "edge"
name = "Edge"
choices = ["Black", "Clamp", "Wrap", "Mirror"]
default = "Mirror"
*/

// This sample's place in the picture: centered, in picture heights, with
// y down and x scaled by the display aspect so circles come out round.
fn here(ctx: Ctx) -> vec2<f32> {
    return (ctx.uv - vec2<f32>(0.5)) * vec2<f32>(ctx.aspect, 1.0);
}

// Index `i` reflected back into `0 ..< n`.
fn mirror_index(i: i32, n: i32) -> i32 {
    let m = ((i % (2 * n)) + 2 * n) % (2 * n);
    return select(m, 2 * n - 1 - m, m >= n);
}

// Sample `x` of this field's line `l`, wherever it is, by the Edge rule.
fn tap(x: i32, l: i32, edge: u32) -> vec3<f32> {
    let w = i32(u.info.width);
    let n = i32(u.info.lines);
    var xi = x;
    var li = l;
    switch edge {
        case 0u: {
            if (x < 0 || x >= w || l < 0 || l >= n) {
                return vec3<f32>(0.0);
            }
        }
        case 2u: {
            xi = ((x % w) + w) % w;
            li = ((l % n) + n) % n;
        }
        case 3u: {
            xi = mirror_index(x, w);
            li = mirror_index(l, n);
        }
        default: {}
    }
    return fetch_sig(xi, li * i32(u.info.row_stride) + i32(u.info.parity));
}

// The input at `q` (measured as `here` does), blended from the four nearest
// samples of this field so the bend is smooth.
fn read(q: vec2<f32>, edge: u32) -> vec3<f32> {
    let uv = q / vec2<f32>(u.info.aspect, 1.0) + vec2<f32>(0.5);
    // Samples across, and this field's lines down.
    let s = vec2<f32>(
        uv.x * f32(u.info.width) - 0.5,
        (uv.y * f32(u.info.height) - 0.5 - f32(u.info.parity)) / f32(u.info.row_stride),
    );
    // On a 1/256-sample grid, so an untouched picture comes through exactly.
    let i = floor(s);
    let f = round((s - i) * 256.0) / 256.0;
    let x = i32(i.x);
    let l = i32(i.y);
    let top = mix(tap(x, l, edge), tap(x + 1, l, edge), f.x);
    let bottom = mix(tap(x, l + 1, edge), tap(x + 1, l + 1, edge), f.x);
    return mix(top, bottom, f.y);
}

fn turn(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let q = here(ctx);
    let center = vec2<f32>(p.x * 0.5 * ctx.aspect, p.y * 0.5);
    let d = q - center;
    let r = length(d);
    let radius = max(p.radius, 0.01);
    // 0 at the center, 1 at the edge of the bend.
    let t = r / radius;
    let inside = max(1.0 - t, 0.0);
    // One wavelength, in picture heights.
    let wave = 1.0 / max(p.freq, 0.1);

    var src = q;
    switch p.mode {
        case 0u: {
            // Most turn at the center, none at the radius.
            src = center + turn(d, p.amount * TAU * inside * inside);
        }
        case 1u: {
            // The radius stays put; inside it, distances from the center
            // are raised to a power: above 1 magnifies, below 1 pinches.
            if (t < 1.0) {
                let e = exp2(2.0 * p.amount);
                src = center + d * pow(max(t, 1.0e-4), e - 1.0);
            }
        }
        case 2u: {
            // Rings move in and out by up to a quarter wavelength, fading
            // out toward the radius.
            let push = 0.25 * wave * p.amount * sin(TAU * (r / wave - p.phase)) * inside * inside;
            src = center + d * ((r + push) / max(r, 1.0e-4));
        }
        default: {
            // Lines across move sideways and columns move up and down.
            let k = 0.25 * wave * p.amount;
            src = q + k * vec2<f32>(
                sin(TAU * (d.y / wave - p.phase)),
                sin(TAU * (d.x / wave - p.phase)),
            );
        }
    }
    var o: Outputs;
    o.out = read(src, p.edge);
    return o;
}
