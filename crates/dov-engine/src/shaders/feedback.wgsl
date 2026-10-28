// Frame store with a geometric transform. Reads last field's input (the
// store) and writes this field's output. The transform is worked out per
// sample, so video-rate CV warps it: each pixel can zoom, turn and shift by
// its own amount.
//
// `u.extra` (filled on the CPU), for params k = 0 zoom, 1 rotate (degrees),
// 2 x, 3 y, 4 gain:
//   extra[k] = (knob value, CV scale = attenuverter × span)
//   extra[5].x = mirror flags
//   extra[6] = hue (degrees a pass) and blur (samples): knob and CV scale each
//   extra[7].x = key
// CV inputs are module inputs 1..=7: zoom, rotate, x, y, gain, hue, blur.

@group(0) @binding(1) var store: texture_2d<f32>;
@group(0) @binding(2) var cv_zoom: texture_2d<f32>;
@group(0) @binding(3) var cv_rotate: texture_2d<f32>;
@group(0) @binding(4) var cv_x: texture_2d<f32>;
@group(0) @binding(5) var cv_y: texture_2d<f32>;
@group(0) @binding(6) var cv_gain: texture_2d<f32>;
@group(0) @binding(7) var cv_hue: texture_2d<f32>;
@group(0) @binding(8) var cv_blur: texture_2d<f32>;
@group(0) @binding(16) var out_0: texture_storage_2d<rgba16float, write>;

// Param `k` at this sample: its knob plus whatever is patched into its CV
// jack (input `k + 1`): a video signal, a control value, or nothing.
fn param_at(k: u32, patched: vec3<f32>) -> f32 {
    return cv_param(u.extra[k].xy, k + 1u, patched);
}

// A param whose knob and CV scale are `knob`, with its CV at input `j`.
fn cv_param(knob: vec2<f32>, j: u32, patched: vec3<f32>) -> f32 {
    let cv = select(u.inputs[j].xyz, patched, u.inputs[j].w > 0.5);
    return knob.x + luma(cv) * knob.y;
}

// `rgb` with its hue turned by `angle` radians, about the YIQ gray axis.
fn turn_hue(rgb: vec3<f32>, angle: f32) -> vec3<f32> {
    let y = dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
    let i = dot(rgb, vec3<f32>(0.596, -0.274, -0.322));
    let q = dot(rgb, vec3<f32>(0.211, -0.523, 0.312));
    let c = cos(angle);
    let s = sin(angle);
    let i2 = i * c - q * s;
    let q2 = i * s + q * c;
    return vec3<f32>(
        y + 0.956 * i2 + 0.621 * q2,
        y - 0.272 * i2 - 0.647 * q2,
        y - 1.106 * i2 + 1.703 * q2,
    );
}

fn load_line(x: i32, line: i32, fresh: i32) -> vec3<f32> {
    let w = i32(u.info.width);
    let l = clamp(line, 0, i32(u.info.lines) - 1);
    let row = l * i32(u.info.row_stride) + fresh;
    return textureLoad(store, vec2<i32>(clamp(x, 0, w - 1), row), 0).rgb;
}

// Bilinear sample using only the lines written by the previous field. In an
// interlaced store the other lines are a field older; mixing them combs
// under rotation, which feedback then amplifies.
fn sample_field(uv: vec2<f32>) -> vec3<f32> {
    let stride = i32(u.info.row_stride);
    let fresh = (i32(u.info.parity) + 1) % stride;
    let px = uv.x * f32(u.info.width) - 0.5;
    let py = uv.y * f32(u.info.height) - 0.5;
    let fy = (py - f32(fresh)) / f32(stride);
    let x0 = floor(px);
    let l0 = floor(fy);
    let tx = px - x0;
    let ty = fy - l0;
    let xi = i32(x0);
    let li = i32(l0);
    let top = mix(load_line(xi, li, fresh), load_line(xi + 1, li, fresh), tx);
    let bottom = mix(load_line(xi, li + 1, fresh), load_line(xi + 1, li + 1, fresh), tx);
    return mix(top, bottom, ty);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.info.width || gid.y >= u.info.lines) {
        return;
    }
    let ctx = make_ctx(gid.x, gid.y);
    let coord = vec2<u32>(ctx.x, ctx.row);
    let zoom = max(param_at(0u, textureLoad(cv_zoom, coord, 0).rgb), 0.01);
    let angle = radians(param_at(1u, textureLoad(cv_rotate, coord, 0).rgb));
    let shift = vec2<f32>(
        param_at(2u, textureLoad(cv_x, coord, 0).rgb) * ctx.aspect,
        param_at(3u, textureLoad(cv_y, coord, 0).rgb),
    );
    let gain = param_at(4u, textureLoad(cv_gain, coord, 0).rgb);

    let scale = vec2<f32>(ctx.aspect, 1.0);
    var q = (ctx.uv - vec2<f32>(0.5)) * scale;
    let mirror = u32(u.extra[5].x);
    if ((mirror & 1u) != 0u) {
        q.x = -abs(q.x);
    }
    if ((mirror & 2u) != 0u) {
        q.y = -abs(q.y);
    }

    // The picture was zoomed, rotated, then shifted; sample the store
    // through the inverse: R(-θ) (q - shift) / zoom.
    let c = cos(angle);
    let s = sin(angle);
    let m = mat2x2<f32>(vec2<f32>(c, -s), vec2<f32>(s, c));
    let src = (m * (q - shift)) / zoom / scale + vec2<f32>(0.5);
    var color = vec3<f32>(0.0);
    if (all(src >= vec2<f32>(0.0)) && all(src <= vec2<f32>(1.0))) {
        color = sample_field(src);
        // Blur: four more taps around it, `blur` samples out, so each pass
        // round the loop spreads the picture a little further.
        let blur = max(cv_param(u.extra[6].zw, 7u, textureLoad(cv_blur, coord, 0).rgb), 0.0);
        if (blur > 0.01) {
            let d = blur / vec2<f32>(f32(u.info.width), f32(u.info.height));
            color = (color
                + sample_field(src + d) + sample_field(src - d)
                + sample_field(src + vec2<f32>(d.x, -d.y)) + sample_field(src + vec2<f32>(-d.x, d.y))) / 5.0;
        }
        let hue = cv_param(u.extra[6].xy, 6u, textureLoad(cv_hue, coord, 0).rgb);
        if (hue != 0.0) {
            color = turn_hue(color, radians(hue));
        }
        // Key: only what's brighter than it comes round again.
        let key = u.extra[7].x;
        if (key > 0.0) {
            color = color * smoothstep(key - 0.04, key + 0.04, luma(color));
        }
        color = color * gain;
    }
    textureStore(out_0, coord, vec4<f32>(color, 1.0));
}
