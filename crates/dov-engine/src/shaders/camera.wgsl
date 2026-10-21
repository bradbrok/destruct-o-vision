// Camera input scaled into the raster. `u.extra[0]` (filled on the CPU):
//   (uv scale x, uv scale y, mirror, unused)
// Camera frames are already gamma-encoded, like every signal here.

@group(0) @binding(1) var cam: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(16) var out_0: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.info.width || gid.y >= u.info.lines) {
        return;
    }
    let ctx = make_ctx(gid.x, gid.y);
    var uv = ctx.uv;
    if (u.extra[0].z > 0.5) {
        uv.x = 1.0 - uv.x;
    }
    let src = (uv - vec2<f32>(0.5)) * u.extra[0].xy + vec2<f32>(0.5);
    var c = vec3<f32>(0.0);
    if (all(src >= vec2<f32>(0.0)) && all(src <= vec2<f32>(1.0))) {
        c = textureSampleLevel(cam, samp, src, 0.0).rgb;
    }
    textureStore(out_0, vec2<u32>(ctx.x, ctx.row), vec4<f32>(c, 1.0));
}
