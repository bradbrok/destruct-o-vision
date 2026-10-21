// TV pass 1, sync separator: per line, the first falling edge through the
// slicer level, and how much of each half-line sits below it.
// Knob 8: slicer level.

@group(0) @binding(20) var<storage, read_write> sync_info: array<vec4<f32>>;

const NO_EDGE: f32 = -1.0e6;

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
    var edge = NO_EDGE;
    var below0 = 0.0;
    var below1 = 0.0;
    for (var s = 0; s < c.spl; s = s + 1) {
        let v = tv_signal(c, row, s);
        if (edge == NO_EDGE && prev >= slicer && v < slicer) {
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
    sync_info[row] = vec4<f32>(edge, below0 / f32(half), below1 / f32(c.spl - half), 0.0);
}
