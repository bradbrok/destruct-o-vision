//! Ableton Link: one tempo and beat shared with Live and any other Link app
//! on this Mac or the local network. Bars line up across the session.

use std::time::Instant;

use rusty_link::{AblLink, SessionState};

/// Beats in a bar: Link lines bars up across the session.
pub const QUANTUM: f64 = 4.0;

/// This app in a Link session, for as long as it's held.
pub struct LinkSession {
    link: AblLink,
    state: SessionState,
}

impl LinkSession {
    /// Joins (or starts) a session. A new session starts at `bpm`; joining
    /// one takes on its tempo.
    pub fn join(bpm: f64) -> Self {
        let link = AblLink::new(bpm);
        link.enable(true);
        Self {
            link,
            state: SessionState::new(),
        }
    }

    /// The session's beat and tempo at `t`.
    pub fn at(&mut self, t: Instant) -> (f64, f64) {
        self.link.capture_app_session_state(&mut self.state);
        let time = self.micros(t);
        (self.state.beat_at_time(time, QUANTUM), self.state.tempo())
    }

    /// Changes the whole session's tempo.
    pub fn set_tempo(&mut self, bpm: f64) {
        self.link.capture_app_session_state(&mut self.state);
        self.state.set_tempo(bpm, self.link.clock_micros());
        self.link.commit_app_session_state(&self.state);
    }

    /// Other apps in the session.
    pub fn peers(&self) -> u64 {
        self.link.num_peers()
    }

    /// `t` on Link's clock, in microseconds.
    fn micros(&self, t: Instant) -> i64 {
        let now = Instant::now();
        let offset = if t >= now {
            (t - now).as_micros() as i64
        } else {
            -((now - t).as_micros() as i64)
        };
        self.link.clock_micros() + offset
    }
}

impl Drop for LinkSession {
    fn drop(&mut self) {
        self.link.enable(false);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Two sessions on this Mac find each other: a tempo set in one reaches
    /// the other, on the same beat. Needs the local network, so run it by
    /// hand: `cargo test -p dov-io -- --ignored link`.
    #[test]
    #[ignore]
    fn link_sessions_share_tempo_and_beat() {
        let mut a = LinkSession::join(100.0);
        let mut b = LinkSession::join(140.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while (a.peers() == 0 || b.peers() == 0) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(a.peers() > 0 && b.peers() > 0, "they never met");
        a.set_tempo(123.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        // Link sends tempos as whole microseconds a beat.
        let near = |bpm: f64| (bpm - 123.0).abs() < 1.0e-3;
        while !near(b.at(Instant::now()).1) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let now = Instant::now();
        let (beat_a, tempo_a) = a.at(now);
        let (beat_b, tempo_b) = b.at(now);
        assert!(near(tempo_a) && near(tempo_b), "{tempo_a}, {tempo_b}");
        assert!((beat_a - beat_b).abs() < 0.01, "{beat_a} vs {beat_b}");
    }
}
