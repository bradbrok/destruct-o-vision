// Encoder: R'G'B' to a composite waveform, one line per invocation.
// Knobs: 0 luma, 1 chroma, 2 chroma bandwidth (MHz), 3 burst, 4 sync.
// Behavior: extra[0] = (carrier phase at row 0 of the frame, cycles per line
//           (fraction), cycles per sample) for a possibly detuned subcarrier.

@group(0) @binding(1) var vin: texture_2d<f32>;
@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let fl = field_line(c, row);
    let raster_row = (fl.x - c.first_active) * i32(u.info.row_stride) + fl.y;
    let a = lp_coef(c, knob(2u));
    let black = setup(c);
    let base = u.extra[0].x;
    let line_fract = u.extra[0].y;
    let inc = u.extra[0].z;
    let pal_sign = select(1.0, -1.0, c.pal && (row % 2) == 1);
    var uv = vec2<f32>(0.0);
    for (var s = 0; s < c.spl; s = s + 1) {
        let t = standard_at(c, row, s);
        var v = t.x * knob(4u);
        var want = vec2<f32>(0.0);
        if (t.y == 2.0) {
            let x = (s - c.active_start) * i32(u.info.width) / c.active_samples;
            let rgb = textureLoad(vin, vec2<i32>(x, raster_row), 0).rgb;
            let y = luma(rgb);
            want = vec2<f32>(0.492 * (rgb.b - y), 0.877 * (rgb.r - y));
            v = black + (1.0 - black) * y * knob(0u);
        }
        // Chroma is band-limited before modulation, so color edges smear.
        uv = uv + a * (want - uv);
        if (c.color) {
            let ph = TAU * carrier(base, line_fract, inc, row, f32(s));
            v = v + (1.0 - black) * knob(1u) * (uv.x * sin(ph) + pal_sign * uv.y * cos(ph));
            if (t.y == 1.0) {
                // NTSC burst sits at 180° on the U axis; PAL swings ±45° around it.
                var b = -sin(ph);
                if (c.pal) {
                    b = (-sin(ph) + pal_sign * cos(ph)) * 0.70710678;
                }
                v = v + BURST_LEVEL * knob(3u) * b;
            }
        }
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
