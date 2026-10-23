//! The global beat clock every tempo-synced module follows: the patch's own
//! tempo, an Ableton Link session, or MIDI clock from Live or any other
//! device. Beats are counted, not computed from the time, so changing the
//! tempo carries on from the beat it's on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dov_io::midi::CLOCK_TICKS;
use dov_io::{ClockReading, LinkSession, MidiSource};

/// Where the tempo comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TempoSync {
    /// The patch's own tempo.
    #[default]
    Internal,
    /// An Ableton Link session: Live, and any Link app nearby.
    Link,
    /// MIDI clock from any device.
    MidiClock,
}

/// The beat clock, for the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TempoStatus {
    pub bpm: f64,
    /// Quarter-note beats counted, at the last field.
    pub beats: f64,
    pub sync: TempoSync,
    /// Other apps in the Link session.
    pub peers: u64,
    /// MIDI clock is coming in.
    pub receiving: bool,
}

/// The beat clock at the start of a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    /// Quarter-note beats; bars are four.
    pub beats: f64,
    pub bpm: f64,
}

/// MIDI clock further off than this snaps the beat into place (a start, a
/// jump in the song); nearer, the beat is pulled in gently.
const SNAP_BEATS: f64 = 0.25;
/// Share of the phase error taken up each field.
const PULL: f64 = 0.1;
/// MIDI clock silent this long has stopped; the beat runs on at its tempo.
const CLOCK_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) struct Transport {
    sync: TempoSync,
    /// The beat here: `beats` at `at`, going on at `bpm`.
    at: Instant,
    beats: f64,
    bpm: f64,
    link: Option<LinkSession>,
    midi: Option<Arc<MidiSource>>,
    status: TempoStatus,
}

/// `b - a` in seconds, negative when `b` comes first.
fn seconds(a: Instant, b: Instant) -> f64 {
    if b >= a {
        (b - a).as_secs_f64()
    } else {
        -(a - b).as_secs_f64()
    }
}

impl Transport {
    pub(crate) fn new(bpm: f64, now: Instant) -> Self {
        Self {
            sync: TempoSync::Internal,
            at: now,
            beats: 0.0,
            bpm,
            link: None,
            midi: None,
            status: TempoStatus {
                bpm,
                ..Default::default()
            },
        }
    }

    /// The beat at `t` on the clock here.
    fn own(&self, t: Instant) -> f64 {
        self.beats + seconds(self.at, t) * self.bpm / 60.0
    }

    fn anchor(&mut self, t: Instant, beats: f64, bpm: f64) {
        (self.at, self.beats, self.bpm) = (t, beats, bpm);
    }

    /// Changes the tempo from the beat it's on; in a Link session, for
    /// everyone in it. MIDI clock sets its own.
    pub(crate) fn set_tempo(&mut self, bpm: f64, now: Instant) {
        let bpm = bpm.clamp(20.0, 999.0);
        self.anchor(now, self.own(now), bpm);
        if let Some(link) = &mut self.link {
            link.set_tempo(bpm);
        }
    }

    pub(crate) fn set_sync(&mut self, sync: TempoSync, now: Instant) {
        if sync == self.sync {
            return;
        }
        self.anchor(now, self.own(now), self.bpm);
        self.link = (sync == TempoSync::Link).then(|| LinkSession::join(self.bpm));
        self.midi = (sync == TempoSync::MidiClock).then(MidiSource::open);
        self.sync = sync;
        self.status.peers = 0;
        self.status.receiving = false;
    }

    /// The beat at `t`. `offline` (recording, faster than real time) runs
    /// on the clock here at the last tempo: nothing outside can keep up.
    pub(crate) fn at(&mut self, t: Instant, offline: bool) -> Beat {
        if !offline {
            match self.sync {
                TempoSync::Internal => {}
                TempoSync::Link => {
                    if let Some(link) = &mut self.link {
                        let (beats, bpm) = link.at(t);
                        self.status.peers = link.peers();
                        self.anchor(t, beats, bpm);
                    }
                }
                TempoSync::MidiClock => {
                    let reading = self.midi.as_ref().and_then(|m| m.clock());
                    self.follow_midi(t, reading, Instant::now());
                }
            }
        }
        let beat = Beat {
            beats: self.own(t),
            bpm: self.bpm,
        };
        self.status.bpm = beat.bpm;
        self.status.beats = beat.beats;
        self.status.sync = self.sync;
        beat
    }

    /// Follows MIDI clock while it's playing: snaps to it after a start or
    /// a jump, and otherwise pulls in gently, so the beat moves smoothly
    /// though the ticks come unevenly.
    fn follow_midi(&mut self, t: Instant, reading: Option<ClockReading>, now: Instant) {
        let Some(r) = reading.filter(|r| now.saturating_duration_since(r.at) < CLOCK_TIMEOUT) else {
            self.status.receiving = false;
            return;
        };
        self.status.receiving = true;
        if !r.running {
            return;
        }
        // On from the last tick at the clock's tempo, but not past the next.
        let since = (seconds(r.at, t) / r.period).min(1.0);
        let clock = (r.ticks as f64 + since) / CLOCK_TICKS as f64;
        let own = self.own(t);
        let error = clock - own;
        let beats = if error.abs() > SNAP_BEATS {
            clock
        } else {
            own + error * PULL
        };
        self.anchor(t, beats, r.bpm());
    }

    pub(crate) fn status(&self) -> TempoStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MIDI clock at 100 BPM whose ticks come up to 1.5 ms early or late,
    /// read once a field: the beat follows smoothly and stays on it.
    #[test]
    fn the_beat_follows_midi_clock_through_jitter() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut transport = Transport::new(120.0, t0);
        transport.sync = TempoSync::MidiClock;
        let tick = 60.0 / 100.0 / CLOCK_TICKS as f64;
        let jitter = |n: i64| ((n * 7919) % 31 - 15) as f64 * 1.0e-4;
        let field = 1.0 / 59.94;
        let mut last_beats: Option<f64> = None;
        let mut worst: f64 = 0.0;
        for f in 0..600 {
            let t = f as f64 * field;
            // Ticks heard so far; the clock started at 0.
            let heard = (t / tick).floor() as i64;
            let times: Vec<f64> = ((heard - 48).max(0)..=heard)
                .map(|n| n as f64 * tick + jitter(n))
                .collect();
            let reading = (times.len() >= 3).then(|| ClockReading {
                ticks: heard,
                at: at(*times.last().unwrap()),
                period: (times.last().unwrap() - times[0]) / (times.len() - 1) as f64,
                running: true,
            });
            transport.follow_midi(at(t), reading, at(t));
            let beat = transport.at(at(t), true);
            if f > 120 {
                worst = worst.max((beat.beats - t * 100.0 / 60.0).abs());
                assert!((beat.bpm - 100.0).abs() < 0.5, "{beat:?}");
                assert!(
                    beat.beats >= last_beats.unwrap(),
                    "the beat went backward at field {f}"
                );
            }
            last_beats = Some(beat.beats);
        }
        // A field at 100 BPM is 0.028 beats.
        assert!(worst < 0.01, "{worst} beats off");
    }

    #[test]
    fn a_tempo_change_carries_on_from_the_beat() {
        let t0 = Instant::now();
        let mut transport = Transport::new(120.0, t0);
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        assert!((transport.at(at(1.0), false).beats - 2.0).abs() < 1e-9);
        transport.set_tempo(60.0, at(1.0));
        let beat = transport.at(at(3.0), false);
        assert!((beat.beats - 4.0).abs() < 1e-9, "{beat:?}");
        assert_eq!(beat.bpm, 60.0);
    }
}
