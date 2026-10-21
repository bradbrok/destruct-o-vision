/* dov-module
format = 1
id = "dov.text"
name = "Text"
category = "source"
description = "Type on the panel; it comes out as a matte: filled, outlined, glowing, or as contour bands around the letters. Size, position, rotation and weight take video-rate CV, so a picture patched in warps the lettering. Dist is the distance field itself, for soft keys and displacement."
kernel = "pointwise"

[[output]]
id = "out"
name = "Out"
kind = "mono"

[[output]]
id = "dist"
name = "Dist"
kind = "mono"

[[param]]
id = "font"
name = "Font"
choices = ["Sans", "Black", "Impact", "Serif", "Mono", "Script", "Chalk"]
default = "Black"

[[param]]
id = "look"
name = "Look"
choices = ["Fill", "Outline", "Glow", "Bands"]

[[param]]
id = "size"
name = "Size"
range = [0.02, 1.0]
default = 0.2
curve = "exp"
cv = "video"

[[param]]
id = "x"
name = "X"
range = [0.0, 1.0]
default = 0.5
cv = "video"

[[param]]
id = "y"
name = "Y"
range = [0.0, 1.0]
default = 0.5
cv = "video"

[[param]]
id = "rotate"
name = "Rotate"
range = [-180.0, 180.0]
default = 0.0
unit = "°"
cv = "video"

[[param]]
id = "weight"
name = "Weight"
range = [-0.1, 0.1]
default = 0.0
cv = "video"

[[param]]
id = "line"
name = "Line"
range = [0.005, 0.2]
default = 0.03
curve = "exp"
cv = "video"

[[param]]
id = "soft"
name = "Soft"
range = [0.0, 0.2]
default = 0.0

[text]
default = "DESTRUCT-O-VISION"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    // Into text coordinates: line heights from the text's center, y up.
    let size = max(p.size, 0.005);
    var q = (ctx.uv - vec2<f32>(p.x, p.y)) * vec2<f32>(ctx.aspect, 1.0) / size;
    q.y = -q.y;
    let a = radians(p.rotate);
    let c = cos(a);
    let s = sin(a);
    q = vec2<f32>(c * q.x + s * q.y, -s * q.x + c * q.y);
    let d = text_sdf(q) - p.weight;

    // One pixel, in line heights, for antialiasing.
    let edge = 1.0 / (f32(u.info.height) * size) + max(p.soft, 0.0);
    let line = max(p.line, 1e-3);
    // The field stops at TEXT_SPREAD; fade glows and bands out before it.
    let reach = 1.0 - smoothstep(TEXT_SPREAD * 0.6, TEXT_SPREAD, d);
    var v: f32;
    switch p.look {
        case 0u: {
            v = 1.0 - smoothstep(-edge, edge, d);
        }
        case 1u: {
            v = 1.0 - smoothstep(line - edge, line + edge, abs(d));
        }
        case 2u: {
            v = exp(-max(d, 0.0) / line) * reach;
        }
        default: {
            v = (0.5 + 0.5 * cos(TAU * d / line)) * reach;
        }
    }
    var o: Outputs;
    o.out = v;
    o.dist = clamp(0.5 - d / (2.0 * TEXT_SPREAD), 0.0, 1.0);
    return o;
}
