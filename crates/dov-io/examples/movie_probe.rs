//! Opens a movie, plays it through forward and then backward as fast as it
//! decodes, and saves one frame as a PNG.
//! Usage: cargo run -p dov-io --example movie_probe -- FILE [max height] [frame to save] [png]

use std::time::{Duration, Instant};

use dov_io::{Movie, MovieStatus, Order, Want};

fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: movie_probe FILE [max height] [frame to save] [png]");
        std::process::exit(2);
    };
    let max_height = args.next().and_then(|a| a.parse().ok()).unwrap_or(480);
    let save = args.next().and_then(|a| a.parse().ok());
    let png = args.next().unwrap_or_else(|| "frame.png".into());

    let started = Instant::now();
    let movie = Movie::open(&path, max_height);
    let MovieStatus::Ready(info) = movie.wait_loaded(Duration::from_secs(30)) else {
        eprintln!("{:?}", movie.status());
        std::process::exit(1);
    };
    println!("{info:?}, opened in {:?}", started.elapsed());

    for forward in [true, false] {
        let started = Instant::now();
        let mut levels = Vec::new();
        for i in 0..info.frames {
            let n = if forward { i } else { info.frames - 1 - i };
            movie.want(Want {
                frame: n,
                first: 0,
                last: info.frames - 1,
                order: Order::Loop { forward },
            });
            let Some(f) = movie.wait_for(n, Duration::from_secs(5)) else {
                eprintln!("frame {n} didn't arrive: {:?}", movie.status());
                std::process::exit(1);
            };
            let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
            levels.push(f.bgra[mid + 1]);
        }
        let took = started.elapsed();
        let way = if forward { "forward" } else { "backward" };
        println!(
            "{way}: {} frames in {took:?} ({:.0} fps); middle green: {:?}…",
            info.frames,
            f64::from(info.frames) / took.as_secs_f64(),
            &levels[..levels.len().min(12)]
        );
    }

    if let Some(n) = save {
        movie.want(Want {
            frame: n,
            first: 0,
            last: info.frames - 1,
            order: Order::Around,
        });
        let f = movie.wait_for(n, Duration::from_secs(5)).expect("frame");
        let rgba: Vec<u8> = f.bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let file = std::fs::File::create(&png).expect("create png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), f.width, f.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&rgba).expect("png data");
        println!("saved frame {n} ({}×{}) to {png}", f.width, f.height);
    }
}
