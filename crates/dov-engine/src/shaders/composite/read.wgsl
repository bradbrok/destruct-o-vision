// Reading a composite input bound at binding 1.

@group(0) @binding(1) var cin: texture_2d<f32>;

fn c_at(c: Comp, row: i32, s: i32) -> f32 {
    let r = ((row % c.lines) + c.lines) % c.lines;
    return textureLoad(cin, vec2<i32>(clamp(s, 0, c.spl - 1), r), 0).r;
}
