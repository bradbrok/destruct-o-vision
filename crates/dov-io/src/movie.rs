//! Movie files. A worker thread decodes frames into memory ahead of the
//! playhead, in the order the playhead will reach them, so loops, reverse
//! play and scrubbing run from memory once their frames are in. The engine
//! never waits on the decoder, except while recording, when every frame
//! has to be the right one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::Frame;

/// Frames a backward read decodes at a time. Every read starts at a
/// keyframe, so short reads mostly decode frames they then throw away.
const CHUNK: u32 = 32;
/// A frame this close ahead of the decoder is reached by decoding on rather
/// than by seeking.
const REACH: u32 = 48;
/// Room the cache keeps beyond the frames it reads ahead.
const SLACK: usize = 4;

/// A movie as decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct MovieInfo {
    /// Size of the decoded pictures: upright, square pixels, and no taller
    /// than asked for.
    pub width: u32,
    pub height: u32,
    /// Frames in the movie. Estimated from its duration until the decoder
    /// reaches the end.
    pub frames: u32,
    /// Frames per second.
    pub fps: f64,
}

impl MovieInfo {
    pub fn duration(&self) -> f64 {
        f64::from(self.frames) / self.fps
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MovieStatus {
    Loading,
    Ready(MovieInfo),
    Failed(String),
}

/// What the decoder should have ready: the loop from `first` to `last`
/// (inclusive), starting at the playhead's `frame` and going on in `order`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub frame: u32,
    pub first: u32,
    pub last: u32,
    pub order: Order,
}

/// The order a playhead goes through a loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Round and round, backward if not `forward`.
    Loop { forward: bool },
    /// Back and forth between the ends; `forward` is the way it's going now.
    Bounce { forward: bool },
    /// To the end and stop.
    Once { forward: bool },
    /// Outward both ways, nearest first: paused or scrubbing.
    Around,
}

impl Want {
    /// Keeps the loop and playhead inside a movie of `frames` frames.
    fn clamped(self, frames: u32) -> Self {
        let last = self.last.min(frames.saturating_sub(1));
        let first = self.first.min(last);
        Self {
            frame: self.frame.clamp(first, last),
            first,
            last,
            order: self.order,
        }
    }

    fn len(&self) -> u32 {
        self.last - self.first + 1
    }

    /// Steps before the frames repeat.
    fn steps(&self) -> u32 {
        match self.order {
            Order::Loop { .. } | Order::Once { .. } => self.len(),
            Order::Bounce { .. } => (2 * (self.len() - 1)).max(1),
            Order::Around => 2 * self.len(),
        }
    }

    /// The frame the playhead reaches `k` steps from now (0 is the one it's
    /// on), and whether it's going backward there. None for steps that fall
    /// outside the loop.
    fn at(&self, k: u32) -> Option<(u32, bool)> {
        let (a, b, p, len) = (self.first, self.last, self.frame, self.len());
        let o = p - a;
        match self.order {
            Order::Loop { forward: true } => Some((a + (o + k % len) % len, false)),
            Order::Loop { forward: false } => Some((a + (o + len - k % len) % len, true)),
            Order::Once { forward: true } => (k <= b - p).then(|| (p + k, false)),
            Order::Once { forward: false } => (k <= o).then(|| (p - k, true)),
            Order::Bounce { .. } if len == 1 => Some((a, false)),
            Order::Bounce { forward } => {
                // Unfolded, the path climbs from `a` to `b` and comes back
                // down over one period.
                let period = 2 * (len - 1);
                let start = if forward || o == 0 { o } else { period - o };
                let s = (start + k % period) % period;
                Some(if s < len {
                    (a + s, false)
                } else {
                    (a + period - s, true)
                })
            }
            Order::Around if k == 0 => Some((p, false)),
            Order::Around if k % 2 == 1 => {
                let n = p + k.div_ceil(2);
                (n <= b).then_some((n, false))
            }
            Order::Around => (k / 2 <= o).then(|| (p - k / 2, true)),
        }
    }

    /// Steps until the playhead reaches frame `n` (the inverse of `at`), or
    /// `u32::MAX` if it never does.
    fn distance(&self, n: u32) -> u32 {
        let (a, b, p, len) = (self.first, self.last, self.frame, self.len());
        if n < a || n > b {
            return u32::MAX;
        }
        match self.order {
            Order::Loop { forward: true } => (n + len - p) % len,
            Order::Loop { forward: false } => (p + len - n) % len,
            Order::Once { forward: true } if n >= p => n - p,
            Order::Once { forward: false } if n <= p => p - n,
            Order::Once { .. } => u32::MAX,
            Order::Bounce { forward: true } if n >= p => n - p,
            Order::Bounce { forward: true } => (b - p) + (b - n),
            Order::Bounce { forward: false } if n <= p => p - n,
            Order::Bounce { forward: false } => (p - a) + (n - a),
            Order::Around if n >= p => (2 * (n - p)).saturating_sub(1),
            Order::Around => 2 * (p - n),
        }
    }
}

struct State {
    status: MovieStatus,
    want: Option<Want>,
    /// Decoded pictures by frame. Frames with no picture of their own (a
    /// gap in the timestamps) share the one before.
    frames: Vec<Option<Arc<Frame>>>,
    /// The frames that hold a picture, for eviction.
    held: Vec<u32>,
    /// Picture bytes held, counting a shared picture once per frame.
    bytes: usize,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    /// The worker waits on this for something to do.
    changed: Condvar,
    /// Signalled when a frame lands or the status changes.
    landed: Condvar,
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                status: MovieStatus::Loading,
                want: None,
                frames: Vec::new(),
                held: Vec::new(),
                bytes: 0,
                stop: false,
            }),
            changed: Condvar::new(),
            landed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_status(&self, status: MovieStatus) {
        let mut s = self.lock();
        if let MovieStatus::Ready(info) = &status {
            s.frames.resize(info.frames as usize, None);
        }
        s.status = status;
        drop(s);
        self.landed.notify_all();
    }
}

/// Movies open now, which share the memory budget.
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// Memory for decoded frames, shared by every open movie: an eighth of the
/// machine's memory, within limits.
fn total_budget() -> usize {
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let ram = platform::memory().unwrap_or(8 << 30);
        (ram / 8).clamp(512 << 20, 4 << 30) as usize
    })
}

fn movie_budget() -> usize {
    total_budget() / OPEN.load(Ordering::Relaxed).max(1)
}

/// A movie file being decoded. Dropping it stops the decoder.
pub struct Movie {
    path: String,
    max_height: u32,
    shared: Arc<Shared>,
}

impl Movie {
    /// Starts decoding the movie at `path`, scaling pictures taller than
    /// `max_height` down to it.
    pub fn open(path: &str, max_height: u32) -> Self {
        let shared = Arc::new(Shared::new());
        OPEN.fetch_add(1, Ordering::Relaxed);
        let worker = shared.clone();
        let file = path.to_owned();
        let spawned = std::thread::Builder::new()
            .name("dov-movie".into())
            .spawn(move || match platform::Decoder::open(&file, max_height) {
                Ok((decoder, info)) => {
                    log::info!(
                        "movie: {file}: {}×{}, {} frames at {:.3} fps",
                        info.width,
                        info.height,
                        info.frames,
                        info.fps
                    );
                    let bytes = info.width as usize * info.height as usize * 4;
                    worker.set_status(MovieStatus::Ready(info));
                    Worker::new(decoder, worker, bytes, movie_budget).run();
                }
                Err(e) => {
                    log::warn!("movie: {file}: {e}");
                    worker.set_status(MovieStatus::Failed(e));
                }
            });
        if let Err(e) = spawned {
            shared.set_status(MovieStatus::Failed(e.to_string()));
        }
        Self {
            path: path.to_owned(),
            max_height,
            shared,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn max_height(&self) -> u32 {
        self.max_height
    }

    pub fn status(&self) -> MovieStatus {
        self.shared.lock().status.clone()
    }

    /// Tells the decoder what to have ready next.
    pub fn want(&self, want: Want) {
        let mut s = self.shared.lock();
        if s.want != Some(want) {
            s.want = Some(want);
            drop(s);
            self.shared.changed.notify_one();
        }
    }

    /// Frame `n`'s picture, if it has been decoded.
    pub fn frame(&self, n: u32) -> Option<Arc<Frame>> {
        self.shared.lock().frames.get(n as usize)?.clone()
    }

    /// Waits up to `timeout` for the movie to finish loading.
    pub fn wait_loaded(&self, timeout: Duration) -> MovieStatus {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        while s.status == MovieStatus::Loading {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s = self
                .shared
                .landed
                .wait_timeout(s, left)
                .map_or_else(|e| e.into_inner().0, |r| r.0);
        }
        s.status.clone()
    }

    /// Waits up to `timeout` for frame `n`, once it's wanted.
    pub fn wait_for(&self, n: u32, timeout: Duration) -> Option<Arc<Frame>> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if let Some(f) = s.frames.get(n as usize).and_then(Clone::clone) {
                return Some(f);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || matches!(s.status, MovieStatus::Failed(_)) {
                return None;
            }
            s = self
                .shared
                .landed
                .wait_timeout(s, left)
                .map_or_else(|e| e.into_inner().0, |r| r.0);
        }
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::Relaxed);
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
    }
}

/// A source of decoded frames.
trait Decode {
    /// Restarts decoding at frame `frame`.
    fn seek(&mut self, frame: u32) -> Result<(), String>;
    /// The next frame in presentation order, with its index, or None at the
    /// end of the movie.
    fn next(&mut self) -> Result<Option<(u32, Frame)>, String>;
}

/// The next frame to decode.
#[derive(Debug, PartialEq)]
struct Target {
    frame: u32,
    /// Where to start reading if the decoder has to seek: before the frame
    /// when it's played backward, so one read fills a run of frames.
    from: u32,
}

/// A read in progress.
struct Run {
    start: u32,
    /// The frame the decoder is expected to return next.
    next: u32,
    last: Option<(u32, Arc<Frame>)>,
}

struct Worker<D> {
    decoder: D,
    shared: Arc<Shared>,
    /// Bytes in one picture.
    frame_bytes: usize,
    budget: fn() -> usize,
    run: Option<Run>,
    /// The frame last sought, and how many times running.
    sought: (u32, u32),
    errors: u32,
}

impl<D: Decode> Worker<D> {
    fn new(decoder: D, shared: Arc<Shared>, frame_bytes: usize, budget: fn() -> usize) -> Self {
        Self {
            decoder,
            shared,
            frame_bytes: frame_bytes.max(1),
            budget,
            run: None,
            sought: (u32::MAX, 0),
            errors: 0,
        }
    }

    fn run(mut self) {
        while let Some(target) = self.wait() {
            self.step(&target);
        }
    }

    /// Waits for a frame to decode. None once the movie is closed or failed.
    fn wait(&self) -> Option<Target> {
        let mut s = self.shared.lock();
        loop {
            if s.stop || matches!(s.status, MovieStatus::Failed(_)) {
                return None;
            }
            if let Some(t) = self.target(&s) {
                return Some(t);
            }
            s = self.shared.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Frames the cache holds, beyond the slack.
    fn lookahead(&self, want: &Want) -> u32 {
        let fit = ((self.budget)() / self.frame_bytes).saturating_sub(SLACK).max(1);
        want.steps().min(u32::try_from(fit).unwrap_or(u32::MAX))
    }

    /// The first frame the playhead will reach that isn't decoded yet.
    fn target(&self, s: &State) -> Option<Target> {
        let frames = u32::try_from(s.frames.len()).ok().filter(|&n| n > 0)?;
        let want = s.want?.clamped(frames);
        let ahead = self.lookahead(&want);
        let missing = |n: u32| s.frames[n as usize].is_none();
        let (frame, backward) = (0..ahead).filter_map(|k| want.at(k)).find(|&(n, _)| missing(n))?;
        if !backward {
            return Some(Target { frame, from: frame });
        }
        // Played backward: read the run of missing frames that leads up to
        // it, as far as the playhead will get soon.
        let chunk = CHUNK.min(ahead);
        let mut from = frame;
        while from > want.first
            && frame - from + 1 < chunk
            && missing(from - 1)
            && want.distance(from - 1) < ahead
        {
            from -= 1;
        }
        // A short run can wait to grow, unless the playhead is nearly there.
        let urgent = want.distance(frame) < (ahead - chunk).max(ahead / 4).max(1);
        (frame - from + 1 >= chunk || urgent).then_some(Target { frame, from })
    }

    /// Decodes one frame on the way to `target`.
    fn step(&mut self, target: &Target) {
        let on_the_way = self
            .run
            .as_ref()
            .is_some_and(|r| r.next <= target.frame && target.frame < r.next + REACH);
        if !on_the_way {
            self.sought = match self.sought {
                (f, n) if f == target.frame => (f, n + 1),
                _ => (target.frame, 1),
            };
            if self.sought.1 > 3 {
                // Seeking there never produces it: show its neighbor.
                self.stand_in(target.frame);
                return;
            }
            if let Err(e) = self.decoder.seek(target.from) {
                self.error(e);
                return;
            }
            self.run = Some(Run {
                start: target.from,
                next: target.from,
                last: None,
            });
        }
        match self.decoder.next() {
            Ok(Some((n, frame))) => {
                self.errors = 0;
                self.land(n, Arc::new(frame));
            }
            Ok(None) => self.end(),
            Err(e) => self.error(e),
        }
    }

    fn land(&mut self, n: u32, frame: Arc<Frame>) {
        let Some(run) = self.run.as_mut() else { return };
        // Frames between the last one and this had no picture of their own;
        // they hold the last one. At the start of a read they take this one.
        let (gap, fill) = match &run.last {
            Some((m, last)) if *m < n => (m + 1..n, last.clone()),
            Some(_) => (n..n, frame.clone()),
            None => (run.start.min(n)..n, frame.clone()),
        };
        run.next = run.next.max(n + 1);
        run.last = Some((n, frame.clone()));

        let budget = (self.budget)();
        let mut s = self.shared.lock();
        if n as usize >= s.frames.len() {
            s.frames.resize(n as usize + 1, None);
            set_frames(&mut s, n + 1);
        }
        let Some(want) = s.want.map(|w| w.clamped(s.frames.len() as u32)) else {
            return;
        };
        let ahead = self.lookahead(&want);
        for i in gap {
            self.insert(&mut s, &want, ahead, i, fill.clone());
        }
        self.insert(&mut s, &want, ahead, n, frame);
        while s.bytes > budget && self.evict(&mut s, &want) {}
        drop(s);
        self.shared.landed.notify_all();
    }

    fn insert(&self, s: &mut State, want: &Want, ahead: u32, n: u32, frame: Arc<Frame>) {
        let slot = &mut s.frames[n as usize];
        // Frames the playhead won't reach soon would only push out ones it will.
        if slot.is_some() || want.distance(n) >= ahead {
            return;
        }
        *slot = Some(frame);
        s.held.push(n);
        s.bytes += self.frame_bytes;
    }

    /// Drops the frame the playhead will reach last. False if none is held.
    fn evict(&self, s: &mut State, want: &Want) -> bool {
        let key = |n: u32| (want.distance(n), n.abs_diff(want.frame));
        let Some((i, _)) = s.held.iter().enumerate().max_by_key(|&(_, &n)| key(n)) else {
            return false;
        };
        let n = s.held.swap_remove(i);
        s.frames[n as usize] = None;
        s.bytes -= self.frame_bytes;
        true
    }

    /// The decoder ran out of frames: the movie ends where the read did.
    fn end(&mut self) {
        let Some(run) = self.run.take() else { return };
        let end = match run.last {
            Some((m, _)) => m + 1,
            None if run.start == 0 => {
                self.fail("no pictures in this file".into());
                return;
            }
            None => run.start,
        };
        let mut s = self.shared.lock();
        if (end as usize) < s.frames.len() {
            s.frames.truncate(end as usize);
            let held = std::mem::take(&mut s.held);
            s.held = held.into_iter().filter(|&n| n < end).collect();
            s.bytes = s.held.len() * self.frame_bytes;
            set_frames(&mut s, end);
            drop(s);
            self.shared.landed.notify_all();
        }
    }

    /// Fills frame `n` with the nearest decoded picture.
    fn stand_in(&mut self, n: u32) {
        let mut s = self.shared.lock();
        let len = s.frames.len() as u32;
        let near = (1..len).find_map(|d| {
            let before = n.checked_sub(d).and_then(|i| s.frames.get(i as usize)?.clone());
            before.or_else(|| s.frames.get((n + d) as usize)?.clone())
        });
        let Some(picture) = near else {
            drop(s);
            self.fail(format!("can't decode frame {n}"));
            return;
        };
        log::warn!("movie: frame {n} won't decode; showing its neighbor");
        if let Some(slot) = s.frames.get_mut(n as usize) {
            *slot = Some(picture);
            s.held.push(n);
            s.bytes += self.frame_bytes;
        }
        drop(s);
        self.shared.landed.notify_all();
    }

    fn error(&mut self, e: String) {
        self.run = None;
        self.errors += 1;
        log::warn!("movie: {e}");
        if self.errors >= 3 {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: String) {
        self.run = None;
        self.shared.set_status(MovieStatus::Failed(e));
    }
}

fn set_frames(s: &mut State, frames: u32) {
    if let MovieStatus::Ready(info) = &mut s.status {
        info.frames = frames;
    }
}

/// How a movie's pictures are turned for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    Upright,
    /// A quarter turn clockwise, as phones record portrait video.
    Right,
    Half,
    Left,
}

impl Turn {
    /// From a track's preferred transform (a b c d of the affine matrix).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn of(a: f64, b: f64, c: f64, d: f64) -> Self {
        match (
            a.round() as i32,
            b.round() as i32,
            c.round() as i32,
            d.round() as i32,
        ) {
            (0, 1, -1, 0) => Self::Right,
            (0, -1, 1, 0) => Self::Left,
            (-1, 0, 0, -1) => Self::Half,
            _ => Self::Upright,
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn sideways(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }
}

/// Copies `height` rows of `width` BGRA pixels, `stride` bytes apart,
/// turning them upright.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn upright(data: &[u8], width: usize, height: usize, stride: usize, turn: Turn) -> Option<Frame> {
    let row = width * 4;
    if width == 0 || height == 0 || stride < row || data.len() < stride * (height - 1) + row {
        return None;
    }
    let pixel = |x: usize, y: usize| &data[y * stride + x * 4..y * stride + x * 4 + 4];
    let (w, h) = if turn.sideways() {
        (height, width)
    } else {
        (width, height)
    };
    let mut bgra = Vec::with_capacity(w * h * 4);
    match turn {
        Turn::Upright => {
            for y in 0..height {
                bgra.extend_from_slice(&data[y * stride..y * stride + row]);
            }
        }
        Turn::Half => {
            for y in (0..height).rev() {
                for x in (0..width).rev() {
                    bgra.extend_from_slice(pixel(x, y));
                }
            }
        }
        // Shown column x is stored row height-1-x, read top to bottom.
        Turn::Right => {
            for y in 0..h {
                for x in 0..w {
                    bgra.extend_from_slice(pixel(y, height - 1 - x));
                }
            }
        }
        Turn::Left => {
            for y in 0..h {
                for x in 0..w {
                    bgra.extend_from_slice(pixel(width - 1 - y, x));
                }
            }
        }
    }
    Some(Frame {
        width: w as u32,
        height: h as u32,
        bgra,
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeVideo,
        AVURLAsset, AVURLAssetPreferPreciseDurationAndTimingKey,
    };
    use objc2_core_media::{
        CMTime, CMTimeRange, CMVideoFormatDescription, CMVideoFormatDescriptionGetPresentationDimensions,
        kCMTimePositiveInfinity,
    };
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey,
        kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    use super::{Decode, MovieInfo, Turn, upright};
    use crate::Frame;

    /// Timescale for the times we ask the reader for.
    const TIMESCALE: i32 = 1_000_000;

    /// Installed memory, in bytes.
    pub(super) fn memory() -> Option<u64> {
        let mut bytes: u64 = 0;
        let mut len = size_of::<u64>();
        // SAFETY: hw.memsize is a u64, and `len` says how big the buffer is.
        let r = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut bytes).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0 && bytes > 0).then_some(bytes)
    }

    fn describe(e: &objc2_foundation::NSError) -> String {
        e.localizedDescription().to_string()
    }

    pub(super) struct Decoder {
        asset: Retained<AVURLAsset>,
        track: Retained<AVAssetTrack>,
        settings: Retained<NSDictionary<NSString, AnyObject>>,
        /// Track time of frame 0, in seconds.
        start: f64,
        fps: f64,
        turn: Turn,
        reader: Option<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>)>,
    }

    impl Decoder {
        pub(super) fn open(path: &str, max_height: u32) -> Result<(Self, MovieInfo), String> {
            if !std::path::Path::new(path).is_file() {
                return Err("file not found".into());
            }
            // SAFETY: AVFoundation calls with valid arguments, all made from
            // this thread; the format description cast follows CoreMedia's
            // documented type for a video track's descriptions.
            autoreleasepool(|_| unsafe {
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                let precise = NSNumber::new_bool(true);
                let value: &AnyObject = precise.as_ref();
                let options =
                    NSDictionary::from_slices(&[AVURLAssetPreferPreciseDurationAndTimingKey], &[value]);
                let asset = AVURLAsset::URLAssetWithURL_options(&url, Some(&options));
                let video = AVMediaTypeVideo.ok_or("no video media type")?;
                #[allow(deprecated)]
                let track = asset
                    .tracksWithMediaType(video)
                    .firstObject()
                    .ok_or("no video in this file")?;
                if !track.isPlayable() {
                    return Err("this Mac can't decode the file's video".into());
                }

                let range = track.timeRange();
                let start = range.start.seconds();
                let duration = range.duration.seconds();
                let mut fps = f64::from(track.nominalFrameRate());
                if !fps.is_finite() || fps <= 0.0 {
                    let min = track.minFrameDuration().seconds();
                    fps = if min > 0.0 { 1.0 / min } else { 30.0 };
                }
                if !duration.is_finite() || duration <= 0.0 {
                    return Err("the video is empty".into());
                }
                let frames = (duration * fps).round().max(1.0) as u32;

                // Stored size with the pixel aspect ratio applied.
                let natural = track.naturalSize();
                let (mut w, mut h) = (natural.width, natural.height);
                if let Some(desc) = track.formatDescriptions().firstObject() {
                    let desc = &*Retained::as_ptr(&desc).cast::<CMVideoFormatDescription>();
                    let size = CMVideoFormatDescriptionGetPresentationDimensions(desc, true, false);
                    if size.width > 0.0 && size.height > 0.0 {
                        (w, h) = (size.width, size.height);
                    }
                }
                let t = track.preferredTransform();
                let turn = Turn::of(t.a, t.b, t.c, t.d);
                let shown_h = if turn.sideways() { w } else { h };
                let scale = (f64::from(max_height) / shown_h).min(1.0);
                let even = |v: f64| ((v * scale / 2.0).round() as u32).max(1) * 2;
                let (dw, dh) = (even(w), even(h));

                let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
                let (width, height) = (NSNumber::new_u32(dw), NSNumber::new_u32(dh));
                let key = |k: &objc2_core_foundation::CFString| &*(k as *const _ as *const NSString);
                let values: [&AnyObject; 3] = [format.as_ref(), width.as_ref(), height.as_ref()];
                let settings = NSDictionary::from_slices(
                    &[
                        key(kCVPixelBufferPixelFormatTypeKey),
                        key(kCVPixelBufferWidthKey),
                        key(kCVPixelBufferHeightKey),
                    ],
                    &values,
                );
                let (width, height) = if turn.sideways() { (dh, dw) } else { (dw, dh) };
                let info = MovieInfo {
                    width,
                    height,
                    frames,
                    fps,
                };
                let decoder = Self {
                    asset,
                    track,
                    settings,
                    start,
                    fps,
                    turn,
                    reader: None,
                };
                Ok((decoder, info))
            })
        }

        fn stop(&mut self) {
            if let Some((reader, _)) = self.reader.take() {
                // SAFETY: cancelling a reader this thread started.
                unsafe { reader.cancelReading() };
            }
        }
    }

    impl Decode for Decoder {
        fn seek(&mut self, frame: u32) -> Result<(), String> {
            self.stop();
            // SAFETY: AVFoundation calls with valid arguments, made from this thread.
            autoreleasepool(|_| unsafe {
                let reader =
                    AVAssetReader::assetReaderWithAsset_error(&self.asset).map_err(|e| describe(&e))?;
                let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                    &self.track,
                    Some(&self.settings),
                );
                output.setAlwaysCopiesSampleData(false);
                // Half a frame early, so rounding lands on the frame itself.
                let t = self.start + (f64::from(frame) - 0.5).max(0.0) / self.fps;
                reader.setTimeRange(CMTimeRange {
                    start: CMTime::with_seconds(t, TIMESCALE),
                    duration: kCMTimePositiveInfinity,
                });
                if !reader.canAddOutput(&output) {
                    return Err("can't read the video track".into());
                }
                reader.addOutput(&output);
                if !reader.startReading() {
                    return Err(reader
                        .error()
                        .map_or_else(|| "can't start reading".into(), |e| describe(&e)));
                }
                self.reader = Some((reader, output));
                Ok(())
            })
        }

        fn next(&mut self) -> Result<Option<(u32, Frame)>, String> {
            let Some((reader, output)) = &self.reader else {
                return Ok(None);
            };
            // SAFETY: reading a started reader from the thread that started
            // it; pixel buffers are locked read-only around the copy.
            autoreleasepool(|_| unsafe {
                loop {
                    let Some(sample) = output.copyNextSampleBuffer() else {
                        return match reader.status() {
                            AVAssetReaderStatus::Failed => Err(reader
                                .error()
                                .map_or_else(|| "decoding failed".into(), |e| describe(&e))),
                            _ => Ok(None),
                        };
                    };
                    // Samples without a picture (empty edits) are skipped.
                    let Some(pixels) = sample.image_buffer() else {
                        continue;
                    };
                    if CVPixelBufferGetPixelFormatType(&pixels) != kCVPixelFormatType_32BGRA {
                        continue;
                    }
                    let pts = sample.presentation_time_stamp().seconds();
                    let n = ((pts - self.start) * self.fps).round().max(0.0) as u32;
                    if CVPixelBufferLockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly) != 0 {
                        continue;
                    }
                    let base = CVPixelBufferGetBaseAddress(&pixels);
                    let stride = CVPixelBufferGetBytesPerRow(&pixels);
                    let (w, h) = (CVPixelBufferGetWidth(&pixels), CVPixelBufferGetHeight(&pixels));
                    let frame = (!base.is_null() && h > 0).then(|| {
                        let data = std::slice::from_raw_parts(base as *const u8, stride * h);
                        upright(data, w, h, stride, self.turn)
                    });
                    CVPixelBufferUnlockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly);
                    if let Some(Some(frame)) = frame {
                        return Ok(Some((n, frame)));
                    }
                }
            })
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Decode, MovieInfo};
    use crate::Frame;

    pub(super) fn memory() -> Option<u64> {
        None
    }

    pub(super) enum Decoder {}

    impl Decoder {
        pub(super) fn open(_path: &str, _max_height: u32) -> Result<(Self, MovieInfo), String> {
            Err("movie playback isn't implemented on this platform yet".into())
        }
    }

    impl Decode for Decoder {
        fn seek(&mut self, _frame: u32) -> Result<(), String> {
            match *self {}
        }

        fn next(&mut self) -> Result<Option<(u32, Frame)>, String> {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(frame: u32, first: u32, last: u32, order: Order) -> Want {
        Want {
            frame,
            first,
            last,
            order,
        }
    }

    #[test]
    fn steps_follow_the_playhead_and_distance_inverts_them() {
        let orders = [
            Order::Loop { forward: true },
            Order::Loop { forward: false },
            Order::Bounce { forward: true },
            Order::Bounce { forward: false },
            Order::Once { forward: true },
            Order::Once { forward: false },
            Order::Around,
        ];
        for order in orders {
            for frame in 3..=9 {
                let w = want(frame, 3, 9, order);
                for k in 0..w.steps() {
                    if let Some((n, _)) = w.at(k) {
                        assert!((3..=9).contains(&n), "{order:?} from {frame}: step {k} at {n}");
                        assert_eq!(
                            w.distance(n),
                            k.min(w.distance(n)),
                            "{order:?} from {frame} to {n}"
                        );
                    }
                }
                // Every frame the playhead reaches comes first at its distance.
                for n in 3..=9 {
                    let d = w.distance(n);
                    if d != u32::MAX {
                        assert_eq!(w.at(d).map(|s| s.0), Some(n), "{order:?} from {frame} to {n}");
                    }
                }
            }
        }
        let w = want(5, 3, 9, Order::Bounce { forward: true });
        let path: Vec<u32> = (0..8).filter_map(|k| w.at(k).map(|s| s.0)).collect();
        assert_eq!(path, [5, 6, 7, 8, 9, 8, 7, 6]);
        let w = want(5, 3, 9, Order::Loop { forward: false });
        let path: Vec<u32> = (0..7).filter_map(|k| w.at(k).map(|s| s.0)).collect();
        assert_eq!(path, [5, 4, 3, 9, 8, 7, 6]);
        assert_eq!(want(5, 3, 9, Order::Once { forward: true }).distance(4), u32::MAX);
    }

    /// A pretend movie: each picture is one pixel whose blue byte is its
    /// frame number. Decoding starts at the keyframe before the seek, every
    /// `gop` frames, and counts the pictures it decodes.
    struct Fake {
        frames: u32,
        gop: u32,
        /// Frames with no picture of their own.
        holes: Vec<u32>,
        next: u32,
        from: u32,
        decoded: u32,
        seeks: u32,
    }

    impl Fake {
        fn new(frames: u32, gop: u32) -> Self {
            Self {
                frames,
                gop,
                holes: Vec::new(),
                next: 0,
                from: 0,
                decoded: 0,
                seeks: 0,
            }
        }
    }

    impl Decode for Fake {
        fn seek(&mut self, frame: u32) -> Result<(), String> {
            self.seeks += 1;
            self.from = frame;
            self.next = frame / self.gop * self.gop;
            Ok(())
        }

        fn next(&mut self) -> Result<Option<(u32, Frame)>, String> {
            loop {
                if self.next >= self.frames {
                    return Ok(None);
                }
                let n = self.next;
                self.next += 1;
                self.decoded += 1;
                if n >= self.from && !self.holes.contains(&n) {
                    let bgra = vec![n as u8, 0, 0, 255];
                    return Ok(Some((
                        n,
                        Frame {
                            width: 1,
                            height: 1,
                            bgra,
                        },
                    )));
                }
            }
        }
    }

    /// Room for 40 one-pixel frames.
    fn budget() -> usize {
        (40 + SLACK) * 4
    }

    fn worker(fake: Fake, estimate: u32) -> Worker<Fake> {
        let shared = Arc::new(Shared::new());
        shared.set_status(MovieStatus::Ready(MovieInfo {
            width: 1,
            height: 1,
            frames: estimate,
            fps: 30.0,
        }));
        Worker::new(fake, shared, 4, budget)
    }

    impl Worker<Fake> {
        fn set_want(&self, w: Want) {
            self.shared.lock().want = Some(w);
        }

        /// Decodes until nothing more is wanted.
        fn settle(&mut self) {
            for _ in 0..10_000 {
                let target = self.target(&self.shared.lock());
                match target {
                    Some(t) => self.step(&t),
                    None => return,
                }
            }
            panic!("the decoder never settled");
        }

        fn held(&self) -> Vec<u32> {
            let s = self.shared.lock();
            (0..s.frames.len() as u32)
                .filter(|&n| s.frames[n as usize].is_some())
                .collect()
        }

        fn shows(&self, n: u32) -> Option<u8> {
            self.shared.lock().frames[n as usize].as_ref().map(|f| f.bgra[0])
        }
    }

    #[test]
    fn a_loop_that_fits_is_decoded_once_then_plays_from_memory() {
        let mut w = worker(Fake::new(300, 12), 300);
        w.set_want(want(100, 100, 129, Order::Loop { forward: true }));
        w.settle();
        assert_eq!(w.held(), (100..130).collect::<Vec<_>>());
        assert_eq!(w.decoder.seeks, 1, "one read covers the loop");
        let decoded = w.decoder.decoded;
        for frame in 100..130 {
            w.set_want(want(frame, 100, 129, Order::Loop { forward: true }));
            w.settle();
        }
        assert_eq!(w.decoder.decoded, decoded, "nothing decoded again");
    }

    #[test]
    fn backward_play_reads_in_chunks() {
        let mut w = worker(Fake::new(300, 12), 300);
        for frame in (0..=299).rev() {
            w.set_want(want(frame, 0, 299, Order::Loop { forward: false }));
            w.settle();
            assert_eq!(w.shows(frame), Some(frame as u8));
        }
        // Without chunks it would seek once a frame.
        assert!(w.decoder.seeks < 300 / 20, "{} seeks", w.decoder.seeks);
    }

    #[test]
    fn a_long_movie_keeps_the_frames_just_ahead() {
        let mut w = worker(Fake::new(1000, 12), 1000);
        for frame in 0..500 {
            w.set_want(want(frame, 0, 999, Order::Loop { forward: true }));
            w.settle();
            assert_eq!(w.shows(frame), Some(frame as u8));
        }
        let held = w.held();
        assert!((499..539).all(|n| held.contains(&n)), "{held:?}");
        assert!(held.len() <= 40 + SLACK, "{held:?}");
        assert_eq!(w.decoder.seeks, 1);
    }

    #[test]
    fn scrubbing_fills_around_the_playhead() {
        let mut w = worker(Fake::new(300, 12), 300);
        w.set_want(want(150, 0, 299, Order::Around));
        w.settle();
        assert_eq!(w.held(), (131..171).collect::<Vec<_>>());
    }

    #[test]
    fn the_movie_ends_where_the_pictures_do() {
        // The duration promised 120 frames; there are 100.
        let mut w = worker(Fake::new(100, 12), 120);
        w.set_want(want(90, 0, 119, Order::Loop { forward: true }));
        w.settle();
        let MovieStatus::Ready(info) = w.shared.lock().status.clone() else {
            panic!("not ready");
        };
        assert_eq!(info.frames, 100);
        assert_eq!(w.shows(99), Some(99));
        assert_eq!(w.shows(0), Some(0), "wrapped around to the start");
    }

    #[test]
    fn frames_without_a_picture_hold_the_one_before() {
        let mut fake = Fake::new(60, 12);
        fake.holes = vec![20, 21, 40];
        let mut w = worker(fake, 60);
        w.set_want(want(10, 0, 59, Order::Loop { forward: true }));
        w.settle();
        assert_eq!(w.shows(20), Some(19));
        assert_eq!(w.shows(21), Some(19));
        assert_eq!(w.shows(40), Some(39));
        // Seeking right onto a hole shows the next picture.
        let mut fake = Fake::new(60, 12);
        fake.holes = vec![30];
        let mut w = worker(fake, 60);
        w.set_want(want(30, 30, 35, Order::Once { forward: true }));
        w.settle();
        assert_eq!(w.shows(30), Some(31));
    }

    #[test]
    fn pictures_turn_upright() {
        // 3×2, pixels numbered in reading order.
        let data: Vec<u8> = (0..6).flat_map(|i| [i, 0, 0, 255]).collect();
        let turned = |turn| {
            let f = upright(&data, 3, 2, 12, turn).unwrap();
            let order: Vec<u8> = f.bgra.chunks(4).map(|p| p[0]).collect();
            (f.width, f.height, order)
        };
        assert_eq!(turned(Turn::Upright), (3, 2, vec![0, 1, 2, 3, 4, 5]));
        assert_eq!(turned(Turn::Half), (3, 2, vec![5, 4, 3, 2, 1, 0]));
        // 0 1 2      3 0
        // 3 4 5  ->  4 1
        //            5 2
        assert_eq!(turned(Turn::Right), (2, 3, vec![3, 0, 4, 1, 5, 2]));
        assert_eq!(turned(Turn::Left), (2, 3, vec![2, 5, 1, 4, 0, 3]));
    }

    /// Decodes the test movie, whose frame `n` is gray level 16 + 3n with a
    /// white block in the top-left corner.
    #[cfg(target_os = "macos")]
    #[test]
    fn decodes_the_test_movie() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/count.mov");
        let movie = Movie::open(path, 480);
        let MovieStatus::Ready(info) = movie.wait_loaded(Duration::from_secs(10)) else {
            panic!("{:?}", movie.status());
        };
        assert_eq!((info.width, info.height, info.frames), (64, 48, 60));
        assert!((info.fps - 30.0).abs() < 1e-6);
        let level = |f: &Frame| {
            // A pixel away from the corner block.
            let p = &f.bgra[(30 * 64 + 40) * 4..];
            (f32::from(p[1]) - 16.0) / 3.0
        };
        // Backward, so every read has to start before the frame it wants.
        for n in (0..60).rev() {
            movie.want(Want {
                frame: n,
                first: 0,
                last: 59,
                order: Order::Loop { forward: false },
            });
            let f = movie.wait_for(n, Duration::from_secs(5)).expect("frame decodes");
            assert!(
                (level(&f) - n as f32).abs() < 0.6,
                "frame {n} looks like {}",
                level(&f)
            );
            assert!(f.bgra[1] > 200, "the corner block is top left");
        }
    }
}
