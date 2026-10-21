//! Asynchronous readback of a texture to a PNG file. The engine never waits
//! on the GPU for this; finished readbacks are encoded on a worker thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

pub struct Capture {
    buffer: wgpu::Buffer,
    padded_row: u32,
    /// BGRA source: swap red and blue when writing.
    swap_rb: bool,
    size: [u32; 2],
    path: PathBuf,
    state: Arc<AtomicU8>,
}

impl Capture {
    /// Records a copy of `texture`, which must be 8-bit RGBA or BGRA with `COPY_SRC`.
    pub fn encode(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        path: PathBuf,
    ) -> Self {
        let extent = texture.size();
        let padded_row = (extent.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: u64::from(padded_row) * u64::from(extent.height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(extent.height),
                },
            },
            extent,
        );
        let swap_rb = matches!(
            texture.format(),
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        Self {
            buffer,
            padded_row,
            swap_rb,
            size: [extent.width, extent.height],
            path,
            state: Arc::new(AtomicU8::new(PENDING)),
        }
    }

    /// Call once the copy has been submitted.
    pub fn map(&self) {
        let state = self.state.clone();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
    }

    /// Returns true once the capture is finished, successfully or not.
    /// The device must be polled for this to make progress.
    pub fn try_finish(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            PENDING => false,
            READY => {
                let [w, h] = self.size;
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                match self.buffer.slice(..).get_mapped_range() {
                    Ok(view) => {
                        for row in view.chunks_exact(self.padded_row as usize) {
                            rgba.extend_from_slice(&row[..(w * 4) as usize]);
                        }
                    }
                    Err(err) => {
                        log::error!("capture {}: {err}", self.path.display());
                        return true;
                    }
                }
                self.buffer.unmap();
                if self.swap_rb {
                    for px in rgba.as_chunks_mut::<4>().0 {
                        px.swap(0, 2);
                    }
                }
                let path = self.path.clone();
                std::thread::spawn(move || match write_png(&path, w, h, &rgba) {
                    Ok(()) => log::info!("captured {}", path.display()),
                    Err(err) => log::error!("capture {}: {err}", path.display()),
                });
                true
            }
            _ => {
                log::error!("capture {}: buffer mapping failed", self.path.display());
                true
            }
        }
    }
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}
