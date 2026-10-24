/* dov-module
format = 1
id = "dov.transform"
name = "Transform"
category = "space"
description = "Zooms, turns, stretches and moves the picture, smoothly between pixels. Every knob takes video-rate CV, so a ramp into Rotate twists the picture and noise into Zoom crumples it. Edge says what fills in past the picture's sides: black, the edge pixels, the picture repeated, or the picture mirrored."
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
id = "zoom"
name = "Zoom"
range = [0.25, 4.0]
default = 1.0
curve = "exp"
cv = "video"

[[param]]
id = "rotate"
name = "Rotate"
range = [-180.0, 180.0]
default = 0.0
unit = "°"
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
id = "stretch"
name = "Stretch"
range = [-1.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "edge"
name = "Edge"
choices = ["Black", "Clamp", "Wrap", "Mirror"]
*/

// This sample's place in the picture: centered, in picture heights, with
// y down and x scaled by the display aspect so turns and circles come out
// round on screen.
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
// samples of this field so slow moves glide instead of stepping.
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

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    // Undo the move, then the turn, then the scale.
    let d = here(ctx) - vec2<f32>(p.x * ctx.aspect, p.y);
    let a = radians(p.rotate);
    let c = cos(a);
    let s = sin(a);
    let turned = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
    let scale = max(p.zoom, 0.01) * vec2<f32>(exp2(p.stretch), exp2(-p.stretch));
    var o: Outputs;
    o.out = read(turned / scale, p.edge);
    return o;
}
