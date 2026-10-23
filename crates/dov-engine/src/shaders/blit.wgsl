// Fullscreen blit of a woven signal texture, letterboxed to the display
// aspect. (A CRT module draws the output window itself; see tube/.)

struct Blit {
    // Fraction of the target covered by the picture, per axis.
    scale: vec2<f32>,
    _pad: vec2<f32>,
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let q = in.ndc / blit.scale;
    if (any(abs(q) > vec2<f32>(1.0))) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let uv = vec2<f32>(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5);
    return vec4<f32>(textureSampleLevel(src, samp, uv, 0.0).rgb, 1.0);
}
