// VCR, behavioral tier: what a videotape deck does to a signal on the way
// to tape and back. Timebase errors move whole lines (sync included), so the
// TV's sync circuits react to them rather than a filter faking the result.
// The deck tapes what comes in (the `tape` output, which the engine keeps a
// field per slot while the tape runs) and plays back from the tape: what's
// coming in, until the tape is stopped or spooled, then wherever it is.
// Knobs: 0 format, 1 speed (SP/LP/EP), 2 transport, 3 search speed,
//        4 tracking, 5 wow, 6 jitter, 7 flagging, 8 head switch, 9 dropouts,
//        10 clog, 11 crease, 12 chroma noise, 13 luma noise, 14 sharpness.
// Behavior: extra[0] = (fields back on the tape at the top of the picture,
//           change per band between noise bars, noise bars, their phase);
//           extra[1] = (transport: 0 play, 1 pause, 2 search; servo
//           relock, 1 as the transport changes and falling to 0; the tape's
//           field number at the top of the picture; parity of the newest
//           field on the tape); extra[2].x = lines the picture jumps.

@group(0) @binding(16) var cout: texture_storage_2d<r32float, write>;
@group(0) @binding(17) var tape_out: texture_storage_2d<r32float, write>;

struct Format {
    // Luma bandwidth at standard speed, and color-under bandwidth, in MHz.
    luma_mhz: f32,
    chroma_mhz: f32,
    // Noise, timebase error and dropouts, relative to VHS.
    noise: f32,
    timebase: f32,
    dropouts: f32,
}

fn tape_format(i: u32) -> Format {
    switch i {
        // S-VHS: more FM deviation, about 400 lines across.
        case 1u: {
            return Format(5.0, 0.5, 0.7, 0.9, 1.0);
        }
        // Betamax: a little sharper and quieter than VHS.
        case 2u: {
            return Format(3.2, 0.5, 0.85, 0.9, 1.0);
        }
        // U-matic: wide tape, a steadier timebase and better color, but
        // the tapes around now are old.
        case 3u: {
            return Format(3.3, 0.8, 0.6, 0.5, 1.5);
        }
        // Video8 and Hi8: metal tape, small dropouts and plenty of them.
        case 4u: {
            return Format(3.0, 0.5, 0.9, 0.8, 1.8);
        }
        case 5u: {
            return Format(5.0, 0.5, 0.65, 0.8, 1.8);
        }
        default: {
            return Format(3.0, 0.5, 1.0, 1.0, 1.0);
        }
    }
}

// Snow: what a head puts out with no tape signal under it, before the
// playback filters smear it along the line.
fn snow(s: i32, row: u32, field: u32, k: u32) -> f32 {
    return (rnd(u32(s), row, field + k) - 0.35) * 1.2;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    // Tape what's coming in. The engine keeps it unless the tape is stopped.
    for (var s = 0; s < c.spl; s = s + 1) {
        textureStore(tape_out, vec2<i32>(s, row), vec4<f32>(c_at(c, row, s), 0.0, 0.0, 1.0));
    }

    let fl = field_line(c, row);
    let fr = f32(fl.x);
    let rows_f = f32(field_rows(c, fl.y));
    let y_pos = fr / rows_f;
    let field = u.info.field;
    let urow = u32(row);
    let f = tape_format(u32(round(knob(0u))));
    let speed = u32(round(knob(1u)));
    let luma_mhz = f.luma_mhz * select(select(1.0, 0.83, speed == 1u), 0.67, speed == 2u);
    let noise_scale = f.noise * select(select(1.0, 1.6, speed == 1u), 2.4, speed == 2u);
    // Lines of this raster to one of NTSC's, for things measured in lines.
    let ls = max(rows_f / 262.5, 1.0);

    // Transport. In Pause and Search the heads cross from track to track, and
    // between tracks the signal fades into noise: noise bars, with the
    // picture between them from fields further along the tape.
    let mode = u32(round(u.extra[1].x));
    let relock = u.extra[1].y;
    let bars = u.extra[0].z;
    var band = 0.0;
    var bar = 0.0;
    var within = 0.0;
    if (bars > 0.0) {
        let v = y_pos * bars - u.extra[0].w;
        band = floor(v) + 1.0;
        within = fract(v);
        let j = round(v);
        if (j >= 0.0 && j < bars) {
            // About 4% of the picture tall.
            bar = 1.0 - smoothstep(0.006 * bars, 0.024 * bars, abs(v - j));
        }
    }
    let age_f = max(u.extra[0].x + band * u.extra[0].y, 0.0);
    play_age = i32(round(age_f));
    // The tape's field number under the heads: dropouts and creases are on
    // the tape, so a paused field keeps its own.
    let tape_field = u32(max(u.extra[1].z - (age_f - u.extra[0].x), 0.0));

    // A stopped or spooling deck plays one field's track over and over, so
    // both fields show it (vertical sync stays each field's own).
    var src_row = row;
    if (mode >= 1u && c.field2_row < c.lines && fl.x >= 10) {
        let head_parity = i32(round(u.extra[1].w));
        let p = select((head_parity + play_age + 1) % 2, fl.y, play_age == 0);
        let start = select(0, c.field2_row, p == 1);
        src_row = start + min(fl.x, field_rows(c, p) - 1);
    }
    // The servo hunting after the transport changes, and a paused frame's
    // jitter: the whole field, sync and all, comes early or late.
    src_row = src_row - i32(round(u.extra[2].x));

    // Timebase error: tape stretch (wow), per-line jitter, and the skew at the
    // top of the field while the tape tension settles (flagging).
    let wow = knob(5u) * f.timebase * samples(c, 8.0) * sin(TAU * (y_pos * 0.7 + f32(field % 4096u) * 0.031));
    let jitter = knob(6u) * f.timebase * samples(c, 3.0) * (rnd(u32(fr / ls), field, 1u) - 0.5);
    let flag = knob(7u) * samples(c, 30.0) * exp(-fr / (10.0 * ls));
    var shift = wow + jitter + flag;
    // Head switching: the last lines before vertical sync come off the other
    // head, displaced, with a burst of noise at the changeover.
    let switch_line = rows_f - 8.0 * ls;
    let switching = fr >= switch_line;
    if (switching) {
        shift = shift + knob(8u) * samples(c, 24.0);
    }
    // Spooling, the tape runs past the heads at another speed, so each band
    // slants a little, and near a noise bar the sync tears.
    if (mode == 2u) {
        shift = shift + samples(c, 10.0) * within * sign(u.extra[0].y);
    }
    shift = shift + bar * samples(c, 40.0) * (rnd(urow, field, 21u) - 0.5);

    // Mistracking: a noise band that drifts through the picture. Stopped or
    // spooling, Tracking moves the noise bars instead.
    let mistrack = select(1.0 - knob(4u), 0.0, mode >= 1u);
    let band_pos = fract(f32(field % 100000u) * 0.0023 + 0.3);
    let band_width = max(mistrack * 0.25, 1e-4);
    var noise_band = (1.0 - smoothstep(0.0, band_width, abs(y_pos - band_pos))) * smoothstep(0.0, 0.15, mistrack);
    // While the servo relocks, a bar rolls through the picture.
    if (relock > 0.01) {
        let roll = fract(f32(field % 100000u) * 0.11);
        noise_band = max(noise_band, relock * (1.0 - smoothstep(0.02, 0.08, abs(y_pos - roll))));
    }
    noise_band = max(noise_band, bar);

    // A tape crease passes the drum: for a few fields of tape a torn, noisy
    // band crosses the picture, top to bottom. The crushed edge of the tape
    // ripples the bottom lines.
    let crease = knob(11u);
    var crease_k = 0.0;
    var edge = 0.0;
    if (crease > 0.0) {
        let event = tape_field / 16u;
        if (rnd(event, 0u, 91u) < crease * 0.5) {
            let cy = f32(tape_field % 16u) / 16.0 * 1.3 - 0.15;
            crease_k = crease * (1.0 - smoothstep(0.02, 0.07, abs(y_pos - cy)));
            shift = shift + crease_k * samples(c, 50.0) * (rnd(urow, field, 23u) - 0.5);
        }
        edge = crease * smoothstep(0.92, 1.0, y_pos);
        shift = shift + edge * samples(c, 12.0) * sin(fr * 0.8 / ls + f32(tape_field % 1024u) * 1.7);
    }

    // A clogged head: the two heads take turns a field at a time, and one
    // clogs before the other. Part clogged, it comes and goes; fully, it's
    // all snow and the TV loses sync.
    let clog = knob(10u);
    let even_head = field % 2u == 0u;
    var clogged = select(smoothstep(0.5, 1.0, clog), smoothstep(0.0, 0.5, clog), even_head);
    if (clogged < 1.0) {
        clogged = clogged * (0.5 + rnd(field / 3u, 0u, 41u));
    }
    clogged = clamp(clogged, 0.0, 1.0);

    // Dropouts: oxide missing from the tape, so they stay with what's taped.
    let tape_row = u32(src_row);
    let dropout = rnd(tape_row, tape_field, 7u) < knob(9u) * f.dropouts * 0.06;
    let d0 = rnd(tape_row, tape_field, 8u) * f32(c.spl);
    let dlen = samples(c, 8.0 + rnd(tape_row, tape_field, 9u) * 140.0);

    let b = burst(c, src_row, shift);
    let sep = lp_coef(c, 2.4);
    let lp_y = lp_coef(c, luma_mhz);
    let lp_y2 = lp_coef(c, luma_mhz * 0.4);
    let lp_c = lp_coef(c, f.chroma_mhz);
    // Snow comes out of the FM demodulator as streaks along the line,
    // without color: well below the subcarrier.
    let lp_snow = lp_coef(c, 1.0);
    // Spooling, the color-under phase slips: rainbows.
    let chroma_err = knob(12u) * select(1.0, 4.0, mode == 2u) + 0.3 * crease_k;
    let line_phase_error = chroma_err * 0.05 * (rnd(urow, field, 3u) - 0.5);
    let blotch_len = max(samples(c, 6.0), 1.0);
    let wander_len = samples(c, 96.0);

    let row_t = f32(src_row * c.spl);
    let first = c_time(c, row_t - shift);
    var ysep = first;
    var yrec = first;
    var yrec2 = first;
    var uv = vec2<f32>(0.0);
    var sn = 0.3;
    var sn2 = 0.3;
    for (var s = 0; s < c.spl; s = s + 1) {
        let src = f32(s) - shift;
        let x = c_time(c, row_t + src);
        // Chroma: the band around the subcarrier, demodulated with the
        // burst's phase and squeezed into the color-under bandwidth.
        ysep = ysep + sep * (x - ysep);
        let ph = TAU * (reference(c, src_row, src) + b.x);
        uv = uv + lp_c * (2.0 * (x - ysep) * vec2<f32>(sin(ph), cos(ph)) - uv);
        // Luma: what's left once the recovered chroma is taken out. The
        // chroma channel is gated off during sync, where there's no burst to
        // lock to and the "chroma" is just the edges of the sync pulses.
        let gate = select(0.0, smoothstep(-0.2, -0.05, yrec), b.y > 0.02);
        let chroma_est = gate * (uv.x * sin(ph) + uv.y * cos(ph));
        let luma_noise = knob(13u) * noise_scale * 0.12 * (rnd(u32(s), urow, field) - 0.5);
        var y_in = x - chroma_est + luma_noise;
        // Snow from a clogged head, a noise bar or a crease, smeared like
        // the picture it replaces.
        let lost = max(max(clogged, noise_band * 0.9), crease_k * 0.6);
        if (lost > 0.0) {
            sn = sn + lp_snow * (snow(s, urow, field, 13u) * 3.0 - sn);
            sn2 = sn2 + lp_snow * (sn - sn2);
            y_in = mix(y_in, sn2, lost);
        }
        yrec = yrec + lp_y * (y_in - yrec);
        yrec2 = yrec2 + lp_y2 * (y_in - yrec2);
        // Playback sharpening overshoots edges.
        let yv = yrec + knob(14u) * 1.5 * (yrec - yrec2);

        // Back to the subcarrier with noise and a per-line phase error.
        let bk = u32(f32(s) / blotch_len);
        let blotch = vec2<f32>(rnd(bk, urow, field + 1u), rnd(bk, urow, field + 2u)) - 0.5;
        let n_uv = (uv + blotch * knob(12u) * noise_scale * 0.12) * (1.0 - lost);
        // Phase error that wanders along the line: burst lock can't fix it.
        let k = f32(s) / wander_len;
        let w0 = rnd(u32(k), urow, field + 4u);
        let w1 = rnd(u32(k) + 1u, urow, field + 4u);
        let wander = chroma_err * 0.08 * (mix(w0, w1, fract(k)) - 0.5);
        let ph_out = ph + TAU * (line_phase_error + wander);
        var v = yv + gate * (n_uv.x * sin(ph_out) + n_uv.y * cos(ph_out));

        if (dropout && f32(s) >= d0 && f32(s) < d0 + dlen) {
            v = 0.75 + 0.3 * (rnd(u32(s), urow, field + 5u) - 0.5);
        }
        if (switching && fr < switch_line + 2.0 * ls) {
            v = v + knob(8u) * (rnd(u32(s), urow, field + 11u) - 0.5) * 0.8;
        }
        if (edge > 0.0) {
            v = mix(v, snow(s, urow, field, 17u), edge * 0.35);
        }
        textureStore(cout, vec2<i32>(s, row), vec4<f32>(v, 0.0, 0.0, 1.0));
    }
}
