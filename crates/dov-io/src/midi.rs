//! MIDI input. A worker thread connects to every MIDI source, and to new
//! ones as they're plugged in. Messages land in a shared queue that any
//! number of readers drain at their own pace (each MIDI module, once per
//! field). All MIDI modules share the one connection to each device, and
//! the tempo follows MIDI clock through it too.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};

/// How often to look for devices that were plugged in or out.
const SCAN: Duration = Duration::from_millis(500);
/// Messages kept for readers; a reader further behind loses the oldest.
const QUEUE_LEN: usize = 1024;
const CLIENT_NAME: &str = "Destruct-O-Vision";
/// MIDI clock ticks a beat.
pub const CLOCK_TICKS: i64 = 24;
/// Ticks the tempo is measured over: two beats.
const CLOCK_HISTORY: usize = 2 * CLOCK_TICKS as usize + 1;
/// A pause this long between ticks starts the tempo measurement over.
const CLOCK_GAP: Duration = Duration::from_secs(1);

/// Where a MIDI clock was at its last tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockReading {
    /// Ticks from the start of the song at the last tick.
    pub ticks: i64,
    /// When the last tick came.
    pub at: Instant,
    /// Seconds between ticks, lately.
    pub period: f64,
    /// Started (or continued) and not stopped since.
    pub running: bool,
}

impl ClockReading {
    pub fn bpm(&self) -> f64 {
        60.0 / (self.period * CLOCK_TICKS as f64)
    }
}

/// Follows MIDI clock: ticks, start, stop, continue and song position.
#[derive(Default)]
struct Clock {
    /// Where the next tick lands, in ticks from the start of the song.
    next: i64,
    /// Where the last one landed.
    last: Option<i64>,
    /// When recent ticks came, oldest first.
    times: VecDeque<Instant>,
    running: bool,
    /// A start or continue has come: the song stops when it's stopped.
    started: bool,
}

impl Clock {
    fn message(&mut self, bytes: &[u8], now: Instant) {
        match *bytes {
            [0xf8, ..] => {
                if self
                    .times
                    .back()
                    .is_some_and(|&t| now.duration_since(t) > CLOCK_GAP)
                {
                    self.times.clear();
                }
                if self.times.len() == CLOCK_HISTORY {
                    self.times.pop_front();
                }
                self.times.push_back(now);
                // Stopped, the song stays where it is; a clock that never
                // started just runs.
                if self.running || !self.started {
                    self.last = Some(self.next);
                    self.next += 1;
                }
            }
            // Start: the next tick is the top of the song.
            [0xfa, ..] => {
                self.next = 0;
                self.running = true;
                self.started = true;
            }
            [0xfb, ..] => {
                self.running = true;
                self.started = true;
            }
            [0xfc, ..] => self.running = false,
            // Song position, in sixteenths: where the next tick lands.
            [0xf2, lsb, msb, ..] => self.next = ((i64::from(msb & 0x7f) << 7) | i64::from(lsb & 0x7f)) * 6,
            _ => {}
        }
    }

    fn reading(&self) -> Option<ClockReading> {
        let (&first, &at) = (self.times.front()?, self.times.back()?);
        let n = self.times.len();
        if n < 3 {
            return None;
        }
        Some(ClockReading {
            ticks: self.last?,
            at,
            period: (at - first).as_secs_f64() / (n - 1) as f64,
            running: self.running || !self.started,
        })
    }
}

/// A channel message. Channels count from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    Control {
        channel: u8,
        controller: u8,
        value: u8,
    },
    /// -8192..=8191, centered on 0.
    PitchBend {
        channel: u8,
        value: i16,
    },
}

impl MidiMessage {
    /// Parses one complete message; `None` for anything else (system
    /// messages, aftertouch, program changes).
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (&status, data) = bytes.split_first()?;
        let channel = status & 0x0f;
        let byte = |i: usize| data.get(i).copied().filter(|b| *b < 0x80);
        Some(match status & 0xf0 {
            0x80 => Self::NoteOff {
                channel,
                note: byte(0)?,
            },
            // A note on with velocity 0 is a note off.
            0x90 => match (byte(0)?, byte(1)?) {
                (note, 0) => Self::NoteOff { channel, note },
                (note, velocity) => Self::NoteOn {
                    channel,
                    note,
                    velocity,
                },
            },
            0xb0 => Self::Control {
                channel,
                controller: byte(0)?,
                value: byte(1)?,
            },
            0xe0 => Self::PitchBend {
                channel,
                value: (i16::from(byte(1)?) << 7 | i16::from(byte(0)?)) - 8192,
            },
            _ => return None,
        })
    }

    pub fn channel(&self) -> u8 {
        match *self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::Control { channel, .. }
            | Self::PitchBend { channel, .. } => channel,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MidiStatus {
    Starting,
    /// Listening to these devices (possibly none).
    Running {
        ports: Vec<String>,
    },
    Failed(String),
}

#[derive(Default)]
struct Queue {
    messages: VecDeque<MidiMessage>,
    /// Sequence number of `messages[0]`.
    first: u64,
}

struct Shared {
    queue: Mutex<Queue>,
    status: Mutex<MidiStatus>,
    clock: Mutex<Clock>,
}

impl Shared {
    fn push(&self, m: MidiMessage) {
        let Ok(mut q) = self.queue.lock() else { return };
        if q.messages.len() == QUEUE_LEN {
            q.messages.pop_front();
            q.first += 1;
        }
        q.messages.push_back(m);
    }

    fn set_status(&self, status: MidiStatus) {
        let Ok(mut s) = self.status.lock() else { return };
        if *s == status {
            return;
        }
        match &status {
            MidiStatus::Running { ports } if ports.is_empty() => log::info!("midi: no devices"),
            MidiStatus::Running { ports } => log::info!("midi: {}", ports.join(", ")),
            MidiStatus::Failed(e) => log::warn!("midi: {e}"),
            MidiStatus::Starting => {}
        }
        *s = status;
    }
}

/// MIDI input from every connected device. Dropping the last handle
/// closes the connections.
pub struct MidiSource {
    shared: Arc<Shared>,
    stop: mpsc::Sender<()>,
}

impl MidiSource {
    /// The running MIDI input, started if nothing is using it yet.
    pub fn open() -> Arc<Self> {
        static OPEN: Mutex<Weak<MidiSource>> = Mutex::new(Weak::new());
        let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(source) = open.upgrade() {
            return source;
        }
        let source = Arc::new(Self::start());
        *open = Arc::downgrade(&source);
        source
    }

    fn start() -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::default(),
            status: Mutex::new(MidiStatus::Starting),
            clock: Mutex::default(),
        });
        let (stop, rx) = mpsc::channel();
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("dov-midi".into())
            .spawn(move || run(&worker, &rx));
        if let Err(e) = spawned {
            shared.set_status(MidiStatus::Failed(e.to_string()));
        }
        Self { shared, stop }
    }

    pub fn status(&self) -> MidiStatus {
        self.shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(MidiStatus::Starting)
    }

    /// Where MIDI clock from any device was at its last tick, once a few
    /// ticks have come.
    pub fn clock(&self) -> Option<ClockReading> {
        self.shared.clock.lock().ok()?.reading()
    }

    /// Calls `f` with each message received since `cursor` and moves
    /// `cursor` past them. A new cursor (`None`) starts now.
    pub fn read(&self, cursor: &mut Option<u64>, mut f: impl FnMut(MidiMessage)) {
        let Ok(q) = self.shared.queue.lock() else { return };
        let end = q.first + q.messages.len() as u64;
        let start = cursor.unwrap_or(end).max(q.first);
        *cursor = Some(end);
        for m in q.messages.range((start - q.first) as usize..) {
            f(*m);
        }
    }
}

impl Drop for MidiSource {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

/// An open port: its id, its name, and the connection that keeps it open.
type Connection = (String, String, MidiInputConnection<()>);

fn run(shared: &Arc<Shared>, stop: &mpsc::Receiver<()>) {
    let scanner = match MidiInput::new(CLIENT_NAME) {
        Ok(m) => m,
        Err(e) => {
            shared.set_status(MidiStatus::Failed(e.to_string()));
            return;
        }
    };
    let mut open: Vec<Connection> = Vec::new();
    // Ports that wouldn't open; tried again once they've gone and come back.
    let mut failed: HashSet<String> = HashSet::new();
    loop {
        let ports = scanner.ports();
        let ids: HashSet<String> = ports.iter().map(MidiInputPort::id).collect();
        open.retain(|(id, ..)| ids.contains(id));
        failed.retain(|id| ids.contains(id));
        for port in &ports {
            let id = port.id();
            if failed.contains(&id) || open.iter().any(|(open_id, ..)| *open_id == id) {
                continue;
            }
            match connect(shared, &scanner, port) {
                Ok(c) => open.push(c),
                Err(e) => {
                    log::warn!("midi: {e}");
                    failed.insert(id);
                }
            }
        }
        let ports = open.iter().map(|(_, name, _)| name.clone()).collect();
        shared.set_status(MidiStatus::Running { ports });
        match stop.recv_timeout(SCAN) {
            Err(RecvTimeoutError::Timeout) => platform::pump(),
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn connect(shared: &Arc<Shared>, scanner: &MidiInput, port: &MidiInputPort) -> Result<Connection, String> {
    let name = scanner.port_name(port).map_err(|e| e.to_string())?;
    let mut input = MidiInput::new(CLIENT_NAME).map_err(|e| e.to_string())?;
    // Clock and song position come through, for the tempo.
    input.ignore(Ignore::SysexAndActiveSense);
    let shared = shared.clone();
    let connection = input
        .connect(
            port,
            "dov-in",
            move |_, bytes, _| match bytes.first() {
                Some(0xf2 | 0xf8 | 0xfa | 0xfb | 0xfc) => {
                    if let Ok(mut clock) = shared.clock.lock() {
                        clock.message(bytes, Instant::now());
                    }
                }
                _ => {
                    if let Some(m) = MidiMessage::parse(bytes) {
                        shared.push(m);
                    }
                }
            },
            (),
        )
        .map_err(|e| format!("{name}: {e}"))?;
    Ok((port.id(), name, connection))
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};

    /// CoreMIDI delivers its notifications on the run loop of the thread
    /// that made the process's first client: this one. Give it a turn, so
    /// nothing waits on it to see devices plugged in later.
    pub(super) fn pump() {
        // SAFETY: a framework constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        CFRunLoop::run_in_mode(mode, 0.0, false);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn pump() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_follows_tempo_start_and_song_position() {
        let mut clock = Clock::default();
        let t0 = Instant::now();
        // 120 BPM: a tick every 1/48 s.
        let tick = Duration::from_secs_f64(0.5 / 24.0);
        clock.message(&[0xfa], t0);
        for i in 0..30 {
            clock.message(&[0xf8], t0 + tick * i);
        }
        let r = clock.reading().unwrap();
        assert_eq!(r.ticks, 29, "the first tick after Start is the top");
        assert!((r.bpm() - 120.0).abs() < 1e-4, "{}", r.bpm());
        assert!(r.running);
        // Stop, then continue from bar 3 (32 sixteenths in).
        clock.message(&[0xfc], t0 + tick * 30);
        assert!(!clock.reading().unwrap().running);
        clock.message(&[0xf2, 32, 0], t0 + tick * 31);
        clock.message(&[0xfb], t0 + tick * 31);
        clock.message(&[0xf8], t0 + tick * 32);
        let r = clock.reading().unwrap();
        assert_eq!(r.ticks, 32 * 6);
        assert!(r.running);
    }

    #[test]
    fn parses_channel_messages() {
        use MidiMessage::*;
        let cases: [(&[u8], Option<MidiMessage>); 8] = [
            (
                &[0x93, 60, 100],
                Some(NoteOn {
                    channel: 3,
                    note: 60,
                    velocity: 100,
                }),
            ),
            (&[0x90, 60, 0], Some(NoteOff { channel: 0, note: 60 })),
            (
                &[0x8f, 61, 64],
                Some(NoteOff {
                    channel: 15,
                    note: 61,
                }),
            ),
            (
                &[0xb1, 74, 127],
                Some(Control {
                    channel: 1,
                    controller: 74,
                    value: 127,
                }),
            ),
            (&[0xe0, 0x00, 0x40], Some(PitchBend { channel: 0, value: 0 })),
            (
                &[0xe0, 0x7f, 0x7f],
                Some(PitchBend {
                    channel: 0,
                    value: 8191,
                }),
            ),
            (&[0xf8], None),
            (&[0x90, 60], None),
        ];
        for (bytes, expected) in cases {
            assert_eq!(MidiMessage::parse(bytes), expected, "{bytes:02x?}");
        }
    }

    fn source() -> MidiSource {
        let (stop, _) = mpsc::channel();
        MidiSource {
            shared: Arc::new(Shared {
                queue: Mutex::default(),
                status: Mutex::new(MidiStatus::Starting),
                clock: Mutex::default(),
            }),
            stop,
        }
    }

    #[test]
    fn readers_each_see_every_message_once() {
        let src = source();
        let note = |note| MidiMessage::NoteOff { channel: 0, note };
        let (mut a, mut b) = (None, None);
        src.read(&mut a, |_| panic!("a new reader starts now"));
        src.shared.push(note(1));
        src.read(&mut b, |_| panic!("so does b"));
        src.shared.push(note(2));
        let mut seen = Vec::new();
        src.read(&mut a, |m| seen.push(m));
        assert_eq!(seen, [note(1), note(2)]);
        seen.clear();
        src.read(&mut b, |m| seen.push(m));
        assert_eq!(seen, [note(2)]);
        src.read(&mut a, |_| panic!("nothing new"));
    }

    #[test]
    fn a_reader_left_behind_skips_what_was_dropped() {
        let src = source();
        let mut cursor = Some(0);
        for i in 0..QUEUE_LEN + 10 {
            src.shared.push(MidiMessage::NoteOff {
                channel: 0,
                note: (i % 128) as u8,
            });
        }
        let mut count = 0;
        src.read(&mut cursor, |_| count += 1);
        assert_eq!(count, QUEUE_LEN);
        assert_eq!(cursor, Some((QUEUE_LEN + 10) as u64));
    }

    /// Needs CoreMIDI (or ALSA): a virtual device appears after the source
    /// started, and its notes arrive. `cargo test -p dov-io -- --ignored`
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn finds_a_device_plugged_in_later() {
        use midir::MidiOutput;
        use midir::os::unix::VirtualOutput;

        let src = MidiSource::open();
        let mut cursor = None;
        src.read(&mut cursor, |_| {});
        std::thread::sleep(Duration::from_millis(200));
        let out = MidiOutput::new("dov-test").unwrap();
        let mut port = out.create_virtual("DOV loopback").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !matches!(src.status(), MidiStatus::Running { ports } if ports.iter().any(|p| p.contains("DOV loopback")))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "never found: {:?}",
                src.status()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        port.send(&[0x92, 64, 90]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let mut seen = Vec::new();
        src.read(&mut cursor, |m| seen.push(m));
        assert_eq!(
            seen,
            [MidiMessage::NoteOn {
                channel: 2,
                note: 64,
                velocity: 90
            }]
        );
    }
}
