// TV pass 4, picture tube: each scan line paints the screen row its
// vertical deflection puts it on, reading the decoded signal from where the
// line oscillator says the line starts. Rows nothing lands on keep their
// old content, like phosphor.
// Knobs: 9 V position (lines), 10 H position (samples).

@group(0) @binding(22) var<storage, read> beams: array<vec4<f32>>;
@group(0) @binding(24) var decoded: texture_2d<f32>;
@group(0) @binding(16) var out_0: texture_storage_2d<rgba16float, write>;

// Lines the beam spends flying back to the top, blanked.
const V_RETRACE_LINES: f32 = 3.0;
// Lines from the start of the vertical sync pulses to the retrace trigger
// (two broad half-lines; see V_INTEGRATOR in tv_scan.wgsl).
const V_TRIGGER_LINES: f32 = 1.0;

fn decoded_at(c: Comp, n: i32) -> vec3<f32> {
    let row = i32(floor(f32(n) / f32(c.spl)));
    let r = ((row % c.lines) + c.lines) % c.lines;
    return textureLoad(decoded, vec2<i32>(clamp(n - row * c.spl, 0, c.spl - 1), r), 0).rgb;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = comp();
    if (gid.x >= u.info.width || f32(gid.y) >= beams[0].x) {
        return;
    }
    let b = beams[1u + gid.y];
    if (b.y < V_RETRACE_LINES) {
        return;
    }
    // Vertical sync begins 3 lines into the field; the picture 'first_active' lines in.
    let v_off = f32(c.first_active) - 3.0 - V_TRIGGER_LINES + knob(9u);
    let row = i32(round(f32(u.info.row_stride) * (b.y - v_off)));
    if (row < 0 || row >= i32(u.info.height)) {
        return;
    }
    let t = b.x + 0.5 + f32(c.active_start) + knob(10u) + f32(gid.x) * f32(c.active_samples) / f32(u.info.width);
    let i = i32(floor(t));
    let rgb = mix(decoded_at(c, i), decoded_at(c, i + 1), t - floor(t));
    textureStore(out_0, vec2<i32>(i32(gid.x), row), vec4<f32>(rgb, 1.0));
}
