//! Movie files out: a picture recorded as it plays, encoded by AVFoundation
//! on a thread of its own, so the engine never waits on the encoder. When
//! the encoder falls behind, frames are dropped rather than held up; the
//! ones written keep their times, so the movie still plays at speed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::JoinHandle;

/// How a movie is compressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// H.264 in an MP4: small, and plays anywhere.
    H264,
    /// Apple ProRes 422 in a QuickTime movie: big, and keeps every detail.
    ProRes,
}

impl Codec {
    pub fn extension(self) -> &'static str {
        match self {
            Self::H264 => "mp4",
            Self::ProRes => "mov",
        }
    }
}

/// Writes one frame's pixels: BGRA rows, the given number of bytes apart.
/// Dropped unused when the frame is.
pub type Fill = Box<dyn FnOnce(&mut [u8], usize) + Send>;

/// A movie written to the end.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub path: PathBuf,
    pub frames: u64,
    /// Frames the encoder couldn't keep up with.
    pub dropped: u64,
}

/// Frames waiting for the encoder before more are dropped.
const QUEUE: usize = 4;

/// A movie being written. Dropping it, or [`MovieWriter::finish`], closes
/// the file; `done` hears how that went.
pub struct MovieWriter {
    frames: Option<SyncSender<(f64, Fill)>>,
    thread: Option<JoinHandle<()>>,
}

impl MovieWriter {
    /// Starts a movie at `path`, `width` × `height` (both even) at `fps`.
    /// `done` is called from the writer's thread once the file is closed.
    pub fn start(
        path: &Path,
        width: u32,
        height: u32,
        fps: f64,
        codec: Codec,
        done: impl FnOnce(Result<Written, String>) + Send + 'static,
    ) -> Result<Self, String> {
        let (frames, queue) = mpsc::sync_channel::<(f64, Fill)>(QUEUE);
        let (ready, started) = mpsc::channel();
        let path = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("dov-movie-writer".into())
            .spawn(move || {
                let mut encoder = match platform::Encoder::open(&path, width, height, fps, codec) {
                    Ok(encoder) => {
                        let _ = ready.send(Ok(()));
                        encoder
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                let mut written = Written {
                    path,
                    frames: 0,
                    dropped: 0,
                };
                for (time, fill) in queue {
                    if encoder.append(time, fill) {
                        written.frames += 1;
                    } else {
                        written.dropped += 1;
                    }
                }
                done(encoder.finish().map(|()| written));
            })
            .map_err(|e| e.to_string())?;
        started
            .recv()
            .map_err(|_| "the movie writer stopped".to_string())??;
        Ok(Self {
            frames: Some(frames),
            thread: Some(thread),
        })
    }

    /// Hands over the frame `time` seconds in. Returns false, dropping it,
    /// when the encoder is behind.
    pub fn push(&self, time: f64, fill: Fill) -> bool {
        match self.frames.as_ref().map(|f| f.try_send((time, fill))) {
            Some(Ok(())) => true,
            Some(Err(TrySendError::Full(_) | TrySendError::Disconnected(_))) | None => false,
        }
    }

    /// Closes the movie and waits until the file is written.
    pub fn finish(mut self) {
        self.frames = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MovieWriter {
    /// Closes the movie without waiting; the writer finishes it.
    fn drop(&mut self) {
        self.frames = None;
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
        AVFileTypeMPEG4, AVFileTypeQuickTimeMovie, AVMediaTypeVideo, AVVideoAverageBitRateKey,
        AVVideoCodecKey, AVVideoCodecTypeAppleProRes422, AVVideoCodecTypeH264,
        AVVideoColorPrimaries_ITU_R_709_2, AVVideoColorPrimariesKey, AVVideoColorPropertiesKey,
        AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey,
        AVVideoMaxKeyFrameIntervalKey, AVVideoTransferFunction_ITU_R_709_2, AVVideoTransferFunctionKey,
        AVVideoWidthKey, AVVideoYCbCrMatrix_ITU_R_709_2, AVVideoYCbCrMatrixKey,
    };
    use objc2_core_foundation::{CFRetained, CFString};
    use objc2_core_media::{CMTime, kCMTimeZero};
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA, kCVReturnSuccess,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    use super::{Codec, Fill};

    /// Exact for NTSC (1001), PAL (1200) and 60 Hz (1000) frames.
    const TIMESCALE: i32 = 60_000;
    /// H.264 bits per pixel per frame: generous, since scan lines and
    /// phosphor masks are fine detail.
    const BITS_PER_PIXEL: f64 = 0.45;
    /// How long a frame waits for the encoder before it's dropped.
    const PATIENCE: Duration = Duration::from_millis(100);

    fn describe(e: &objc2_foundation::NSError) -> String {
        e.localizedDescription().to_string()
    }

    fn key(k: &CFString) -> &NSString {
        // SAFETY: CFString and NSString are toll-free bridged.
        unsafe { &*(k as *const CFString).cast::<NSString>() }
    }

    fn dict(keys: &[&NSString], values: &[&AnyObject]) -> Retained<NSDictionary<NSString, AnyObject>> {
        NSDictionary::from_slices(keys, values)
    }

    pub(super) struct Encoder {
        writer: Retained<AVAssetWriter>,
        input: Retained<AVAssetWriterInput>,
        adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
        height: usize,
    }

    impl Encoder {
        pub(super) fn open(
            path: &Path,
            width: u32,
            height: u32,
            fps: f64,
            codec: Codec,
        ) -> Result<Self, String> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            // SAFETY: AVFoundation calls with valid arguments, all on this
            // thread; the settings dictionaries hold the documented types.
            autoreleasepool(|_| unsafe {
                let missing = || "this Mac's AVFoundation is missing a movie setting".to_string();
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                let file_type = match codec {
                    Codec::H264 => AVFileTypeMPEG4,
                    Codec::ProRes => AVFileTypeQuickTimeMovie,
                }
                .ok_or_else(missing)?;
                let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type)
                    .map_err(|e| describe(&e))?;

                // Video signals are encoded like Rec. 709's.
                let colors = dict(
                    &[
                        AVVideoColorPrimariesKey.ok_or_else(missing)?,
                        AVVideoTransferFunctionKey.ok_or_else(missing)?,
                        AVVideoYCbCrMatrixKey.ok_or_else(missing)?,
                    ],
                    &[
                        AVVideoColorPrimaries_ITU_R_709_2.ok_or_else(missing)?.as_ref(),
                        AVVideoTransferFunction_ITU_R_709_2.ok_or_else(missing)?.as_ref(),
                        AVVideoYCbCrMatrix_ITU_R_709_2.ok_or_else(missing)?.as_ref(),
                    ],
                );
                let (w, h) = (NSNumber::new_u32(width), NSNumber::new_u32(height));
                let mut keys = vec![
                    AVVideoCodecKey.ok_or_else(missing)?,
                    AVVideoWidthKey.ok_or_else(missing)?,
                    AVVideoHeightKey.ok_or_else(missing)?,
                    AVVideoColorPropertiesKey.ok_or_else(missing)?,
                ];
                let codec_name = match codec {
                    Codec::H264 => AVVideoCodecTypeH264,
                    Codec::ProRes => AVVideoCodecTypeAppleProRes422,
                }
                .ok_or_else(missing)?;
                let mut values: Vec<&AnyObject> =
                    vec![codec_name.as_ref(), w.as_ref(), h.as_ref(), colors.as_ref()];
                let bits = NSNumber::new_f64(f64::from(width) * f64::from(height) * fps * BITS_PER_PIXEL);
                let keyframes = NSNumber::new_i32((fps * 2.0).round() as i32);
                let rate = NSNumber::new_f64(fps);
                let compression;
                if codec == Codec::H264 {
                    compression = dict(
                        &[
                            AVVideoAverageBitRateKey.ok_or_else(missing)?,
                            AVVideoMaxKeyFrameIntervalKey.ok_or_else(missing)?,
                            AVVideoExpectedSourceFrameRateKey.ok_or_else(missing)?,
                        ],
                        &[bits.as_ref(), keyframes.as_ref(), rate.as_ref()],
                    );
                    keys.push(AVVideoCompressionPropertiesKey.ok_or_else(missing)?);
                    values.push(compression.as_ref());
                }
                let settings = dict(&keys, &values);
                let video = AVMediaTypeVideo.ok_or_else(missing)?;
                let input =
                    AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(video, Some(&settings));
                input.setExpectsMediaDataInRealTime(true);
                // Frame times exactly as given, not rounded to 1/600 s.
                input.setMediaTimeScale(TIMESCALE);
                if !writer.canAddInput(&input) {
                    return Err("can't write video in this format".into());
                }
                writer.addInput(&input);

                let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
                let source = dict(
                    &[
                        key(kCVPixelBufferPixelFormatTypeKey),
                        key(kCVPixelBufferWidthKey),
                        key(kCVPixelBufferHeightKey),
                    ],
                    &[format.as_ref(), w.as_ref(), h.as_ref()],
                );
                let adaptor =
                    AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                        &input,
                        Some(&source),
                    );
                if !writer.startWriting() {
                    return Err(writer
                        .error()
                        .map_or_else(|| "couldn't start the movie".into(), |e| describe(&e)));
                }
                writer.startSessionAtSourceTime(kCMTimeZero);
                Ok(Self {
                    writer,
                    input,
                    adaptor,
                    height: height as usize,
                })
            })
        }

        /// Encodes the frame `time` seconds in; false if it was dropped.
        pub(super) fn append(&mut self, time: f64, fill: Fill) -> bool {
            // SAFETY: as in `open`; the pixel buffer is locked while written,
            // and `fill` writes inside its rows.
            autoreleasepool(|_| unsafe {
                let start = Instant::now();
                while !self.input.isReadyForMoreMediaData() {
                    if start.elapsed() > PATIENCE || self.writer.status() != AVAssetWriterStatus::Writing {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                let Some(pool) = self.adaptor.pixelBufferPool() else {
                    return false;
                };
                let Some(pixels) = new_buffer(&pool) else {
                    return false;
                };
                if CVPixelBufferLockBaseAddress(&pixels, CVPixelBufferLockFlags(0)) != kCVReturnSuccess {
                    return false;
                }
                let stride = CVPixelBufferGetBytesPerRow(&pixels);
                let rows = CVPixelBufferGetHeight(&pixels).min(self.height);
                let base = CVPixelBufferGetBaseAddress(&pixels).cast::<u8>();
                if !base.is_null() {
                    fill(std::slice::from_raw_parts_mut(base, stride * rows), stride);
                }
                CVPixelBufferUnlockBaseAddress(&pixels, CVPixelBufferLockFlags(0));
                !base.is_null()
                    && self.adaptor.appendPixelBuffer_withPresentationTime(
                        &pixels,
                        CMTime::with_seconds(time, TIMESCALE),
                    )
            })
        }

        /// Closes the file, waiting until it's written.
        pub(super) fn finish(self) -> Result<(), String> {
            // SAFETY: as in `open`. The completion block only signals.
            autoreleasepool(|_| unsafe {
                self.input.markAsFinished();
                let (tx, rx) = mpsc::channel();
                let block = RcBlock::new(move || {
                    let _ = tx.send(());
                });
                self.writer.finishWritingWithCompletionHandler(&block);
                let _ = rx.recv_timeout(Duration::from_secs(30));
                if self.writer.status() == AVAssetWriterStatus::Completed {
                    Ok(())
                } else {
                    Err(self
                        .writer
                        .error()
                        .map_or_else(|| "the movie wasn't finished".into(), |e| describe(&e)))
                }
            })
        }
    }

    /// A pixel buffer from the encoder's pool.
    fn new_buffer(pool: &CVPixelBufferPool) -> Option<CFRetained<CVPixelBuffer>> {
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: `out` is a valid place for the new buffer, which the call
        // returns retained.
        let status = unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) };
        if status != kCVReturnSuccess {
            return None;
        }
        // SAFETY: on success `out` is a +1 retained buffer.
        NonNull::new(out).map(|p| unsafe { CFRetained::from_raw(p) })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use super::{Codec, Fill};

    pub(super) struct Encoder;

    impl Encoder {
        pub(super) fn open(_: &Path, _: u32, _: u32, _: f64, _: Codec) -> Result<Self, String> {
            Err("recording movies needs macOS".into())
        }

        pub(super) fn append(&mut self, _: f64, _: Fill) -> bool {
            false
        }

        pub(super) fn finish(self) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Writes a second of a moving gradient and reads it back as a movie.
    #[test]
    fn writes_a_movie_that_opens() {
        for codec in [Codec::H264, Codec::ProRes] {
            let dir = std::env::temp_dir().join(format!("dov-writer-{}", std::process::id()));
            let path = dir.join(format!("test.{}", codec.extension()));
            let _ = std::fs::remove_file(&path);
            let (tx, rx) = mpsc::channel();
            let (w, h) = (320, 240);
            let writer = MovieWriter::start(&path, w, h, 60.0, codec, move |r| {
                let _ = tx.send(r);
            })
            .unwrap();
            let mut pushed = 0;
            for frame in 0..60 {
                let fill: Fill = Box::new(move |pixels, stride| {
                    for (y, row) in pixels.chunks_exact_mut(stride).enumerate() {
                        for (x, px) in row[..w as usize * 4]
                            .as_chunks_mut::<4>()
                            .0
                            .iter_mut()
                            .enumerate()
                        {
                            px.copy_from_slice(&[(x + frame * 4) as u8, y as u8, 128, 255]);
                        }
                    }
                });
                // The test feeds faster than real time; wait out a full queue.
                let mut fill = Some(fill);
                for _ in 0..200 {
                    if writer.push(f64::from(frame as u32) / 60.0, fill.take().unwrap()) {
                        pushed += 1;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    let refill: Fill = Box::new(move |pixels, stride| {
                        for row in pixels.chunks_exact_mut(stride) {
                            row.fill(frame as u8);
                        }
                    });
                    fill = Some(refill);
                }
            }
            writer.finish();
            let written = rx.recv().unwrap().unwrap();
            assert_eq!(pushed, 60, "{codec:?}");
            assert!(written.frames >= 58, "{codec:?}: {written:?}");
            let movie = crate::Movie::open(path.to_str().unwrap(), 480);
            let info = loop {
                match movie.status() {
                    crate::MovieStatus::Ready(info) => break info,
                    crate::MovieStatus::Failed(e) => panic!("{codec:?}: {e}"),
                    crate::MovieStatus::Loading => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            };
            assert_eq!((info.width, info.height), (w, h), "{codec:?}");
            assert!((info.fps - 60.0).abs() < 1.0, "{codec:?}: {info:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
