//! The patch loaded at startup when none is given: two raster oscillators
//! folded and colorized, keyed through a pulsing circle over a feedback tunnel.

use dov_core::{ModuleId, PortAddr};

use crate::rack::Rack;

pub fn demo() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let vco_a = add("dov.vco", 0);
    let vco_b = add("dov.vco", 0);
    let mixer = add("dov.mixer", 0);
    let fold = add("dov.fold", 0);
    let colorize = add("dov.colorize", 0);
    let lfo = add("dov.lfo", 1);
    let shape = add("dov.shape", 1);
    let feedback = add("dov.feedback", 1);
    let keyer = add("dov.keyer", 1);
    let output = add("dov.output", 1);

    set(&rack, vco_b, "range", 1.0);
    set(&rack, vco_b, "multiple", 6.0);
    set(&rack, vco_b, "detune", 0.25);
    set(&rack, vco_b, "shape", 1.0);
    set(&rack, mixer, "lb", -1.0);
    set(&rack, lfo, "rate", 0.25);
    set(&rack, shape, "size", 0.28);
    set_cv(&rack, shape, "size", 0.12);
    set(&rack, feedback, "zoom", 1.03);
    set(&rack, feedback, "rotate", 1.5);
    set(&rack, feedback, "decay", 0.93);

    for (from, out, to, inp) in [
        (vco_a, "out", mixer, "a"),
        (vco_b, "out", mixer, "b"),
        (mixer, "out", fold, "sig"),
        (fold, "out", colorize, "sig"),
        (lfo, "out", shape, "size_cv"),
        (shape, "out", keyer, "key"),
        (colorize, "out", keyer, "fg"),
        (feedback, "out", keyer, "bg"),
        (keyer, "out", feedback, "sig"),
        (keyer, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// Camera outlines, colorized and smeared through feedback, with a little
/// per-line timebase wobble.
pub fn camera() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let camera = add("dov.camera", 0);
    let edge = add("dov.edge", 0);
    let colorize = add("dov.colorize", 0);
    let mixer = add("dov.mixer", 0);
    let noise = add("dov.noise", 1);
    let displace = add("dov.displace", 1);
    let feedback = add("dov.feedback", 1);
    let output = add("dov.output", 1);

    set(&rack, edge, "gain", 6.0);
    set(&rack, colorize, "spread", 2.5);
    set(&rack, mixer, "la", 0.4);
    set(&rack, mixer, "lb", 0.7);
    set(&rack, mixer, "lc", 0.6);
    set(&rack, noise, "mode", 1.0);
    set(&rack, displace, "h_amount", 0.006);
    set(&rack, feedback, "zoom", 1.012);
    set(&rack, feedback, "rotate", 0.6);
    set(&rack, feedback, "decay", 0.9);

    for (from, out, to, inp) in [
        (camera, "out", edge, "sig"),
        (edge, "out", colorize, "sig"),
        (camera, "out", mixer, "a"),
        (colorize, "out", mixer, "b"),
        (feedback, "out", mixer, "c"),
        (mixer, "out", feedback, "sig"),
        (mixer, "out", displace, "sig"),
        (noise, "out", displace, "h"),
        (displace, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// The feedback demo played by sound: the kick band pumps the circle and
/// pushes the tunnel forward, and the highs fold the oscillators harder.
pub fn audio() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let vco_a = add("dov.vco", 0);
    let vco_b = add("dov.vco", 0);
    let mixer = add("dov.mixer", 0);
    let fold = add("dov.fold", 0);
    let colorize = add("dov.colorize", 0);
    let kick = add("dov.audio", 1);
    let highs = add("dov.audio", 1);
    let shape = add("dov.shape", 1);
    let feedback = add("dov.feedback", 1);
    let keyer = add("dov.keyer", 1);
    let output = add("dov.output", 1);

    set(&rack, vco_b, "range", 1.0);
    set(&rack, vco_b, "multiple", 6.0);
    set(&rack, vco_b, "detune", 0.25);
    set(&rack, vco_b, "shape", 1.0);
    set(&rack, mixer, "lb", -1.0);
    set(&rack, kick, "filter", 1.0);
    set(&rack, kick, "freq", 120.0);
    set(&rack, kick, "release", 180.0);
    set(&rack, highs, "filter", 3.0);
    set(&rack, highs, "freq", 5000.0);
    set(&rack, highs, "release", 80.0);
    set(&rack, shape, "size", 0.18);
    set_cv(&rack, shape, "size", 0.4);
    set(&rack, feedback, "zoom", 1.015);
    set_cv(&rack, feedback, "zoom", 0.04);
    set(&rack, feedback, "rotate", 1.5);
    set(&rack, feedback, "decay", 0.93);
    set_cv(&rack, fold, "gain", 0.4);

    for (from, out, to, inp) in [
        (vco_a, "out", mixer, "a"),
        (vco_b, "out", mixer, "b"),
        (mixer, "out", fold, "sig"),
        (fold, "out", colorize, "sig"),
        (kick, "env", shape, "size_cv"),
        (kick, "env", feedback, "zoom_cv"),
        (highs, "env", fold, "gain_cv"),
        (shape, "out", keyer, "key"),
        (colorize, "out", keyer, "fg"),
        (feedback, "out", keyer, "bg"),
        (keyer, "out", feedback, "sig"),
        (keyer, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// The analog signal path: oscillator patterns encoded to composite, through
/// a VCR and a cable into a TV. A field-locked VCO waits on Inject; turn up
/// Macros A to push it into the sync and watch the TV lose lock.
pub fn analog() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let vco_a = add("dov.vco", 0);
    let vco_b = add("dov.vco", 0);
    let mixer = add("dov.mixer", 0);
    let fold = add("dov.fold", 0);
    let colorize = add("dov.colorize", 0);
    let encoder = add("dov.encoder", 0);
    let vcr = add("dov.vcr", 1);
    let cable = add("dov.cable", 1);
    let inject = add("dov.inject", 1);
    let tv = add("dov.tv", 1);
    let output = add("dov.output", 1);
    let wobble = add("dov.vco", 2);
    let macros = add("dov.macros", 2);

    set(&rack, vco_b, "range", 1.0);
    set(&rack, vco_b, "multiple", 6.0);
    set(&rack, vco_b, "detune", 0.25);
    set(&rack, vco_b, "shape", 1.0);
    set(&rack, mixer, "lb", -1.0);
    set(&rack, wobble, "range", 1.0);
    set(&rack, wobble, "multiple", 1.0);
    set(&rack, wobble, "detune", 0.5);
    set(&rack, inject, "amount", 0.0);
    set_cv(&rack, inject, "amount", 1.0);
    set(&rack, output, "display", 1.0);

    for (from, out, to, inp) in [
        (vco_a, "out", mixer, "a"),
        (vco_b, "out", mixer, "b"),
        (mixer, "out", fold, "sig"),
        (fold, "out", colorize, "sig"),
        (colorize, "out", encoder, "sig"),
        (encoder, "out", vcr, "sig"),
        (vcr, "out", cable, "sig"),
        (cable, "out", inject, "sig"),
        (inject, "out", tv, "sig"),
        (tv, "out", output, "sig"),
        (wobble, "out", inject, "mod"),
        (macros, "a", inject, "amount_cv"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// The shortest composite path: a pattern through an Encoder into a TV,
/// with the given composite stages (module type ids) in between. With none,
/// it's the reference for what encoding and decoding do on their own.
pub fn tv(stages: &[&str]) -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(0);
        rack.add_module(ty, 0, col)
    };
    let vco = add("dov.vco");
    let shape = add("dov.shape");
    let colorize = add("dov.colorize");
    let encoder = add("dov.encoder");
    let chain: Vec<_> = stages.iter().map(|kind| add(kind)).collect();
    let tv = add("dov.tv");
    let output = add("dov.output");
    set(&rack, vco, "multiple", 4.0);
    set(&rack, vco, "shape", 2.0);
    for (from, out, to, inp) in [
        (vco, "out", colorize, "sig"),
        (colorize, "out", encoder, "sig"),
        (tv, "out", output, "sig"),
        (shape, "out", colorize, "hue_cv"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    let mut prev = encoder;
    for &stage in chain.iter().chain([&tv]) {
        rack.connect(addr(prev, "out"), addr(stage, "sig"), None);
        prev = stage;
    }
    rack.clear_history();
    rack
}

/// Delays and keys. Drifting bars are slit-scanned (a vertical ramp on the
/// Delay's time, so each row is older than the one above) and bend; a disc
/// on a beat-locked Lissajous path leaves echo trails; a chroma key turns
/// the green parts of the bars into windows onto the trails.
pub fn time() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let vco = add("dov.vco", 0);
    let bar_color = add("dov.colorize", 0);
    let ramps = add("dov.ramps", 0);
    let slit = add("dov.delay", 0);
    let slit_view = add("dov.monitor", 0);
    let lfo_x = add("dov.lfo", 1);
    let lfo_y = add("dov.lfo", 1);
    let shape = add("dov.shape", 1);
    let disc_color = add("dov.colorize", 1);
    let echo = add("dov.delay", 1);
    let echo_view = add("dov.monitor", 1);
    let key = add("dov.chromakey", 2);
    let output = add("dov.output", 2);

    set(&rack, vco, "multiple", 5.0);
    set(&rack, vco, "detune", 0.6);
    set(&rack, vco, "shape", 2.0);
    set(&rack, slit, "time", 0.0);
    set_cv(&rack, slit, "time", 0.5);
    for (lfo, div, phase) in [(lfo_x, 2.0, 0.0), (lfo_y, 3.0, 0.25)] {
        set(&rack, lfo, "mode", 1.0);
        set(&rack, lfo, "bpm", 120.0);
        set(&rack, lfo, "div", div);
        set(&rack, lfo, "amount", 0.7);
        set(&rack, lfo, "offset", -0.35);
        set(&rack, lfo, "phase", phase);
    }
    set(&rack, shape, "size", 0.12);
    set_cv(&rack, shape, "x", 1.0);
    set_cv(&rack, shape, "y", 1.0);
    set(&rack, disc_color, "hue", 0.6);
    set(&rack, disc_color, "spread", 0.3);
    set(&rack, disc_color, "sat", 1.4);
    set(&rack, echo, "time", 4.0);
    set(&rack, echo, "feedback", 0.8);
    set(&rack, key, "width", 0.1);

    for (from, out, to, inp) in [
        (vco, "out", bar_color, "sig"),
        (bar_color, "out", slit, "sig"),
        (ramps, "v", slit, "time_cv"),
        (slit, "out", slit_view, "sig"),
        (lfo_x, "out", shape, "x_cv"),
        (lfo_y, "out", shape, "y_cv"),
        (shape, "out", disc_color, "sig"),
        (disc_color, "out", echo, "sig"),
        (echo, "out", echo_view, "sig"),
        (slit, "out", key, "fg"),
        (echo, "out", key, "bg"),
        (key, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// Spinning 3D text, painted with VCO stripes, keyed over swirling feedback
/// trails of itself. The text's matte (how much of the screen it covers)
/// sets the stripe count through a control jack, and a radial ramp patched
/// into Feedback's rotation makes the swirl. Banded type along the bottom
/// feeds the trails too.
pub fn lab() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    let vco = add("dov.vco", 0);
    let stripes = add("dov.colorize", 0);
    let text3d = add("dov.text3d", 0);
    let ramps = add("dov.ramps", 1);
    let banner = add("dov.text", 1);
    let mixer = add("dov.mixer", 1);
    let feedback = add("dov.feedback", 1);
    let keyer = add("dov.keyer", 1);
    let output = add("dov.output", 1);

    set(&rack, vco, "range", 1.0);
    set(&rack, vco, "multiple", 10.0);
    set_cv(&rack, vco, "multiple", 1.0);
    set(&rack, stripes, "spread", 2.0);
    set(&rack, text3d, "rot_x", -15.0);
    set(&rack, text3d, "spin_y", 35.0);
    set(&rack, text3d, "bevel", 0.05);
    set(&rack, banner, "font", 2.0);
    set(&rack, banner, "look", 3.0);
    set(&rack, banner, "size", 0.09);
    set(&rack, banner, "y", 0.88);
    set(&rack, banner, "line", 0.02);
    set(&rack, mixer, "la", 0.5);
    set(&rack, mixer, "lc", 0.25);
    set(&rack, feedback, "zoom", 0.985);
    set(&rack, feedback, "rotate", 0.0);
    set_cv(&rack, feedback, "rotate", 0.25);
    set(&rack, feedback, "decay", 0.9);
    set(&rack, output, "display", 1.0);

    for (from, out, to, inp) in [
        (vco, "out", stripes, "sig"),
        (stripes, "out", text3d, "tex"),
        (text3d, "matte", vco, "multiple_cv"),
        (text3d, "out", mixer, "a"),
        (feedback, "out", mixer, "b"),
        (banner, "out", mixer, "c"),
        (mixer, "out", feedback, "sig"),
        (ramps, "radial", feedback, "rotate_cv"),
        (text3d, "matte", keyer, "key"),
        (text3d, "out", keyer, "fg"),
        (feedback, "out", keyer, "bg"),
        (keyer, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// The showcase, and the patch the app opens with. Spinning 3D letters
/// painted with an oscillator's color bars swirl through feedback over a
/// contour-lit torus, with a banner keyed on top. Then all of it goes
/// through the analog chain: encoded to composite, played through a VCR
/// whose tracking drifts with an LFO, and shown by a TV on a CRT. The
/// letters' own matte sets how many bars paint them, through a control jack.
pub fn showcase() -> Rack {
    let mut rack = Rack::default();
    let mut add = |kind: &str, row: i32| {
        let ty = dov_modules::find(kind).unwrap_or_else(|| panic!("missing module {kind}"));
        let col = rack.row_end(row);
        rack.add_module(ty, row, col)
    };
    // Picture.
    let bars = add("dov.vco", 0);
    let paint = add("dov.colorize", 0);
    let letters = add("dov.text3d", 0);
    let torus = add("dov.object", 0);
    let torus_color = add("dov.colorize", 0);
    // Space and time.
    let ramps = add("dov.ramps", 1);
    let mixer = add("dov.mixer", 1);
    let drift = add("dov.lfo", 1);
    let rainbow = add("dov.colorize", 1);
    let feedback = add("dov.feedback", 1);
    let over = add("dov.keyer", 1);
    let banner = add("dov.text", 1);
    let banner_color = add("dov.colorize", 1);
    let titles = add("dov.keyer", 1);
    // The analog chain.
    let encoder = add("dov.encoder", 2);
    let wobble = add("dov.lfo", 2);
    let vcr = add("dov.vcr", 2);
    let tv = add("dov.tv", 2);
    let output = add("dov.output", 2);

    // Broad, smooth color bands: composite video carries color at a much
    // lower resolution than brightness, so fine stripes would come out of
    // the TV grey.
    set(&rack, bars, "range", 1.0);
    set(&rack, bars, "multiple", 3.0);
    set(&rack, bars, "detune", 0.35);
    set_cv(&rack, bars, "multiple", 0.6);
    set(&rack, paint, "hue", 0.6);
    set(&rack, paint, "spread", 1.1);
    set(&rack, paint, "sat", 1.7);
    if let Some(m) = rack.module(letters) {
        m.params.set_text("DOV");
    }
    set(&rack, letters, "size", 1.2);
    set(&rack, letters, "rot_x", -14.0);
    set(&rack, letters, "spin_y", 32.0);
    set(&rack, letters, "extrude", 0.55);
    set(&rack, letters, "bevel", 0.06);
    set(&rack, torus, "shape", 2.0);
    set(&rack, torus, "mode", 3.0);
    set(&rack, torus, "size", 1.6);
    set(&rack, torus, "spin_x", 11.0);
    set(&rack, torus, "spin_y", -17.0);
    set(&rack, torus, "dist", 5.0);
    set(&rack, torus_color, "hue", 0.52);
    set(&rack, torus_color, "spread", 0.5);
    set(&rack, torus_color, "sat", 1.6);

    set(&rack, mixer, "la", 0.55);
    set(&rack, mixer, "lb", 0.92);
    set(&rack, mixer, "lc", 0.35);
    // The trails run through a colorizer whose hue sweeps once every 14 s.
    set(&rack, drift, "rate", 0.07);
    set(&rack, drift, "shape", 2.0);
    set(&rack, rainbow, "spread", 2.4);
    set(&rack, rainbow, "sat", 1.5);
    set_cv(&rack, rainbow, "hue", 1.0);
    set(&rack, feedback, "zoom", 0.975);
    set(&rack, feedback, "rotate", 0.0);
    set_cv(&rack, feedback, "rotate", 0.3);
    set(&rack, feedback, "decay", 0.9);
    if let Some(m) = rack.module(banner) {
        m.params.set_text("DESTRUCT-O-VISION");
    }
    set(&rack, banner, "font", 2.0);
    set(&rack, banner, "size", 0.085);
    set(&rack, banner, "y", 0.87);
    set(&rack, banner_color, "hue", 0.92);
    set(&rack, banner_color, "spread", 0.15);
    set(&rack, banner_color, "sat", 1.2);

    set(&rack, wobble, "mode", 1.0);
    set(&rack, wobble, "div", 2.0);
    set(&rack, wobble, "shape", 5.0);
    set(&rack, wobble, "amount", 0.5);
    set(&rack, wobble, "offset", -0.25);
    set(&rack, vcr, "tracking", 0.93);
    set_cv(&rack, vcr, "tracking", 0.3);
    set(&rack, vcr, "wow", 0.3);
    set(&rack, vcr, "jitter", 0.35);
    set(&rack, vcr, "dropouts", 0.3);
    set(&rack, encoder, "chroma", 1.2);
    set(&rack, tv, "saturation", 1.4);
    set(&rack, output, "display", 1.0);
    set(&rack, output, "glow", 0.45);
    set(&rack, output, "curvature", 0.5);

    for (from, out, to, inp) in [
        (bars, "out", paint, "sig"),
        (paint, "out", letters, "tex"),
        (letters, "matte", bars, "multiple_cv"),
        (letters, "out", mixer, "a"),
        (feedback, "out", mixer, "b"),
        (torus, "out", torus_color, "sig"),
        (torus_color, "out", mixer, "c"),
        (mixer, "out", rainbow, "sig"),
        (drift, "out", rainbow, "hue_cv"),
        (rainbow, "out", feedback, "sig"),
        (ramps, "radial", feedback, "rotate_cv"),
        (letters, "matte", over, "key"),
        (letters, "out", over, "fg"),
        (rainbow, "out", over, "bg"),
        (banner, "out", banner_color, "sig"),
        (banner, "out", titles, "key"),
        (banner_color, "out", titles, "fg"),
        (over, "out", titles, "bg"),
        (titles, "out", encoder, "sig"),
        (encoder, "out", vcr, "sig"),
        (wobble, "out", vcr, "tracking_cv"),
        (vcr, "out", tv, "sig"),
        (tv, "out", output, "sig"),
    ] {
        rack.connect(addr(from, out), addr(to, inp), None);
    }
    rack.clear_history();
    rack
}

/// An empty rack with just an Output module.
pub fn empty() -> Rack {
    let mut rack = Rack::default();
    if let Some(ty) = dov_modules::find("dov.output") {
        rack.add_module(ty, 0, 0);
    }
    rack.clear_history();
    rack
}

fn addr(module: ModuleId, port: &str) -> PortAddr {
    PortAddr {
        module,
        port: port.into(),
    }
}

fn set(rack: &Rack, id: ModuleId, param: &str, value: f32) {
    let m = rack.module(id).expect("module");
    let slot =
        m.ty.desc
            .param_index(param)
            .unwrap_or_else(|| panic!("{}: no param {param}", m.ty.desc.id));
    m.params.set(slot, value);
}

fn set_cv(rack: &Rack, id: ModuleId, param: &str, amount: f32) {
    let m = rack.module(id).expect("module");
    let desc = &m.ty.desc;
    let (_, atten) = desc
        .param_index(param)
        .and_then(|i| desc.cv_slot(i))
        .expect("param with CV");
    m.params.set(atten, amount);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_patch_is_fully_wired() {
        let rack = demo();
        let snap = rack.snapshot();
        assert_eq!(snap.cables.len(), rack.cables.len());
        let schedule = dov_engine::graph::schedule(&snap);
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert!(schedule.output.is_some());
        assert!(schedule.delayed_outputs.is_empty(), "feedback closes the loop");
    }

    #[test]
    fn time_patch_is_fully_wired() {
        let rack = time();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
    }

    #[test]
    fn analog_patch_is_fully_wired() {
        let rack = analog();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
    }

    #[test]
    fn showcase_patch_is_fully_wired() {
        let rack = showcase();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
        assert!(schedule.output.is_some());
    }

    #[test]
    fn lab_patch_is_fully_wired() {
        let rack = lab();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
        assert_eq!(schedule.sampled_outputs.len(), 1, "the matte drives the VCO");
    }

    #[test]
    fn audio_patch_is_fully_wired() {
        let rack = audio();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
        assert_eq!(schedule.control.len(), 2, "both followers run on the CPU");
    }

    #[test]
    fn camera_patch_is_fully_wired() {
        let rack = camera();
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(rack.snapshot().cables.len(), rack.cables.len());
    }
}
