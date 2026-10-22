//! Audio input. A worker thread owns the input stream. The audio callback
//! writes frames into a lock-free ring that any number of readers drain at
//! their own pace (each Audio In module, once per field), so the audio
//! thread never waits on the engine and the engine never waits on audio.
//! Modules on the same device share one stream. Devices are known by name,
//! so a patch finds its device again whatever else is plugged in.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample};

use crate::access::{self, Device};

/// Audio a ring holds, whatever the rate.
const RING_SECONDS: f64 = 0.5;
/// How long to wait before trying a missing or failed device again.
const RETRY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub enum AudioStatus {
    Starting,
    Running { name: String },
    Denied,
    Failed(String),
}

/// An audio input device, as the system reports it now.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDevice {
    pub name: String,
    pub channels: usize,
    /// The system's default input.
    pub default: bool,
}

/// The audio inputs connected now. This asks the system, which can take a
/// moment: call it when a list is wanted, not every frame.
pub fn input_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|d| d.to_string());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .map(|d| {
            let name = d.to_string();
            InputDevice {
                channels: d.default_input_config().map_or(0, |c| usize::from(c.channels())),
                default: default.as_ref() == Some(&name),
                name,
            }
        })
        .collect()
}

/// One stream's audio: every channel the device has, interleaved.
struct Ring {
    channels: usize,
    rate: u32,
    frames: u64,
    samples: Box<[AtomicU32]>,
    /// Frames written so far. Frame `n` starts at `(n % frames) * channels`.
    written: AtomicU64,
}

impl Ring {
    fn new(channels: usize, rate: u32) -> Self {
        let frames = ((f64::from(rate) * RING_SECONDS) as usize)
            .next_power_of_two()
            .max(1024);
        Self {
            channels: channels.max(1),
            rate,
            frames: frames as u64,
            samples: (0..frames * channels.max(1)).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
        }
    }

    /// Called on the audio thread: no locks, no allocation.
    fn write<T>(&self, data: &[T])
    where
        T: Sample,
        f32: FromSample<T>,
    {
        // Only this stream's callback writes, so a relaxed load is current.
        let mut n = self.written.load(Ordering::Relaxed);
        for frame in data.chunks_exact(self.channels) {
            let base = (n % self.frames) as usize * self.channels;
            for (slot, x) in self.samples[base..base + self.channels].iter().zip(frame) {
                slot.store(x.to_sample::<f32>().to_bits(), Ordering::Relaxed);
            }
            n += 1;
        }
        self.written.store(n, Ordering::Release);
    }
}

struct Shared {
    /// The open stream's audio; replaced when the stream is opened again.
    ring: Mutex<Option<Arc<Ring>>>,
    status: Mutex<AudioStatus>,
}

impl Shared {
    fn new() -> Self {
        Self {
            ring: Mutex::new(None),
            status: Mutex::new(AudioStatus::Starting),
        }
    }

    /// Logs only changes, so retrying a missing device doesn't flood the log.
    fn set_status(&self, status: AudioStatus) {
        let Ok(mut s) = self.status.lock() else { return };
        if *s == status {
            return;
        }
        match &status {
            AudioStatus::Running { name } => log::info!("audio in: {name}"),
            AudioStatus::Denied => log::warn!("audio in: microphone access denied"),
            AudioStatus::Failed(e) => log::warn!("audio in: {e}"),
            AudioStatus::Starting => {}
        }
        *s = status;
    }
}

enum Event {
    Stop,
    /// The stream broke (the device was unplugged, say): reopen it.
    Lost(String),
}

/// A reader's place in a source's audio (see [`AudioSource::read`]). A new
/// one starts from now.
#[derive(Default)]
pub struct AudioCursor {
    ring: Option<Arc<Ring>>,
    next: u64,
}

/// A running audio input. Dropping the last handle stops it.
pub struct AudioSource {
    device: String,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
}

impl AudioSource {
    /// The input stream of the device called `device` (empty for the
    /// system default), started if nothing is using it yet. A device that
    /// isn't connected opens when it is.
    pub fn open(device: &str) -> Arc<Self> {
        static OPEN: Mutex<Vec<Weak<AudioSource>>> = Mutex::new(Vec::new());
        let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        open.retain(|s| s.strong_count() > 0);
        if let Some(source) = open.iter().filter_map(Weak::upgrade).find(|s| s.device == device) {
            return source;
        }
        let source = Arc::new(Self::start(device.to_string()));
        open.push(Arc::downgrade(&source));
        source
    }

    fn start(device: String) -> Self {
        let shared = Arc::new(Shared::new());
        let (events, rx) = mpsc::channel();
        let (worker, name, lost) = (shared.clone(), device.clone(), events.clone());
        let spawned = std::thread::Builder::new()
            .name("dov-audio".into())
            .spawn(move || run(&name, &worker, &rx, &lost));
        if let Err(e) = spawned {
            shared.set_status(AudioStatus::Failed(e.to_string()));
        }
        Self {
            device,
            shared,
            events,
        }
    }

    /// The device name it was opened with (empty for the default).
    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn status(&self) -> AudioStatus {
        self.shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(AudioStatus::Starting)
    }

    fn ring(&self) -> Option<Arc<Ring>> {
        self.shared.ring.lock().ok()?.clone()
    }

    /// Frames per second, or 0 before the stream has opened.
    pub fn sample_rate(&self) -> u32 {
        self.ring().map_or(0, |r| r.rate)
    }

    /// The device's input channels, or 0 before the stream has opened.
    pub fn channels(&self) -> usize {
        self.ring().map_or(0, |r| r.channels)
    }

    /// Calls `f` with each frame (a sample per channel) captured since
    /// `cursor`, at most the newest `max`, and moves `cursor` past them.
    /// After the stream reopens, the cursor starts again from now. Returns
    /// how many frames it read.
    pub fn read(&self, cursor: &mut AudioCursor, max: usize, mut f: impl FnMut(&[f32])) -> usize {
        let Some(ring) = self.ring() else { return 0 };
        let end = ring.written.load(Ordering::Acquire);
        if !cursor.ring.as_ref().is_some_and(|r| Arc::ptr_eq(r, &ring)) {
            cursor.next = end;
            cursor.ring = Some(ring.clone());
        }
        // Never further back than half the ring, so the writer can't lap us.
        let max = max.min(ring.frames as usize / 2) as u64;
        let start = cursor.next.max(end.saturating_sub(max));
        cursor.next = end;
        let mut frame = vec![0.0; ring.channels];
        for n in start..end {
            let base = (n % ring.frames) as usize * ring.channels;
            for (v, s) in frame.iter_mut().zip(&ring.samples[base..]) {
                *v = f32::from_bits(s.load(Ordering::Relaxed));
            }
            f(&frame);
        }
        (end - start) as usize
    }
}

impl Drop for AudioSource {
    fn drop(&mut self) {
        // Not joined: the worker may be waiting on the permission dialog,
        // and the engine thread mustn't wait with it. It stops on its own.
        let _ = self.events.send(Event::Stop);
    }
}

fn run(device: &str, shared: &Arc<Shared>, events: &mpsc::Receiver<Event>, lost: &mpsc::Sender<Event>) {
    if !access::authorized(Device::Microphone) {
        shared.set_status(AudioStatus::Denied);
        // Access can be granted later in System Settings.
        while !access::granted(Device::Microphone) {
            if !wait(events) {
                return;
            }
        }
    }
    loop {
        match open_stream(device, shared, lost) {
            Ok((stream, name)) => {
                shared.set_status(AudioStatus::Running { name });
                match events.recv() {
                    Ok(Event::Lost(e)) => {
                        drop(stream);
                        shared.set_status(AudioStatus::Failed(e));
                    }
                    Ok(Event::Stop) | Err(_) => return,
                }
            }
            Err(e) => shared.set_status(AudioStatus::Failed(e)),
        }
        if !wait(events) {
            return;
        }
    }
}

/// Waits before a retry. False when the source was dropped.
fn wait(events: &mpsc::Receiver<Event>) -> bool {
    match events.recv_timeout(RETRY) {
        // A late report from a stream that's already gone.
        Ok(Event::Lost(_)) | Err(RecvTimeoutError::Timeout) => true,
        Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => false,
    }
}

fn open_stream(
    device: &str,
    shared: &Arc<Shared>,
    lost: &mpsc::Sender<Event>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let dev = if device.is_empty() {
        host.default_input_device().ok_or("no audio input found")?
    } else {
        let mut inputs = host.input_devices().map_err(|e| e.to_string())?;
        inputs
            .find(|d| d.to_string() == device)
            .ok_or_else(|| format!("{device} isn't connected"))?
    };
    let name = dev.to_string();
    let config = dev.default_input_config().map_err(|e| format!("{name}: {e}"))?;
    let ring = Arc::new(Ring::new(usize::from(config.channels()), config.sample_rate()));

    let lost = lost.clone();
    let mut reported = false;
    let on_error = move |e: cpal::Error| match e.kind() {
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
            log::debug!("audio in: {e}")
        }
        _ if !reported => {
            reported = true;
            let _ = lost.send(Event::Lost(e.to_string()));
        }
        _ => {}
    };
    let format = config.sample_format();
    let config = config.config();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&dev, config, &ring, on_error),
        SampleFormat::F64 => build::<f64>(&dev, config, &ring, on_error),
        SampleFormat::I16 => build::<i16>(&dev, config, &ring, on_error),
        SampleFormat::I24 => build::<cpal::I24>(&dev, config, &ring, on_error),
        SampleFormat::I32 => build::<i32>(&dev, config, &ring, on_error),
        SampleFormat::U8 => build::<u8>(&dev, config, &ring, on_error),
        SampleFormat::U16 => build::<u16>(&dev, config, &ring, on_error),
        f => return Err(format!("{name}: unsupported sample format {f}")),
    }
    .map_err(|e| format!("{name}: {e}"))?;
    stream.play().map_err(|e| format!("{name}: {e}"))?;
    if let Ok(mut current) = shared.ring.lock() {
        *current = Some(ring);
    }
    Ok((stream, name))
}

fn build<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    ring: &Arc<Ring>,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ring = ring.clone();
    dev.build_input_stream(config, move |data: &[T], _: &_| ring.write(data), on_error, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source with an open stream of `channels` at 48 kHz, and its ring
    /// to write into.
    fn source(channels: usize) -> (AudioSource, Arc<Ring>) {
        let ring = Arc::new(Ring::new(channels, 48_000));
        let shared = Shared::new();
        *shared.ring.lock().unwrap() = Some(ring.clone());
        let (events, _) = mpsc::channel();
        let src = AudioSource {
            device: String::new(),
            shared: Arc::new(shared),
            events,
        };
        (src, ring)
    }

    #[test]
    fn readers_each_see_every_frame_once() {
        let (src, ring) = source(2);
        let (mut a, mut b) = (AudioCursor::default(), AudioCursor::default());
        assert_eq!(src.read(&mut a, 1000, |_| {}), 0, "a new reader starts now");
        ring.write(&[0.25f32, -0.5, 0.75, 1.0]);
        let mut seen = Vec::new();
        assert_eq!(src.read(&mut a, 1000, |f| seen.push(f.to_vec())), 2);
        assert_eq!(seen, [vec![0.25, -0.5], vec![0.75, 1.0]]);
        assert_eq!(src.read(&mut a, 1000, |_| {}), 0, "nothing new");
        assert_eq!(src.read(&mut b, 1000, |_| {}), 0, "b started after the write");
    }

    #[test]
    fn a_late_reader_gets_only_the_newest_frames() {
        let (src, ring) = source(1);
        let mut cursor = AudioCursor::default();
        src.read(&mut cursor, 0, |_| {});
        let samples: Vec<i16> = (0..100).collect();
        ring.write(&samples);
        let mut first = None;
        let read = src.read(&mut cursor, 10, |f| {
            first.get_or_insert(f[0]);
        });
        assert_eq!(read, 10);
        assert_eq!(first, Some(90.0 / 32768.0));
    }

    #[test]
    fn every_channel_of_a_big_interface_is_kept() {
        let (src, ring) = source(34);
        assert_eq!(src.channels(), 34);
        let mut cursor = AudioCursor::default();
        src.read(&mut cursor, 8, |_| {});
        let frame: Vec<f32> = (0..34).map(|c| c as f32 / 64.0).collect();
        ring.write(&frame);
        src.read(&mut cursor, 8, |f| assert_eq!(f, frame.as_slice()));
    }

    #[test]
    fn a_reopened_stream_starts_readers_over() {
        let (src, ring) = source(1);
        let mut cursor = AudioCursor::default();
        src.read(&mut cursor, 8, |_| {});
        ring.write(&[0.5f32; 4]);
        let reopened = Arc::new(Ring::new(2, 44_100));
        *src.shared.ring.lock().unwrap() = Some(reopened.clone());
        assert_eq!(
            src.read(&mut cursor, 8, |_| {}),
            0,
            "the old stream's audio is gone"
        );
        assert_eq!(src.sample_rate(), 44_100);
        reopened.write(&[0.1f32, 0.2]);
        assert_eq!(src.read(&mut cursor, 8, |f| assert_eq!(f, [0.1, 0.2])), 1);
    }
}
