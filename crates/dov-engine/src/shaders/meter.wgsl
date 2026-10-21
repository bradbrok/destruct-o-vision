// Average of a video signal, for cables from video outputs into control
// inputs. One workgroup sums a grid of samples across the woven frame and
// reduces it; the result is (mean R, G, B, mean luma).

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> result: vec4<f32>;

const THREADS: u32 = 256u;
var<workgroup> partial: array<vec4<f32>, 256>;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_index) i: u32) {
    let size = textureDimensions(src);
    // About 360 × 240 samples is plenty for an average, even in HD.
    let step = max(size / vec2<u32>(360u, 240u), vec2<u32>(1u));
    let grid = (size + step - vec2<u32>(1u)) / step;
    var acc = vec4<f32>(0.0);
    for (var n = i; n < grid.x * grid.y; n = n + THREADS) {
        let c = textureLoad(src, vec2<u32>(n % grid.x, n / grid.x) * step, 0).rgb;
        acc = acc + vec4<f32>(c, luma(c));
    }
    partial[i] = acc;
    workgroupBarrier();
    for (var w = THREADS / 2u; w > 0u; w = w / 2u) {
        if (i < w) {
            partial[i] = partial[i] + partial[i + w];
        }
        workgroupBarrier();
    }
    if (i == 0u) {
        result = partial[0] / f32(grid.x * grid.y);
    }
}
