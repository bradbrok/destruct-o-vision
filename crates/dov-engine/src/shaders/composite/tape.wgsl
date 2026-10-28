// Reading a VCR's input (binding 1) and its tape: the history ring of what
// it taped, a field per slot. `c_at` reads the tape as it was `play_age`
// fields ago, or the input itself at 0, so the shared helpers (`c_time`,
// `burst`) see whatever is playing back.

@group(0) @binding(1) var cin: texture_2d<f32>;
@group(0) @binding(24) var tape: texture_2d_array<f32>;

var<private> play_age: i32 = 0;

fn c_at(c: Comp, row: i32, s: i32) -> f32 {
    let r = ((row % c.lines) + c.lines) % c.lines;
    let x = clamp(s, 0, c.spl - 1);
    let depth = i32(u.info.ring_depth);
    if (play_age <= 0 || depth == 0) {
        return textureLoad(cin, vec2<i32>(x, r), 0).r;
    }
    let slot = ((i32(u.info.ring_head) - (min(play_age, depth) - 1)) % depth + depth) % depth;
    return textureLoad(tape, vec2<i32>(x, r), slot, 0).r;
}
