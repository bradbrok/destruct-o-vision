// Fullscreen blit of a woven signal texture, letterboxed to the display
// aspect. In CRT mode it draws the picture tube at the target's resolution:
// a Gaussian beam per scan line that widens with brightness, phosphor mask,
// halation and screen curvature, all in linear light.

struct Blit {
    // Fraction of the target covered by the picture, per axis.
    scale: vec2<f32>,
    // 0 plain, 1 CRT.
    mode: f32,
    // 0 aperture grille, 1 shadow mask, 2 slot mask.
    mask_type: f32,
    mask: f32,
    scanlines: f32,
    glow: f32,
    curvature: f32,
    // Source size in pixels (width, rows) and target size in pixels.
    src_size: vec2<f32>,
    dst_size: vec2<f32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> blit: Blit;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the screen: (-1,-1), (3,-1), (-1,3).
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// Signals are gamma-encoded; the tube's light output is roughly V^2.4.
fn to_light(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.4));
}

fn sample_light(uv: vec2<f32>) -> vec3<f32> {
    return to_light(textureSampleLevel(src, samp, uv, 0.0).rgb);
}

// Phosphor pattern at a target pixel, averaging about 1/3 per channel.
fn phosphors(px: vec2<f32>, kind: u32) -> vec3<f32> {
    let dim = vec3<f32>(0.25);
    var x = px.x;
    if (kind == 1u) {
        // Shadow mask: triads offset on alternate rows.
        x = x + select(0.0, 1.5, (u32(px.y) % 2u) == 1u);
    }
    let i = u32(x) % 3u;
    var m = select(dim, vec3<f32>(1.0, 0.25, 0.25), i == 0u);
    m = select(m, vec3<f32>(0.25, 1.0, 0.25), i == 1u);
    m = select(m, vec3<f32>(0.25, 0.25, 1.0), i == 2u);
    if (kind == 2u) {
        // Slot mask: short vertical slots, staggered per triad.
        let triad = u32(px.x / 3.0);
        if ((u32(px.y) + triad * 2u) % 4u == 0u) {
            m = m * 0.35;
        }
    }
    return m;
}

fn crt(uv: vec2<f32>, frag: vec2<f32>) -> vec3<f32> {
    // Scan lines: each source row is a beam with a Gaussian profile. Bright
    // lines bloom wider; dark ones leave visible gaps.
    let rows = blit.src_size.y;
    let y = uv.y * rows - 0.5;
    let r0 = floor(y);
    var beams = vec3<f32>(0.0);
    for (var k = -1; k <= 2; k = k + 1) {
        let r = r0 + f32(k);
        let c = sample_light(vec2<f32>(uv.x, (r + 0.5) / rows));
        let width = mix(0.3, 0.55, sqrt(clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0)));
        let d = (y - r) / width;
        // Normalized so a flat field keeps its brightness.
        beams = beams + c * exp(-d * d) / (width * 1.7725);
    }
    var light = mix(sample_light(uv), beams, blit.scanlines);

    // Halation: light scattering in the glass.
    let px = 1.0 / blit.src_size;
    var halo = vec3<f32>(0.0);
    for (var k = 0; k < 8; k = k + 1) {
        let a = f32(k) * 0.785398;
        halo = halo + sample_light(uv + vec2<f32>(cos(a), sin(a)) * px * 4.0);
    }
    light = light + halo / 8.0 * blit.glow * 0.6;

    // Phosphors, with gain to make up for the mask's darkening.
    let m = phosphors(frag, u32(round(blit.mask_type)));
    light = light * mix(vec3<f32>(1.0), m * 2.0, blit.mask);

    // A little corner falloff.
    let e = uv * (1.0 - uv);
    light = light * mix(1.0, clamp(pow(e.x * e.y * 30.0, 0.25), 0.0, 1.0), blit.curvature);
    return pow(light, vec3<f32>(1.0 / 2.2));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var q = in.ndc / blit.scale;
    if (blit.mode > 0.5) {
        // Barrel distortion of the tube face.
        q = q * (1.0 + blit.curvature * 0.12 * dot(q, q));
    }
    if (any(abs(q) > vec2<f32>(1.0))) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let uv = vec2<f32>(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5);
    if (blit.mode < 0.5) {
        return vec4<f32>(textureSampleLevel(src, samp, uv, 0.0).rgb, 1.0);
    }
    return vec4<f32>(crt(uv, in.pos.xy), 1.0);
}
