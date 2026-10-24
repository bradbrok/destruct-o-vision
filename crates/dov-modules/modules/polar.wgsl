/* dov-module
format = 1
id = "dov.polar"
name = "Polar"
category = "space"
description = "Wraps the picture around a point. Planet bends it into a disc, its bottom edge at the center; Tunnel stretches it down an endless tube; Unwrap opens a circle back out into a strip. Repeat goes around more than once, Twist turns it into a spiral, and a saw patched into Turn or Travel keeps it spinning or flying."
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
choices = ["Planet", "Tunnel", "Unwrap"]

[[param]]
id = "turn"
name = "Turn"
range = [0.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "travel"
name = "Travel"
range = [0.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "size"
name = "Size"
range = [0.1, 2.0]
default = 0.5
curve = "exp"
cv = "video"

[[param]]
id = "repeat"
name = "Repeat"
range = [1.0, 8.0]
default = 1.0

[[param]]
id = "twist"
name = "Twist"
range = [-4.0, 4.0]
default = 0.0
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
id = "edge"
name = "Edge"
choices = ["Black", "Clamp", "Wrap", "Mirror"]
default = "Wrap"
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

// The input at `uv` (0..1 across and down the picture), blended from the
// four nearest samples of this field.
fn read_uv(uv: vec2<f32>, edge: u32) -> vec3<f32> {
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

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let center = vec2<f32>(p.x * 0.5 * ctx.aspect, p.y * 0.5);
    let size = max(p.size, 0.01);
    let repeat = max(round(p.repeat), 1.0);
    var o: Outputs;

    if (p.mode == 2u) {
        // Planet backward: across the strip is around the circle, clockwise
        // from the bottom, and down the strip is in toward the center.
        let r = max(1.0 - ctx.uv.y + p.travel, 0.0) * size;
        let a = TAU * (ctx.uv.x * repeat - p.turn - p.twist * r - 0.5);
        let q = center + r * vec2<f32>(sin(a), -cos(a));
        o.out = read_uv(q / vec2<f32>(ctx.aspect, 1.0) + vec2<f32>(0.5), p.edge);
        return o;
    }

    let d = here(ctx) - center;
    let r = length(d);
    // Turns around the center, clockwise: the picture's middle goes at the
    // top and its sides meet at the bottom.
    let around = atan2(d.x, -d.y) / TAU + 0.5;
    let x = fract(around * repeat + p.turn + p.twist * r);
    if (p.mode == 1u) {
        // The picture repeats down the tube, once per Size of distance at
        // the rim, ever smaller toward the vanishing point, which fades.
        let depth = size * 0.5 / max(r, 1.0e-4) + p.travel;
        let fade = clamp(r / (0.1 * size), 0.0, 1.0);
        o.out = read_uv(vec2<f32>(x, fract(depth)), p.edge) * fade;
        return o;
    }
    o.out = read_uv(vec2<f32>(x, 1.0 - r / size + p.travel), p.edge);
    return o;
}
