// VCR, behavioral tier: what a VHS deck does to a signal on the way to tape
// and back. Timebase errors move whole lines (sync included), so the TV's
// sync circuits react to them rather than a filter faking the result.
// Knobs: 0 speed (SP/LP/EP), 1 tracking, 2 wow, 3 jitter, 4 flagging,
//        5 head switch, 6 dropouts, 7 chroma noise, 8 luma noise, 9 sharpness.

@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let fl = field_line(c, row);
    let fr = f32(fl.x);
    let rows_f = f32(field_rows(c, fl.y));
    let field = u.info.field;
    let urow = u32(row);
    let speed = u32(round(knob(0u)));
    let luma_mhz = select(select(3.0, 2.5, speed == 1u), 2.0, speed == 2u);
    let noise_scale = select(select(1.0, 1.6, speed == 1u), 2.4, speed == 2u);

    // Timebase error: tape stretch (wow), per-line jitter, and the skew at the
    // top of the field while the tape tension settles (flagging).
    let wow = knob(2u) * 8.0 * sin(TAU * (fr / rows_f * 0.7 + f32(field % 4096u) * 0.031));
    let jitter = knob(3u) * 3.0 * (rnd(urow, field, 1u) - 0.5);
    let flag = knob(4u) * 30.0 * exp(-fr / 10.0);
    var shift = wow + jitter + flag;
    // Head switching: the last lines before vertical sync come off the other
    // head, displaced, with a burst of noise at the changeover.
    let switch_line = rows_f - 8.0;
    let switching = fr >= switch_line;
    if (switching) {
        shift = shift + knob(5u) * 24.0;
    }
    // Mistracking: a noise band that drifts through the picture.
    let mistrack = 1.0 - knob(1u);
    let band_pos = fract(f32(field % 100000u) * 0.0023 + 0.3);
    let band_width = max(mistrack * 0.25, 1e-4);
    let band = (1.0 - smoothstep(0.0, band_width, abs(fr / rows_f - band_pos))) * smoothstep(0.0, 0.15, mistrack);
    // Dropouts: oxide missing from the tape.
    let dropout = rnd(urow, field, 7u) < knob(6u) * 0.06;
    let d0 = rnd(urow, field, 8u) * f32(c.spl);
    let dlen = 8.0 + rnd(urow, field, 9u) * 140.0;

    let b = burst(c, row, shift);
    let sep = lp_coef(c, 2.4);
    let lp_y = lp_coef(c, luma_mhz);
    let lp_y2 = lp_coef(c, luma_mhz * 0.4);
    let lp_c = lp_coef(c, 0.5);
    let line_phase_error = knob(7u) * 0.05 * (rnd(urow, field, 3u) - 0.5);

    let row_t = f32(row * c.spl);
    let first = c_time(c, row_t - shift);
    var ysep = first;
    var yrec = first;
    var yrec2 = first;
    var uv = vec2<f32>(0.0);
    for (var s = 0; s < c.spl; s = s + 1) {
        let src = f32(s) - shift;
        let x = c_time(c, row_t + src);
        // Chroma: the band around the subcarrier, demodulated with the
        // burst's phase and squeezed into ~0.5 MHz (color-under).
        ysep = ysep + sep * (x - ysep);
        let ph = TAU * (reference(c, row, src) + b.x);
        uv = uv + lp_c * (2.0 * (x - ysep) * vec2<f32>(sin(ph), cos(ph)) - uv);
        // Luma: what's left once the recovered chroma is taken out. The
        // chroma channel is gated off during sync, where there's no burst to
        // lock to and the "chroma" is just the edges of the sync pulses.
        let gate = select(0.0, smoothstep(-0.2, -0.05, yrec), b.y > 0.02);
        let chroma_est = gate * (uv.x * sin(ph) + uv.y * cos(ph));
        let luma_noise = knob(8u) * noise_scale * 0.12 * (rnd(u32(s), urow, field) - 0.5);
        let y_in = x - chroma_est + luma_noise;
        yrec = yrec + lp_y * (y_in - yrec);
        yrec2 = yrec2 + lp_y2 * (y_in - yrec2);
        // Playback sharpening overshoots edges.
        let y = yrec + knob(9u) * 1.5 * (yrec - yrec2);

        // Back to the subcarrier with noise and a per-line phase error.
        let blotch = vec2<f32>(rnd(u32(s) / 6u, urow, field + 1u), rnd(u32(s) / 6u, urow, field + 2u)) - 0.5;
        let n_uv = uv + blotch * knob(7u) * noise_scale * 0.12;
        // Phase error that wanders along the line: burst lock can't fix it.
        let k = f32(s) / 96.0;
        let w0 = rnd(u32(k), urow, field + 4u);
        let w1 = rnd(u32(k) + 1u, urow, field + 4u);
        let wander = knob(7u) * 0.08 * (mix(w0, w1, fract(k)) - 0.5);
        let ph_out = ph + TAU * (line_phase_error + wander);
        var v = y + gate * (n_uv.x * sin(ph_out) + n_uv.y * cos(ph_out));

        if (dropout && f32(s) >= d0 && f32(s) < d0 + dlen) {
            v = 0.75 + 0.3 * (rnd(u32(s), urow, field + 5u) - 0.5);
        }
        v = mix(v, (rnd(u32(s), urow, field + 9u) - 0.3) * 1.2, band * 0.9);
        if (switching && fr < switch_line + 2.0) {
            v = v + knob(5u) * (rnd(u32(s), urow, field + 11u) - 0.5) * 0.8;
        }
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
