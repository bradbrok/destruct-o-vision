// XY display pass 2: the phosphor. It glows with the energy the beam left
// this field, spread by the spot's focus, and fades as Persist lets it.
// Knobs (see `library::VECTOR`): 6 focus, 7 brightness, 8 persistence,
// 9 phosphor.

@group(0) @binding(20) var<storage, read_write> beam: array<u32>;
@group(0) @binding(21) var<storage, read_write> glow: array<f32>;
@group(0) @binding(16) var out_0: texture_storage_2d<rgba16float, write>;

fn energy(x: i32, line: i32) -> f32 {
    let w = i32(u.info.width);
    let l = clamp(line, 0, i32(u.info.lines) - 1);
    return f32(beam[u32(l * w + clamp(x, 0, w - 1))]) / ENERGY_SCALE;
}

fn phosphor(kind: u32) -> vec3<f32> {
    switch kind {
        // P7: blue-white.
        case 1u: {
            return vec3<f32>(0.45, 0.65, 1.0);
        }
        // P134: amber.
        case 2u: {
            return vec3<f32>(1.0, 0.62, 0.12);
        }
        // P4: white.
        case 3u: {
            return vec3<f32>(0.92, 0.94, 1.0);
        }
        // P31: green.
        default: {
            return vec3<f32>(0.3, 1.0, 0.4);
        }
    }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.info.width || gid.y >= u.info.lines) {
        return;
    }
    let ctx = make_ctx(gid.x, gid.y);
    let x = i32(gid.x);
    let l = i32(gid.y);
    // Focus: a soft spot spreads the energy over its neighbors.
    let f = clamp(knob(6u), 0.0, 1.0);
    let edge = 0.6 * f;
    let corner = 0.35 * f;
    let e = (energy(x, l)
        + edge * (energy(x - 1, l) + energy(x + 1, l) + energy(x, l - 1) + energy(x, l + 1))
        + corner * (energy(x - 1, l - 1) + energy(x + 1, l - 1) + energy(x - 1, l + 1) + energy(x + 1, l + 1)))
        / (1.0 + 4.0 * edge + 4.0 * corner);
    // Persist: from gone by the next field to a long fade over seconds.
    let p = clamp(knob(8u), 0.0, 1.0);
    let decay = exp(-1.0 / (0.5 + p * p * 90.0));
    let i = ctx.row * u.info.width + ctx.x;
    let g = glow[i] * decay + e * max(knob(7u), 0.0);
    glow[i] = g;
    let level = 1.0 - exp(-g);
    // Where the beam lingers the phosphor saturates toward white.
    let color = mix(phosphor(u32(round(knob(9u)))) * level, vec3<f32>(level), smoothstep(0.6, 1.0, level) * 0.6);
    textureStore(out_0, vec2<u32>(ctx.x, ctx.row), vec4<f32>(color, 1.0));
}
