//! Opens a camera, reports its status and frame size, and saves one frame.
//! Usage: cargo run -p dov-io --example camera_probe -- [device] [out.png]

use std::time::{Duration, Instant};

use dov_io::{CameraSource, CameraStatus};

fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let device = args.next().and_then(|a| a.parse().ok()).unwrap_or(0);
    let out = args.next().unwrap_or_else(|| "captures/camera-probe.png".into());

    let camera = CameraSource::start(device);
    let start = Instant::now();
    let (mut seen, mut frames, mut last_status) = (0, 0, None);
    while start.elapsed() < Duration::from_secs(60) && frames < 30 {
        let status = camera.status();
        if last_status.as_ref() != Some(&status) {
            println!("{:>6.2}s  {status:?}", start.elapsed().as_secs_f32());
            last_status = Some(status.clone());
        }
        if matches!(status, CameraStatus::Denied | CameraStatus::Failed(_)) {
            std::process::exit(1);
        }
        camera.with_new_frame(&mut seen, |f| {
            frames += 1;
            if frames == 1 {
                println!(
                    "{:>6.2}s  first frame {}x{}",
                    start.elapsed().as_secs_f32(),
                    f.width,
                    f.height
                );
            }
            if frames == 30 {
                let mut rgba = f.bgra.clone();
                for px in rgba.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                std::fs::create_dir_all("captures").unwrap();
                let file = std::io::BufWriter::new(std::fs::File::create(&out).unwrap());
                let mut enc = png::Encoder::new(file, f.width, f.height);
                enc.set_color(png::ColorType::Rgba);
                enc.write_header().unwrap().write_image_data(&rgba).unwrap();
                println!("{:>6.2}s  saved {out}", start.elapsed().as_secs_f32());
            }
        });
        std::thread::sleep(Duration::from_millis(5));
    }
    let fps = frames as f32 / start.elapsed().as_secs_f32();
    println!("{frames} frames ({fps:.1} fps incl. startup)");
}
