// RF: the composite signal broadcast on a channel and tuned in by a TV's
// front end, worked out at baseband. A weak signal brings snow up as the
// TV's gain rises to meet it; fine tuning slides the picture carrier along
// the IF filter's slope; reflections arrive late; other stations on and
// next to the channel bleed in; the sound carrier beats with the color.
// Knobs: 0 signal, 1 fine tuning, 2 ghost, 3 adjacent channel,
//        4 co-channel, 5 sound.
// Behavior: per row of this field, from row0: (audio at the row, sound beat
//           phase at its start, co-channel beat phase, 0); extra[0] = (the
//           adjacent channel's slip: samples across and lines down; sound
//           beat cycles per sample).

@group(0) @binding(2) var other: texture_2d<f32>;
@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;
@group(0) @binding(32) var<storage, read> per_row: array<vec4<f32>>;

// The adjacent channel `t` samples into the frame. Its sync isn't ours, so
// it's read slipped: its blanking crawls across the picture as dark bars.
// Without a picture patched in, it's a gray picture and those bars.
fn other_at(c: Comp, t: f32) -> f32 {
    let frame = f32(c.spl * c.lines);
    let n = t + u.extra[0].x + u.extra[0].y * f32(c.spl);
    let m = n - floor(n / frame) * frame;
    let row = i32(m / f32(c.spl));
    let s = i32(m - f32(row) * f32(c.spl));
    if (u.inputs[1].w > 0.5) {
        return textureLoad(other, vec2<i32>(clamp(s, 0, c.spl - 1), clamp(row, 0, c.lines - 1)), 0).r;
    }
    let blank = f32(s) < f32(c.active_start) || field_line(c, row).x < c.first_active;
    return select(0.4, -0.4, blank);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let pr = per_row[gid.x];
    let field = u.info.field;
    let urow = u32(row);

    let strength = clamp(knob(0u), 0.0, 1.0);
    let fine = clamp(knob(1u), -1.0, 1.0);
    let ghost = knob(2u);
    let adjacent = knob(3u);
    let cochannel = knob(4u);
    let sound = knob(5u);
    // Tuned away from the sound, the picture carrier slides down the IF
    // slope: detail and then color fall away. Tuned toward it, edges ring,
    // color runs hot and the sound beats with the chroma.
    let away = max(-fine, 0.0);
    let soft = lp_coef(c, mix(6.0, 1.6, away));
    // The color band falls faster than the rest, and with it the burst,
    // until the TV's color killer gives up on it.
    let chroma_cut = 0.95 * pow(away, 0.7);
    let sep = lp_coef(c, 2.4);
    let peak = lp_coef(c, 2.0);
    let ring = 0.9 * max(fine, 0.0);
    // Either way off, the low frequencies smear: trailing streaks.
    let slow = lp_coef(c, 0.3);
    let smear = 0.3 * abs(fine);
    let beat = sound * (0.08 + 0.5 * max(fine, 0.0));
    // The tuner's gain rises to meet a weak signal, and its noise with it.
    let snow = pow(1.0 - strength, 1.6) * 1.2;
    // Sparks: impulse noise from passing cars and appliances, on a weak
    // signal.
    let sparks = rnd(urow, field, 31u) < (1.0 - strength) * 0.04;
    let delay1 = picture_us(c, 2.2);
    let delay2 = picture_us(c, 4.7);
    let adj_lp = lp_coef(c, 1.0);

    let row_t = f32(row * c.spl);
    let first = c_at(c, row, 0);
    var lp_soft = first;
    var lp_peak = first;
    var lp_slow = first;
    var lp_sep = first;
    var adj = other_at(c, row_t);
    var sn = 0.0;
    for (var s = 0; s < c.spl; s = s + 1) {
        let t = row_t + f32(s);
        // Multipath: a reflection 2.2 µs late, another 4.7 µs late and
        // upside down.
        var x = c_at(c, row, s) + ghost * (0.55 * c_time(c, t - delay1) - 0.3 * c_time(c, t - delay2));
        // The next channel up, through the skirt of the IF filter.
        adj = adj + adj_lp * (other_at(c, t) - adj);
        x = x + adjacent * 0.35 * adj;
        // Co-channel: a distant station on the same channel, its carrier
        // a few kHz off ours: venetian blinds that drift.
        x = x + cochannel * 0.2 * sin(TAU * pr.z);
        lp_sep = lp_sep + sep * (x - lp_sep);
        x = x - chroma_cut * (x - lp_sep);
        lp_soft = lp_soft + soft * (x - lp_soft);
        lp_peak = lp_peak + peak * (x - lp_peak);
        lp_slow = lp_slow + slow * (x - lp_slow);
        var v = lp_soft + ring * (x - lp_peak) + smear * (lp_slow - lp_peak);
        // The sound carrier: its beat with the chroma (920 kHz in NTSC),
        // frequency modulated by the audio, and the audio itself leaking
        // through as bars.
        v = v + beat * sin(TAU * (pr.y + f32(s) * u.extra[0].z)) + sound * 0.1 * pr.x;
        // Snow, smeared a little by the IF.
        sn = sn + soft * ((rnd(u32(s), urow, field) - 0.5) - sn);
        v = v + snow * (sn * 1.6 + (rnd(u32(s), urow, field + 3u) - 0.5) * 0.4);
        if (sparks && rnd(u32(s) / 3u, urow, field + 7u) < 0.25) {
            v = v + 0.9;
        }
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
