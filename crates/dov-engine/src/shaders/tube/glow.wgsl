// Tube pass 3, halation: light scattered in the faceplate glass, as a soft
// glow around bright parts. A pyramid of ever-smaller copies of the light,
// blurred on the way down and gathered on the way back up (after Jimenez,
// "Next generation post processing in Call of Duty: Advanced Warfare").

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var wider: texture_2d<f32>;

// How much of each level's glow comes from the wider levels below it.
const SPREAD: f32 = 0.5;

fn from_src(uv: vec2<f32>, px: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(src, samp, uv + vec2<f32>(x, y) * px, 0.0).rgb;
}

fn from_wider(uv: vec2<f32>, px: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(wider, samp, uv + vec2<f32>(x, y) * px, 0.0).rgb;
}

@compute @workgroup_size(8, 8)
fn down(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let px = 1.0 / vec2<f32>(textureDimensions(src));
    let center = from_src(uv, px, 0.0, 0.0);
    let inner = from_src(uv, px, -1.0, -1.0) + from_src(uv, px, 1.0, -1.0) + from_src(uv, px, -1.0, 1.0)
        + from_src(uv, px, 1.0, 1.0);
    let sides = from_src(uv, px, -2.0, 0.0) + from_src(uv, px, 2.0, 0.0) + from_src(uv, px, 0.0, -2.0)
        + from_src(uv, px, 0.0, 2.0);
    let corners = from_src(uv, px, -2.0, -2.0) + from_src(uv, px, 2.0, -2.0) + from_src(uv, px, -2.0, 2.0)
        + from_src(uv, px, 2.0, 2.0);
    let c = (center + inner) * 0.125 + sides * 0.0625 + corners * 0.03125;
    textureStore(dst, id.xy, vec4<f32>(c, 1.0));
}

@compute @workgroup_size(8, 8)
fn up(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let px = 1.0 / vec2<f32>(textureDimensions(wider));
    let corners = from_wider(uv, px, -1.0, -1.0) + from_wider(uv, px, 1.0, -1.0) + from_wider(uv, px, -1.0, 1.0)
        + from_wider(uv, px, 1.0, 1.0);
    let sides = from_wider(uv, px, 0.0, -1.0) + from_wider(uv, px, 0.0, 1.0) + from_wider(uv, px, -1.0, 0.0)
        + from_wider(uv, px, 1.0, 0.0);
    let tent = (corners + 2.0 * sides + 4.0 * from_wider(uv, px, 0.0, 0.0)) / 16.0;
    let here = textureLoad(src, id.xy, 0).rgb;
    textureStore(dst, id.xy, vec4<f32>(mix(here, tent, SPREAD), 1.0));
}
