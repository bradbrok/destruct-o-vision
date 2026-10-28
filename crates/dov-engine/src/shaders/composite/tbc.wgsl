// Time base corrector: finds each line's falling sync edge and re-times
// the line so the edge lands where the standard puts it, undoing wow,
// jitter, flagging and head-switch skew. Plus a proc amp for the picture.
// Knobs: 0 correction (0 off, 1 full), 1 video gain, 2 black level.

@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;

const SLICE: f32 = -0.2;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let row_t = f32(row * c.spl);
    // Search window around 0H: 40 samples early to 80 late, at NTSC's scale.
    let early = i32(samples(c, 40.0));
    let late = i32(samples(c, 80.0));
    var edge = 0.0;
    var found = false;
    var prev = c_time(c, row_t - f32(early + 1));
    for (var k = -early; k < late; k = k + 1) {
        let v = c_time(c, row_t + f32(k));
        if (!found && prev >= SLICE && v < SLICE) {
            edge = f32(k) - 1.0 + (prev - SLICE) / max(prev - v, 1e-6);
            found = true;
        }
        prev = v;
    }
    // A clean line's edge falls half a sample before 0H.
    let shift = select(0.0, (edge + 0.5) * knob(0u), found);
    let black = setup(c);
    for (var s = 0; s < c.spl; s = s + 1) {
        var v = c_time(c, row_t + f32(s) + shift);
        if (standard_at(c, row, s).y == 2.0) {
            v = (v - black) * knob(1u) + black + knob(2u);
        }
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
