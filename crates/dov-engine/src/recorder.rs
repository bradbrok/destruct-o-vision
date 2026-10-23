//! Recording the output window to a movie while it plays. Each new field is
//! drawn as the window shows it into a texture of the recording's own, read
//! back without the engine waiting, and handed to the movie writer, which
//! copies it straight into the encoder's buffers. A CRT gets a tube of its
//! own that shows whole fields, so the movie is the same whatever display
//! the window is on.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use dov_core::RasterStd;
use dov_io::{Codec, Fill, MovieWriter, Written};

use crate::gpu::{self, BlitBinding, Blitter};
use crate::runtime::Runtime;
use crate::tube::{self, Refresh, Tube, ViewGpu};

/// Pixels as the encoder takes them.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// Readbacks in flight or waiting for the encoder before frames are dropped.
const BUFFERS: usize = 6;
/// How long stopping waits for the last frames.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// A frame on its way back from the GPU.
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Seconds into the movie.
    time: f64,
    mapping: bool,
}

pub(crate) struct Recorder {
    raster: RasterStd,
    size: [u32; 2],
    padded_row: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    tube: Tube,
    blit: BlitBinding,
    writer: MovieWriter,
    /// Buffers free for the next frame; the writer hands them back.
    free: Arc<Mutex<Vec<wgpu::Buffer>>>,
    in_flight: VecDeque<Readback>,
    first_field: Option<u64>,
    /// Frames dropped here because the encoder was behind.
    dropped: Arc<AtomicU64>,
}

impl Recorder {
    /// Starts a movie of the output window at `height` lines.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        device: &wgpu::Device,
        blitter: &mut Blitter,
        runtime: &Runtime,
        raster: RasterStd,
        height: u32,
        path: &Path,
        codec: Codec,
        done: impl FnOnce(Result<Written, String>) + Send + 'static,
    ) -> Result<Self, String> {
        let even = |v: f32| ((v / 2.0).round() as u32).max(1) * 2;
        let size = [even(height as f32 * raster.display_aspect), even(height as f32)];
        let dropped = Arc::new(AtomicU64::new(0));
        let ours = dropped.clone();
        let done = move |written: Result<Written, String>| {
            done(written.map(|mut w| {
                w.dropped += ours.load(Ordering::Relaxed);
                w
            }));
        };
        let writer = MovieWriter::start(path, size[0], size[1], raster.field_rate_hz(), codec, done)?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("movie frame"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded_row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let free = (0..BUFFERS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("movie readback"),
                    size: u64::from(padded_row) * u64::from(size[1]),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        blitter.prepare(device, FORMAT);
        let mut tube = Tube::new(device);
        tube.prepare(device, FORMAT);
        tube.bind(device, &raster, runtime.output_view(), runtime.yoke_view());
        Ok(Self {
            raster,
            size,
            padded_row,
            view: texture.create_view(&Default::default()),
            texture,
            tube,
            blit: blitter.bind(device, runtime.output_view()),
            writer,
            free: Arc::new(Mutex::new(free)),
            in_flight: VecDeque::new(),
            first_field: None,
            dropped,
        })
    }

    /// The raster the movie was started in; it can't change size midway.
    pub(crate) fn raster(&self) -> RasterStd {
        self.raster
    }

    /// Follows the patch's signals after it changes.
    pub(crate) fn rebind(&mut self, device: &wgpu::Device, blitter: &Blitter, runtime: &Runtime) {
        self.tube
            .bind(device, &self.raster, runtime.output_view(), runtime.yoke_view());
        self.blit = blitter.bind(device, runtime.output_view());
    }

    /// Draws `field`, rendered just now after `fresh` new fields, and copies
    /// it for reading back. Call [`Self::submitted`] once the encoder is
    /// submitted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        blitter: &Blitter,
        runtime: &Runtime,
        field: u64,
        fresh: u64,
    ) {
        let Some(buffer) = self.free.lock().unwrap_or_else(PoisonError::into_inner).pop() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let first = *self.first_field.get_or_insert(field);
        let time = (field - first) as f64 * self.raster.field_period();
        let [w, h] = self.size;
        let scale = gpu::letterbox(self.raster.display_aspect, w, h);
        match runtime.crt() {
            Some(crt) => {
                let fields = fresh.clamp(1, 4) as u32;
                let settings = tube::settings(&crt, &self.raster, Refresh::Hold { fields }, field);
                self.tube.update(queue, &settings);
                self.tube.advance(encoder);
                let view = ViewGpu {
                    scale,
                    size: [w as f32, h as f32],
                };
                self.tube
                    .draw(device, queue, encoder, FORMAT, &self.view, view, true);
            }
            None => blitter.encode(
                queue,
                encoder,
                &self.blit,
                FORMAT,
                &self.view,
                gpu::BlitGpu::plain(scale),
            ),
        }
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(h),
                },
            },
            self.texture.size(),
        );
        self.in_flight.push_back(Readback {
            buffer,
            state: Arc::new(AtomicU8::new(PENDING)),
            time,
            mapping: false,
        });
    }

    /// Starts reading back the frames just submitted.
    pub(crate) fn submitted(&mut self) {
        for r in self.in_flight.iter_mut().filter(|r| !r.mapping) {
            r.mapping = true;
            let state = r.state.clone();
            r.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
        }
    }

    /// Hands frames that have come back to the writer, in order. The
    /// device must be polled for them to come back.
    pub(crate) fn deliver(&mut self) {
        while let Some(r) = self.in_flight.front() {
            match r.state.load(Ordering::Acquire) {
                PENDING => break,
                READY => {
                    let r = self.in_flight.pop_front().expect("front exists");
                    let back = Back {
                        buffer: Some(r.buffer),
                        free: self.free.clone(),
                    };
                    let (width, height, padded) =
                        (self.size[0] as usize * 4, self.size[1] as usize, self.padded_row);
                    let fill: Fill = Box::new(move |pixels, stride| {
                        let Some(buffer) = &back.buffer else { return };
                        let Ok(mapped) = buffer.slice(..).get_mapped_range() else {
                            return;
                        };
                        let rows = mapped.chunks_exact(padded as usize).take(height);
                        for (src, dst) in rows.zip(pixels.chunks_mut(stride)) {
                            let n = width.min(dst.len());
                            dst[..n].copy_from_slice(&src[..n]);
                        }
                    });
                    if !self.writer.push(r.time, fill) {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                _ => {
                    let r = self.in_flight.pop_front().expect("front exists");
                    log::warn!("movie frame at {:.3} s: readback failed", r.time);
                    r.buffer.unmap();
                    self.free
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(r.buffer);
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pub(crate) fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// Writes out the frames still coming back and closes the movie. With
    /// `wait`, returns only once the file is written (when quitting).
    pub(crate) fn stop(mut self, device: &wgpu::Device, wait: bool) {
        let deadline = Instant::now() + FLUSH_WAIT;
        while self.busy() && Instant::now() < deadline {
            let _ = device.poll(wgpu::PollType::Poll);
            self.deliver();
            if self.busy() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            log::warn!("movie: {dropped} frames dropped keeping up");
        }
        if wait {
            self.writer.finish();
        }
    }
}

/// A readback buffer on loan to the writer: unmapped and handed back for
/// the next frame once the writer is done with it, or drops the frame.
struct Back {
    buffer: Option<wgpu::Buffer>,
    free: Arc<Mutex<Vec<wgpu::Buffer>>>,
}

impl Drop for Back {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.unmap();
            self.free
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(buffer);
        }
    }
}
