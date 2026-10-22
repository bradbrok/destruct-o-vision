//! Lists audio inputs, then opens one by name (the default if none is
//! given) and prints the level of its first two channels for a few seconds.
//! Usage: cargo run -p dov-io --example audio_probe -- ["device name"] [seconds]

use std::time::{Duration, Instant};

use dov_io::{AudioCursor, AudioSource, AudioStatus};

fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let device = args.next().unwrap_or_default();
    let seconds = args.next().and_then(|a| a.parse().ok()).unwrap_or(5.0);

    let started = Instant::now();
    let inputs = dov_io::audio::input_devices();
    println!("{} inputs, listed in {:?}:", inputs.len(), started.elapsed());
    for d in &inputs {
        let default = if d.default { "  (default)" } else { "" };
        println!("  {:>3} ch  {}{default}", d.channels, d.name);
    }

    let source = AudioSource::open(&device);
    let start = Instant::now();
    let (mut cursor, mut last_status, mut frames) = (AudioCursor::default(), None, 0);
    while start.elapsed() < Duration::from_secs_f64(seconds) {
        let status = source.status();
        if last_status.as_ref() != Some(&status) {
            println!("{:>6.2}s  {status:?}", start.elapsed().as_secs_f32());
            last_status = Some(status.clone());
        }
        if matches!(status, AudioStatus::Denied) {
            std::process::exit(1);
        }
        let mut peak = [0f32; 2];
        frames += source.read(&mut cursor, 48_000, |f| {
            for (p, s) in peak.iter_mut().zip(f) {
                *p = p.max(s.abs());
            }
        });
        if matches!(status, AudioStatus::Running { .. }) {
            let bar = |v: f32| "#".repeat((v * 40.0).min(40.0) as usize);
            println!(
                "{:>6.2}s  1 {:<40} 2 {}",
                start.elapsed().as_secs_f32(),
                bar(peak[0]),
                bar(peak[1])
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let (rate, channels) = (source.sample_rate(), source.channels());
    let plural = if channels == 1 { "" } else { "s" };
    println!("{frames} frames in {seconds} s ({rate} Hz, {channels} channel{plural})");
}
