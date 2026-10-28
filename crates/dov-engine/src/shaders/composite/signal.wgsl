// Reading the waveform through `c_at`, which a pass defines (read.wgsl
// for its input; the VCR's for its tape).

// The sample `n` samples after row 0, sample 0 of the frame.
fn c_abs(c: Comp, n: i32) -> f32 {
    let row = i32(floor(f32(n) / f32(c.spl)));
    return c_at(c, row, n - row * c.spl);
}

// The waveform at a fractional time in samples since row 0 of the frame.
fn c_time(c: Comp, t: f32) -> f32 {
    let i = i32(floor(t));
    return mix(c_abs(c, i), c_abs(c, i + 1), t - floor(t));
}

// Burst phase relative to the reference oscillator (cycles) and burst
// amplitude, measured on the line starting at `shift` samples late.
fn burst(c: Comp, row: i32, shift: f32) -> vec2<f32> {
    let start = i32(us(c, BURST_START_US)) + 2;
    let end = i32(us(c, BURST_START_US + BURST_US)) - 2;
    var bs = 0.0;
    var bc = 0.0;
    for (var s = start; s < end; s = s + 1) {
        let src = f32(s) - shift;
        let ph = TAU * reference(c, row, src);
        let v = c_time(c, f32(row * c.spl) + src);
        bs = bs + v * sin(ph);
        bc = bc + v * cos(ph);
    }
    let n = f32(max(end - start, 1));
    let power = bs * bs + bc * bc;
    if (power < 1e-10) {
        // No burst at all (e.g. vertical sync lines). atan2(0, 0) is NaN on
        // some GPUs, and a NaN here would wipe out the whole line downstream.
        return vec2<f32>(0.0, 0.0);
    }
    // The burst is -A sin(ph + d): the correlations are -A cos(d) and -A sin(d) (× n/2).
    var d = atan2(-bc, -bs) / TAU;
    if (c.pal) {
        // Undo the ±45° PAL burst swing.
        d = d + select(0.125, -0.125, (row % 2) == 1);
    }
    return vec2<f32>(d, 2.0 * sqrt(power) / n);
}
