//! Module definitions. Each kernel follows the contract in the crate docs.
//!
//! The WGSL kernels in this file are under the MIT license (`LICENSE-MIT`), as
//! the module format is, so module authors can copy them freely. The rest of
//! the file is GPL-3.0-or-later like the rest of the app.

use dov_core::{
    Category, CompositeOp, FONTS, Kernel, Live, ModuleDesc, ParamDesc, PortDesc, PortKind, RingSpec, TextSpec,
};

use crate::audio::AudioIn;
use crate::behaviors::{Cable, Encoder, Lfo, Macros, Spin, Vco};
use crate::crt::{self, Tube};
use crate::midi::{self, MidiCc, MidiNotes};
use crate::movie::MoviePlayer;
use crate::{ModuleType, Origin};

const MONO: PortKind = PortKind::Mono;
const RGB: PortKind = PortKind::Rgb;
const CONTROL: PortKind = PortKind::Control;
const COMPOSITE: PortKind = PortKind::Composite;

/// Note values for tempo-synced rates (see `behaviors::DIVISION_CYCLES`).
static DIVISIONS: [&str; 12] = [
    "4 bars", "2 bars", "1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/2T", "1/4T", "1/8T", "1/16T",
];

/// A built-in module: Rust, or a module file in `modules/` (the same
/// format community modules use).
enum Builtin {
    Rust(&'static ModuleType),
    File(&'static str, &'static str),
}

macro_rules! file {
    ($name:literal) => {
        Builtin::File($name, include_str!(concat!("../modules/", $name)))
    };
}

/// Built-ins in browser order.
static BUILTINS: &[Builtin] = &[
    file!("ramps.wgsl"),
    Builtin::Rust(&VCO),
    file!("noise.wgsl"),
    file!("shape.wgsl"),
    file!("mixer.wgsl"),
    file!("blend.wgsl"),
    file!("vca.wgsl"),
    file!("fold.wgsl"),
    file!("keyer.wgsl"),
    file!("filter.wgsl"),
    file!("colorize.wgsl"),
    file!("combine.wgsl"),
    file!("split.wgsl"),
    Builtin::Rust(&FEEDBACK),
    Builtin::Rust(&LFO),
    Builtin::Rust(&MACROS),
    Builtin::Rust(&AUDIO_IN),
    Builtin::Rust(&VECTOR),
    Builtin::Rust(&MIDI_NOTES),
    Builtin::Rust(&MIDI_CC),
    file!("output.wgsl"),
    Builtin::Rust(&CRT),
    file!("displace.wgsl"),
    file!("mirror.wgsl"),
    file!("mosaic.wgsl"),
    file!("tile.wgsl"),
    file!("transform.wgsl"),
    file!("warp.wgsl"),
    file!("polar.wgsl"),
    file!("kaleido.wgsl"),
    file!("blur.wgsl"),
    file!("edge.wgsl"),
    file!("posterize.wgsl"),
    file!("monitor.wgsl"),
    Builtin::Rust(&CAMERA),
    file!("tubecam.wgsl"),
    Builtin::Rust(&MOVIE),
    Builtin::Rust(&ENCODER),
    Builtin::Rust(&INJECT),
    Builtin::Rust(&CABLE),
    Builtin::Rust(&RF),
    Builtin::Rust(&VCR),
    Builtin::Rust(&TBC),
    Builtin::Rust(&TV),
    file!("telecine.wgsl"),
    file!("fields.wgsl"),
    file!("delay.wgsl"),
    file!("chromakey.wgsl"),
    Builtin::Rust(&DATAMOSH),
    Builtin::Rust(&PIXEL_SORT),
    Builtin::Rust(&JPEG),
    file!("slice.wgsl"),
    file!("bits.wgsl"),
    Builtin::Rust(&OBJECT),
    file!("text.wgsl"),
    Builtin::Rust(&TEXT_3D),
];

/// The built-in module types. Module files are checked as they load, and
/// the tests keep them valid.
pub(crate) fn builtins() -> Vec<&'static ModuleType> {
    BUILTINS
        .iter()
        .map(|b| match *b {
            Builtin::Rust(ty) => ty,
            Builtin::File(name, source) => crate::files::load(source, name, Origin::BuiltIn)
                .unwrap_or_else(|e| panic!("built-in module {name}: {e}")),
        })
        .collect()
}

// ---------------------------------------------------------------- sources

static VCO: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.vco",
        name: "VCO",
        category: Category::Source,
        description: "Video oscillator in raster time. Multiples of f_H make bars, of f_V bands; \
                      detune to slant and drift. Or set it in Hz, or as a note value on the global tempo. \
                      Phase CV is video-rate phase modulation.",
        inputs: &[],
        outputs: &[PortDesc::new("out", "Out", MONO)],
        params: &[
            ParamDesc::choice("range", "Range", &["×f_H", "×f_V", "Hz", "BPM"]),
            ParamDesc::knob("multiple", "Freq", 0.0, 64.0, 8.0).control_cv(),
            ParamDesc::knob("detune", "Detune", -60.0, 60.0, 0.0).unit("Hz"),
            ParamDesc::choice("sync", "Sync", &["Free", "Line", "Field"]),
            ParamDesc::choice("shape", "Shape", &["Sine", "Tri", "Saw", "Square"]),
            ParamDesc::knob("phase", "Phase", 0.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("level", "Level", 0.0, 1.0, 1.0).video_cv(),
            ParamDesc::knob("hz", "Hz", 0.01, 10_000_000.0, 1.0)
                .exp()
                .unit("Hz")
                .control_cv(),
            ParamDesc::choice("div", "Note", &DIVISIONS).default_choice(4),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Pointwise(
            r#"
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    let ph = u.extra[0];
    let phase = fract(ph.x + fract(f32(ctx.line) * ph.y) + f32(ctx.x) * ph.z + p.phase);
    var o: Outputs;
    o.out = osc_shape(phase, p.shape) * p.level;
    return o;
}
"#,
        ),
    },
    behavior: Some(|| Box::new(Vco::default())),
    origin: Origin::BuiltIn,
};

// ------------------------------------------------------------- processors

// ------------------------------------------------------------------ color

// ----------------------------------------------------------- space & time

static FEEDBACK: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.feedback",
        name: "Feedback",
        category: Category::Space,
        description: "Frame store. Out is last field's In, zoomed, rotated and shifted: \
                      patch it back around for video feedback. Inside the loop, Hue turns the \
                      color a little more each time round, for rainbow trails, Blur spreads it \
                      like a defocused camera, and Key lets only what's brighter than it come \
                      round again. The CVs are video-rate, so a picture patched into them warps \
                      the feedback pixel by pixel.",
        inputs: &[PortDesc::new("sig", "In", RGB)],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::knob("zoom", "Zoom", 0.5, 2.0, 1.02).exp().video_cv(),
            ParamDesc::knob("rotate", "Rotate", -10.0, 10.0, 0.5)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("x", "X", -0.1, 0.1, 0.0).video_cv(),
            ParamDesc::knob("y", "Y", -0.1, 0.1, 0.0).video_cv(),
            ParamDesc::knob("decay", "Gain", 0.0, 1.2, 0.95).video_cv(),
            ParamDesc::choice("mirror", "Mirror", &["Off", "H", "V", "Both"]),
            ParamDesc::knob("hue", "Hue", -30.0, 30.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("blur", "Blur", 0.0, 4.0, 0.0).video_cv(),
            ParamDesc::knob("key", "Key", 0.0, 1.0, 0.0),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Feedback,
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

// ----------------------------------------------------------------- glitch

static DATAMOSH: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.datamosh",
        name: "Datamosh",
        category: Category::Glitch,
        description: "Video compression with the keyframes cut out. Each block finds where its \
                      part of the input moved and drags the old picture along with it, so a new \
                      scene never quite arrives: the old one smears and melts in the new one's \
                      motion. Keyframe On shows the input clean, and turning it off starts the \
                      mosh from there. Bleed lets the input seep back in, Refresh redraws random \
                      blocks, Motion scales the movement found, and Drift pushes every block \
                      along.",
        inputs: &[PortDesc::new("sig", "In", RGB)],
        outputs: &[
            PortDesc::new("out", "Out", RGB),
            PortDesc::new("write", "Write", RGB).hidden(),
        ],
        params: &[
            ParamDesc::choice("keyframe", "Keyframe", &["Off", "On"]).control_cv(),
            ParamDesc::choice("block", "Block", &["8", "16", "32"]).default_choice(1),
            ParamDesc::knob("motion", "Motion", 0.0, 4.0, 1.0).video_cv(),
            ParamDesc::knob("bleed", "Bleed", 0.0, 1.0, 0.1).video_cv(),
            ParamDesc::knob("refresh", "Refresh", 0.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("drift_x", "Drift X", -8.0, 8.0, 0.0)
                .unit("px")
                .video_cv(),
            ParamDesc::knob("drift_y", "Drift Y", -8.0, 8.0, 0.0)
                .unit("lines")
                .video_cv(),
        ],
        screen: false,
        ring: Some(RingSpec {
            output: 1,
            freeze_param: None,
            fields: 1,
            lazy: false,
        }),
        text: None,
        kernel: Kernel::Staged {
            source: include_str!("kernels/datamosh.wgsl"),
            stages: &[8],
        },
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

static PIXEL_SORT: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.pixelsort",
        name: "Pixel Sort",
        category: Category::Glitch,
        description: "Sorts runs of pixels by brightness, hue or saturation, or by a signal \
                      patched into Key. Only pixels whose value lies between Low and High are \
                      sorted, so bright parts melt into streaks while the rest stays put. Dir is \
                      where the highest values end up; Length caps how far a run reaches. Low \
                      and High take video CV.",
        inputs: &[PortDesc::new("sig", "In", RGB), PortDesc::new("key", "Key", MONO)],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::choice("by", "By", &["Luma", "Hue", "Sat", "Key"]),
            ParamDesc::knob("low", "Low", 0.0, 1.0, 0.3).video_cv(),
            ParamDesc::knob("high", "High", 0.0, 1.0, 1.0).video_cv(),
            ParamDesc::knob("length", "Length", 4.0, 256.0, 96.0)
                .exp()
                .unit("px"),
            ParamDesc::choice("dir", "Dir", &["Right", "Left", "Down", "Up"]),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Staged {
            source: include_str!("kernels/pixelsort.wgsl"),
            stages: &[1, 1, 1],
        },
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

static JPEG: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.jpeg",
        name: "JPEG",
        category: Category::Glitch,
        description: "JPEG compression, done for real on 8×8 blocks every field: turn Quality \
                      down for blocking and ringing. Corrupt damages the file as it's read, as \
                      a bad bit would: blocks smeared into stripes or knocked flat, and colors \
                      and positions that slip and stay slipped for the rest of the row. Both \
                      take video CV, so a key can crush one part of the picture. Rate is how \
                      often the damage moves.",
        inputs: &[PortDesc::new("sig", "In", RGB)],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::knob("quality", "Quality", 1.0, 100.0, 20.0).video_cv(),
            ParamDesc::knob("corrupt", "Corrupt", 0.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("rate", "Rate", 0.5, 60.0, 4.0)
                .exp()
                .unit("Hz")
                .control_cv(),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Staged {
            source: include_str!("kernels/jpeg.wgsl"),
            stages: &[8, 1, 1, 1],
        },
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

// ---------------------------------------------------------------- control

static LFO: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.lfo",
        name: "LFO",
        category: Category::Control,
        description: "Control oscillator, evaluated once per field. Rate in Hz, or BPM: a note \
                      value on the global tempo, in step with every other module on it. Trig \
                      fires at the start of each cycle.",
        inputs: &[],
        outputs: &[
            PortDesc::new("out", "Out", CONTROL),
            PortDesc::new("trig", "Trig", CONTROL),
        ],
        params: &[
            ParamDesc::knob("rate", "Rate", 0.001, 100.0, 0.5)
                .exp()
                .unit("Hz")
                .control_cv(),
            ParamDesc::choice(
                "shape",
                "Shape",
                &["Sine", "Tri", "Saw", "Square", "Random", "Drift", "Ramp ↓"],
            ),
            ParamDesc::knob("amount", "Amount", 0.0, 1.0, 1.0).control_cv(),
            ParamDesc::knob("offset", "Offset", -1.0, 1.0, 0.0),
            ParamDesc::choice("mode", "Mode", &["Hz", "BPM"]),
            ParamDesc::choice("div", "Note", &DIVISIONS).default_choice(4),
            ParamDesc::knob("phase", "Phase", 0.0, 1.0, 0.0),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Control,
    },
    behavior: Some(|| Box::new(Lfo::default())),
    origin: Origin::BuiltIn,
};

static MACROS: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.macros",
        name: "Macros",
        category: Category::Control,
        description: "Four knobs as control outputs, for performing a patch.",
        inputs: &[],
        outputs: &[
            PortDesc::new("a", "A", CONTROL),
            PortDesc::new("b", "B", CONTROL),
            PortDesc::new("c", "C", CONTROL),
            PortDesc::new("d", "D", CONTROL),
        ],
        params: &[
            ParamDesc::knob("a", "A", 0.0, 1.0, 0.0),
            ParamDesc::knob("b", "B", 0.0, 1.0, 0.0),
            ParamDesc::knob("c", "C", 0.0, 1.0, 0.0),
            ParamDesc::knob("d", "D", 0.0, 1.0, 0.0),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Control,
    },
    behavior: Some(|| Box::new(Macros)),
    origin: Origin::BuiltIn,
};

static VECTOR: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.vector",
        name: "Vector",
        category: Category::Source,
        description: "An XY display drawing audio with its beam: one channel sweeps it across, \
                      another up and down, for Lissajous figures and oscilloscope music. Scope \
                      draws Y against time instead, triggered as it rises through zero. The \
                      phosphor holds the trace as long as Persist lets it, and glows hottest \
                      where the beam lingers; a fast jump leaves only a faint line.",
        inputs: &[],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::choice("mode", "Mode", &crate::vector::MODES),
            // Mixed, then each channel the device has.
            ParamDesc::choice("x_channel", "X ch", &["1+2"])
                .live(Live::AudioChannels)
                .default_choice(1),
            ParamDesc::choice("y_channel", "Y ch", &["1+2"])
                .live(Live::AudioChannels)
                .default_choice(2),
            ParamDesc::knob("size", "Size", 0.1, 8.0, 1.0).exp().control_cv(),
            ParamDesc::knob("x", "X", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("y", "Y", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("focus", "Focus", 0.0, 1.0, 0.4),
            ParamDesc::knob("bright", "Bright", 0.0, 4.0, 1.0).control_cv(),
            ParamDesc::knob("persist", "Persist", 0.0, 1.0, 0.4),
            ParamDesc::choice("phosphor", "Phosphor", &["Green", "Blue", "Amber", "White"]),
            ParamDesc::knob("time", "Time", 1.0, 200.0, 20.0).exp().unit("ms"),
        ],
        screen: true,
        ring: None,
        // The input device's name; empty for the system default.
        text: Some(TextSpec {
            default: "",
            live: Some(Live::AudioInputs),
        }),
        kernel: Kernel::Vector,
    },
    behavior: Some(|| Box::new(crate::vector::Vector::default())),
    origin: Origin::BuiltIn,
};

static AUDIO_IN: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.audio",
        name: "Audio In",
        category: Category::Control,
        description: "Audio as control. Pick an input and a channel from what's connected. A \
                      resonant filter picks out a band (Low for the kick, High for hats) and an \
                      envelope follower turns its loudness into Env. Gate is on while Env is \
                      over Thresh, and Trig fires as it opens.",
        inputs: &[],
        outputs: &[
            PortDesc::new("env", "Env", CONTROL),
            PortDesc::new("gate", "Gate", CONTROL),
            PortDesc::new("trig", "Trig", CONTROL),
        ],
        params: &[
            // Mixed, then each channel the device has.
            ParamDesc::choice("channel", "Channel", &["1+2"]).live(Live::AudioChannels),
            ParamDesc::choice("filter", "Filter", &["Full", "Low", "Band", "High"]),
            ParamDesc::knob("freq", "Freq", 20.0, 16_000.0, 150.0)
                .exp()
                .unit("Hz")
                .control_cv(),
            ParamDesc::knob("q", "Q", 0.5, 20.0, 0.707).exp().control_cv(),
            ParamDesc::knob("gain", "Gain", -24.0, 48.0, 12.0)
                .unit("dB")
                .control_cv(),
            ParamDesc::knob("attack", "Attack", 0.1, 1000.0, 5.0)
                .exp()
                .unit("ms"),
            ParamDesc::knob("release", "Release", 1.0, 5000.0, 150.0)
                .exp()
                .unit("ms"),
            ParamDesc::knob("threshold", "Thresh", 0.0, 1.0, 0.5),
            ParamDesc::choice("response", "Response", &["Lin", "Log"]),
        ],
        screen: false,
        ring: None,
        // The input device's name; empty for the system default.
        text: Some(TextSpec {
            default: "",
            live: Some(Live::AudioInputs),
        }),
        kernel: Kernel::Control,
    },
    behavior: Some(|| Box::new(AudioIn::default())),
    origin: Origin::BuiltIn,
};

static MIDI_NOTES: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.midinotes",
        name: "MIDI Notes",
        category: Category::Control,
        description: "Notes from any MIDI device as control. Pitch runs 0..1 from Low to High \
                      (notes outside are ignored, for splits), Gate is on while a key is held, \
                      Vel is how hard it was struck and Trig fires on every note. The last \
                      note held wins.",
        inputs: &[],
        outputs: &[
            PortDesc::new("pitch", "Pitch", CONTROL),
            PortDesc::new("gate", "Gate", CONTROL),
            PortDesc::new("vel", "Vel", CONTROL),
            PortDesc::new("trig", "Trig", CONTROL),
        ],
        params: &[
            ParamDesc::choice("channel", "Channel", &midi::CHANNELS),
            ParamDesc::knob("low", "Low", 0.0, 127.0, 0.0),
            ParamDesc::knob("high", "High", 0.0, 127.0, 127.0),
            ParamDesc::knob("bend", "Bend", 0.0, 24.0, 2.0).unit("st"),
            ParamDesc::knob("glide", "Glide", 0.0, 2000.0, 0.0).unit("ms"),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Control,
    },
    behavior: Some(|| Box::new(MidiNotes::default())),
    origin: Origin::BuiltIn,
};

static MIDI_CC: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.midicc",
        name: "MIDI CC",
        category: Category::Control,
        description: "Four MIDI controllers (knobs, faders, the mod wheel) as control outputs, \
                      0..1. Set each output's CC number, or set Learn to an output and move a \
                      control. Smooth glides over the steps between MIDI values.",
        inputs: &[],
        outputs: &[
            PortDesc::new("a", "A", CONTROL),
            PortDesc::new("b", "B", CONTROL),
            PortDesc::new("c", "C", CONTROL),
            PortDesc::new("d", "D", CONTROL),
        ],
        params: &[
            ParamDesc::choice("channel", "Channel", &midi::CHANNELS),
            ParamDesc::choice("learn", "Learn", &["Off", "A", "B", "C", "D"]),
            ParamDesc::knob("cc_a", "CC A", 0.0, 127.0, 1.0),
            ParamDesc::knob("cc_b", "CC B", 0.0, 127.0, 2.0),
            ParamDesc::knob("cc_c", "CC C", 0.0, 127.0, 3.0),
            ParamDesc::knob("cc_d", "CC D", 0.0, 127.0, 4.0),
            ParamDesc::knob("smooth", "Smooth", 0.0, 1000.0, 30.0).unit("ms"),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Control,
    },
    behavior: Some(|| Box::new(MidiCc::default())),
    origin: Origin::BuiltIn,
};

// -------------------------------------------------------------------- i/o

static CRT: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: crt::ID,
        name: "CRT",
        category: Category::Io,
        description: "A picture tube for the output window, in place of Output. An electron beam \
                      sweeps the picture onto phosphors behind a mask and curved glass, at the \
                      window's own resolution and refresh rate. With Scan on Beam, a display at a \
                      whole multiple of the field rate (120 Hz for NTSC, 100 Hz for PAL) shows \
                      each field drawn top to bottom as the beam does; other displays, and Hold, \
                      show whole fields. Bright parts bloom the beam, Persist makes the phosphors \
                      linger, and a Magnet leaves color blotches that stay until you degauss. H pos \
                      and V pos take video-rate CV, so a VCO can bend the raster like a wobbulator.",
        inputs: &[PortDesc::new("sig", "In", RGB)],
        outputs: &[
            PortDesc::new("out", "Out", RGB).hidden(),
            PortDesc::new("yoke", "Yoke", RGB).hidden(),
        ],
        params: &[
            ParamDesc::choice("power", "Power", &["Off", "On"])
                .default_choice(1)
                .control_cv(),
            ParamDesc::choice("degauss", "Degauss", &["Push", "Thunk"]).control_cv(),
            ParamDesc::choice("mask_type", "Mask", &["Grille", "Shadow", "Slot"]),
            ParamDesc::choice("phosphor", "Phosphor", &["Color", "White", "Green", "Amber"]),
            ParamDesc::choice("scan", "Scan", &["Beam", "Hold"]),
            ParamDesc::knob("bright", "Bright", -0.5, 0.5, 0.0),
            ParamDesc::knob("contrast", "Contrast", 0.0, 2.0, 1.0),
            ParamDesc::knob("white", "White", 5000.0, 10000.0, 6500.0).unit("K"),
            ParamDesc::knob("mask", "Mask amt", 0.0, 1.0, 0.5),
            ParamDesc::knob("pitch", "Pitch", 200.0, 1200.0, 560.0).unit("triads"),
            ParamDesc::knob("spot", "Spot", 0.0, 1.0, 0.25).control_cv(),
            ParamDesc::knob("bloom", "Bloom", 0.0, 1.0, 0.5).control_cv(),
            ParamDesc::knob("glow", "Halation", 0.0, 1.0, 0.3),
            ParamDesc::knob("persist", "Persist", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("converge", "Converge", 0.0, 1.0, 0.15).control_cv(),
            ParamDesc::knob("curvature", "Curve", 0.0, 1.0, 0.3).control_cv(),
            ParamDesc::knob("overscan", "Overscan", 0.0, 15.0, 4.0).unit("%"),
            ParamDesc::knob("room", "Room", 0.0, 1.0, 0.1),
            ParamDesc::knob("magnet", "Magnet", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("h_pos", "H pos", -0.5, 0.5, 0.0).video_cv(),
            ParamDesc::knob("v_pos", "V pos", -0.5, 0.5, 0.0).video_cv(),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Pointwise(
            r#"
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = inp.sig;
    // Where the yoke throws each sample, CV and all, for the engine's tube.
    o.yoke = vec3<f32>(p.h_pos, p.v_pos, 0.0);
    return o;
}
"#,
        ),
    },
    behavior: Some(|| Box::new(Tube::default())),
    origin: Origin::BuiltIn,
};

// --------------------------------------------------------- gather kernels

static CAMERA: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.camera",
        name: "Camera",
        category: Category::Io,
        description: "Live camera input, scaled into the raster.",
        inputs: &[],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::choice("device", "Device", &["Cam 1", "Cam 2", "Cam 3", "Cam 4"]),
            ParamDesc::choice("fit", "Fit", &["Fill", "Fit", "Stretch"]),
            ParamDesc::choice("mirror", "Mirror", &["Off", "On"]).default_choice(1),
        ],
        screen: true,
        ring: None,
        text: None,
        kernel: Kernel::Camera,
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

static MOVIE: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.movie",
        name: "Movie",
        category: Category::Io,
        description: "Plays a movie file in a loop from In to Out. Click the picture to set \
                      In and Out to the frame and to cut the movie into splices, as on a \
                      Morphagene: Organize picks the splice to play and Next steps on to the \
                      next one. Below 0, Speed plays backward. Loop goes round, Bounce goes \
                      back and forth, Once stops at the end. In Scrub mode Position picks the \
                      frame, so any CV can scrub. FPS holds frames for a choppier picture, \
                      and BPM sync fits the loop to a note. Restart jumps back to Position; \
                      Pos follows the playhead and End fires at the end of each loop.",
        inputs: &[
            PortDesc::new("restart", "Restart", CONTROL),
            PortDesc::new("next", "Next", CONTROL),
        ],
        outputs: &[
            PortDesc::new("out", "Out", RGB),
            PortDesc::new("pos", "Pos", CONTROL),
            PortDesc::new("end", "End", CONTROL),
        ],
        params: &[
            ParamDesc::knob("speed", "Speed", -4.0, 4.0, 1.0)
                .unit("×")
                .control_cv(),
            // Of the splice playing.
            ParamDesc::knob("in_point", "In", 0.0, 100.0, 0.0)
                .unit("%")
                .control_cv()
                .in_editor(),
            ParamDesc::knob("out_point", "Out", 0.0, 100.0, 100.0)
                .unit("%")
                .control_cv()
                .in_editor(),
            ParamDesc::knob("organize", "Organize", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::choice("loop", "Loop", &["Loop", "Bounce", "Once"]),
            ParamDesc::choice("mode", "Mode", &["Play", "Scrub"]),
            ParamDesc::knob("position", "Position", 0.0, 100.0, 0.0)
                .unit("%")
                .control_cv(),
            ParamDesc::knob("fps", "FPS", 1.0, 60.0, 60.0)
                .exp()
                .unit("fps")
                .control_cv(),
            ParamDesc::choice("sync", "Sync", &["Free", "BPM"]),
            ParamDesc::choice("length", "Length", &DIVISIONS).default_choice(2),
            ParamDesc::choice("fit", "Fit", &["Fill", "Fit", "Stretch"]),
        ],
        screen: true,
        ring: None,
        // The movie file's path.
        text: Some(TextSpec {
            default: "",
            live: Some(Live::MovieFile),
        }),
        kernel: Kernel::Picture,
    },
    behavior: Some(|| Box::new(MoviePlayer::default())),
    origin: Origin::BuiltIn,
};

// ----------------------------------------------------- analog signal path

static ENCODER: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.encoder",
        name: "Encoder",
        category: Category::Analog,
        description: "RGB to composite video: sync, blanking, color burst and modulated chroma \
                      (NTSC or PAL, following the raster). Detune the subcarrier for rainbows.",
        inputs: &[PortDesc::new("sig", "In", RGB)],
        outputs: &[PortDesc::new("out", "Out", COMPOSITE)],
        params: &[
            ParamDesc::knob("luma", "Luma", 0.0, 2.0, 1.0).control_cv(),
            ParamDesc::knob("chroma", "Chroma", 0.0, 2.0, 1.0).control_cv(),
            ParamDesc::knob("chroma_bw", "C band", 0.2, 3.0, 1.3)
                .exp()
                .unit("MHz"),
            ParamDesc::knob("burst", "Burst", 0.0, 2.0, 1.0),
            ParamDesc::knob("sync", "Sync", 0.0, 2.0, 1.0).control_cv(),
            ParamDesc::knob("detune", "Detune", -2000.0, 2000.0, 0.0)
                .unit("Hz")
                .control_cv(),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Composite(CompositeOp::Encode),
    },
    behavior: Some(|| Box::new(Encoder::default())),
    origin: Origin::BuiltIn,
};

static INJECT: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.inject",
        name: "Inject",
        category: Category::Analog,
        description: "Mixes a video signal straight into the composite waveform, stretched over \
                      the whole field so it reaches the sync pulses too. Bend, tear and roll.",
        inputs: &[
            PortDesc::new("sig", "In", COMPOSITE),
            PortDesc::new("mod", "Mod", MONO),
        ],
        outputs: &[PortDesc::new("out", "Out", COMPOSITE)],
        params: &[
            ParamDesc::knob("amount", "Amount", -1.0, 1.0, 0.3).control_cv(),
            ParamDesc::choice("region", "Region", &["All", "Blank", "Picture"]),
            ParamDesc::knob("offset", "Offset", -1.0, 1.0, -0.5),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Composite(CompositeOp::Inject),
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

static CABLE: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.cable",
        name: "Cable",
        category: Category::Analog,
        description: "Transmission damage: snow, limited bandwidth, ghosts, hum bars and stray \
                      carriers.",
        inputs: &[PortDesc::new("sig", "In", COMPOSITE)],
        outputs: &[PortDesc::new("out", "Out", COMPOSITE)],
        params: &[
            ParamDesc::knob("snow", "Snow", 0.0, 1.0, 0.08).control_cv(),
            ParamDesc::knob("bandwidth", "Band", 0.5, 13.5, 5.0)
                .exp()
                .unit("MHz"),
            ParamDesc::knob("ghost", "Ghost", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("delay", "Delay", 0.0, 12.0, 2.0).unit("µs"),
            ParamDesc::knob("hum", "Hum", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("interference", "Interf", 0.0, 0.5, 0.0).control_cv(),
            ParamDesc::knob("int_freq", "Int freq", 0.3, 6.0, 2.2)
                .exp()
                .unit("MHz"),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Composite(CompositeOp::Cable),
    },
    behavior: Some(|| Box::new(Cable::default())),
    origin: Origin::BuiltIn,
};

static RF: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.rf",
        name: "RF",
        category: Category::Analog,
        description: "Broadcast on a channel and pulled in by a TV's tuner. Signal: turn it down \
                      and snow comes up as the TV's gain rises to meet it, then the picture \
                      loses its hold. Fine tunes one way to a soft picture that loses its color, \
                      the other way to ringing edges and the sound beating with the chroma. \
                      Ghost is a reflection off a building. Adjacent is the next channel up \
                      bleeding in, its blanking bars crawling past (patch a picture into Other \
                      for it to show); Co-chan is a distant station on the same channel. Sound \
                      puts the audio from the input you pick on the sound carrier: it bends the \
                      beat pattern and comes through as buzz bars.",
        inputs: &[
            PortDesc::new("sig", "In", COMPOSITE),
            PortDesc::new("other", "Other", COMPOSITE),
        ],
        outputs: &[PortDesc::new("out", "Out", COMPOSITE)],
        params: &[
            ParamDesc::knob("signal", "Signal", 0.0, 1.0, 0.9).control_cv(),
            ParamDesc::knob("fine", "Fine", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("ghost", "Ghost", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("adjacent", "Adjacent", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("cochannel", "Co-chan", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("sound", "Sound", 0.0, 1.0, 0.2).control_cv(),
            // The audio on the sound carrier: mixed, then each channel.
            ParamDesc::choice("channel", "Audio ch", &["1+2"]).live(Live::AudioChannels),
        ],
        screen: false,
        ring: None,
        // The audio input device's name; empty for the system default.
        text: Some(TextSpec {
            default: "",
            live: Some(Live::AudioInputs),
        }),
        kernel: Kernel::Composite(CompositeOp::Rf),
    },
    behavior: Some(|| Box::new(crate::rf::Rf::default())),
    origin: Origin::BuiltIn,
};

static VCR: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.vcr",
        name: "VCR",
        category: Category::Analog,
        description: "A videotape deck's record and playback path: luma bandwidth by format and \
                      tape speed, color-under chroma, timebase wobble, flagging, head switching, \
                      dropouts and mistracking. Deck: the first time it's stopped or spooled, \
                      the deck starts taping what comes in. Pause holds a field with the noise \
                      bar, Search spools through the tape with noise bars across the picture, and \
                      Play carries on from wherever the tape is. Chain several for generation \
                      loss.",
        inputs: &[PortDesc::new("sig", "In", COMPOSITE)],
        outputs: &[
            PortDesc::new("out", "Out", COMPOSITE),
            PortDesc::new("tape", "Tape", COMPOSITE).hidden(),
        ],
        params: &[
            ParamDesc::choice("format", "Format", &crate::vcr::FORMATS),
            ParamDesc::choice("speed", "Speed", &["SP", "LP", "EP"]),
            ParamDesc::choice("transport", "Deck", &crate::vcr::TRANSPORT).control_cv(),
            ParamDesc::knob("search", "Search", 2.0, 15.0, 5.0)
                .unit("×")
                .control_cv(),
            ParamDesc::knob("tracking", "Tracking", 0.0, 1.0, 0.95).control_cv(),
            ParamDesc::knob("wow", "Wow", 0.0, 1.0, 0.2).control_cv(),
            ParamDesc::knob("jitter", "Jitter", 0.0, 1.0, 0.25).control_cv(),
            ParamDesc::knob("flagging", "Flagging", 0.0, 1.0, 0.3),
            ParamDesc::knob("head_switch", "Head sw", 0.0, 1.0, 0.7),
            ParamDesc::knob("dropouts", "Dropouts", 0.0, 1.0, 0.2).control_cv(),
            ParamDesc::knob("clog", "Clog", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("crease", "Crease", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("chroma_noise", "C noise", 0.0, 1.0, 0.3),
            ParamDesc::knob("luma_noise", "Y noise", 0.0, 1.0, 0.25),
            ParamDesc::knob("sharpness", "Sharp", 0.0, 1.0, 0.4),
        ],
        screen: false,
        // The tape: what came in, field by field, while the deck ran. It's
        // threaded the first time the deck stops or spools.
        ring: Some(RingSpec {
            output: 1,
            freeze_param: Some(crate::vcr::TRANSPORT_PARAM),
            fields: 90,
            lazy: true,
        }),
        text: None,
        kernel: Kernel::Composite(CompositeOp::Vcr),
    },
    behavior: Some(|| Box::new(crate::vcr::Deck::default())),
    origin: Origin::BuiltIn,
};

static TBC: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.tbc",
        name: "TBC",
        category: Category::Analog,
        description: "Time base corrector: re-times every line to its sync pulse, steadying a \
                      wobbly tape. Proc amp for picture gain and black level.",
        inputs: &[PortDesc::new("sig", "In", COMPOSITE)],
        outputs: &[PortDesc::new("out", "Out", COMPOSITE)],
        params: &[
            ParamDesc::knob("correction", "Correct", 0.0, 1.0, 1.0).control_cv(),
            ParamDesc::knob("gain", "Video", 0.0, 2.0, 1.0).control_cv(),
            ParamDesc::knob("black", "Black", -0.3, 0.3, 0.0).control_cv(),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Composite(CompositeOp::Tbc),
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

static TV: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.tv",
        name: "TV",
        category: Category::Analog,
        description: "A receiver that has to lock to what it's fed: sync separator, line \
                      oscillator with AFC, field oscillator with V hold, burst-locked color. \
                      Decoder picks how it parts color from detail: a notch filter, a comb, or \
                      an adaptive comb that keeps vertical edges clean. AGC turns dark pictures \
                      up and bright ones down, and pumps as the picture changes; Killer is how \
                      weak a burst can get before color goes off. Unplugged, it shows snow.",
        inputs: &[PortDesc::new("sig", "In", COMPOSITE)],
        outputs: &[PortDesc::new("out", "Out", RGB)],
        params: &[
            ParamDesc::knob("brightness", "Bright", -0.5, 0.5, 0.0),
            ParamDesc::knob("contrast", "Contrast", 0.0, 2.0, 1.0),
            ParamDesc::knob("saturation", "Color", 0.0, 2.0, 1.0),
            ParamDesc::knob("tint", "Tint", -0.5, 0.5, 0.0),
            ParamDesc::choice("decoder", "Decoder", &["Notch", "Comb", "Adaptive"]),
            ParamDesc::knob("h_hold", "H hold", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("afc", "AFC", 0.02, 1.0, 0.25).exp(),
            ParamDesc::knob("v_hold", "V hold", -1.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("slicer", "Slicer", -0.4, 0.2, -0.2),
            ParamDesc::knob("v_pos", "V pos", -10.0, 10.0, 0.0).unit("lines"),
            ParamDesc::knob("h_pos", "H pos", -60.0, 60.0, 0.0).unit("smp"),
            ParamDesc::knob("sharpness", "Sharp", 0.0, 1.0, 0.0),
            ParamDesc::knob("agc", "AGC", 0.0, 1.0, 0.0).control_cv(),
            ParamDesc::knob("killer", "Killer", 0.0, 1.0, 0.25),
        ],
        screen: true,
        ring: None,
        text: None,
        kernel: Kernel::Composite(CompositeOp::Tv),
    },
    behavior: None,
    origin: Origin::BuiltIn,
};

// ------------------------------------------------------------ time & keys

// ------------------------------------------------------------------- 3D

/// WGSL shared by the raymarched 3D modules: the camera, rotations, box
/// mapping of the `tex` input, and lighting. A macro so it can be spliced
/// into each kernel's source with `concat!`.
macro_rules! wgsl_3d {
    () => {
        r#"
// The camera sits on -z looking at the origin; FOCAL sets the lens.
const FOCAL: f32 = 1.6;

fn camera_ray(ctx: Ctx) -> vec3<f32> {
    let q = (ctx.uv - vec2<f32>(0.5)) * vec2<f32>(ctx.aspect, 1.0);
    return normalize(vec3<f32>(q.x, -q.y, FOCAL));
}

// Angle between neighbouring pixels' rays: the width of an antialiased edge.
fn pixel_angle() -> f32 {
    return 1.0 / (f32(u.info.height) * FOCAL);
}

// Rotation by Euler angles in degrees: x, then y, then z.
fn rot_xyz(deg: vec3<f32>) -> mat3x3<f32> {
    let a = radians(deg);
    let cx = cos(a.x);
    let sx = sin(a.x);
    let cy = cos(a.y);
    let sy = sin(a.y);
    let cz = cos(a.z);
    let sz = sin(a.z);
    let rx = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, cx, sx), vec3<f32>(0.0, -sx, cx));
    let ry = mat3x3<f32>(vec3<f32>(cy, 0.0, -sy), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(sy, 0.0, cy));
    let rz = mat3x3<f32>(vec3<f32>(cz, sz, 0.0), vec3<f32>(-sz, cz, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    return rz * ry * rx;
}

// The `tex` input at texture coordinates 0..1 (wrapping), read from the
// current field's lines.
fn tex_at(uv: vec2<f32>, stride: i32) -> vec3<f32> {
    let x = i32(fract(uv.x) * f32(u.info.width));
    let line = i32(fract(uv.y) * f32(u.info.lines));
    return fetch_tex(x, line * stride + i32(u.info.parity));
}

// Box mapping: three planar projections of `tex`, blended by the normal.
// `p` is in object space, about -1..1 across the object.
fn tex_box(p: vec3<f32>, n: vec3<f32>, stride: i32) -> vec3<f32> {
    var w = pow(abs(n), vec3<f32>(4.0));
    w = w / max(w.x + w.y + w.z, 1e-5);
    let s = p * 0.5 + vec3<f32>(0.5);
    return tex_at(vec2<f32>(s.z, 1.0 - s.y), stride) * w.x
        + tex_at(vec2<f32>(s.x, s.z), stride) * w.y
        + tex_at(vec2<f32>(s.x, 1.0 - s.y), stride) * w.z;
}

// A key light from the upper left, specular highlight and a rim.
fn shade(n: vec3<f32>, rd: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let l = normalize(vec3<f32>(-0.5, 0.8, -0.6));
    let diffuse = max(dot(n, l), 0.0);
    let spec = pow(max(dot(n, normalize(l - rd)), 0.0), 32.0);
    let rim = pow(1.0 - max(dot(n, -rd), 0.0), 3.0);
    return albedo * (0.12 + 0.88 * diffuse) + vec3<f32>(0.35 * spec + 0.2 * rim);
}
"#
    };
}

static OBJECT: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.object",
        name: "3D Object",
        category: Category::Source,
        description: "A raymarched solid, worked out per pixel: video-rate CV on rotation, \
                      twist or size bends it line by line. Patch video into Tex to wrap it \
                      around the surface, and turn up Displace to emboss it. Depth and \
                      Matte are ready for keys and displacement.",
        inputs: &[PortDesc::new("tex", "Tex", RGB).default_value(0.85)],
        outputs: &[
            PortDesc::new("out", "Out", RGB),
            PortDesc::new("depth", "Depth", MONO),
            PortDesc::new("matte", "Matte", MONO),
        ],
        params: &[
            ParamDesc::choice("shape", "Shape", &["Cube", "Sphere", "Torus", "Octa", "Hex"]),
            ParamDesc::choice("mode", "Look", &["Shaded", "Normals", "Depth", "Contour"]),
            ParamDesc::knob("size", "Size", 0.1, 2.0, 0.8).video_cv(),
            ParamDesc::knob("x", "X", -1.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("y", "Y", -1.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("rot_x", "Rot X", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("rot_y", "Rot Y", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("rot_z", "Rot Z", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("spin_x", "Spin X", -180.0, 180.0, 17.0)
                .unit("°/s")
                .control_cv(),
            ParamDesc::knob("spin_y", "Spin Y", -180.0, 180.0, 40.0)
                .unit("°/s")
                .control_cv(),
            ParamDesc::knob("dist", "Dist", 2.0, 12.0, 4.0).exp(),
            ParamDesc::knob("bevel", "Bevel", 0.0, 0.5, 0.06),
            ParamDesc::knob("twist", "Twist", -4.0, 4.0, 0.0).video_cv(),
            ParamDesc::knob("displace", "Displace", 0.0, 1.0, 0.0).video_cv(),
        ],
        screen: false,
        ring: None,
        text: None,
        kernel: Kernel::Pointwise(concat!(
            wgsl_3d!(),
            r#"
struct Obj {
    // World to object space: the inverse of the object's rotation.
    m: mat3x3<f32>,
    t: vec3<f32>,
    shape: u32,
    size: f32,
    bevel: f32,
    twist: f32,
    displace: f32,
    stride: i32,
}

fn sd_hex_prism(p: vec3<f32>, h: vec2<f32>) -> f32 {
    let k = vec3<f32>(-0.8660254, 0.5, 0.57735);
    let a = abs(p);
    let xy = a.xy - 2.0 * min(dot(k.xy, a.xy), 0.0) * k.xy;
    let d = vec2<f32>(
        length(xy - vec2<f32>(clamp(xy.x, -k.z * h.x, k.z * h.x), h.x)) * sign(xy.y - h.x),
        a.z - h.y,
    );
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0)));
}

// Object space position of world point `w`, twisted about the vertical axis.
fn to_object(o: Obj, w: vec3<f32>) -> vec3<f32> {
    let p = o.m * (w - o.t);
    let a = p.y * o.twist / max(o.size, 0.1);
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * p.x - s * p.z, p.y, s * p.x + c * p.z);
}

// Signed distance from world point `w` to the object's surface.
fn obj_sdf(o: Obj, w: vec3<f32>) -> f32 {
    let p = to_object(o, w);
    let r = o.bevel * o.size;
    let s = max(o.size - r, 0.01);
    var d: f32;
    switch o.shape {
        case 0u: {
            let q = abs(p) - vec3<f32>(s * 0.7);
            d = length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
        }
        case 1u: {
            d = length(p) - s;
        }
        case 2u: {
            d = length(vec2<f32>(length(p.xz) - s * 0.75, p.y)) - s * 0.3;
        }
        case 3u: {
            d = (abs(p.x) + abs(p.y) + abs(p.z) - s * 1.2) * 0.57735;
        }
        default: {
            d = sd_hex_prism(p, vec2<f32>(s * 0.75, s * 0.55));
        }
    }
    d = d - r;
    if (o.displace > 0.0 && patched_tex()) {
        // Displacement moves the surface by at most `amp`. Far from it, a
        // bound is enough and saves reading the texture on every step.
        let amp = o.displace * o.size * 0.125;
        if (d > 2.0 * amp + 0.02) {
            return d - amp;
        }
        let bump = luma(tex_box(p / o.size, normalize(p + vec3<f32>(1e-5)), o.stride));
        d = d - 2.0 * amp * (bump - 0.5);
    }
    return d;
}

fn obj_normal(o: Obj, w: vec3<f32>, t: f32) -> vec3<f32> {
    let e = vec2<f32>(1.0, -1.0) * 0.0005 * max(t, 1.0);
    return normalize(
        e.xyy * obj_sdf(o, w + e.xyy) + e.yyx * obj_sdf(o, w + e.yyx)
            + e.yxy * obj_sdf(o, w + e.yxy) + e.xxx * obj_sdf(o, w + e.xxx),
    );
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var obj: Obj;
    let spin = vec3<f32>(u.extra[0].x, u.extra[0].y, 0.0);
    obj.m = transpose(rot_xyz(vec3<f32>(p.rot_x, p.rot_y, p.rot_z) + spin));
    obj.t = vec3<f32>(p.x * ctx.aspect, p.y, 0.0) * 1.25;
    obj.shape = p.shape;
    obj.size = max(p.size, 0.01);
    obj.bevel = clamp(p.bevel, 0.0, 0.5);
    obj.twist = p.twist;
    obj.displace = max(p.displace, 0.0);
    obj.stride = ctx.stride;

    let dist = max(p.dist, 0.5);
    let ro = vec3<f32>(0.0, 0.0, -dist);
    let rd = camera_ray(ctx);
    var t = 0.0;
    var hit = false;
    // Closest approach, as an angle seen from the camera, for soft edges.
    var closest = 1e9;
    var t_closest = dist;
    // Twist and displacement stretch the distance field, so step more
    // carefully as they grow, or rays overshoot the surface and speckle.
    let step = 0.75 / (1.0 + abs(obj.twist) * 0.6 + obj.displace * 3.0);
    for (var i = 0; i < 256; i = i + 1) {
        let d = obj_sdf(obj, ro + rd * t);
        if (d / max(t, 1e-3) < closest) {
            closest = d / max(t, 1e-3);
            t_closest = t;
        }
        if (d < 0.0005 * t) {
            hit = true;
            break;
        }
        t = t + d * step;
        if (t > dist + 4.0 * obj.size + 2.0) {
            break;
        }
    }
    var matte = 1.0;
    if (!hit) {
        matte = 1.0 - smoothstep(0.0, pixel_angle(), closest);
        t = t_closest;
    }
    var o: Outputs;
    if (matte <= 0.0) {
        o.out = vec3<f32>(0.0);
        o.depth = 0.0;
        o.matte = 0.0;
        return o;
    }
    let w = ro + rd * t;
    let n = obj_normal(obj, w, t);
    let op = to_object(obj, w);
    let depth = clamp(0.5 + (dist - t) / (2.0 * obj.size), 0.0, 1.0);
    var color: vec3<f32>;
    switch p.mode {
        case 0u: {
            color = shade(n, rd, tex_box(op / obj.size, obj.m * n, ctx.stride));
        }
        case 1u: {
            color = n * 0.5 + vec3<f32>(0.5);
        }
        case 2u: {
            color = vec3<f32>(depth);
        }
        default: {
            // Contour lines across the object, lit like the surface.
            let k = abs(fract(op.y * 5.0 / obj.size) - 0.5);
            let line = 1.0 - smoothstep(0.04, 0.1, k);
            color = shade(n, rd, vec3<f32>(1.0)) * line;
        }
    }
    o.out = color * matte;
    o.depth = depth * matte;
    o.matte = matte;
    return o;
}
"#
        )),
    },
    behavior: Some(|| Box::new(Spin::default())),
    origin: Origin::BuiltIn,
};

static TEXT_3D: ModuleType = ModuleType {
    desc: ModuleDesc {
        id: "dov.text3d",
        name: "3D Text",
        category: Category::Source,
        description: "Extruded, bevelled lettering, raymarched per pixel like 3D Object. Type \
                      on the panel; spin it, tilt it, patch video into Tex to paint the faces, \
                      or into the rotation CVs to twist it line by line.",
        inputs: &[PortDesc::new("tex", "Tex", RGB).default_value(0.85)],
        outputs: &[
            PortDesc::new("out", "Out", RGB),
            PortDesc::new("depth", "Depth", MONO),
            PortDesc::new("matte", "Matte", MONO),
        ],
        params: &[
            ParamDesc::choice("font", "Font", &FONTS).default_choice(1),
            ParamDesc::choice("mode", "Look", &["Shaded", "Normals", "Depth", "Contour"]),
            ParamDesc::knob("size", "Size", 0.1, 3.0, 1.0).exp().video_cv(),
            ParamDesc::knob("x", "X", -1.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("y", "Y", -1.0, 1.0, 0.0).video_cv(),
            ParamDesc::knob("rot_x", "Rot X", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("rot_y", "Rot Y", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("rot_z", "Rot Z", -180.0, 180.0, 0.0)
                .unit("°")
                .video_cv(),
            ParamDesc::knob("spin_x", "Spin X", -180.0, 180.0, 0.0)
                .unit("°/s")
                .control_cv(),
            ParamDesc::knob("spin_y", "Spin Y", -180.0, 180.0, 25.0)
                .unit("°/s")
                .control_cv(),
            ParamDesc::knob("extrude", "Depth", 0.02, 2.0, 0.35)
                .exp()
                .video_cv(),
            ParamDesc::knob("bevel", "Bevel", 0.0, 0.15, 0.03),
            ParamDesc::knob("dist", "Dist", 2.0, 20.0, 5.0).exp(),
        ],
        screen: false,
        ring: None,
        text: Some(TextSpec {
            default: "DOV",
            live: None,
        }),
        kernel: Kernel::Pointwise(concat!(
            wgsl_3d!(),
            r#"
struct Txt {
    // World to object space, and the object's position.
    m: mat3x3<f32>,
    t: vec3<f32>,
    // Line height and extrusion depth in world units.
    size: f32,
    half_depth: f32,
    bevel: f32,
}

fn to_text(o: Txt, w: vec3<f32>) -> vec3<f32> {
    return o.m * (w - o.t);
}

// The 2D text pushed out along z, with its edges rounded by `bevel`.
fn txt_sdf(o: Txt, w: vec3<f32>) -> f32 {
    let p = to_text(o, w);
    let d2 = text_sdf(p.xy / o.size) * o.size;
    let e = vec2<f32>(d2 + o.bevel, abs(p.z) - max(o.half_depth - o.bevel, 0.0));
    return min(max(e.x, e.y), 0.0) + length(max(e, vec2<f32>(0.0))) - o.bevel;
}

fn txt_normal(o: Txt, w: vec3<f32>, t: f32) -> vec3<f32> {
    let e = vec2<f32>(1.0, -1.0) * 0.0008 * max(t, 1.0);
    return normalize(
        e.xyy * txt_sdf(o, w + e.xyy) + e.yyx * txt_sdf(o, w + e.yyx)
            + e.yxy * txt_sdf(o, w + e.yxy) + e.xxx * txt_sdf(o, w + e.xxx),
    );
}

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var obj: Txt;
    let spin = vec3<f32>(u.extra[0].x, u.extra[0].y, 0.0);
    obj.m = transpose(rot_xyz(vec3<f32>(p.rot_x, p.rot_y, p.rot_z) + spin));
    obj.t = vec3<f32>(p.x * ctx.aspect, p.y, 0.0) * 1.25;
    obj.size = max(p.size, 0.01);
    obj.half_depth = max(p.extrude, 0.005) * obj.size * 0.5;
    obj.bevel = clamp(p.bevel, 0.0, 0.15) * obj.size;

    let dist = max(p.dist, 0.5);
    let ro = vec3<f32>(0.0, 0.0, -dist);
    let rd = camera_ray(ctx);
    let extent = length(vec3<f32>(text_size() * obj.size, obj.half_depth * 2.0));
    var t = 0.0;
    var hit = false;
    var closest = 1e9;
    var t_closest = dist;
    for (var i = 0; i < 200; i = i + 1) {
        let d = txt_sdf(obj, ro + rd * t);
        if (d / max(t, 1e-3) < closest) {
            closest = d / max(t, 1e-3);
            t_closest = t;
        }
        if (d < 0.0005 * t) {
            hit = true;
            break;
        }
        t = t + d * 0.8;
        if (t > dist + extent + 1.0) {
            break;
        }
    }
    var matte = 1.0;
    if (!hit) {
        matte = 1.0 - smoothstep(0.0, pixel_angle(), closest);
        t = t_closest;
    }
    var o: Outputs;
    if (matte <= 0.0) {
        o.out = vec3<f32>(0.0);
        o.depth = 0.0;
        o.matte = 0.0;
        return o;
    }
    let w = ro + rd * t;
    let n = txt_normal(obj, w, t);
    let op = to_text(obj, w);
    let depth = clamp(0.5 + (dist - t) / max(extent, 0.1), 0.0, 1.0);
    // Texture coordinates: the text block spans -1..1 on the faces.
    let half_size = max(text_size() * obj.size * 0.5, vec2<f32>(1e-3));
    let tp = vec3<f32>(op.xy / half_size, op.z / max(obj.half_depth, 1e-3));
    var color: vec3<f32>;
    switch p.mode {
        case 0u: {
            color = shade(n, rd, tex_box(tp, obj.m * n, ctx.stride));
        }
        case 1u: {
            color = n * 0.5 + vec3<f32>(0.5);
        }
        case 2u: {
            color = vec3<f32>(depth);
        }
        default: {
            // Contour lines through the letters, lit like the surface.
            let k = abs(fract(op.y * 8.0 / obj.size) - 0.5);
            color = shade(n, rd, vec3<f32>(1.0)) * (1.0 - smoothstep(0.04, 0.1, k));
        }
    }
    o.out = color * matte;
    o.depth = depth * matte;
    o.matte = matte;
    return o;
}
"#
        )),
    },
    behavior: Some(|| Box::new(Spin::default())),
    origin: Origin::BuiltIn,
};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_are_unique_and_namespaced() {
        let mut seen = HashSet::new();
        for m in builtins() {
            assert!(m.desc.id.starts_with("dov."), "{}", m.desc.id);
            assert!(seen.insert(m.desc.id), "duplicate {}", m.desc.id);
        }
    }

    #[test]
    fn port_and_param_ids_are_unique_per_module() {
        for m in builtins() {
            let d = &m.desc;
            let mut ins = HashSet::new();
            for p in d.inputs_full() {
                assert!(ins.insert(p.id.clone()), "{}: input {}", d.id, p.id);
            }
            let mut outs = HashSet::new();
            for p in d.outputs {
                assert!(outs.insert(p.id), "{}: output {}", d.id, p.id);
            }
            let mut params = HashSet::new();
            for p in d.params {
                assert!(params.insert(p.id), "{}: param {}", d.id, p.id);
            }
        }
    }

    #[test]
    fn text_limits_agree() {
        assert_eq!(dov_core::TEXT_MAX_CHARS, dov_format::MAX_TEXT_CHARS);
    }

    #[test]
    fn gpu_limits_hold() {
        // Default wgpu limits: 4 storage textures and 16 sampled textures per stage.
        for m in builtins() {
            assert!(m.desc.outputs.len() <= 4, "{}", m.desc.id);
            assert!(m.desc.input_count() <= 15, "{}", m.desc.id);
            assert!(m.desc.param_slots() <= 32, "{}", m.desc.id);
        }
    }
}
