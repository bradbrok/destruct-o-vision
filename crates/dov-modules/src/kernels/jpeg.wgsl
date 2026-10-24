// JPEG: real 8×8 DCT compression every field, and the damage a bad file
// does. Each sample (u, v) of a block holds the block's (u, v) of whatever
// a pass works out, and the 2D transforms go a direction at a time, so no
// pass reads more than eight samples:
//   0. per block: its slips and damage
//   1. the DCT along each row of the block
//   2. down each column: the coefficients, quantized and damaged
//   3. back up each column
//   last: back along the row, to a picture again.
//
// MIT license (LICENSE-MIT), like the module format, so it can be copied.

// The JPEG standard's example quantization tables (Annex K), for quality 50.
const LUMA_Q = array<f32, 64>(
    16.0, 11.0, 10.0, 16.0, 24.0, 40.0, 51.0, 61.0,
    12.0, 12.0, 14.0, 19.0, 26.0, 58.0, 60.0, 55.0,
    14.0, 13.0, 16.0, 24.0, 40.0, 57.0, 69.0, 56.0,
    14.0, 17.0, 22.0, 29.0, 51.0, 87.0, 80.0, 62.0,
    18.0, 22.0, 37.0, 56.0, 68.0, 109.0, 103.0, 77.0,
    24.0, 35.0, 55.0, 64.0, 81.0, 104.0, 113.0, 92.0,
    49.0, 64.0, 78.0, 87.0, 103.0, 121.0, 120.0, 101.0,
    72.0, 92.0, 95.0, 98.0, 112.0, 100.0, 103.0, 99.0,
);

const CHROMA_Q = array<f32, 64>(
    17.0, 18.0, 24.0, 47.0, 99.0, 99.0, 99.0, 99.0,
    18.0, 21.0, 26.0, 66.0, 99.0, 99.0, 99.0, 99.0,
    24.0, 26.0, 56.0, 99.0, 99.0, 99.0, 99.0, 99.0,
    47.0, 66.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0,
    99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0,
    99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0,
    99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0,
    99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0, 99.0,
);

// Blocks to the left a slip is looked for in, at most.
const MAX_BLOCKS: i32 = 256;
// Kinds of damage to one block.
const INTACT: i32 = 0;
const STRIPES: i32 = 1;
const FLAT: i32 = 2;
const GARBAGE: i32 = 3;

// R'G'B' (0..1) to JPEG's Y'CbCr on 0..255. The DCT is linear, so this
// can turn a block's R'G'B' coefficients into Y'CbCr ones; Y' is centered
// on 0, as the DCT wants, by `dc` (8 for a DC coefficient, else 0).
fn to_ycc(c: vec3<f32>, dc: f32) -> vec3<f32> {
    let k = c * 255.0;
    return vec3<f32>(
        dot(k, vec3<f32>(0.299, 0.587, 0.114)) - 128.0 * dc,
        dot(k, vec3<f32>(-0.168736, -0.331264, 0.5)),
        dot(k, vec3<f32>(0.5, -0.418688, -0.081312)),
    );
}

fn from_ycc(v: vec3<f32>) -> vec3<f32> {
    let y = v.x + 128.0;
    return vec3<f32>(
        y + 1.402 * v.z,
        y - 0.344136 * v.y - 0.714136 * v.z,
        y + 1.772 * v.y,
    ) / 255.0;
}

// The orthonormal 8-point DCT, which is JPEG's: frequency `f` at position `i`.
fn basis(f: i32, i: i32) -> f32 {
    let scale = select(0.5, 0.35355339, f == 0);
    return scale * cos(f32((2 * i + 1) * f) * 0.19634954);
}

// Where coefficient (u, v) comes in the order a JPEG file stores them,
// lowest frequencies first, and back.
// The second half of the zigzag mirrors the first.
fn zigzag_index(u: i32, v: i32) -> i32 {
    let late = u + v >= 8;
    let c = select(vec2<i32>(u, v), vec2<i32>(7 - u, 7 - v), late);
    let d = c.x + c.y;
    let k = d * (d + 1) / 2 + select(c.x, c.y, d % 2 == 1);
    return select(k, 63 - k, late);
}

fn zigzag_coefficient(k: i32) -> vec2<i32> {
    var j = k;
    if (k >= 36) {
        j = 63 - k;
    }
    let d = i32(floor((sqrt(8.0 * f32(j) + 1.0) - 1.0) * 0.5 + 1.0e-4));
    let i = j - d * (d + 1) / 2;
    var c = vec2<i32>(i, d - i);
    if (d % 2 == 1) {
        c = vec2<i32>(d - i, i);
    }
    if (k >= 36) {
        c = vec2<i32>(7) - c;
    }
    return c;
}

// Quantizer step for coefficient (u, v) at `quality` (libjpeg's scaling).
fn quantizer(u: i32, v: i32, quality: f32) -> vec3<f32> {
    let q = clamp(quality, 1.0, 100.0);
    let scale = select(200.0 - 2.0 * q, 5000.0 / q, q < 50.0) / 100.0;
    var luma_q = LUMA_Q;
    var chroma_q = CHROMA_Q;
    let i = v * 8 + u;
    let t = vec3<f32>(luma_q[i], chroma_q[i], chroma_q[i]);
    return clamp(floor(t * scale + vec3<f32>(0.5)), vec3<f32>(1.0), vec3<f32>(255.0));
}

// Uniform random in [0, 1) for block (`bx`, `by`), pattern `seed`, question `k`.
fn chance(bx: i32, by: i32, seed: u32, k: u32) -> f32 {
    return hash3(vec3<u32>(bitcast<u32>(bx), bitcast<u32>(by), seed * 16u + k));
}

fn pattern(ctx: Ctx, rate: f32) -> u32 {
    return u32(floor(f32(ctx.field) * max(rate, 0.0) / max(u.info.field_rate, 1.0)));
}

// Stage 0, per block: a bad bit throws the reader off for the rest of the
// row, so from each slip on the colors are off by its amount and blocks
// come from the wrong place. Adds up the slips to this block's left, and
// picks this block's own damage: tint in xyz; in w, the blocks skipped
// (offset by 1024), the kind of damage and a stripe shift.
fn stage0(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let bx = i32(ctx.x) / 8;
    let by = i32(ctx.line) / 8;
    let seed = pattern(ctx, p.rate);
    let corrupt = clamp(p.corrupt, 0.0, 1.0);
    var tint = vec3<f32>(0.0);
    var skip = 0;
    if (corrupt > 0.0) {
        for (var j = max(bx - MAX_BLOCKS, 0); j <= bx; j = j + 1) {
            if (chance(j, by, seed, 0u) < corrupt * 0.04) {
                let n = vec3<f32>(chance(j, by, seed, 1u), chance(j, by, seed, 2u), chance(j, by, seed, 3u));
                tint = tint + (n - vec3<f32>(0.5)) * vec3<f32>(400.0, 600.0, 600.0);
                skip = skip + i32(round((chance(j, by, seed, 4u) - 0.5) * 8.0));
            }
        }
    }
    var kind = INTACT;
    if (chance(bx, by, seed, 5u) < corrupt * 0.3) {
        kind = 1 + min(i32(chance(bx, by, seed, 6u) * 3.0), 2);
    }
    let shift = 1 + min(i32(chance(bx, by, seed, 7u) * 12.0), 11);
    return vec4<f32>(tint, f32((clamp(skip, -1000, 1000) + 1024) * 256 + kind * 16 + shift));
}

// Where sample (`x`, `line`) is: its block, and its place in it.
struct Place {
    block: vec2<i32>,
    at: vec2<i32>,
}

fn place(ctx: Ctx) -> Place {
    var pl: Place;
    pl.block = vec2<i32>(i32(ctx.x), i32(ctx.line)) / 8;
    pl.at = vec2<i32>(i32(ctx.x), i32(ctx.line)) - pl.block * 8;
    return pl;
}

// Stage `k`'s result for place (u, v) of block `b`.
fn in_block(k: u32, b: vec2<i32>, u0: i32, v0: i32) -> vec3<f32> {
    let row = (b.y * 8 + v0) * i32(u.info.row_stride) + i32(u.info.parity);
    return stage_at(k, b.x * 8 + u0, row).xyz;
}

struct Damage {
    tint: vec3<f32>,
    skip: i32,
    kind: i32,
    shift: i32,
}

fn damage(b: vec2<i32>) -> Damage {
    let s = stage_at(0u, b.x, b.y);
    let w = i32(round(s.w));
    var d: Damage;
    d.tint = s.xyz;
    d.skip = w / 256 - 1024;
    d.kind = (w / 16) % 16;
    d.shift = w % 16;
    return d;
}

// Stage 1, per sample: frequency u along row v of the block's R'G'B' (or
// of the block a slip lands on).
fn stage1(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let pl = place(ctx);
    let sx = (pl.block.x + damage(pl.block).skip) * 8;
    let row = i32(ctx.row);
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 8; i = i + 1) {
        sum = sum + fetch_sig(sx + i, row) * basis(pl.at.x, i);
    }
    return vec4<f32>(sum, 0.0);
}

// Stage 2, per sample: coefficient (u, v) in Y'CbCr, quantized and damaged
// as stage 0 says.
fn stage2(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let pl = place(ctx);
    let d = damage(pl.block);
    let ac = pl.at.x != 0 || pl.at.y != 0;
    if (d.kind == FLAT && ac) {
        return vec4<f32>(0.0);
    }
    // Which coefficient lands here: a stripe slides every one up the
    // zigzag, read with this place's quantizer.
    var c = pl.at;
    if (d.kind == STRIPES && ac) {
        let k = zigzag_index(pl.at.x, pl.at.y) - d.shift;
        if (k < 1) {
            return vec4<f32>(0.0);
        }
        c = zigzag_coefficient(k);
    }
    var sum = vec3<f32>(0.0);
    for (var y = 0; y < 8; y = y + 1) {
        sum = sum + in_block(1u, pl.block, c.x, y) * basis(c.y, y);
    }
    let dc = select(0.0, 8.0, c.x == 0 && c.y == 0);
    var steps = round(to_ycc(sum, dc) / quantizer(c.x, c.y, p.quality));
    if (d.kind == GARBAGE && zigzag_index(pl.at.x, pl.at.y) < 10) {
        let seed = pattern(ctx, p.rate);
        let x = i32(ctx.x);
        let line = i32(ctx.line);
        let n = vec3<f32>(chance(x, line, seed, 8u), chance(x, line, seed, 9u), chance(x, line, seed, 10u));
        steps = steps + round((n - vec3<f32>(0.5)) * 8.0);
    }
    var coef = steps * quantizer(pl.at.x, pl.at.y, p.quality);
    if (!ac) {
        coef = coef + d.tint;
    }
    return vec4<f32>(coef, 0.0);
}

// Stage 3, per sample: frequency u back up the columns, at row v.
fn stage3(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let pl = place(ctx);
    var sum = vec3<f32>(0.0);
    for (var v = 0; v < 8; v = v + 1) {
        sum = sum + in_block(2u, pl.block, pl.at.x, v) * basis(v, pl.at.y);
    }
    return vec4<f32>(sum, 0.0);
}

// Last, per sample: back along the row, and into R'G'B'.
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let pl = place(ctx);
    var sum = vec3<f32>(0.0);
    for (var f = 0; f < 8; f = f + 1) {
        sum = sum + in_block(3u, pl.block, f, pl.at.y) * basis(f, pl.at.x);
    }
    var o: Outputs;
    o.out = from_ycc(sum);
    return o;
}
