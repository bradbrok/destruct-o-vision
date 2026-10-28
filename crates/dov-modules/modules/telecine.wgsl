/* dov-module
format = 1
id = "dov.telecine"
name = "Telecine"
category = "analog"
description = "Film run through a telecine onto video. The picture is shot as film frames, Film of them a second, and each is held for fields in the cadence a telecine uses: two fields then three for 24 frames on 60-field video (3:2 pulldown), which gives film on TV its judder. Where a frame's two fields come from different film frames, motion combs. Weave jitters the film in the gate, Grain changes with every frame, Dust and Scratches mark the print, and Flicker pumps the lamp."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[output]]
id = "shot"
name = "Shot"
kind = "rgb"
hidden = true

[[param]]
id = "film"
name = "Film"
choices = ["24 fps", "18 fps", "16 fps", "12 fps"]

[[param]]
id = "weave"
name = "Weave"
range = [0.0, 1.0]
default = 0.3

[[param]]
id = "grain"
name = "Grain"
range = [0.0, 1.0]
default = 0.3
cv = "video"

[[param]]
id = "dust"
name = "Dust"
range = [0.0, 1.0]
default = 0.2

[[param]]
id = "scratches"
name = "Scratches"
range = [0.0, 1.0]
default = 0.2

[[param]]
id = "flicker"
name = "Flicker"
range = [0.0, 1.0]
default = 0.2

[history]
output = "shot"
fields = 8
*/

fn film_rate(choice: u32) -> f32 {
    switch choice {
        case 1u: {
            return 18.0;
        }
        case 2u: {
            return 16.0;
        }
        case 3u: {
            return 12.0;
        }
        default: {
            return 24.0;
        }
    }
}

// Uniform in [0, 1) for film frame `frame`, question `k` and place `a`, `b`.
fn film_rnd(frame: u32, k: u32, a: u32, b: u32) -> f32 {
    return hash3(vec3<u32>(frame * 16u + k, a, b));
}

// Line `line` of the field the film frame was shot on, `age` fields ago.
fn shot_line(x: i32, line: i32, age: i32, parity: i32) -> vec3<f32> {
    let lines = i32(u.info.lines);
    let row = clamp(line, 0, lines - 1) * i32(u.info.row_stride) + parity;
    if (age <= 0) {
        return fetch_sig(x, row);
    }
    return ring_at(x, row, age);
}

// The film frame at (x, y), in samples across and rows down: read from the
// lines of the field it was shot on, blended, so it's whole on every row
// (each field of the frame shows it, and it can slip in the gate).
fn film_at(px: f32, py: f32, age: i32, parity: i32) -> vec3<f32> {
    let stride = f32(u.info.row_stride);
    let fy = (py - f32(parity)) / stride;
    let x0 = floor(px);
    let l0 = floor(fy);
    let tx = px - x0;
    let ty = fy - l0;
    let xi = i32(x0);
    let li = i32(l0);
    let top = mix(shot_line(xi, li, age, parity), shot_line(xi + 1, li, age, parity), tx);
    let bottom = mix(shot_line(xi, li + 1, age, parity), shot_line(xi + 1, li + 1, age, parity), tx);
    return mix(top, bottom, ty);
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    // Which film frame this field shows, and how many fields ago it was
    // shot. Video at 59.94 runs film at 23.976, so the cadence is exact.
    // Counted within 60000 fields, which every cadence divides.
    var fps = film_rate(p.film);
    let rate = u.info.field_rate;
    if (abs(rate - 59.94) < 0.01 || abs(rate - 29.97) < 0.01) {
        fps = fps * 1000.0 / 1001.0;
    }
    let ratio = fps / rate;
    let f = f32(ctx.field % 60000u);
    let frame_f = floor(f * ratio + 1e-3);
    let start = ceil(frame_f / ratio - 1e-3);
    let age = i32(f - start);
    let frame = u32(frame_f) + (ctx.field / 60000u) * 60000u;
    let parity = i32((ctx.field - u32(age)) % u32(ctx.stride));

    // The film slips about in the gate, a little differently every frame.
    let w = p.weave;
    let h = f32(u.info.height);
    let offset = vec2<f32>(
        (film_rnd(frame, 0u, 0u, 0u) - 0.5) * 3.0 * w * h / 480.0,
        (film_rnd(frame, 1u, 0u, 0u) - 0.5) * 2.0 * w * h / 480.0,
    );
    var rgb = film_at(f32(ctx.x) + offset.x, f32(ctx.row) + offset.y, age, parity);

    // The lamp flickers from frame to frame.
    rgb = rgb * (1.0 + p.flicker * 0.2 * (film_rnd(frame, 2u, 0u, 0u) - 0.5));

    // Grain: new with every frame, strongest in the midtones, a touch of
    // color in it. About a sample across at 480 lines, bigger on bigger
    // pictures.
    let size = max(h / 480.0, 1.0);
    let gx = u32(f32(ctx.x) / size);
    let gy = u32(f32(ctx.row) / size);
    let mono = film_rnd(frame, 3u, gx, gy) - 0.5;
    let tint = vec3<f32>(film_rnd(frame, 4u, gx, gy), film_rnd(frame, 5u, gx, gy), film_rnd(frame, 6u, gx, gy)) - 0.5;
    let l = clamp(luma(rgb), 0.0, 1.0);
    rgb = rgb + p.grain * 0.22 * (0.25 + 3.0 * l * (1.0 - l)) * (vec3<f32>(mono) * 0.8 + tint * 0.3);

    // Dust: specks that come and go with the frames, dark from dirt on
    // the print, light from dirt on the negative.
    let cell = 24.0 * size;
    let cx = u32(f32(ctx.x) / cell);
    let cy = u32(f32(ctx.row) / cell);
    if (film_rnd(frame, 7u, cx, cy) < p.dust * 0.03) {
        let center = (vec2<f32>(f32(cx), f32(cy)) + vec2<f32>(film_rnd(frame, 8u, cx, cy), film_rnd(frame, 9u, cx, cy))) * cell;
        let radius = (0.8 + 2.5 * film_rnd(frame, 10u, cx, cy)) * size;
        let d = length(vec2<f32>(f32(ctx.x), f32(ctx.row)) - center);
        let speck = 1.0 - smoothstep(radius * 0.5, radius, d);
        let shade = select(0.05, 1.0, film_rnd(frame, 11u, cx, cy) < 0.3);
        rgb = mix(rgb, vec3<f32>(shade), speck);
    }

    // Scratches: thin lines down the print that stay for a run of frames,
    // wandering as the film runs.
    let width = f32(u.info.width);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let run = frame / 40u;
        if (film_rnd(run, 12u + i, i, 0u) < p.scratches) {
            let home = film_rnd(run, 13u + i, i, 1u) * width;
            let wander = sin(f32(frame % 1000u) * 0.37 + f32(i) * 2.1) * 4.0 * size;
            let sx = home + wander + (film_rnd(frame, 14u, i, 0u) - 0.5) * size;
            let line = 1.0 - smoothstep(0.3 * size, 1.0 * size, abs(f32(ctx.x) - sx));
            rgb = mix(rgb, vec3<f32>(0.85), line * 0.7 * film_rnd(frame, 15u, i, u32(ctx.row) / 8u));
        }
    }

    var o: Outputs;
    o.out = rgb;
    o.shot = inp.sig;
    return o;
}
