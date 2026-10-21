//! What the glue needs to know about a module: its ports, params, kernel
//! kind, and which engine resources it reads. Both module files and the
//! engine's own Rust-defined modules are described this way.

/// What a port carries. The composite waveform is engine-only in format 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    /// Video-rate, one channel: stored as gray, read as luma.
    Mono,
    /// Video-rate R′G′B′.
    Rgb,
    /// One value per field.
    Control,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub id: String,
    pub name: String,
    pub kind: PortKind,
    /// Inputs: the value read when nothing is patched.
    pub default: f32,
    /// Outputs: not drawn on the panel.
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    /// Equal knob travel per octave; needs `min > 0`.
    Exp,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Knob {
        min: f32,
        max: f32,
        default: f32,
        curve: Curve,
        unit: String,
    },
    Choice {
        options: Vec<String>,
        default: u32,
    },
}

/// The rate of a param's CV jack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cv {
    /// Per sample.
    Video,
    /// Per field.
    Control,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub id: String,
    pub name: String,
    pub kind: ParamKind,
    pub cv: Option<Cv>,
}

impl Param {
    /// Full-scale CV (1.0) at attenuverter 1 sweeps this much.
    pub fn span(&self) -> f32 {
        match &self.kind {
            ParamKind::Knob { min, max, .. } => max - min,
            ParamKind::Choice { options, .. } => options.len() as f32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelKind {
    /// `fn process(ctx, inp, p) -> Outputs`, once per sample.
    Pointwise,
    /// `fn process(ctx, inp, p, state) -> Outputs`, left to right along each line.
    LineScan,
}

/// Past fields kept for the kernel to read with `ring_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct History {
    /// Index of the output that is stored.
    pub output: usize,
    /// Index of a param that stops storing while it is ≥ 0.5.
    pub freeze: Option<usize>,
    pub fields: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub params: Vec<Param>,
    pub kernel: KernelKind,
    pub history: Option<History>,
    /// The module has a text field and reads it with `text_sdf`.
    pub text: bool,
}

/// An input as the engine binds it: declared inputs, then one generated
/// input per param with CV (`<param>_cv`).
#[derive(Clone, Debug, PartialEq)]
pub struct InputSlot {
    pub id: String,
    pub kind: PortKind,
    /// For generated CV inputs, the param they modulate.
    pub cv_for: Option<usize>,
}

impl Interface {
    /// Params that have a CV jack, in order.
    pub fn cv_params(&self) -> impl Iterator<Item = usize> + '_ {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.cv.is_some())
            .map(|(i, _)| i)
    }

    pub fn inputs_full(&self) -> Vec<InputSlot> {
        let declared = self.inputs.iter().map(|p| InputSlot {
            id: p.id.clone(),
            kind: p.kind,
            cv_for: None,
        });
        let cv = self.cv_params().map(|i| {
            let p = &self.params[i];
            InputSlot {
                id: format!("{}_cv", p.id),
                kind: match p.cv {
                    Some(Cv::Control) => PortKind::Control,
                    _ => PortKind::Mono,
                },
                cv_for: Some(i),
            }
        });
        declared.chain(cv).collect()
    }

    /// Params followed by one attenuverter per CV jack.
    pub fn param_slots(&self) -> usize {
        self.params.len() + self.cv_params().count()
    }

    /// For a param with CV: (input index, attenuverter slot).
    pub fn cv_slot(&self, param: usize) -> Option<(usize, usize)> {
        let k = self.cv_params().position(|i| i == param)?;
        Some((self.inputs.len() + k, self.params.len() + k))
    }
}
