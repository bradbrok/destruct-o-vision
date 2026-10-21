// Cable / RF path: ghosting, hum, interference, snow, then bandwidth.
// Knobs: 0 snow, 1 bandwidth (MHz), 2 ghost, 3 ghost delay (µs), 4 hum,
//        5 interference, 6 interference frequency (MHz).
// Behavior: extra[0] = (interference phase at row 0, its cycles per line
//           (fraction), its cycles per sample, hum phase at row 0).

@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let a = lp_coef(c, knob(1u));
    let delay = us(c, knob(3u));
    let mains = select(60.0, 50.0, c.pal);
    let field = u.info.field;
    var y = c_at(c, row, 0);
    for (var s = 0; s < c.spl; s = s + 1) {
        let t = f32(row * c.spl + s);
        // A delayed echo: a reflection off a building, or a mismatched cable.
        var v = c_at(c, row, s) + knob(2u) * c_time(c, t - delay);
        // Hum: a slow bar rolling through the picture at mains frequency.
        v = v + knob(4u) * 0.25 * sin(TAU * (u.extra[0].w + t * mains / c.fs));
        // A stray carrier: diagonal herringbone.
        let ip = u.extra[0].x + fract(f32(row) * u.extra[0].y) + f32(s) * u.extra[0].z;
        v = v + knob(5u) * sin(TAU * ip);
        v = v + knob(0u) * (rnd(u32(s), u32(row), field) - 0.5) * 1.2;
        y = y + a * (v - y);
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(y, 0.0, 0.0, 1.0));
    }
}
