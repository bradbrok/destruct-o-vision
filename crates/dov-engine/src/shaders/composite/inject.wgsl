// Inject: adds a video signal into the waveform. The video is stretched over
// the whole field, blanking included, so whatever it carries lands in the
// sync intervals too.
// Knobs: 0 amount, 1 region (0 everywhere, 1 blanking only, 2 picture only), 2 offset.

@group(0) @binding(2) var vin: texture_2d<f32>;
@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let fl = field_line(c, row);
    let lines = i32(u.info.lines);
    let line = clamp(fl.x * lines / max(field_rows(c, fl.y), 1), 0, lines - 1);
    let vrow = line * i32(u.info.row_stride) + fl.y;
    let region = u32(round(knob(1u)));
    let patched = u.inputs[1].w > 0.5;
    for (var s = 0; s < c.spl; s = s + 1) {
        var m = u.inputs[1].x;
        if (patched) {
            let x = clamp(s * i32(u.info.width) / c.spl, 0, i32(u.info.width) - 1);
            m = luma(textureLoad(vin, vec2<i32>(x, vrow), 0).rgb);
        }
        let picture = standard_at(c, row, s).y == 2.0;
        var gate = 1.0;
        if ((region == 1u && picture) || (region == 2u && !picture)) {
            gate = 0.0;
        }
        let v = c_at(c, row, s) + gate * knob(0u) * (m + knob(2u));
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
