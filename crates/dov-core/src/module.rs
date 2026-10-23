//! Module descriptors: the ports, parameters and kernel of a module type.
//!
//! A descriptor is static data shared by the UI (to draw the panel) and the
//! engine (to generate the shader and wire the graph). Every parameter can
//! carry a CV jack; those jacks and their attenuverters are derived from the
//! descriptor rather than declared twice.

use std::borrow::Cow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::AtomicF32;

/// What travels on a cable plugged into this port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    /// Video-rate, one channel. Stored as gray; read as luma.
    Mono,
    /// Video-rate, three channels (R′G′B′).
    Rgb,
    /// One value per field, computed on the CPU (LFOs, sequencers, MIDI).
    Control,
    /// A composite video waveform: sync, blanking, burst and modulated
    /// chroma, sampled along the whole scan including blanking.
    Composite,
}

impl PortKind {
    pub fn is_video(self) -> bool {
        matches!(self, Self::Mono | Self::Rgb)
    }

    /// Can an output of kind `self` drive an input of kind `to`?
    /// Control broadcasts into video inputs. Video into a control input
    /// carries the picture's average brightness, measured each field.
    /// Composite only connects to composite.
    pub fn can_drive(self, to: PortKind) -> bool {
        matches!(
            (self, to),
            (
                PortKind::Mono | PortKind::Rgb | PortKind::Control,
                PortKind::Mono | PortKind::Rgb | PortKind::Control
            ) | (PortKind::Composite, PortKind::Composite)
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PortDesc {
    /// Stable id, used in patch files and as the WGSL field name.
    pub id: &'static str,
    pub name: &'static str,
    pub kind: PortKind,
    /// Value read from an unpatched input.
    pub default: f32,
    /// Internal ports aren't drawn on the panel (e.g. the Output module's feed).
    pub hidden: bool,
}

impl PortDesc {
    pub const fn new(id: &'static str, name: &'static str, kind: PortKind) -> Self {
        Self {
            id,
            name,
            kind,
            default: 0.0,
            hidden: false,
        }
    }

    pub const fn default_value(mut self, v: f32) -> Self {
        self.default = v;
        self
    }

    pub const fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    /// Exponential: equal knob travel per octave. Requires `min > 0`.
    Exp,
}

#[derive(Clone, Copy, Debug)]
pub enum ParamKind {
    Knob {
        min: f32,
        max: f32,
        default: f32,
        curve: Curve,
        unit: &'static str,
    },
    Choice {
        options: &'static [&'static str],
        default: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDesc {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: ParamKind,
    /// Kind of the CV jack that modulates this param, if any.
    pub cv: Option<PortKind>,
    /// For a choice: more options, found at run time, after the fixed ones.
    pub live: Option<Live>,
    /// Set in the module's editor window instead of with a knob on the
    /// panel (a movie's In and Out). Its CV jack stays on the panel.
    pub in_editor: bool,
}

/// Options that depend on what's connected, so the app lists them at run
/// time instead of a module declaring them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live {
    /// Audio input devices. On a text field: the text is the chosen
    /// device's name, or empty for the system's default input.
    AudioInputs,
    /// The input channels of the audio device named in the module's text.
    /// On a choice: one option per channel, numbered from 1.
    AudioChannels,
    /// A movie file. On a text field: the file's path, or empty for none.
    MovieFile,
}

impl ParamDesc {
    pub const fn knob(id: &'static str, name: &'static str, min: f32, max: f32, default: f32) -> Self {
        Self {
            id,
            name,
            kind: ParamKind::Knob {
                min,
                max,
                default,
                curve: Curve::Linear,
                unit: "",
            },
            cv: None,
            live: None,
            in_editor: false,
        }
    }

    pub const fn choice(id: &'static str, name: &'static str, options: &'static [&'static str]) -> Self {
        Self {
            id,
            name,
            kind: ParamKind::Choice { options, default: 0 },
            cv: None,
            live: None,
            in_editor: false,
        }
    }

    pub const fn exp(mut self) -> Self {
        if let ParamKind::Knob { curve, .. } = &mut self.kind {
            *curve = Curve::Exp;
        }
        self
    }

    pub const fn unit(mut self, u: &'static str) -> Self {
        if let ParamKind::Knob { unit, .. } = &mut self.kind {
            *unit = u;
        }
        self
    }

    pub const fn default_choice(mut self, d: u32) -> Self {
        if let ParamKind::Choice { default, .. } = &mut self.kind {
            *default = d;
        }
        self
    }

    /// Adds a video-rate CV jack (per-sample modulation).
    pub const fn video_cv(mut self) -> Self {
        self.cv = Some(PortKind::Mono);
        self
    }

    /// Adds a control-rate CV jack (per-field modulation).
    pub const fn control_cv(mut self) -> Self {
        self.cv = Some(PortKind::Control);
        self
    }

    /// Makes a choice's options run on past the fixed ones, into a list
    /// the app finds at run time. Its value can then be any index.
    pub const fn live(mut self, list: Live) -> Self {
        self.live = Some(list);
        self
    }

    /// Moves the param off the panel into the module's editor window.
    pub const fn in_editor(mut self) -> Self {
        self.in_editor = true;
        self
    }

    /// The name of option `index` of a choice: a fixed option, or past
    /// those, an entry of its live list.
    pub fn option_name(&self, index: usize) -> Option<Cow<'static, str>> {
        let ParamKind::Choice { options, .. } = self.kind else {
            return None;
        };
        match (options.get(index), self.live) {
            (Some(o), _) => Some(Cow::Borrowed(*o)),
            (None, Some(Live::AudioChannels)) => Some(Cow::Owned((index - options.len() + 1).to_string())),
            (None, Some(Live::AudioInputs | Live::MovieFile) | None) => None,
        }
    }

    pub fn default_value(&self) -> f32 {
        match self.kind {
            ParamKind::Knob { default, .. } => default,
            ParamKind::Choice { default, .. } => default as f32,
        }
    }

    /// Full-scale CV (1.0) sweeps the whole range at attenuverter 1.
    pub fn span(&self) -> f32 {
        match self.kind {
            ParamKind::Knob { min, max, .. } => max - min,
            ParamKind::Choice { options, .. } => options.len() as f32,
        }
    }
}

/// How the engine runs a module.
#[derive(Clone, Copy, Debug)]
pub enum Kernel {
    /// Per-sample WGSL: `fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs`.
    Pointwise(&'static str),
    /// Per-line WGSL with state carried along the line:
    /// `fn process(ctx, inp, p, state: ptr<function, State>) -> Outputs` plus
    /// `fn init_state() -> State`. Horizontal filters, slew, integrators.
    LineScan(&'static str),
    /// Frame store with a geometric transform; its output is last field's
    /// input, so it closes feedback loops without an inserted delay.
    Feedback,
    /// A live camera scaled into the raster. The engine owns the device.
    Camera,
    /// A picture the module's behavior supplies (a movie frame), scaled
    /// into the raster by its `fit` choice: fill, fit or stretch.
    Picture,
    /// A composite-domain stage implemented by the engine.
    Composite(CompositeOp),
    /// CPU only, once per field. Outputs are control-rate.
    Control,
}

/// The analog signal-path stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompositeOp {
    /// R′G′B′ to composite: sync, blanking, burst, modulated chroma.
    Encode,
    /// Mixes a video signal into the waveform, sync intervals included.
    Inject,
    /// Transmission damage: snow, bandwidth, ghosts, hum, interference.
    Cable,
    /// A videotape recorder's record and playback path.
    Vcr,
    /// Time base corrector: re-times lines to their sync.
    Tbc,
    /// A receiver: sync separator, deflection, color decoder, picture tube.
    Tv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Source,
    Process,
    Color,
    Space,
    Analog,
    Control,
    Io,
}

impl Category {
    pub const ALL: [Self; 7] = [
        Self::Source,
        Self::Process,
        Self::Color,
        Self::Space,
        Self::Analog,
        Self::Control,
        Self::Io,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Source => "Sources",
            Self::Process => "Processors",
            Self::Color => "Color",
            Self::Space => "Space & time",
            Self::Analog => "Analog signal path",
            Self::Control => "Control",
            Self::Io => "I/O",
        }
    }
}

#[derive(Debug)]
pub struct ModuleDesc {
    /// Stable type id used in patch files, e.g. `dov.vco`.
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
    pub inputs: &'static [PortDesc],
    pub outputs: &'static [PortDesc],
    pub params: &'static [ParamDesc],
    /// The panel shows a live picture of the module's first output.
    pub screen: bool,
    /// Keep a history of past fields for the kernel to read (delays).
    pub ring: Option<RingSpec>,
    /// A line of text the user types on the panel. The engine renders it
    /// and the kernel reads it with `text_sdf`.
    pub text: Option<TextSpec>,
    pub kernel: Kernel,
}

/// A module's text field (see `ModuleDesc::text`). The font is the choice
/// param with id `font`, if the module has one.
#[derive(Clone, Copy, Debug)]
pub struct TextSpec {
    pub default: &'static str,
    /// The text is picked from this list, not typed.
    pub live: Option<Live>,
}

/// Longest text a module keeps. Patch files can hold anything, so text is
/// cut to this on the way in.
pub const TEXT_MAX_CHARS: usize = 256;

/// Fonts a text module can choose, by index of its `font` param.
pub const FONTS: [&str; 7] = ["Sans", "Black", "Impact", "Serif", "Mono", "Script", "Chalk"];

/// A history of past fields kept on the GPU. Each field, the engine stores
/// `output` (usually hidden) and the kernel reads it back with `ring_at`.
#[derive(Clone, Copy, Debug)]
pub struct RingSpec {
    pub output: usize,
    /// While this param is on (≥ 0.5, CV included) nothing is stored: freeze.
    pub freeze_param: Option<usize>,
    /// Fields of history wanted; the engine may keep fewer.
    pub fields: u32,
}

/// An input port, either declared or generated for a parameter's CV.
#[derive(Clone, Debug)]
pub struct InputInfo {
    pub id: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub kind: PortKind,
    pub default: f32,
    pub hidden: bool,
    /// For generated CV inputs, the param they modulate.
    pub cv_for: Option<usize>,
}

impl ModuleDesc {
    /// Params that have a CV jack, in order.
    pub fn cv_params(&self) -> impl Iterator<Item = usize> + '_ {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.cv.is_some())
            .map(|(i, _)| i)
    }

    /// Declared inputs followed by one CV input per modulatable param.
    pub fn input_count(&self) -> usize {
        self.inputs.len() + self.cv_params().count()
    }

    pub fn inputs_full(&self) -> Vec<InputInfo> {
        let declared = self.inputs.iter().map(|p| InputInfo {
            id: Cow::Borrowed(p.id),
            name: Cow::Borrowed(p.name),
            kind: p.kind,
            default: p.default,
            hidden: p.hidden,
            cv_for: None,
        });
        let cv = self.cv_params().map(|i| {
            let p = &self.params[i];
            InputInfo {
                id: Cow::Owned(format!("{}_cv", p.id)),
                name: Cow::Owned(format!("{} CV", p.name)),
                kind: p.cv.expect("cv param"),
                default: 0.0,
                hidden: false,
                cv_for: Some(i),
            }
        });
        declared.chain(cv).collect()
    }

    /// Total param slots: params followed by one attenuverter per CV jack.
    pub fn param_slots(&self) -> usize {
        self.params.len() + self.cv_params().count()
    }

    /// For a param with CV: (input index, attenuverter slot).
    pub fn cv_slot(&self, param: usize) -> Option<(usize, usize)> {
        let k = self.cv_params().position(|i| i == param)?;
        Some((self.inputs.len() + k, self.params.len() + k))
    }

    pub fn input_index(&self, id: &str) -> Option<usize> {
        self.inputs_full().iter().position(|p| p.id == id)
    }

    pub fn output_index(&self, id: &str) -> Option<usize> {
        self.outputs.iter().position(|p| p.id == id)
    }

    pub fn param_index(&self, id: &str) -> Option<usize> {
        self.params.iter().position(|p| p.id == id)
    }

    /// Default values for every param slot (attenuverters default to 0.5).
    pub fn default_slots(&self) -> Vec<f32> {
        let atten = self.cv_params().map(|_| DEFAULT_ATTENUVERTER);
        self.params
            .iter()
            .map(ParamDesc::default_value)
            .chain(atten)
            .collect()
    }
}

pub const DEFAULT_ATTENUVERTER: f32 = 0.5;

/// Live values of a module's param slots, shared by the UI and the engine,
/// plus its text for modules that have a text field.
#[derive(Debug)]
pub struct ParamBank {
    values: Box<[AtomicF32]>,
    text: Mutex<String>,
    /// Bumped on every text change, so the engine knows to re-render.
    text_version: AtomicU64,
    /// Points on the module's timeline in seconds, in order: a movie's
    /// splices.
    marks: Mutex<Vec<f64>>,
    /// The frame the user is holding the module's playhead on, or NaN.
    /// Never saved.
    cue: AtomicF32,
}

impl ParamBank {
    pub fn new(values: &[f32]) -> Self {
        Self {
            values: values.iter().map(|&v| AtomicF32::new(v)).collect(),
            text: Mutex::default(),
            text_version: AtomicU64::new(0),
            marks: Mutex::default(),
            cue: AtomicF32::new(f32::NAN),
        }
    }

    pub fn get(&self, slot: usize) -> f32 {
        self.values.get(slot).map_or(0.0, AtomicF32::load)
    }

    pub fn set(&self, slot: usize, v: f32) {
        if let Some(p) = self.values.get(slot) {
            p.store(v);
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn snapshot(&self) -> Vec<f32> {
        self.values.iter().map(AtomicF32::load).collect()
    }

    pub fn text(&self) -> String {
        self.text.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Sets the text, cut to `TEXT_MAX_CHARS`.
    pub fn set_text(&self, text: &str) {
        let text = match text.char_indices().nth(TEXT_MAX_CHARS) {
            Some((end, _)) => &text[..end],
            None => text,
        };
        let mut t = self.text.lock().unwrap_or_else(|e| e.into_inner());
        if *t != text {
            text.clone_into(&mut t);
            self.text_version.fetch_add(1, Ordering::Release);
        }
    }

    pub fn text_version(&self) -> u64 {
        self.text_version.load(Ordering::Acquire)
    }

    pub fn marks(&self) -> Vec<f64> {
        self.marks.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Sets the marks, putting them in order.
    pub fn set_marks(&self, mut marks: Vec<f64>) {
        marks.retain(|m| m.is_finite());
        marks.sort_by(f64::total_cmp);
        marks.dedup();
        *self.marks.lock().unwrap_or_else(|e| e.into_inner()) = marks;
    }

    /// The frame the user is holding the playhead on, while they hold it.
    pub fn cue(&self) -> Option<f32> {
        Some(self.cue.load()).filter(|f| !f.is_nan())
    }

    pub fn set_cue(&self, frame: Option<f32>) {
        self.cue.store(frame.unwrap_or(f32::NAN));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static PARAMS: [ParamDesc; 3] = [
        ParamDesc::knob("gain", "Gain", 0.0, 8.0, 1.0).video_cv(),
        ParamDesc::choice("mode", "Mode", &["A", "B"]),
        ParamDesc::knob("bias", "Bias", -1.0, 1.0, 0.0).video_cv(),
    ];
    static INPUTS: [PortDesc; 1] = [PortDesc::new("sig", "Signal", PortKind::Mono)];
    static DESC: ModuleDesc = ModuleDesc {
        id: "test",
        name: "Test",
        category: Category::Process,
        description: "",
        inputs: &INPUTS,
        outputs: &[],
        params: &PARAMS,
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Control,
    };

    #[test]
    fn cv_jacks_follow_declared_inputs() {
        let inputs = DESC.inputs_full();
        let ids: Vec<_> = inputs.iter().map(|p| p.id.as_ref()).collect();
        assert_eq!(ids, ["sig", "gain_cv", "bias_cv"]);
        assert_eq!(DESC.cv_slot(0), Some((1, 3)));
        assert_eq!(DESC.cv_slot(1), None);
        assert_eq!(DESC.cv_slot(2), Some((2, 4)));
        assert_eq!(DESC.default_slots(), [1.0, 0.0, 0.0, 0.5, 0.5]);
    }

    #[test]
    fn text_is_cut_to_the_limit() {
        let bank = ParamBank::new(&[]);
        bank.set_text(&"é".repeat(TEXT_MAX_CHARS + 10));
        assert_eq!(bank.text().chars().count(), TEXT_MAX_CHARS);
        assert_eq!(bank.text_version(), 1);
    }

    #[test]
    fn video_and_control_mix_but_composite_stays_apart() {
        assert!(PortKind::Control.can_drive(PortKind::Rgb));
        assert!(PortKind::Mono.can_drive(PortKind::Rgb));
        assert!(PortKind::Rgb.can_drive(PortKind::Control));
        assert!(PortKind::Composite.can_drive(PortKind::Composite));
        assert!(!PortKind::Composite.can_drive(PortKind::Rgb));
        assert!(!PortKind::Rgb.can_drive(PortKind::Composite));
    }
}
