/* dov-module
format = 1
id = "dov.kaleido"
name = "Kaleido"
category = "space"
description = "A kaleidoscope: one wedge of the picture, repeated around the center. Mirror flips every wedge so they meet seamlessly. Segments glides between counts; Turn spins the whole pattern, Angle turns the wedge within the picture, and X and Y move where it's taken from. Patch a saw into Turn to keep it spinning."
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
id = "segments"
name = "Segments"
range = [2.0, 16.0]
default = 6.0
cv = "video"

[[param]]
id = "turn"
name = "Turn"
range = [-180.0, 180.0]
default = 0.0
unit = "°"
cv = "video"

[[param]]
id = "angle"
name = "Angle"
range = [-180.0, 180.0]
default = 0.0
unit = "°"
cv = "video"

[[param]]
id = "zoom"
name = "Zoom"
range = [0.25, 4.0]
default = 1.0
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
id = "mode"
name = "Mode"
choices = ["Mirror", "Repeat"]

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
// samples of this field.
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
    let d = here(ctx);
    let r = length(d);
    // Which wedge this is, and how far into it, centered on its middle.
    let wedge = TAU / max(p.segments, 1.0);
    let a = atan2(d.y, d.x) - radians(p.turn);
    let k = floor(a / wedge + 0.5);
    var f = a - k * wedge;
    if (p.mode == 0u) {
        f = abs(f);
    }
    let dir = f + radians(p.angle);
    let origin = vec2<f32>(p.x * 0.5 * ctx.aspect, p.y * 0.5);
    var o: Outputs;
    o.out = read(origin + r / max(p.zoom, 0.01) * vec2<f32>(cos(dir), sin(dir)), p.edge);
    return o;
}
