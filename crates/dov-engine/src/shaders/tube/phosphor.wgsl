// Tube passes 1 and 2, at the signal's size.

@group(0) @binding(1) var signal: texture_2d<f32>;
@group(0) @binding(2) var charge_in: texture_2d<f32>;
@group(0) @binding(3) var charge_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var light_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var current_out: texture_storage_2d<rgba16float, write>;

@group(0) @binding(6) var light_in: texture_2d<f32>;
@group(0) @binding(7) var current_in: texture_2d<f32>;
@group(0) @binding(8) var beam_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(9) var sigma_out: texture_storage_2d<rgba16float, write>;

// Pass 1, the phosphors. Each refresh, the beam sweeps the rows whose time
// has come; each sweep lays down the light for the time until the next,
// which the phosphor gives off as it decays. Light too bright for one
// refresh waits for the next ones, so on average every sample shows just
// what it should (as in Blur Busters' CRT beam simulator).
@compute @workgroup_size(16, 8)
fn phosphor(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= t.src.x || f32(id.y) >= t.src.y) {
        return;
    }
    let stride = t.src.z;
    let parity = f32(id.y % u32(stride));
    let line = f32(id.y / u32(stride));

    // The video amplifier and guns: the light each sample asks for, as the
    // tube would show it on average.
    let v = max(textureLoad(signal, id.xy, 0).rgb + t.drive.w, vec3<f32>(0.0));
    var want: vec3<f32>;
    var current: vec3<f32>;
    if (t.decay.w > 0.5) {
        // One gun, one phosphor.
        let beam = pow(dot(v, LUMA), t.scan.w) * t.drive.x;
        want = beam * t.phosphor.rgb;
        current = vec3<f32>(beam);
    } else {
        want = pow(v, vec3<f32>(t.scan.w)) * t.drive.rgb;
        current = want;
    }

    // When the beam last swept this row, in fields from the start of the
    // current one.
    let at = (t.scan.x + 0.5 * parity + line) / t.src.w;
    var swept: bool;
    var age: f32;
    if (t.time.w > 0.5) {
        // Whole fields: the current one's rows, or every row after a skip.
        swept = stride < 1.5 || parity == t.time.z || t.time.y > 1.5;
        age = t.time.y;
    } else {
        var field = floor(t.time.y - at);
        if (stride > 1.5) {
            // Interlaced: each row is swept every other field.
            let d = field - (parity - t.time.z);
            if (d - 2.0 * floor(d * 0.5) > 0.5) {
                field = field - 1.0;
            }
        }
        swept = field + at >= t.time.x;
        age = t.time.y - (field + at);
    }

    let dt = max(t.time.y - t.time.x, 1.0e-3);
    let tau = max(t.decay.rgb, vec3<f32>(1.0e-3));
    var stored = textureLoad(charge_in, id.xy, 0).rgb;
    let keep = exp(-dt / tau);
    var emitted = stored * (1.0 - keep);
    stored = stored * keep;
    if (swept) {
        let deposit = want * stride;
        let left = exp(-age / tau);
        emitted = emitted + deposit * (1.0 - left);
        stored = stored + deposit * left;
    }
    var light = emitted / dt;
    let over = max(light - vec3<f32>(t.scan.z), vec3<f32>(0.0));
    light = light - over;
    // A picture brighter than the tube can show doesn't pile up forever.
    stored = min(stored + over * dt, vec3<f32>(t.scan.z * (stride + 1.0)));

    textureStore(charge_out, id.xy, vec4<f32>(stored, 0.0));
    textureStore(light_out, id.xy, vec4<f32>(light, 1.0));
    textureStore(current_out, id.xy, vec4<f32>(current / t.scan.y, 1.0));
}

const MAX_REACH: i32 = 32;
const INV_SQRT_TAU: f32 = 0.39894228;

// Pass 2, the spot across. Each sample's light spreads along its line by
// the beam spot, which is round and grows with the beam current, softened
// by the video amplifier's bandwidth. Written at twice the signal's width,
// so the screen pass can interpolate; the spread down the screen happens
// there, along with each line's spot size.
@compute @workgroup_size(16, 8)
fn spot(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(beam_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let width = i32(t.src.x);
    let row = i32(id.y);
    let x = (f32(id.x) + 0.5) * 0.5 - 0.5;
    let widest = widest_spot() * t.spot.w;
    let reach = min(i32(ceil(3.0 * sqrt(widest * widest + t.spot.z * t.spot.z))), MAX_REACH);
    let x0 = i32(floor(x));
    var sum = vec3<f32>(0.0);
    var spread = vec3<f32>(0.0);
    for (var k = max(x0 - reach, 0); k <= min(x0 + reach + 1, width - 1); k = k + 1) {
        // Each sample spreads by its own spot, so a bright one blooms into
        // its dark neighbors.
        let sigma = spot_sigma(textureLoad(current_in, vec2<i32>(k, row), 0).rgb);
        let across = sqrt(sigma * sigma * t.spot.w * t.spot.w + t.spot.z * t.spot.z);
        let d = (f32(k) - x) / across;
        let light = textureLoad(light_in, vec2<i32>(k, row), 0).rgb * exp(-0.5 * d * d) / across;
        sum = sum + light;
        spread = spread + light * sigma;
    }
    // Light spread here keeps the spot it came from, so it spreads down the
    // screen as far as it did across. Where none came, the spot is this
    // sample's own.
    let near = clamp(i32(round(x)), 0, width - 1);
    let own = spot_sigma(textureLoad(current_in, vec2<i32>(near, row), 0).rgb);
    let sigma = select(own, spread / max(sum, vec3<f32>(1.0e-12)), sum > vec3<f32>(1.0e-6));
    textureStore(beam_out, id.xy, vec4<f32>(sum * INV_SQRT_TAU, 1.0));
    textureStore(sigma_out, id.xy, vec4<f32>(sigma, 1.0));
}
