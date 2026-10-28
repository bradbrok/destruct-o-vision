//! Shaders for modules. Kernels written in WGSL get their glue from the
//! module format (`dov_format::glue`); this converts the engine's module
//! descriptors into the format's interface, and assembles the engine's own
//! passes (frame store, camera, composite chain).

use bytemuck::{Pod, Zeroable};
use dov_core::{Curve, Kernel, ModuleDesc, ParamKind, PortKind};
use dov_format::glue::{EXTRA_SLOTS, MAX_INPUTS, MAX_OUTPUTS, MAX_STAGES, PRELUDE, Pass, STD_LIB};
pub use dov_format::glue::{
    INPUT_BINDING_BASE, MAX_PARAM_SLOTS, OUTPUT_BINDING_BASE, RING_BINDING, STAGE_BINDING_BASE,
    STAGE_WRITE_BINDING, TEXT_BINDING, TEXT_SAMPLER_BINDING,
};

/// Mirrors `FieldInfo` in `dov_format::glue::PRELUDE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct FieldInfoGpu {
    pub width: u32,
    pub height: u32,
    pub lines: u32,
    pub row_stride: u32,
    pub parity: u32,
    pub field: u32,
    /// Fields kept in this module's history ring, and the newest slot.
    pub ring_depth: u32,
    pub ring_head: u32,
    pub aspect: f32,
    pub sample_rate: f32,
    pub line_rate: f32,
    pub field_rate: f32,
}

/// Mirrors `ModuleUniforms` in `dov_format::glue::PRELUDE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ModuleUniformsGpu {
    pub info: FieldInfoGpu,
    /// Param slots, four per vec4.
    pub knobs: [[f32; 4]; MAX_PARAM_SLOTS / 4],
    /// Per input: constant value in xyz; w = 1 when a video cable is patched.
    pub inputs: [[f32; 4]; 16],
    pub extra: [[f32; 4]; EXTRA_SLOTS],
}

impl Default for ModuleUniformsGpu {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// A module's interface in the module format's terms. Only pointwise and
/// line-scan kernels have one; composite ports are engine-only.
pub fn interface(desc: &ModuleDesc) -> Result<dov_format::Interface, String> {
    use dov_format as f;
    let kernel = match desc.kernel {
        Kernel::Pointwise(_) | Kernel::Staged { .. } => f::KernelKind::Pointwise,
        Kernel::LineScan(_) => f::KernelKind::LineScan,
        other => return Err(format!("{}: {other:?} has no generated shader", desc.id)),
    };
    let port = |p: &dov_core::PortDesc| -> Result<f::Port, String> {
        Ok(f::Port {
            id: p.id.into(),
            name: p.name.into(),
            kind: match p.kind {
                PortKind::Mono => f::PortKind::Mono,
                PortKind::Rgb => f::PortKind::Rgb,
                PortKind::Control => f::PortKind::Control,
                PortKind::Composite => {
                    return Err(format!(
                        "{}: generated kernels can't have composite ports",
                        desc.id
                    ));
                }
            },
            default: p.default,
            hidden: p.hidden,
        })
    };
    let params = desc
        .params
        .iter()
        .map(|p| f::Param {
            id: p.id.into(),
            name: p.name.into(),
            kind: match p.kind {
                dov_core::ParamKind::Knob {
                    min,
                    max,
                    default,
                    curve,
                    unit,
                } => f::ParamKind::Knob {
                    min,
                    max,
                    default,
                    curve: match curve {
                        Curve::Linear => f::Curve::Linear,
                        Curve::Exp => f::Curve::Exp,
                    },
                    unit: unit.into(),
                },
                ParamKind::Choice { options, default } => f::ParamKind::Choice {
                    options: options.iter().map(|o| o.to_string()).collect(),
                    default,
                },
            },
            cv: p.cv.map(|k| match k {
                PortKind::Control => f::Cv::Control,
                _ => f::Cv::Video,
            }),
        })
        .collect();
    Ok(f::Interface {
        inputs: desc.inputs.iter().map(port).collect::<Result<_, _>>()?,
        outputs: desc.outputs.iter().map(port).collect::<Result<_, _>>()?,
        params,
        kernel,
        history: desc.ring.map(|r| f::History {
            output: r.output,
            freeze: r.freeze_param,
            fields: r.fields,
        }),
        text: desc.text.is_some(),
    })
}

/// Builds the shaders for a pointwise, line-scan or staged module: one per
/// pass, the stages first.
pub fn module_shaders(desc: &ModuleDesc) -> Result<Vec<String>, String> {
    let (body, stages) = match desc.kernel {
        Kernel::Pointwise(src) | Kernel::LineScan(src) => (src, &[][..]),
        Kernel::Staged { source, stages } => (source, stages),
        other => return Err(format!("{}: {other:?} has no generated shader", desc.id)),
    };
    let iface = interface(desc)?;
    if iface.inputs_full().len() > MAX_INPUTS
        || iface.outputs.len() > MAX_OUTPUTS
        || iface.param_slots() > MAX_PARAM_SLOTS
        || stages.len() > MAX_STAGES
    {
        return Err(format!("{}: too many ports, params or stages", desc.id));
    }
    let mut shaders: Vec<String> = stages
        .iter()
        .enumerate()
        .map(|(index, &block)| {
            dov_format::glue::pass_shader(
                &iface,
                body,
                Pass::Stage {
                    index,
                    block: block.max(1),
                },
            )
        })
        .collect();
    shaders.push(dov_format::glue::pass_shader(
        &iface,
        body,
        Pass::Process { stages: stages.len() },
    ));
    Ok(shaders)
}

/// Averages a video signal for cables into control inputs (see `Meter`).
pub const METER_SHADER: &str = include_str!("shaders/meter.wgsl");

/// The frame-store pass behind [`Kernel::Feedback`].
pub fn feedback_shader() -> String {
    format!("{PRELUDE}{STD_LIB}{}", include_str!("shaders/feedback.wgsl"))
}

/// Shader sources for the analog signal path, assembled by [`composite_shader`].
pub mod composite {
    pub const COMMON: &str = include_str!("shaders/composite/common.wgsl");
    /// Reading a composite input, at binding 1.
    pub const READ: &str = concat!(
        include_str!("shaders/composite/read.wgsl"),
        include_str!("shaders/composite/signal.wgsl")
    );
    /// The VCR's reading: its input, or its tape as it played back.
    pub const TAPE: &str = concat!(
        include_str!("shaders/composite/tape.wgsl"),
        include_str!("shaders/composite/signal.wgsl")
    );
    pub const ENCODE: &str = include_str!("shaders/composite/encode.wgsl");
    pub const INJECT: &str = include_str!("shaders/composite/inject.wgsl");
    pub const CABLE: &str = include_str!("shaders/composite/cable.wgsl");
    pub const RF: &str = include_str!("shaders/composite/rf.wgsl");
    pub const VCR: &str = include_str!("shaders/composite/vcr.wgsl");
    pub const TBC: &str = include_str!("shaders/composite/tbc.wgsl");
    pub const TV_COMMON: &str = include_str!("shaders/composite/tv_common.wgsl");
    pub const TV_STATE: &str = include_str!("shaders/composite/tv_state.wgsl");
    pub const TV_SYNC: &str = include_str!("shaders/composite/tv_sync.wgsl");
    pub const TV_SCAN: &str = include_str!("shaders/composite/tv_scan.wgsl");
    pub const TV_DECODE: &str = include_str!("shaders/composite/tv_decode.wgsl");
    pub const TV_DRAW: &str = include_str!("shaders/composite/tv_draw.wgsl");
}

/// A composite-domain pass: the shared preludes followed by `parts`.
pub fn composite_shader(parts: &[&str]) -> String {
    let mut s = format!("{PRELUDE}{STD_LIB}{}", composite::COMMON);
    for part in parts {
        s.push_str(part);
    }
    s
}

/// The passes of an XY display (`Kernel::Vector`).
pub const VECTOR_BEAM: &str = include_str!("shaders/vector_beam.wgsl");
pub const VECTOR_GLOW: &str = include_str!("shaders/vector_glow.wgsl");

/// A pass of an XY display: the preludes followed by `part`.
pub fn vector_shader(part: &str) -> String {
    format!(
        "{PRELUDE}{STD_LIB}{}{part}",
        include_str!("shaders/vector_common.wgsl")
    )
}

/// The pass behind [`Kernel::Camera`] and [`Kernel::Picture`]: a frame
/// scaled into the raster.
pub fn camera_shader() -> String {
    format!("{PRELUDE}{}", include_str!("shaders/camera.wgsl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(name: &str, src: &str) {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{name}: {}\n{src}", e.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
    }

    #[test]
    fn every_module_shader_validates() {
        for m in dov_modules::all() {
            if let Kernel::Pointwise(_) | Kernel::LineScan(_) | Kernel::Staged { .. } = m.desc.kernel {
                for shader in module_shaders(&m.desc).unwrap() {
                    validate(m.desc.id, &shader);
                }
            }
        }
    }

    #[test]
    fn feedback_shader_validates() {
        validate("feedback", &feedback_shader());
        validate("meter", METER_SHADER);
        validate("camera", &camera_shader());
        validate("blit", include_str!("shaders/blit.wgsl"));
    }

    #[test]
    fn uniform_layout_matches_wgsl() {
        assert_eq!(size_of::<FieldInfoGpu>(), 48);
        assert_eq!(size_of::<ModuleUniformsGpu>(), 48 + 128 + 256 + 128);
    }
}
