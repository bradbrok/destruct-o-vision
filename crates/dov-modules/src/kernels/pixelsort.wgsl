// Pixel Sort: runs of pixels sorted by value. An exact sort, ties kept in
// their order. Each sample:
//   0. reads the values of itself and the three after it
//   1. if it's sorted, finds its run and its rank in it
//   2. gathers its own rank and the three after it
//   last: takes the pixel whose rank matches its place in the run.
// Passes after the first read four places at a time.
//
// MIT license (LICENSE-MIT), like the module format, so it can be copied.

const MAX_LENGTH: i32 = 256;

fn hue(c: vec3<f32>) -> f32 {
    let hi = max(c.r, max(c.g, c.b));
    let d = hi - min(c.r, min(c.g, c.b));
    if (d <= 1.0e-5) {
        return 0.0;
    }
    var h: f32;
    if (hi == c.r) {
        h = (c.g - c.b) / d;
    } else if (hi == c.g) {
        h = 2.0 + (c.b - c.r) / d;
    } else {
        h = 4.0 + (c.r - c.g) / d;
    }
    return fract(h / 6.0);
}

fn saturation(c: vec3<f32>) -> f32 {
    let hi = max(c.r, max(c.g, c.b));
    return select(0.0, (hi - min(c.r, min(c.g, c.b))) / hi, hi > 1.0e-5);
}

// Runs go along lines for Right and Left, down columns for Down and Up.
fn vertical(p: Params) -> bool {
    return p.dir >= 2u;
}

// Where this sample is along its run's line or column, and how long that is.
fn place(ctx: Ctx, p: Params) -> vec2<i32> {
    if (vertical(p)) {
        return vec2<i32>(i32(ctx.line), i32(u.info.lines));
    }
    return vec2<i32>(i32(ctx.x), i32(u.info.width));
}

// The stage-0 or stage-1 result `i` samples along this sample's line or
// column.
fn along(k: u32, ctx: Ctx, p: Params, i: i32) -> vec4<f32> {
    if (vertical(p)) {
        return stage_at(k, i32(ctx.x), i * i32(u.info.row_stride) + i32(u.info.parity));
    }
    return stage_at(k, i, i32(ctx.row));
}

// The value sorted by, `i` samples along this sample's line or column; -1
// if it isn't between Low and High (or is past the end), and so stays put.
fn value_at(ctx: Ctx, p: Params, i: i32) -> f32 {
    let pl = place(ctx, p);
    if (i >= pl.y) {
        return -1.0;
    }
    var x = i;
    var row = i32(ctx.row);
    if (vertical(p)) {
        x = i32(ctx.x);
        row = i * i32(u.info.row_stride) + i32(u.info.parity);
    }
    var v: f32;
    switch p.by {
        case 1u: {
            v = hue(fetch_sig(x, row));
        }
        case 2u: {
            v = saturation(fetch_sig(x, row));
        }
        case 3u: {
            v = clamp(fetch_key(x, row), 0.0, 1.0);
        }
        default: {
            v = clamp(luma(fetch_sig(x, row)), 0.0, 1.0);
        }
    }
    let sorted = v >= min(p.low, p.high) && v <= max(p.low, p.high);
    return select(-1.0, v, sorted);
}

// Stage 0, per sample: the values of this sample and the next three along,
// so the passes after read four at a time.
fn stage0(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let i = place(ctx, p).x;
    return vec4<f32>(value_at(ctx, p, i), value_at(ctx, p, i + 1), value_at(ctx, p, i + 2), value_at(ctx, p, i + 3));
}

// Stage 1, per sample: for a sorted sample, its rank in its run in x, and
// the run's first and last places in y and z; -1 in x otherwise. Runs stop
// at Length, on a grid offset line by line so the breaks don't line up.
fn stage1(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let own = along(0u, ctx, p, place(ctx, p).x).x;
    if (own < 0.0) {
        return vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    }
    let pl = place(ctx, p);
    let i = pl.x;
    let len = clamp(i32(round(p.length)), 2, MAX_LENGTH);
    let across = select(i32(ctx.line), i32(ctx.x), vertical(p));
    let offset = i32(hash3(vec3<u32>(u32(across), select(0u, 1u, vertical(p)), 0x51u)) * f32(len));
    let first = ((i + offset) / len) * len - offset;
    let lo = max(first, 0);
    let hi = min(first + len - 1, pl.y - 1);
    // Walk out both ways to the ends of the run, four at a time, counting
    // the rank on the way: how many in the run come before it, by value and
    // then place.
    var rank = 0;
    var a = i;
    for (var t = i - 4; t + 3 >= lo; t = t - 4) {
        var four: vec4<f32>;
        if (t >= 0) {
            four = along(0u, ctx, p, t);
        } else {
            // Past the start of the line: the first values, moved to line up.
            let first = along(0u, ctx, p, 0);
            four = select(
                select(vec4<f32>(-1.0, -1.0, -1.0, first.x), vec4<f32>(-1.0, -1.0, first.x, first.y), t == -2),
                vec4<f32>(-1.0, first.x, first.y, first.z),
                t == -1,
            );
        }
        let ok = four >= vec4<f32>(0.0) & vec4<i32>(t) + vec4<i32>(0, 1, 2, 3) >= vec4<i32>(lo);
        // How many, back from the last, are in the run before one isn't.
        let n = select(0, select(1, select(2, select(3, 4, ok.x), ok.y), ok.z), ok.w);
        let in_run = vec4<i32>(3, 2, 1, 0) < vec4<i32>(n);
        rank = rank + count(in_run & four <= vec4<f32>(own));
        a = select(a, t + 4 - n, n > 0);
        if (n < 4) {
            break;
        }
    }
    var b = i;
    for (var t = i + 1; t <= hi; t = t + 4) {
        let four = along(0u, ctx, p, t);
        let ok = four >= vec4<f32>(0.0) & vec4<i32>(t) + vec4<i32>(0, 1, 2, 3) <= vec4<i32>(hi);
        // How many, from the first, are in the run before one isn't.
        let n = select(0, select(1, select(2, select(3, 4, ok.w), ok.z), ok.y), ok.x);
        let in_run = vec4<i32>(0, 1, 2, 3) < vec4<i32>(n);
        rank = rank + count(in_run & four < vec4<f32>(own));
        b = select(b, t + n - 1, n > 0);
        if (n < 4) {
            break;
        }
    }
    return vec4<f32>(f32(rank), f32(a), f32(b), 0.0);
}

fn count(m: vec4<bool>) -> i32 {
    return i32(dot(select(vec4<f32>(0.0), vec4<f32>(1.0), m), vec4<f32>(1.0)));
}

// Stage 2, per sample: the ranks of this sample and the next three along,
// so the last pass searches four at a time.
fn stage2(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let i = place(ctx, p).x;
    return vec4<f32>(
        along(1u, ctx, p, i).x,
        along(1u, ctx, p, i + 1).x,
        along(1u, ctx, p, i + 2).x,
        along(1u, ctx, p, i + 3).x,
    );
}

// Last, per sample: the pixel of its run whose rank is this sample's place,
// counted from the end the lowest values go to.
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = inp.sig;
    let i = place(ctx, p).x;
    let run = along(1u, ctx, p, i);
    if (run.x < 0.0) {
        return o;
    }
    let a = i32(run.y);
    let b = i32(run.z);
    var want = i - a;
    if (p.dir == 1u || p.dir == 3u) {
        want = b - i;
    }
    // Look outward from where that pixel would be if the run were already
    // in order, four places at a time: in a picture, it's usually close.
    let home = a + want;
    var found = home;
    for (var d = 0; home + 4 * d <= b || home - 4 * d - 1 >= a; d = d + 1) {
        let later = home + 4 * d;
        let earlier = max(home - 4 * d - 4, a);
        let hit_later = later <= b && match_at(ctx, p, later, a, b, want) >= 0;
        if (hit_later) {
            found = later + match_at(ctx, p, later, a, b, want);
            break;
        }
        if (d > 0 || earlier < home) {
            let k = match_at(ctx, p, earlier, a, b, want);
            if (k >= 0) {
                found = earlier + k;
                break;
            }
        }
    }
    if (vertical(p)) {
        o.out = fetch_sig(i32(ctx.x), found * i32(u.info.row_stride) + i32(u.info.parity));
    } else {
        o.out = fetch_sig(found, i32(ctx.row));
    }
    return o;
}

// Which of the four places from `t` on, within `a` ..= `b`, has rank
// `want`; -1 if none.
fn match_at(ctx: Ctx, p: Params, t: i32, a: i32, b: i32, want: i32) -> i32 {
    let pos = vec4<i32>(t) + vec4<i32>(0, 1, 2, 3);
    let m = along(2u, ctx, p, t) == vec4<f32>(f32(want)) & pos >= vec4<i32>(a) & pos <= vec4<i32>(b);
    return select(select(select(select(-1, 3, m.w), 2, m.z), 1, m.y), 0, m.x);
}
