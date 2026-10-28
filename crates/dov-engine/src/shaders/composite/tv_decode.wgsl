// TV pass 3, decoder: burst-locked color, notch or comb luma/chroma
// separation, brightness/contrast/saturation/tint, one line per invocation.
// The burst, measured again after the chroma filter, sets the chroma gain
// (ACC) and trims any remaining hue error before the picture starts.
// Knobs: 0 brightness, 1 contrast, 2 saturation, 3 tint (cycles),
//        4 decoder (notch, comb, adaptive comb), 11 sharpness, 12 AGC.

@group(0) @binding(21) var<storage, read_write> st: ScanState;
@group(0) @binding(23) var decoded: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (i32(gid.x) >= c.rows) {
        return;
    }
    let row = c.row0 + i32(gid.x);
    let black = setup(c);
    let gain = 1.0 / max(1.0 - black, 0.01);
    // Combs work where the subcarrier inverts from line to line, as it does
    // for NTSC and the rasters that carry it scaled; PAL's doesn't.
    let decoder = select(u32(round(knob(4u))), 0u, c.pal);
    let b = burst(c, row, 0.0);
    // Color killer: no burst on this line, or too weak a burst over the
    // field (tv_scan.wgsl), no color.
    let color_on = c.color && b.y > 0.02 && st.color > 0.5 && u.inputs[0].w > 0.5;
    let a_y = lp_coef(c, 3.0);
    let a_c = lp_coef(c, 1.3);
    // Comparing lines for the adaptive comb, well below the subcarrier.
    let a_cmp = lp_coef(c, 0.8);
    // Sharpness: luma peaking around 1.5 MHz overshoots edges.
    let a_peak = lp_coef(c, 1.5);
    let sharp = knob(11u) * 1.5;
    // The AGC's gain, as much of it as the knob lets in.
    let agc = mix(1.0, st.agc, knob(12u));
    let pal_sign = select(1.0, -1.0, c.pal && (row % 2) == 1);
    let contrast = knob(1u);
    let sat = gain * contrast * knob(2u);
    var ys = tv_signal(c, row, 0) * agc;
    var uv = vec2<f32>(0.0);
    // The adaptive comb's line differences, filtered twice to leave the
    // luma's and not the chroma's.
    var d_up1 = 0.0;
    var d_up2 = 0.0;
    var d_down1 = 0.0;
    var d_down2 = 0.0;
    var yl = 0.0;
    let gate0 = i32(us(c, BURST_START_US)) + 4;
    let gate1 = i32(us(c, BURST_START_US + BURST_US)) - 1;
    var burst_sum = vec2<f32>(0.0);
    // Where the burst should demodulate to: 180° (NTSC), 135° (PAL).
    let expected = select(vec2<f32>(-1.0, 0.0), vec2<f32>(-0.70710678, 0.70710678), c.pal);
    var fix = vec2<f32>(1.0, 0.0);
    for (var s = 0; s < c.spl; s = s + 1) {
        let v = tv_signal(c, row, s) * agc;
        // Notch: split at the subcarrier.
        ys = ys + a_y * (v - ys);
        var chroma = v - ys;
        if (decoder == 1u) {
            // One-line comb: the line above has the subcarrier inverted,
            // so their difference is chroma; it smears vertical detail.
            chroma = 0.5 * (v - tv_signal(c, row - 1, s) * agc);
        } else if (decoder == 2u) {
            // Adaptive: comb with whichever line above or below looks
            // like this one, and where neither does (vertical detail),
            // fall back to the notch.
            let p = tv_signal(c, row - 1, s) * agc;
            let n = tv_signal(c, row + 1, s) * agc;
            d_up1 = d_up1 + a_cmp * (v - p - d_up1);
            d_up2 = d_up2 + a_cmp * (d_up1 - d_up2);
            d_down1 = d_down1 + a_cmp * (v - n - d_down1);
            d_down2 = d_down2 + a_cmp * (d_down1 - d_down2);
            let d_up = abs(d_up2);
            let d_down = abs(d_down2);
            let w_up = 1.0 / (d_up + 0.01);
            let w_down = 1.0 / (d_down + 0.01);
            let combed = (w_up * 0.5 * (v - p) + w_down * 0.5 * (v - n)) / (w_up + w_down);
            chroma = mix(combed, chroma, smoothstep(0.06, 0.16, min(d_up, d_down)));
        }
        let ph = TAU * (reference(c, row, f32(s)) + b.x + knob(3u));
        uv = uv + a_c * (2.0 * chroma * vec2<f32>(sin(ph), pal_sign * cos(ph)) - uv);
        var y: f32;
        if (decoder == 0u) {
            // Take the recovered chroma back out of the luma.
            y = v - select(0.0, uv.x * sin(ph) + pal_sign * uv.y * cos(ph), color_on);
        } else {
            y = v - chroma;
        }
        yl = yl + a_peak * (y - yl);
        y = y + sharp * (y - yl);
        if (s >= gate0 && s < gate1) {
            burst_sum = burst_sum + uv;
        }
        if (s == gate1) {
            // ACC and hue trim: a complex gain mapping the measured burst onto
            // where it belongs, at standard amplitude.
            let m = burst_sum / f32(gate1 - gate0);
            let m2 = dot(m, m);
            if (m2 > 1e-6) {
                let target_v = expected * BURST_LEVEL;
                let g = vec2<f32>(dot(target_v, m), target_v.y * m.x - target_v.x * m.y) / m2;
                let len = clamp(length(g), 0.25, 4.0);
                fix = normalize(g) * len;
            }
        }
        let fixed = vec2<f32>(fix.x * uv.x - fix.y * uv.y, fix.y * uv.x + fix.x * uv.y);
        let yp = (y - black) * gain * contrast + knob(0u);
        var rgb = vec3<f32>(yp);
        if (color_on) {
            let bb = yp + fixed.x * sat / 0.492;
            let rr = yp + fixed.y * sat / 0.877;
            rgb = vec3<f32>(rr, (yp - 0.299 * rr - 0.114 * bb) / 0.587, bb);
        }
        textureStore(decoded, vec2<i32>(s, row), vec4<f32>(rgb, 1.0));
    }
}
