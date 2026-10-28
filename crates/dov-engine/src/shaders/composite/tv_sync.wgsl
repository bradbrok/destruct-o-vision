// TV pass 1, sync separator: per line, the first falling edge through the
// slicer level that stays down like a sync pulse, how much of each
// half-line sits below it, and the burst's strength; and further on in the
// buffer, the line's average level, for the AGC.
// Knob 8: slicer level.

@group(0) @binding(20) var<storage, read_write> sync_info: array<vec4<f32>>;

const NO_EDGE: f32 = -1.0e6;

// The waveform `k` samples into `row`, past its end into the next.
fn tv_along(c: Comp, row: i32, k: i32) -> f32 {
    return tv_signal(c, row + k / c.spl, k % c.spl);
}

// Does the signal stay below the slicer for `width` samples from `s`? A
// sync pulse does; a saturated color dipping below it for part of a
// subcarrier cycle doesn't.
fn held_low(c: Comp, row: i32, s: i32, width: i32, slicer: f32) -> bool {
    var low = 0;
    for (var k = 0; k < width; k = k + 1) {
        if (tv_along(c, row, s + k) < slicer) {
            low = low + 1;
        }
    }
    return f32(low) >= 0.8 * f32(width);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let slicer = knob(8u);
    let half = c.spl / 2;
    // Sync starts at 0H, so the edge falls between the previous line's
    // last sample and this line's first.
    var prev = tv_signal(c, row - 1, c.spl - 1);
    // Narrower than an equalizing pulse, wider than any color.
    let width = max(i32(us(c, 1.5)), 2);
    var edge = NO_EDGE;
    var below0 = 0.0;
    var below1 = 0.0;
    var level = 0.0;
    let a0 = c.active_start;
    let a1 = c.active_start + c.active_samples;
    for (var s = 0; s < c.spl; s = s + 1) {
        let v = tv_signal(c, row, s);
        if (s >= a0 && s < a1) {
            level = level + v;
        }
        if (edge == NO_EDGE && prev >= slicer && v < slicer && held_low(c, row, s, width, slicer)) {
            edge = f32(s) - 1.0 + (prev - slicer) / max(prev - v, 1e-6);
        }
        if (v < slicer) {
            if (s < half) {
                below0 = below0 + 1.0;
            } else {
                below1 = below1 + 1.0;
            }
        }
        prev = v;
    }
    let b = burst(c, row, 0.0);
    sync_info[row] = vec4<f32>(edge, below0 / f32(half), below1 / f32(c.spl - half), b.y);
    sync_info[c.lines + row] = vec4<f32>(level / f32(max(a1 - a0, 1)), 0.0, 0.0, 0.0);
}
