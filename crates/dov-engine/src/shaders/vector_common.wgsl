// An XY display. The module's behavior hands over the points the beam goes
// through this field (u.extra[0].x of them): x, y pairs, oldest first, with
// -1..1 across the screen's graticule, a square 0.9 picture heights across.

// The beam's energy is counted in 1/4096ths of a point's worth.
const ENERGY_SCALE: f32 = 4096.0;

// Where the screen point `p` lands: samples across and lines down this field.
fn to_field(p: vec2<f32>) -> vec2<f32> {
    let w = f32(u.info.width);
    let h = f32(u.info.height);
    let x = w * 0.5 + p.x * 0.45 * w / u.info.aspect;
    let row = h * 0.5 - p.y * 0.45 * h;
    return vec2<f32>(x, (row - f32(u.info.parity)) / f32(u.info.row_stride));
}
