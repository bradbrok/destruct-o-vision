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

// The last samples read, so an edge can be checked once the samples after
// it are in: a sync pulse stays below the slicer for its width; a
// saturated color dips below it for part of a subcarrier cycle.
const HISTORY: i32 = 128;
var<private> recent: array<f32, 128>;

fn recent_at(s: i32) -> f32 {
    return recent[(s + HISTORY) % HISTORY];
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
    // Narrower than an equalizing pulse, wider than any color.
    let width = clamp(i32(us(c, 1.5)), 2, HISTORY - 2);
    // Sync starts at 0H, so the edge can fall between the previous line's
    // last sample (-1) and this line's first.
    recent[HISTORY - 1] = tv_signal(c, row - 1, c.spl - 1);
    var edge = NO_EDGE;
    var below0 = 0.0;
    var below1 = 0.0;
    var level = 0.0;
    // Samples below the slicer among the last `width`.
    var low = 0;
    let a0 = c.active_start;
    let a1 = c.active_start + c.active_samples;
    // Past the line's end only as far as it takes to check an edge near it.
    for (var s = 0; s < c.spl + width; s = s + 1) {
        if (s >= c.spl && edge != NO_EDGE) {
            break;
        }
        let v = tv_along(c, row, s);
        recent[s % HISTORY] = v;
        if (s < c.spl) {
            if (s >= a0 && s < a1) {
                level = level + v;
            }
            if (v < slicer) {
                if (s < half) {
                    below0 = below0 + 1.0;
                } else {
                    below1 = below1 + 1.0;
                }
            }
        }
        low = low + select(0, 1, v < slicer);
        if (s >= width) {
            low = low - select(0, 1, recent_at(s - width) < slicer);
        }
        // An edge `width` samples back counts if the signal stayed down.
        let e = s - width + 1;
        if (edge == NO_EDGE && e >= 0 && e < c.spl) {
            let before = recent_at(e - 1);
            let after = recent_at(e);
            if (before >= slicer && after < slicer && f32(low) >= 0.8 * f32(width)) {
                edge = f32(e) - 1.0 + (before - slicer) / max(before - after, 1e-6);
            }
        }
    }
    let b = burst(c, row, 0.0);
    sync_info[row] = vec4<f32>(edge, below0 / f32(half), below1 / f32(c.spl - half), b.y);
    sync_info[c.lines + row] = vec4<f32>(level / f32(max(a1 - a0, 1)), 0.0, 0.0, 0.0);
}
