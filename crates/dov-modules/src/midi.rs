//! MIDI to control: notes as pitch, gate, velocity and trigger, and
//! controllers as knobs.

use std::sync::Arc;

use dov_io::{MidiMessage, MidiSource, MidiStatus};

use crate::{Behavior, FieldCtx, FieldIo};

/// Choices of the Channel switch: any, then 1–16.
pub(crate) static CHANNELS: [&str; 17] = [
    "Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
];

/// MIDI input with this module's own place in it.
#[derive(Default)]
struct Input {
    source: Option<Arc<MidiSource>>,
    cursor: Option<u64>,
}

impl Input {
    /// The messages received since the last field on `channel` (a
    /// `CHANNELS` index; 0 takes every channel).
    fn read(&mut self, channel: u32) -> Vec<MidiMessage> {
        let source = self.source.get_or_insert_with(MidiSource::open);
        let mut messages = Vec::new();
        source.read(&mut self.cursor, |m| {
            if on_channel(m, channel) {
                messages.push(m);
            }
        });
        messages
    }

    fn warning(&self) -> Option<String> {
        match self.source.as_ref()?.status() {
            MidiStatus::Failed(e) => Some(format!("MIDI: {e}")),
            MidiStatus::Running { ports } if ports.is_empty() => Some("MIDI: no devices connected".into()),
            MidiStatus::Running { .. } | MidiStatus::Starting => None,
        }
    }
}

fn on_channel(m: MidiMessage, channel: u32) -> bool {
    channel == 0 || u32::from(m.channel()) + 1 == channel
}

/// Moves `value` towards `target` with time constant `ms`, over `dt` seconds.
fn glide(value: f32, target: f32, ms: f32, dt: f32) -> f32 {
    if ms <= 0.0 {
        return target;
    }
    value + (target - value) * (1.0 - (-dt / (ms * 1e-3)).exp())
}

// MIDI Notes params, in the order `library::MIDI_NOTES` declares them.
const NOTES_CHANNEL: usize = 0;
const LOW: usize = 1;
const HIGH: usize = 2;
const BEND: usize = 3;
const GLIDE: usize = 4;

/// MIDI Notes: the last note held wins.
#[derive(Default)]
pub(crate) struct MidiNotes {
    input: Input,
    /// Notes held down, oldest first.
    held: Vec<u8>,
    /// The note sounding, or the last one played.
    note: Option<u8>,
    velocity: f32,
    /// Pitch wheel, -1..1.
    bend: f32,
    pitch: Option<f32>,
}

impl MidiNotes {
    /// One field: the messages received during it, then the outputs.
    fn update(&mut self, io: &mut FieldIo, messages: &[MidiMessage], dt: f32) {
        let note = |i| io.param(i).round().clamp(0.0, 127.0) as u8;
        let (low, high) = (note(LOW).min(note(HIGH)), note(LOW).max(note(HIGH)));
        let mut struck = false;
        for &m in messages {
            match m {
                MidiMessage::NoteOn { note, velocity, .. } if (low..=high).contains(&note) => {
                    self.held.retain(|&n| n != note);
                    self.held.push(note);
                    self.note = Some(note);
                    self.velocity = f32::from(velocity) / 127.0;
                    struck = true;
                }
                MidiMessage::NoteOff { note, .. } => {
                    self.held.retain(|&n| n != note);
                    if let Some(&n) = self.held.last() {
                        self.note = Some(n);
                    }
                }
                // All sound off, all notes off.
                MidiMessage::Control {
                    controller: 120 | 123,
                    ..
                } => self.held.clear(),
                MidiMessage::PitchBend { value, .. } => self.bend = f32::from(value) / 8192.0,
                MidiMessage::NoteOn { .. } | MidiMessage::Control { .. } => {}
            }
        }

        let span = f32::from((high - low).max(1));
        let target = self
            .note
            .map(|n| (f32::from(n) - f32::from(low) + self.bend * io.param(BEND)) / span);
        self.pitch = match (self.pitch, target) {
            (Some(p), Some(t)) => Some(glide(p, t, io.param(GLIDE), dt)),
            (p, t) => t.or(p),
        };
        io.outputs[0] = self.pitch.unwrap_or(0.0);
        // A note struck and let go within one field still opens the gate.
        io.outputs[1] = f32::from(u8::from(struck || !self.held.is_empty()));
        io.outputs[2] = self.velocity;
        io.outputs[3] = f32::from(u8::from(struck));
    }
}

impl Behavior for MidiNotes {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let messages = self.input.read(io.choice(NOTES_CHANNEL));
        self.update(io, &messages, ctx.elapsed_seconds() as f32);
    }

    fn warning(&self) -> Option<String> {
        self.input.warning()
    }
}

// MIDI CC params, in the order `library::MIDI_CC` declares them.
const CC_CHANNEL: usize = 0;
const LEARN: usize = 1;
const CC: [usize; 4] = [2, 3, 4, 5];
const SMOOTH: usize = 6;

/// MIDI CC: four controllers as control outputs, each 0..1.
#[derive(Default)]
pub(crate) struct MidiCc {
    input: Input,
    values: [f32; 4],
    smoothed: [f32; 4],
}

impl MidiCc {
    /// One field: the messages received during it, then the outputs.
    /// While Learn names an output, the first controller to move is
    /// assigned to it, and Learn switches off.
    fn update(&mut self, io: &mut FieldIo, messages: &[MidiMessage], dt: f32) {
        let mut ccs = CC.map(|i| io.param(i).round().clamp(0.0, 127.0) as u8);
        let mut learn = (io.choice(LEARN) as usize).checked_sub(1);
        for &m in messages {
            let MidiMessage::Control {
                controller, value, ..
            } = m
            else {
                continue;
            };
            if let Some(k) = learn.take() {
                ccs[k] = controller;
                io.set_param(CC[k], f32::from(controller));
                io.set_param(LEARN, 0.0);
            }
            for (cc, v) in ccs.iter().zip(&mut self.values) {
                if *cc == controller {
                    *v = f32::from(value) / 127.0;
                }
            }
        }
        for k in 0..4 {
            self.smoothed[k] = glide(self.smoothed[k], self.values[k], io.param(SMOOTH), dt);
            io.outputs[k] = self.smoothed[k];
        }
    }
}

impl Behavior for MidiCc {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        let messages = self.input.read(io.choice(CC_CHANNEL));
        self.update(io, &messages, ctx.elapsed_seconds() as f32);
    }

    fn warning(&self) -> Option<String> {
        self.input.warning()
    }
}

#[cfg(test)]
mod tests {
    use dov_core::ParamBank;

    use super::*;
    use crate::{EXTRA_SLOTS, find};

    /// Params of a `kind` module, defaults but for `set`.
    fn bank(kind: &str, set: &[(&str, f32)]) -> ParamBank {
        let desc = &find(kind).unwrap().desc;
        let bank = ParamBank::new(&desc.default_slots());
        for (id, v) in set {
            bank.set(desc.param_index(id).unwrap(), *v);
        }
        bank
    }

    /// Runs `update` for one field of a `kind` module; returns its outputs.
    fn field(kind: &str, params: &ParamBank, update: impl FnOnce(&mut FieldIo)) -> Vec<f32> {
        let desc = &find(kind).unwrap().desc;
        let slots = params.snapshot();
        let inputs: Vec<f32> = desc.inputs_full().iter().map(|p| p.default).collect();
        let mut extra = [[0.0; 4]; EXTRA_SLOTS];
        let mut outputs = vec![0.0; desc.outputs.len()];
        update(&mut FieldIo {
            desc,
            slots: &slots,
            params,
            inputs: &inputs,
            extra: &mut extra,
            outputs: &mut outputs,
        });
        outputs
    }

    const FIELD: f32 = 1.0 / 60.0;

    fn notes(n: &mut MidiNotes, params: &ParamBank, messages: &[MidiMessage]) -> Vec<f32> {
        field("dov.midinotes", params, |io| n.update(io, messages, FIELD))
    }

    fn on(note: u8, velocity: u8) -> MidiMessage {
        MidiMessage::NoteOn {
            channel: 0,
            note,
            velocity,
        }
    }

    fn off(note: u8) -> MidiMessage {
        MidiMessage::NoteOff { channel: 0, note }
    }

    fn cc(controller: u8, value: u8) -> MidiMessage {
        MidiMessage::Control {
            channel: 0,
            controller,
            value,
        }
    }

    #[test]
    fn params_match_the_modules() {
        let notes = &find("dov.midinotes").unwrap().desc;
        for (i, id) in [
            (NOTES_CHANNEL, "channel"),
            (LOW, "low"),
            (HIGH, "high"),
            (BEND, "bend"),
            (GLIDE, "glide"),
        ] {
            assert_eq!(notes.params[i].id, id);
        }
        let cc = &find("dov.midicc").unwrap().desc;
        for (i, id) in [
            (CC_CHANNEL, "channel"),
            (LEARN, "learn"),
            (CC[0], "cc_a"),
            (CC[1], "cc_b"),
            (CC[2], "cc_c"),
            (CC[3], "cc_d"),
            (SMOOTH, "smooth"),
        ] {
            assert_eq!(cc.params[i].id, id);
        }
    }

    #[test]
    fn channels_filter_and_omni_takes_all() {
        let m = MidiMessage::NoteOff { channel: 9, note: 36 };
        assert!(on_channel(m, 0));
        assert!(on_channel(m, 10), "channel 10 is index 9");
        assert!(!on_channel(m, 1));
    }

    #[test]
    fn last_note_wins_and_falls_back_on_release() {
        let params = bank("dov.midinotes", &[("low", 48.0), ("high", 72.0)]);
        let mut n = MidiNotes::default();
        let out = notes(&mut n, &params, &[on(60, 127)]);
        assert_eq!(out, [0.5, 1.0, 1.0, 1.0], "pitch, gate, velocity, trig");
        let out = notes(&mut n, &params, &[on(72, 64)]);
        assert_eq!(out[0], 1.0);
        let out = notes(&mut n, &params, &[off(72)]);
        assert_eq!(out[..2], [0.5, 1.0], "back to the held note");
        assert_eq!(out[3], 0.0, "no new trigger");
        let out = notes(&mut n, &params, &[off(60)]);
        assert_eq!(out[..2], [0.5, 0.0], "pitch holds after the gate closes");
    }

    #[test]
    fn notes_outside_the_range_are_ignored() {
        let params = bank("dov.midinotes", &[("low", 48.0), ("high", 72.0)]);
        let mut n = MidiNotes::default();
        assert_eq!(notes(&mut n, &params, &[on(30, 100)]), [0.0; 4]);
    }

    #[test]
    fn a_quick_tap_still_gates_for_a_field() {
        let params = bank("dov.midinotes", &[]);
        let mut n = MidiNotes::default();
        let out = notes(&mut n, &params, &[on(64, 100), off(64)]);
        assert_eq!((out[1], out[3]), (1.0, 1.0));
        assert_eq!(notes(&mut n, &params, &[])[1], 0.0);
    }

    #[test]
    fn bend_moves_the_pitch_by_its_range() {
        let params = bank("dov.midinotes", &[("low", 60.0), ("high", 72.0), ("bend", 12.0)]);
        let mut n = MidiNotes::default();
        let bend = MidiMessage::PitchBend {
            channel: 0,
            value: 4096,
        };
        let out = notes(&mut n, &params, &[on(60, 100), bend]);
        assert!(
            (out[0] - 0.5).abs() < 1e-6,
            "half of a 12-semitone bend: {}",
            out[0]
        );
    }

    #[test]
    fn glide_eases_into_the_new_note() {
        let params = bank("dov.midinotes", &[("glide", 100.0)]);
        let mut n = MidiNotes::default();
        notes(&mut n, &params, &[on(0, 100)]);
        let p = notes(&mut n, &params, &[on(127, 100)])[0];
        let expected = 1.0 - (-FIELD / 0.1).exp();
        assert!((p - expected).abs() < 1e-5, "{p} vs {expected}");
    }

    #[test]
    fn cc_outputs_follow_their_controllers() {
        let params = bank("dov.midicc", &[("cc_a", 1.0), ("cc_c", 74.0), ("smooth", 0.0)]);
        let mut c = MidiCc::default();
        let out = field("dov.midicc", &params, |io| {
            c.update(io, &[cc(74, 127), cc(1, 0), cc(7, 100)], FIELD)
        });
        assert_eq!(out, [0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn cc_learn_takes_the_next_controller() {
        let params = bank("dov.midicc", &[("learn", 2.0), ("smooth", 0.0)]);
        let mut c = MidiCc::default();
        let out = field("dov.midicc", &params, |io| {
            c.update(io, &[cc(74, 127), cc(75, 127)], FIELD)
        });
        assert_eq!(out, [0.0, 1.0, 0.0, 0.0], "B took CC 74, and only that");
        assert_eq!(params.get(CC[1]), 74.0, "the knob shows it");
        assert_eq!(params.get(LEARN), 0.0, "learning stops");
    }
}
