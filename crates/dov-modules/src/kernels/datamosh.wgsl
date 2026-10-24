// Datamosh: video compression with the keyframes cut out. Two passes: the
// motion of each block, found once per 8×8 cell, then every sample dragging
// last field's picture along by its block's motion.
//
// MIT license (LICENSE-MIT), like the module format, so it can be copied.

// The history holds the moshed picture, and hidden in the three lowest bits
// of each of its half-float channels, the input's luma (8 bits) and a bit
// saying the sample has been written. Motion is found between this field's
// input and that luma. Changing the lowest bits moves a color by well under
// a percent.

// `v` rounded to a half float, with its three lowest bits set to `bits`.
// Half floats are floats with the low 13 bits of the fraction cleared, so
// a half's lowest bits are bits 13 to 15 of a float's.
fn tuck(v: f32, bits: u32) -> f32 {
    // Too small and a half has no bits to spare: use the smallest it holds
    // at full precision, keeping the sign.
    let m = clamp(abs(v), 6.103515625e-5, 60000.0);
    let f = (bitcast<u32>(m) + 0x1000u) & ~0x1fffu;
    return select(1.0, -1.0, v < 0.0) * bitcast<f32>((f & ~0xe000u) | (bits << 13u));
}

fn hide(c: vec3<f32>, luma_in: f32) -> vec3<f32> {
    let b = u32(clamp(luma_in, 0.0, 1.0) * 255.0 + 0.5) | 256u;
    return vec3<f32>(tuck(c.r, b & 7u), tuck(c.g, (b >> 3u) & 7u), tuck(c.b, b >> 6u));
}

// The hidden bits: the luma in the low 8, and bit 8 once written.
fn hidden_bits(c: vec3<f32>) -> u32 {
    let k = (bitcast<vec3<u32>>(c) >> vec3<u32>(13u)) & vec3<u32>(7u);
    return k.r | (k.g << 3u) | (k.b << 6u);
}

// The last field's input luma at sample `x` of this field's line `l`, or -1
// where nothing was stored yet.
fn last_luma(x: i32, l: i32) -> f32 {
    let bits = hidden_bits(ring_at(x, l * i32(u.info.row_stride) + i32(u.info.parity), 1));
    return select(-1.0, f32(bits & 255u) / 255.0, (bits & 256u) != 0u);
}

// How badly the block's sample points match the last field moved by `v`.
fn mismatch(points: ptr<function, array<vec3<f32>, 16>>, v: vec2<i32>) -> f32 {
    var total = 0.0;
    for (var k = 0; k < 16; k = k + 1) {
        let pt = (*points)[k];
        total = total + abs(pt.z - last_luma(i32(pt.x) + v.x, i32(pt.y) + v.y));
    }
    return total;
}

fn block_size(p: Params) -> i32 {
    return 8 << p.block;
}

// Stage 0, per 8×8 cell: where its block was last field, in xy, and in z
// whether there's a last field to drag at all.
fn stage0(ctx: Ctx, inp: Inputs, p: Params) -> vec4<f32> {
    let size = block_size(p);
    let bx = i32(ctx.x) / size;
    let by = i32(ctx.line) / size;
    let stride = i32(u.info.row_stride);
    let parity = i32(u.info.parity);

    // Sixteen points across the block, with this field's luma at each.
    var points: array<vec3<f32>, 16>;
    let gap = size / 4;
    var stored = true;
    for (var k = 0; k < 16; k = k + 1) {
        let px = bx * size + (k % 4) * gap + gap / 2;
        let pl = by * size + (k / 4) * gap + gap / 2;
        points[k] = vec3<f32>(f32(px), f32(pl), luma(fetch_sig(px, pl * stride + parity)));
        stored = stored && last_luma(px, pl) >= 0.0;
    }
    if (!stored) {
        return vec4<f32>(0.0);
    }
    // Try the eight points around the best so far, halving the reach each
    // round, and move only for a clearly better match.
    var best = vec2<i32>(0);
    var best_miss = mismatch(&points, best);
    for (var reach = size / 2; reach >= 1; reach = reach / 2) {
        let center = best;
        for (var k = 0; k < 9; k = k + 1) {
            if (k == 4) {
                continue;
            }
            let v = center + vec2<i32>(k % 3 - 1, k / 3 - 1) * reach;
            let miss = mismatch(&points, v);
            if (miss < best_miss - 0.02) {
                best = v;
                best_miss = miss;
            }
        }
    }
    return vec4<f32>(vec2<f32>(best), 1.0, 0.0);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let x = i32(ctx.x);
    let line = i32(ctx.line);
    let stride = i32(u.info.row_stride);
    let parity = i32(u.info.parity);
    let size = block_size(p);
    let block = vec2<u32>(u32(x / size), u32(line / size));
    let found = stage_at(0u, x / 8, line / 8);

    var o: Outputs;
    o.out = inp.sig;
    o.write = hide(inp.sig, luma(inp.sig));
    let refresh = clamp(p.refresh, 0.0, 1.0);
    let fresh = hash3(vec3<u32>(block, ctx.field * 4u)) < refresh * refresh * 0.5;
    if (p.keyframe == 1u || fresh || found.z < 0.5) {
        return o;
    }

    // Drag last field's picture along. Fractional moves round up or down
    // at random per block, so slow drifts still move on average.
    let moved = found.xy * max(p.motion, 0.0) + vec2<f32>(p.drift_x, p.drift_y);
    let dither = vec2<f32>(
        hash3(vec3<u32>(block, ctx.field * 4u + 1u)),
        hash3(vec3<u32>(block, ctx.field * 4u + 2u)),
    );
    let v = vec2<i32>(floor(moved + dither));
    let last = ring_at(x + v.x, (line + v.y) * stride + parity, 1);
    // The input seeps back in, by at most this much per field.
    let bleed = clamp(p.bleed, 0.0, 1.0);
    let limit = bleed * bleed * 0.5;
    let c = last + clamp(inp.sig - last, vec3<f32>(-limit), vec3<f32>(limit));
    o.out = c;
    o.write = hide(c, luma(inp.sig));
    return o;
}
