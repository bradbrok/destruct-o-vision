//! Lists MIDI devices and prints what they send, as the MIDI modules see it.
//! Usage: cargo run -p dov-io --example midi_probe -- [seconds]

use std::time::{Duration, Instant};

use dov_io::MidiSource;

fn main() {
    env_logger::init();
    let seconds = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10.0);
    let source = MidiSource::open();
    let start = Instant::now();
    let (mut cursor, mut last_status) = (None, None);
    while start.elapsed() < Duration::from_secs_f64(seconds) {
        let status = source.status();
        if last_status.as_ref() != Some(&status) {
            println!("{:>6.2}s  {status:?}", start.elapsed().as_secs_f32());
            last_status = Some(status);
        }
        source.read(&mut cursor, |m| {
            // Channels print from 1, as on the modules' Channel switch.
            println!(
                "{:>6.2}s  ch {:>2}  {m:?}",
                start.elapsed().as_secs_f32(),
                m.channel() + 1
            );
        });
        std::thread::sleep(Duration::from_millis(20));
    }
}
