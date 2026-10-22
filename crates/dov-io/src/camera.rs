//! Camera capture. A worker thread owns the capture session; frames arrive
//! as BGRA on a dispatch queue and are copied into a shared slot that the
//! engine polls, so the engine never waits on the camera (or on the
//! permission dialog).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A captured frame: tightly packed BGRA, 8 bits per channel.
#[derive(Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CameraStatus {
    Starting,
    Running { name: String },
    Denied,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct Shared {
    frame: Mutex<Frame>,
    seq: AtomicU64,
    status: Mutex<Option<CameraStatus>>,
}

impl Shared {
    pub(crate) fn set_status(&self, status: CameraStatus) {
        match &status {
            CameraStatus::Running { name } => log::info!("camera: {name}"),
            CameraStatus::Denied => log::warn!("camera: access denied"),
            CameraStatus::Failed(e) => log::warn!("camera: {e}"),
            CameraStatus::Starting => {}
        }
        if let Ok(mut s) = self.status.lock() {
            *s = Some(status);
        }
    }

    /// Copies `height` rows of `width` BGRA pixels, `stride` bytes apart.
    pub(crate) fn publish(&self, width: u32, height: u32, stride: usize, data: &[u8]) {
        let row = width as usize * 4;
        if stride < row || data.len() < stride * (height as usize - 1) + row {
            return;
        }
        let Ok(mut f) = self.frame.lock() else { return };
        f.width = width;
        f.height = height;
        f.bgra.clear();
        for y in 0..height as usize {
            f.bgra.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        drop(f);
        self.seq.fetch_add(1, Ordering::Release);
    }
}

/// A running camera. Dropping it stops capture.
pub struct CameraSource {
    pub device: usize,
    shared: Arc<Shared>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CameraSource {
    /// Starts capturing from the `device`-th camera (falls back to the default).
    pub fn start(device: usize) -> Self {
        let shared = Arc::new(Shared::default());
        shared.set_status(CameraStatus::Starting);
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dov-camera".into())
            .spawn(move || platform::run(device, &worker, stop_rx))
            .map_err(|e| shared.set_status(CameraStatus::Failed(e.to_string())))
            .ok();
        Self {
            device,
            shared,
            stop: Some(stop_tx),
            thread,
        }
    }

    pub fn status(&self) -> CameraStatus {
        self.shared
            .status
            .lock()
            .ok()
            .and_then(|s| s.clone())
            .unwrap_or(CameraStatus::Starting)
    }

    /// Calls `f` with the latest frame if one arrived since `seen`, and
    /// updates `seen`. The frame stays locked only for the call.
    pub fn with_new_frame(&self, seen: &mut u64, f: impl FnOnce(&Frame)) -> bool {
        let seq = self.shared.seq.load(Ordering::Acquire);
        if seq == *seen {
            return false;
        }
        let Ok(frame) = self.shared.frame.lock() else {
            return false;
        };
        if frame.bgra.is_empty() {
            return false;
        }
        *seen = seq;
        f(&frame);
        true
    }
}

impl Drop for CameraSource {
    fn drop(&mut self) {
        // Closing the channel wakes the worker, which stops the session.
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::Arc;
    use std::sync::mpsc;

    use dispatch2::DispatchQueue;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
    use objc2_av_foundation::{
        AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession,
        AVCaptureSessionPreset1280x720, AVCaptureVideoDataOutput,
        AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
    };
    use objc2_core_media::CMSampleBuffer;
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelFormatType_32BGRA,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSObjectProtocol, NSString};

    use super::{CameraStatus, Shared};

    struct Ivars {
        shared: Arc<Shared>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and we don't implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "DOVCameraDelegate"]
        #[ivars = Ivars]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
            #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
            fn did_output(
                &self,
                _output: &AVCaptureOutput,
                sample: &CMSampleBuffer,
                _connection: &AVCaptureConnection,
            ) {
                // SAFETY: the sample buffer is valid for the duration of the callback.
                let Some(pixels) = (unsafe { sample.image_buffer() }) else {
                    return;
                };
                if CVPixelBufferGetPixelFormatType(&pixels) != kCVPixelFormatType_32BGRA {
                    return;
                }
                // SAFETY: locked read-only around the reads and unlocked with the same flags.
                unsafe {
                    if CVPixelBufferLockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly) != 0 {
                        return;
                    }
                    let base = CVPixelBufferGetBaseAddress(&pixels);
                    let stride = CVPixelBufferGetBytesPerRow(&pixels);
                    let (w, h) = (CVPixelBufferGetWidth(&pixels), CVPixelBufferGetHeight(&pixels));
                    if !base.is_null() && w > 0 && h > 0 {
                        let data = std::slice::from_raw_parts(base as *const u8, stride * h);
                        self.ivars().shared.publish(w as u32, h as u32, stride, data);
                    }
                    CVPixelBufferUnlockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly);
                }
            }
        }
    );

    impl Delegate {
        fn new(shared: Arc<Shared>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Ivars { shared });
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn run(index: usize, shared: &Arc<Shared>, stop: mpsc::Receiver<()>) {
        if !crate::access::authorized(crate::access::Device::Camera) {
            shared.set_status(CameraStatus::Denied);
            return;
        }
        let session = match autoreleasepool(|_| open(index, shared)) {
            Ok(session) => session,
            Err(e) => {
                shared.set_status(CameraStatus::Failed(e));
                return;
            }
        };
        // Wait until the source is dropped.
        let _ = stop.recv();
        // SAFETY: the session was started by this thread.
        unsafe { session.session.stopRunning() };
    }

    /// Everything that must stay alive while capturing.
    struct Running {
        session: Retained<AVCaptureSession>,
        _delegate: Retained<Delegate>,
        _queue: dispatch2::DispatchRetained<DispatchQueue>,
    }

    fn open(index: usize, shared: &Arc<Shared>) -> Result<Running, String> {
        // SAFETY: AVFoundation calls with valid arguments, made from one
        // thread; the delegate and queue are kept alive in `Running`.
        unsafe {
            let video = AVMediaTypeVideo.ok_or("no video media type")?;
            #[allow(deprecated)]
            let devices = AVCaptureDevice::devicesWithMediaType(video);
            let device = if index < devices.count() {
                devices.objectAtIndex(index)
            } else {
                AVCaptureDevice::defaultDeviceWithMediaType(video).ok_or("no camera found")?
            };
            let name = device.localizedName().to_string();
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| e.localizedDescription().to_string())?;

            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                session.setSessionPreset(AVCaptureSessionPreset1280x720);
            }
            if !session.canAddInput(&input) {
                return Err(format!("can't use {name}"));
            }
            session.addInput(&input);

            let output = AVCaptureVideoDataOutput::new();
            let key: &NSString = &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
            let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let value: &AnyObject = format.as_ref();
            let settings = NSDictionary::from_slices(&[key], &[value]);
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
            let delegate = Delegate::new(shared.clone());
            let queue = DispatchQueue::new("dov.camera", None);
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(&queue));
            if !session.canAddOutput(&output) {
                return Err("can't capture video frames".into());
            }
            session.addOutput(&output);
            session.commitConfiguration();
            session.startRunning();
            shared.set_status(CameraStatus::Running { name });
            Ok(Running {
                session,
                _delegate: delegate,
                _queue: queue,
            })
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::sync::{Arc, mpsc};

    use super::{CameraStatus, Shared};

    pub(super) fn run(_index: usize, shared: &Arc<Shared>, _stop: mpsc::Receiver<()>) {
        shared.set_status(CameraStatus::Failed(
            "camera capture isn't implemented on this platform yet".into(),
        ));
    }
}
