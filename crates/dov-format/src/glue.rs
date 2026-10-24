//! The engine's side of the contract: the WGSL wrapped around a module's code
//! before it runs. Bindings, the `Ctx`/`Inputs`/`Params`/`Outputs` structs,
//! CV handling, the gather and history helpers, and the compute entry point.

use std::fmt::Write;

use crate::interface::{Interface, KernelKind, ParamKind, PortKind};

pub const INPUT_BINDING_BASE: u32 = 1;
pub const OUTPUT_BINDING_BASE: u32 = 16;
pub const RING_BINDING: u32 = 24;
pub const TEXT_BINDING: u32 = 25;
pub const TEXT_SAMPLER_BINDING: u32 = 26;
/// Earlier stages of a staged kernel, read with `stage_at`: stage `k` is at
/// `STAGE_BINDING_BASE + k`.
pub const STAGE_BINDING_BASE: u32 = 27;
/// Where a stage writes its results.
pub const STAGE_WRITE_BINDING: u32 = 31;
/// Passes a staged kernel can have before its last.
pub const MAX_STAGES: usize = 4;
/// Inputs, counting the jacks generated for CV.
pub const MAX_INPUTS: usize = 15;
pub const MAX_OUTPUTS: usize = 4;
/// Params plus one attenuverter per CV jack.
pub const MAX_PARAM_SLOTS: usize = 32;
/// `vec4` slots the engine (and built-in CPU code) can pass in `u.extra`.
pub const EXTRA_SLOTS: usize = 8;
/// History a module can ask for, in fields.
pub const MAX_HISTORY_FIELDS: u32 = 120;
/// How far the rendered text's distance field reaches, in line heights.
pub const TEXT_SPREAD: f32 = 0.25;

/// Shared declarations every module shader includes. The engine's Rust
/// mirror of `ModuleUniforms` must match it field for field.
pub const PRELUDE: &str = r#"
struct FieldInfo {
    width: u32,
    height: u32,
    lines: u32,
    row_stride: u32,
    parity: u32,
    field: u32,
    ring_depth: u32,
    ring_head: u32,
    aspect: f32,
    sample_rate: f32,
    line_rate: f32,
    field_rate: f32,
}

struct ModuleUniforms {
    info: FieldInfo,
    knobs: array<vec4<f32>, 8>,
    inputs: array<vec4<f32>, 16>,
    extra: array<vec4<f32>, 8>,
}

@group(0) @binding(0) var<uniform> u: ModuleUniforms;

struct Ctx {
    x: u32,
    line: u32,
    row: u32,
    field: u32,
    uv: vec2<f32>,
    aspect: f32,
    // Rows between consecutive lines of one field (2 when interlaced).
    stride: i32,
}

// Line `line` of the current field lands on row `row` of the woven frame.
fn make_ctx(x: u32, line: u32) -> Ctx {
    var c: Ctx;
    c.x = x;
    c.line = line;
    c.row = line * u.info.row_stride + u.info.parity;
    c.field = u.info.field;
    c.uv = vec2<f32>(
        (f32(x) + 0.5) / f32(u.info.width),
        (f32(c.row) + 0.5) / f32(u.info.height),
    );
    c.aspect = u.info.aspect;
    c.stride = i32(u.info.row_stride);
    return c;
}

fn knob(i: u32) -> f32 {
    return u.knobs[i / 4u][i % 4u];
}
"#;

/// The standard library: helpers available to every module.
pub const STD_LIB: &str = r#"
const TAU: f32 = 6.283185307179586;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// Triangle fold: identity on 0..1, reflects everything beyond.
fn fold(x: f32) -> f32 {
    return 1.0 - abs(fract(x * 0.5) * 2.0 - 1.0);
}

// Unipolar 0..1 waveforms: sine, triangle, saw, square.
fn osc_shape(ph: f32, kind: u32) -> f32 {
    switch kind {
        case 0u: { return 0.5 - 0.5 * cos(TAU * ph); }
        case 1u: { return 1.0 - abs(2.0 * ph - 1.0); }
        case 2u: { return ph; }
        default: { return select(0.0, 1.0, ph < 0.5); }
    }
}

// FCC Y'IQ -> R'G'B'. Output may leave 0..1; signals keep their headroom.
fn yiq_to_rgb(y: f32, i: f32, q: f32) -> vec3<f32> {
    return vec3<f32>(
        y + 0.956 * i + 0.621 * q,
        y - 0.272 * i - 0.647 * q,
        y - 1.106 * i + 1.703 * q,
    );
}

fn pcg3d(v_in: vec3<u32>) -> vec3<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

// Uniform random in [0, 1) from three integers.
fn hash3(v: vec3<u32>) -> f32 {
    return f32(pcg3d(v).x >> 8u) / 16777216.0;
}
"#;

/// Names the standard library and the glue declare; modules can't reuse them.
pub const RESERVED_NAMES: &[&str] = &[
    "u",
    "main",
    "knob",
    "make_ctx",
    "ring",
    "ring_at",
    "FieldInfo",
    "ModuleUniforms",
    "Ctx",
    "Inputs",
    "Params",
    "Outputs",
    "TAU",
    "luma",
    "fold",
    "osc_shape",
    "yiq_to_rgb",
    "pcg3d",
    "hash3",
    "text_tex",
    "text_sampler",
    "text_sdf",
    "text_size",
    "TEXT_SPREAD",
];

/// Prefixes the glue uses for generated names, plus `dov_` for the future.
pub const RESERVED_PREFIXES: &[&str] = &["in_", "out_", "fetch_", "patched_", "dov_"];

fn port_type(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Rgb => "vec3<f32>",
        PortKind::Mono | PortKind::Control => "f32",
    }
}

/// Which pass of a kernel a shader runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// `process`: a module's only pass, or the last after `stages` earlier ones.
    Process { stages: usize },
    /// `stage<index>` of a staged kernel, once per `block`×`block` samples.
    Stage { index: usize, block: u32 },
}

/// The complete compute shader for a module: its own code first, so compile
/// errors point at the author's line numbers, then the glue. WGSL resolves
/// module-scope declarations in any order.
pub fn shader(iface: &Interface, body: &str) -> String {
    pass_shader(iface, body, Pass::Process { stages: 0 })
}

/// The shader for one pass of a module's kernel. Every pass of a staged
/// kernel compiles the same code; each runs its own entry function and
/// reads the stages before it.
pub fn pass_shader(iface: &Interface, body: &str, pass: Pass) -> String {
    let inputs = iface.inputs_full();
    let line_scan = iface.kernel == KernelKind::LineScan;

    let mut s = String::with_capacity(body.len() + 8 * 1024);
    s.push_str(body);
    s.push_str("\n// ---- glue generated by Destruct-O-Vision ----\n");
    s.push_str(PRELUDE);
    s.push_str(STD_LIB);
    for i in 0..inputs.len() {
        let _ = writeln!(
            s,
            "@group(0) @binding({}) var in_{i}: texture_2d<f32>;",
            INPUT_BINDING_BASE + i as u32
        );
    }
    for k in 0..iface.outputs.len() {
        let _ = writeln!(
            s,
            "@group(0) @binding({}) var out_{k}: texture_storage_2d<rgba16float, write>;",
            OUTPUT_BINDING_BASE + k as u32
        );
    }

    // Structs handed to `process`. WGSL has no empty structs.
    s.push_str("\nstruct Inputs {\n");
    for p in &iface.inputs {
        let _ = writeln!(s, "    {}: {},", p.id, port_type(p.kind));
    }
    if iface.inputs.is_empty() {
        s.push_str("    _unused: f32,\n");
    }
    s.push_str("}\n\nstruct Params {\n");
    for p in &iface.params {
        let ty = match p.kind {
            ParamKind::Knob { .. } => "f32",
            ParamKind::Choice { .. } => "u32",
        };
        let _ = writeln!(s, "    {}: {ty},", p.id);
    }
    if iface.params.is_empty() {
        s.push_str("    _unused: f32,\n");
    }
    s.push_str("}\n\nstruct Outputs {\n");
    for p in &iface.outputs {
        let _ = writeln!(s, "    {}: {},", p.id, port_type(p.kind));
    }
    s.push_str("}\n");

    // History: `ring_at(x, row, age)` reads the stored output as it was
    // `age` fields ago (1 = the previous field).
    if iface.history.is_some() {
        let _ = write!(
            s,
            r#"
@group(0) @binding({RING_BINDING}) var ring: texture_2d_array<f32>;

fn ring_at(x: i32, row: i32, age: i32) -> vec3<f32> {{
    let depth = i32(max(u.info.ring_depth, 1u));
    let slot = ((i32(u.info.ring_head) - (clamp(age, 1, depth) - 1)) % depth + depth) % depth;
    let c = vec2<i32>(clamp(x, 0, i32(u.info.width) - 1), clamp(row, 0, i32(u.info.height) - 1));
    return textureLoad(ring, c, slot, 0).rgb;
}}
"#
        );
    }

    // Text: `text_sdf(p)` is the distance from `p` to the rendered text, in
    // line heights (negative inside), with `p` in line heights from the
    // text's center, y up. `text_size()` is the laid-out text's size.
    if iface.text {
        let _ = write!(
            s,
            r#"
@group(0) @binding({TEXT_BINDING}) var text_tex: texture_2d<f32>;
@group(0) @binding({TEXT_SAMPLER_BINDING}) var text_sampler: sampler;

const TEXT_SPREAD: f32 = {TEXT_SPREAD:?};

fn text_size() -> vec2<f32> {{
    return u.extra[6].xy;
}}

fn text_sdf(p: vec2<f32>) -> f32 {{
    // The texture's extent: left, bottom, right, top.
    let lo = u.extra[7].xy;
    let hi = u.extra[7].zw;
    let uv = vec2<f32>((p.x - lo.x) / (hi.x - lo.x), (hi.y - p.y) / (hi.y - lo.y));
    let d = textureSampleLevel(text_tex, text_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).r;
    // Beyond the texture, add the distance to its edge.
    return d + length(max(max(lo - p, p - hi), vec2<f32>(0.0)));
}}
"#
        );
    }

    // Staged kernels: `stage_at(k, x, y)` reads what stage `k` stored, for
    // the stages run before this pass, and zero for the rest.
    let (staged, readable) = match pass {
        Pass::Process { stages } => (stages > 0, stages),
        Pass::Stage { index, .. } => (true, index),
    };
    if staged {
        for k in 0..readable {
            let _ = writeln!(
                s,
                "@group(0) @binding({}) var dov_stage_{k}: texture_2d<f32>;",
                STAGE_BINDING_BASE + k as u32
            );
        }
        s.push_str("\nfn stage_at(k: u32, x: i32, y: i32) -> vec4<f32> {\n    switch k {\n");
        for k in 0..readable {
            let _ = writeln!(
                s,
                "        case {k}u: {{\n            let last = vec2<i32>(textureDimensions(dov_stage_{k})) - vec2<i32>(1);\n            return textureLoad(dov_stage_{k}, clamp(vec2<i32>(x, y), vec2<i32>(0), last), 0);\n        }}"
            );
        }
        s.push_str("        default: {\n            return vec4<f32>(0.0);\n        }\n    }\n}\n");
    }

    // Per declared input: `fetch_<input>(x, row)` reads it anywhere in the
    // woven frame, clamped to the edges (lines of the current field are
    // `ctx.stride` rows apart), and `patched_<input>()` says whether a video
    // cable feeds it.
    for (i, p) in iface.inputs.iter().enumerate() {
        let value = match p.kind {
            PortKind::Rgb => "raw",
            PortKind::Mono | PortKind::Control => "luma(raw)",
        };
        let _ = write!(
            s,
            r#"
fn fetch_{id}(x: i32, row: i32) -> {ty} {{
    let c = vec2<i32>(clamp(x, 0, i32(u.info.width) - 1), clamp(row, 0, i32(u.info.height) - 1));
    let raw = select(u.inputs[{i}].xyz, textureLoad(in_{i}, c, 0).rgb, u.inputs[{i}].w > 0.5);
    return {value};
}}

fn patched_{id}() -> bool {{
    return u.inputs[{i}].w > 0.5;
}}
"#,
            id = p.id,
            ty = port_type(p.kind),
        );
    }

    // Per-sample body shared by both drivers.
    let mut sample = String::new();
    let _ = writeln!(sample, "        let coord = vec2<u32>(ctx.x, ctx.row);");
    for i in 0..inputs.len() {
        let _ = writeln!(
            sample,
            "        let raw_{i} = select(u.inputs[{i}].xyz, textureLoad(in_{i}, coord, 0).rgb, u.inputs[{i}].w > 0.5);"
        );
    }
    let _ = writeln!(sample, "        var inp: Inputs;");
    for (i, p) in iface.inputs.iter().enumerate() {
        let value = match p.kind {
            PortKind::Rgb => format!("raw_{i}"),
            PortKind::Mono | PortKind::Control => format!("luma(raw_{i})"),
        };
        let _ = writeln!(sample, "        inp.{} = {value};", p.id);
    }
    let _ = writeln!(sample, "        var p: Params;");
    for (i, p) in iface.params.iter().enumerate() {
        let mut value = format!("knob({i}u)");
        if let Some((input, atten)) = iface.cv_slot(i) {
            let _ = write!(value, " + luma(raw_{input}) * knob({atten}u) * {:?}", p.span());
        }
        let value = match &p.kind {
            ParamKind::Knob { .. } => value,
            ParamKind::Choice { options, .. } => {
                format!(
                    "u32(clamp(round({value}), 0.0, {:?}))",
                    (options.len().max(1) - 1) as f32
                )
            }
        };
        let _ = writeln!(sample, "        p.{} = {value};", p.id);
    }
    if let Pass::Stage { index, block } = pass {
        let at = if block > 1 { "gid.xy" } else { "coord" };
        let _ = write!(
            s,
            r#"
@group(0) @binding({STAGE_WRITE_BINDING}) var dov_stage_out: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    if (gid.x * {block}u >= u.info.width || gid.y * {block}u >= u.info.lines) {{
        return;
    }}
    {{
        let ctx = make_ctx(gid.x * {block}u, gid.y * {block}u);
{sample}        textureStore(dov_stage_out, {at}, stage{index}(ctx, inp, p));
    }}
}}
"#
        );
        return s;
    }
    if line_scan {
        let _ = writeln!(sample, "        let o = process(ctx, inp, p, &state);");
    } else {
        let _ = writeln!(sample, "        let o = process(ctx, inp, p);");
    }
    for (k, p) in iface.outputs.iter().enumerate() {
        let value = match p.kind {
            PortKind::Rgb => format!("o.{}", p.id),
            PortKind::Mono | PortKind::Control => format!("vec3<f32>(o.{})", p.id),
        };
        let _ = writeln!(
            sample,
            "        textureStore(out_{k}, coord, vec4<f32>({value}, 1.0));"
        );
    }

    if line_scan {
        // One invocation per line; state flows left to right like an analog circuit.
        let _ = write!(
            s,
            r#"
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    if (gid.x >= u.info.lines) {{
        return;
    }}
    var state = init_state();
    for (var x = 0u; x < u.info.width; x = x + 1u) {{
        let ctx = make_ctx(x, gid.x);
{sample}    }}
}}
"#
        );
    } else {
        let _ = write!(
            s,
            r#"
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    if (gid.x >= u.info.width || gid.y >= u.info.lines) {{
        return;
    }}
    {{
        let ctx = make_ctx(gid.x, gid.y);
{sample}    }}
}}
"#
        );
    }
    s
}
