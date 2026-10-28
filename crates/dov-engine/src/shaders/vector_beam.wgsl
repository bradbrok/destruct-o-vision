// XY display pass 1: the beam. One invocation per stretch between two
// points: the beam spends one sample period on it, so its energy spreads
// along it, and a fast jump leaves only a faint trace.

@group(0) @binding(32) var<storage, read> points: array<vec2<f32>>;
@group(0) @binding(20) var<storage, read_write> beam: array<atomic<u32>>;

fn deposit(x: i32, line: i32, e: f32) {
    if (x < 0 || line < 0 || x >= i32(u.info.width) || line >= i32(u.info.lines) || e <= 0.0) {
        return;
    }
    atomicAdd(&beam[u32(line) * u.info.width + u32(x)], u32(e * ENERGY_SCALE));
}

// Energy `e` at `q`, shared among the four nearest samples.
fn splat(q: vec2<f32>, e: f32) {
    let p = q - vec2<f32>(0.5);
    let f = floor(p);
    let t = p - f;
    let x = i32(f.x);
    let y = i32(f.y);
    deposit(x, y, e * (1.0 - t.x) * (1.0 - t.y));
    deposit(x + 1, y, e * t.x * (1.0 - t.y));
    deposit(x, y + 1, e * (1.0 - t.x) * t.y);
    deposit(x + 1, y + 1, e * t.x * t.y);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let count = u32(u.extra[0].x);
    let i = gid.x;
    if (i + 1u >= count) {
        return;
    }
    let a = to_field(points[i]);
    let b = to_field(points[i + 1u]);
    let steps = u32(clamp(ceil(length(b - a)), 1.0, 512.0));
    let e = 1.0 / f32(steps);
    for (var k = 0u; k < steps; k = k + 1u) {
        splat(mix(a, b, (f32(k) + 0.5) / f32(steps)), e);
    }
}
