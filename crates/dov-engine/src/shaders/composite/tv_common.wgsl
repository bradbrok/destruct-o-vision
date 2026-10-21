// The TV's incoming waveform, or snow when nothing is plugged in.
fn tv_signal(c: Comp, row: i32, s: i32) -> f32 {
    if (u.inputs[0].w > 0.5) {
        return c_at(c, row, s);
    }
    let r = ((row % c.lines) + c.lines) % c.lines;
    return (rnd(u32(s), u32(r), u.info.field) - 0.5) * 0.9 + 0.3;
}
