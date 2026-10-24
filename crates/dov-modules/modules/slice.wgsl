/* dov-module
format = 1
id = "dov.slice"
name = "Slice"
category = "glitch"
description = "Digital tearing: the picture breaks into bands that jump sideways, smear out one line, or split and swap their colors. Amount is the share of bands hit, Size how tall they are, and Rate how often a new pattern comes. Dir V cuts columns that slip up and down instead."
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
range = [0.0, 1.0]
default = 0.3
cv = "video"

[[param]]
id = "shift"
name = "Shift"
range = [0.0, 1.0]
default = 0.2
cv = "video"

[[param]]
id = "size"
name = "Size"
range = [1.0, 128.0]
default = 16.0
curve = "exp"
unit = "lines"

[[param]]
id = "split"
name = "Split"
range = [0.0, 32.0]
default = 6.0
unit = "px"
cv = "video"

[[param]]
id = "smear"
name = "Smear"
range = [0.0, 1.0]
default = 0.2

[[param]]
id = "swap"
name = "Swap"
range = [0.0, 1.0]
default = 0.2

[[param]]
id = "rate"
name = "Rate"
range = [0.5, 60.0]
default = 6.0
curve = "exp"
unit = "Hz"
cv = "control"

[[param]]
id = "dir"
name = "Dir"
choices = ["H", "V"]
*/

// Uniform random in [0, 1) for band `band`, pattern `seed`, question `k`.
fn chance(band: i32, seed: u32, k: u32) -> f32 {
    return hash3(vec3<u32>(bitcast<u32>(band), seed, k));
}

// The input `along` samples into the line (or lines down the column, for V)
// and `across` lines down (or samples across), wrapping along.
fn at(along: i32, across: i32, vertical: bool) -> vec3<f32> {
    let w = i32(u.info.width);
    let n = i32(u.info.lines);
    var x = across;
    var l = ((along % n) + n) % n;
    if (!vertical) {
        x = ((along % w) + w) % w;
        l = across;
    }
    return fetch_sig(x, l * i32(u.info.row_stride) + i32(u.info.parity));
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let vertical = p.dir == 1u;
    let seed = u32(floor(f32(ctx.field) * max(p.rate, 0.0) / max(u.info.field_rate, 1.0)));
    // Bands run across the shift: lines for H, columns for V. A column
    // band as wide on screen as a band of Size lines is tall.
    var across = i32(ctx.line);
    var along = i32(ctx.x);
    var span = f32(u.info.width);
    var size = max(round(p.size), 1.0);
    if (vertical) {
        across = i32(ctx.x);
        along = i32(ctx.line);
        span = f32(u.info.lines);
        size = max(round(size * f32(u.info.width) / (f32(u.info.lines) * ctx.aspect)), 1.0);
    }

    // Cells of Size, offset anew with each pattern, each cut in two at a
    // random point: bands of uneven heights.
    let pos = f32(across) + chance(0, seed, 0u) * size;
    let cell = floor(pos / size);
    let cut = 0.2 + 0.6 * chance(i32(cell), seed, 1u);
    let second = fract(pos / size) >= cut;
    let band = i32(cell) * 2 + select(0, 1, second);

    var o: Outputs;
    o.out = inp.sig;
    if (chance(band, seed, 2u) >= p.amount) {
        return o;
    }
    let jump = i32(round((chance(band, seed, 3u) * 2.0 - 1.0) * p.shift * span));
    var row = across;
    if (chance(band, seed, 4u) < p.smear) {
        // The band's first line, over and over.
        let first = cell * size - chance(0, seed, 0u) * size + select(0.0, cut * size, second);
        row = i32(ceil(first));
    }
    let s = i32(round(p.split * chance(band, seed, 5u)));
    var c = vec3<f32>(
        at(along - jump - s, row, vertical).r,
        at(along - jump, row, vertical).g,
        at(along - jump + s, row, vertical).b,
    );
    if (chance(band, seed, 6u) < p.swap) {
        switch u32(chance(band, seed, 7u) * 5.0) {
            case 0u: {
                c = c.gbr;
            }
            case 1u: {
                c = c.brg;
            }
            case 2u: {
                c = c.rbg;
            }
            case 3u: {
                c = c.bgr;
            }
            default: {
                c = c.grb;
            }
        }
    }
    o.out = c;
    return o;
}
