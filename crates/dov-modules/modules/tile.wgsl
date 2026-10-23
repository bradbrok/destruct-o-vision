/* dov-module
format = 1
id = "dov.tile"
name = "Tile"
category = "space"
description = "Repeats the picture in a grid. Mirror flips every other tile so neighbors meet seamlessly; Brick shifts every other row by half a tile. Scroll slides the grid (patch a saw to keep it moving), and Gap draws black lines between tiles. Counts can be fractional."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "across"
name = "Across"
range = [1.0, 16.0]
default = 3.0
cv = "video"

[[param]]
id = "down"
name = "Down"
range = [1.0, 16.0]
default = 3.0
cv = "video"

[[param]]
id = "mode"
name = "Mode"
choices = ["Repeat", "Mirror", "Brick"]

[[param]]
id = "scroll_x"
name = "Scroll X"
range = [-1.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "scroll_y"
name = "Scroll Y"
range = [-1.0, 1.0]
default = 0.0
cv = "video"

[[param]]
id = "gap"
name = "Gap"
range = [0.0, 0.5]
default = 0.0
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let w = f32(u.info.width);
    let lines = f32(u.info.lines);
    // Tile coordinates: whole numbers at tile edges, scrolled in tiles.
    let counts = max(vec2<f32>(p.across, p.down), vec2<f32>(1.0));
    var t = vec2<f32>((f32(ctx.x) + 0.5) / w, (f32(ctx.line) + 0.5) / lines) * counts;
    t = t - vec2<f32>(p.scroll_x, p.scroll_y);
    let row = floor(t.y);
    if (p.mode == 2u && i32(row) % 2 != 0) {
        t.x = t.x + 0.5;
    }
    let cell = floor(t);
    var f = t - cell;

    var o: Outputs;
    let half_gap = 0.5 * clamp(p.gap, 0.0, 0.5);
    if (any(f < vec2<f32>(half_gap)) || any(f > vec2<f32>(1.0 - half_gap))) {
        o.out = vec3<f32>(0.0);
        return o;
    }
    if (p.mode == 1u) {
        // Odd tiles flip, so every edge meets its mirror image.
        let odd = abs(cell - 2.0 * floor(cell * 0.5)) > vec2<f32>(0.5);
        f = select(f, 1.0 - f, odd);
    }
    let x = i32(f.x * w);
    let line = i32(f.y * lines);
    o.out = fetch_sig(x, line * ctx.stride + i32(u.info.parity));
    return o;
}
