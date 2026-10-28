// TV pass 2, deflection: one invocation simulates the line and field
// oscillators across this field and records where each scan line starts
// and how far down the screen it is. It also follows the signal's level
// for the AGC and the burst's strength for the color killer.
// Knobs: 5 H hold, 6 AFC speed, 7 V hold, 13 killer.

@group(0) @binding(20) var<storage, read> sync_info: array<vec4<f32>>;
@group(0) @binding(21) var<storage, read_write> st: ScanState;
// beams[0].x = count; beams[1 + i] = (start time in samples, lines since vertical retrace, 0, 0).
@group(0) @binding(22) var<storage, read_write> beams: array<vec4<f32>>;

// Per-half-line RC constant and trigger level of the vertical integrator.
// Normal lines sit near 0.07, broad pulses at 0.85: the integrator reads
// about 0.46 after one broad half-line and 0.66 after two, so it fires at
// the end of the second with margin either side.
const V_INTEGRATOR: f32 = 0.5;
const V_THRESHOLD: f32 = 0.56;
const NO_EDGE: f32 = -1.0e6;

@compute @workgroup_size(1)
fn main() {
    let c = comp();
    let spl = f32(c.spl);
    let frame = spl * f32(c.lines);
    let t0 = f32(c.row0) * spl;
    let t1 = f32(c.row0 + c.rows) * spl;
    let field_lines = f32(c.lines) / select(1.0, 2.0, c.field2_row < c.lines);
    let nominal = spl * (1.0 + 0.01 * knob(5u));

    var t = st.next_t;
    var trig = st.trig;
    var period = st.period;
    var integ = st.integ;
    if (st.valid < 0.5) {
        st.agc = 1.0;
        st.burst = BURST_LEVEL;
        st.color = 1.0;
        t = t0;
        trig = t0 - field_lines * spl;
        period = nominal;
        integ = 0.0;
    }
    // Times count samples from row 0 of the current frame.
    if (c.row0 == 0 && t > frame * 0.5) {
        t = t - frame;
        trig = trig - frame;
    }
    if (abs(t - t0) > spl * 4.0) {
        t = t0;
    }

    // Vertical: integrate the separated sync per half-line. The broad
    // vertical pulses charge it past threshold and retrigger the ramp, but
    // only near the end of a field; with no sync the ramp free-runs a
    // little slow, so the picture rolls. Triggering on half-line boundaries
    // keeps the two fields exactly half a line apart, which interlaces them.
    let v_free = field_lines * spl * (1.04 + 0.08 * knob(7u));
    let v_min = field_lines * spl * 0.85;
    let prev_trig = trig;
    var trigs: array<f32, 4>;
    var nt = 0;
    for (var h = 0; h < c.rows * 2; h = h + 1) {
        let row = c.row0 + h / 2;
        let info = sync_info[row];
        let below = select(info.y, info.z, (h % 2) == 1);
        integ = integ + V_INTEGRATOR * (below - integ);
        let th = f32(row) * spl + f32(h % 2 + 1) * spl * 0.5;
        let since = th - trig;
        if ((integ >= V_THRESHOLD && since > v_min) || since > v_free) {
            trig = th;
            if (nt < 4) {
                trigs[nt] = th;
                nt = nt + 1;
            }
        }
    }

    // Horizontal: a phase-locked loop pulls the line oscillator onto the
    // nearest sync edge. Fast AFC follows timebase errors (and bends with
    // them); slow AFC averages them out.
    let kp = knob(6u);
    let ki = kp * kp * 0.25;
    if (abs(period / nominal - 1.0) > 0.1) {
        period = nominal;
    }
    // The buffer holds the count and then as many lines as a field can scan.
    let max_beams = i32(arrayLength(&beams)) - 1;
    var n = 0;
    loop {
        if (t >= t1 || n >= max_beams) {
            break;
        }
        let r = i32(floor(t / spl));
        var err = 1.0e9;
        for (var dr = -1; dr <= 1; dr = dr + 1) {
            let rr = r + dr;
            if (rr < 0 || rr >= c.lines) {
                continue;
            }
            let e = sync_info[rr].x;
            if (e == NO_EDGE) {
                continue;
            }
            let d = f32(rr) * spl + e - t;
            if (abs(d) < abs(err)) {
                err = d;
            }
        }
        if (abs(err) < spl * 0.2) {
            t = t + kp * err;
            period = period + ki * err;
        }
        period = clamp(period + (nominal - period) * 0.002, nominal * 0.9, nominal * 1.1);
        var tr = prev_trig;
        for (var j = 0; j < nt; j = j + 1) {
            if (trigs[j] <= t) {
                tr = trigs[j];
            }
        }
        beams[1 + n] = vec4<f32>(t, (t - tr) / spl, 0.0, 0.0);
        n = n + 1;
        t = t + period;
    }
    beams[0] = vec4<f32>(f32(n), 0.0, 0.0, 0.0);

    // The picture's average level and the burst's strength over this
    // field's picture lines.
    var level = 0.0;
    var strength = 0.0;
    var counted = 0.0;
    for (var r = c.row0 + c.first_active; r < c.row0 + c.rows; r = r + 1) {
        level = level + sync_info[c.lines + r].x;
        strength = strength + sync_info[r].w;
        counted = counted + 1.0;
    }
    counted = max(counted, 1.0);
    // An AGC that averages the picture, as cheap sets had: it turns a dark
    // picture up and a bright one down, and pumps as the picture changes.
    let gain = clamp(0.38 / max(level / counted, 0.08), 0.6, 1.8);
    st.agc = st.agc + 0.15 * (gain - st.agc);
    // The killer decides for the field, with some hysteresis, so color goes
    // off and on cleanly instead of in streaks.
    st.burst = st.burst + 0.5 * (strength / counted - st.burst);
    let threshold = knob(13u) * BURST_LEVEL;
    if (st.burst < threshold * 0.8) {
        st.color = 0.0;
    } else if (st.burst > threshold * 1.25) {
        st.color = 1.0;
    }
    st.next_t = t;
    st.period = period;
    st.trig = trig;
    st.integ = integ;
    st.valid = 1.0;
}
