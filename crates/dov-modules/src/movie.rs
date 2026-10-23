//! Movie: plays a movie file in a loop between In and Out, at any speed
//! (backward too), back and forth or once, scrubbed by CV, held to a frame
//! rate, or locked to the beat grid. Splices cut the movie into pieces, as
//! on a Morphagene: Organize picks the one to play and Next steps on.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dov_io::{Frame, Movie, MovieStatus, Order, Want};

use crate::behaviors::{beat_hz, cycles_per_beat};
use crate::{Behavior, FieldCtx, FieldIo};

// Params, in the order `library::MOVIE` declares them. The file is the
// module's text: its path.
const SPEED: usize = 0;
const IN: usize = 1;
const OUT: usize = 2;
const ORGANIZE: usize = 3;
const LOOP: usize = 4;
const MODE: usize = 5;
const POSITION: usize = 6;
const FPS: usize = 7;
const SYNC: usize = 8;
const LENGTH: usize = 9;

// Inputs and outputs.
const RESTART: usize = 0;
const NEXT: usize = 1;
const POS_OUT: usize = 1;
const END_OUT: usize = 2;

/// Slower than this is standing still.
const STILL: f64 = 1e-4;
/// While recording, how long to wait for the decoder before going on.
const RECORD_WAIT: Duration = Duration::from_secs(10);

/// Where a movie module is in its movie, for its editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    /// Frames in the movie, and how many a second.
    pub frames: u32,
    pub fps: f64,
    /// The splice playing, counted from 0.
    pub splice: usize,
    /// The loop as it plays, CV included: its first and last frame.
    pub first: u32,
    pub last: u32,
    /// The frame showing.
    pub frame: u32,
}

/// The first frame of each splice of a movie `frames` long at `fps`, cut
/// at `marks` (seconds). The first splice starts the movie.
pub fn splice_starts(marks: &[f64], fps: f64, frames: u32) -> Vec<u32> {
    let mut starts = vec![0];
    starts.extend(
        marks
            .iter()
            .map(|&s| (s * fps).round())
            .filter(|&f| f >= 1.0 && f < f64::from(frames))
            .map(|f| f as u32),
    );
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// The loop In and Out make of a splice from frame `bounds.0` up to
/// `bounds.1`: its first and last frame. In and Out are percentages of the
/// splice, either way round, rounded to the nearest frame so a mark set on
/// a frame stays on it.
pub fn loop_frames(in_point: f32, out_point: f32, bounds: (u32, u32)) -> (u32, u32) {
    let (start, end) = bounds;
    let len = end - start;
    let percent = |v: f32| f64::from(v / 100.0).clamp(0.0, 1.0);
    let (a, b) = (percent(in_point), percent(out_point));
    let first = start + ((a.min(b) * f64::from(len)).round() as u32).min(len - 1);
    let last = (start + (a.max(b) * f64::from(len)).round() as u32)
        .saturating_sub(1)
        .clamp(first, end - 1);
    (first, last)
}

#[derive(Default)]
pub(crate) struct MoviePlayer {
    movie: Option<Movie>,
    transport: Transport,
    splicer: Splicer,
    picture: Option<Arc<Frame>>,
    warning: Option<String>,
    note: Option<String>,
    timeline: Option<Timeline>,
}

impl Behavior for MoviePlayer {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo) {
        io.outputs[POS_OUT] = 0.0;
        io.outputs[END_OUT] = 0.0;
        let path = io.params.text();
        let max_height = ctx.raster.active_size().1;
        let stale = self.movie.as_ref().map_or(!path.is_empty(), |m| {
            m.path() != path || m.max_height() != max_height
        });
        if stale {
            // Close the old movie first, so its memory is free for the new one.
            self.movie = None;
            self.picture = None;
            self.transport = Transport::default();
            if !path.is_empty() {
                self.movie = Some(Movie::open(&path, max_height));
            }
        }
        self.note = None;
        self.timeline = None;
        let Some(movie) = &self.movie else {
            self.warning = None;
            return;
        };
        let mut status = movie.status();
        if ctx.offline && status == MovieStatus::Loading {
            status = movie.wait_loaded(RECORD_WAIT);
        }
        let info = match status {
            MovieStatus::Ready(info) => info,
            MovieStatus::Loading => {
                self.note = Some("Loading…".into());
                return;
            }
            MovieStatus::Failed(e) => {
                let name = Path::new(&path)
                    .file_name()
                    .map_or(path.as_str(), |n| n.to_str().unwrap_or_default());
                self.warning = Some(format!("Movie: {name}: {e}"));
                return;
            }
        };
        self.warning = None;

        let frames = info.frames.max(1);
        let starts = splice_starts(&io.params.marks(), info.fps, frames);
        let splice = self
            .splicer
            .pick(io.param(ORGANIZE), io.inputs[NEXT] > 0.5, starts.len());
        let bounds = (starts[splice], starts.get(splice + 1).copied().unwrap_or(frames));
        let controls = Controls::read(ctx, io, frames, splice, bounds);
        let step = self
            .transport
            .step(&controls, info.fps, ctx.elapsed_seconds(), ctx.seconds());
        let (first, last) = match controls.cue {
            // Held anywhere in the movie: read around it.
            Some(_) => (0, frames - 1),
            None => (controls.first, controls.last),
        };
        movie.want(Want {
            frame: step.frame,
            first,
            last,
            order: step.order,
        });
        let frame = movie.frame(step.frame).or_else(|| {
            // Recording: every frame has to be the right one.
            ctx.offline
                .then(|| movie.wait_for(step.frame, RECORD_WAIT))
                .flatten()
        });
        // Until a frame is decoded, the last one stays up.
        if frame.is_some() {
            self.picture = frame;
        }
        io.outputs[POS_OUT] = step.phase;
        io.outputs[END_OUT] = f32::from(u8::from(step.end));
        let at = |frame: u32| clock(f64::from(frame) / info.fps);
        let of = if starts.len() > 1 {
            format!("Splice {} of {} · ", splice + 1, starts.len())
        } else {
            String::new()
        };
        self.note = Some(format!(
            "{}×{}, {} fps, {}\n{of}In {} · Out {} · at {}",
            info.width,
            info.height,
            trim(info.fps),
            clock(info.duration()),
            at(controls.first),
            at(controls.last + 1),
            at(step.frame),
        ));
        self.timeline = Some(Timeline {
            frames,
            fps: info.fps,
            splice,
            first: controls.first,
            last: controls.last,
            frame: step.frame,
        });
    }

    fn warning(&self) -> Option<String> {
        self.warning.clone()
    }

    fn note(&self) -> Option<String> {
        self.note.clone()
    }

    fn picture(&self) -> Option<Arc<Frame>> {
        self.picture.clone()
    }

    fn timeline(&self) -> Option<Timeline> {
        self.timeline
    }
}

/// Picks the splice to play: the one Organize points at, stepped on by
/// Next. Turning Organize to another splice starts counting again.
#[derive(Default)]
struct Splicer {
    organized: Option<usize>,
    stepped: usize,
    next_held: bool,
}

impl Splicer {
    fn pick(&mut self, organize: f32, next: bool, count: usize) -> usize {
        let count = count.max(1);
        let picked = ((organize.clamp(0.0, 1.0) * count as f32) as usize).min(count - 1);
        if self.organized != Some(picked) {
            self.organized = Some(picked);
            self.stepped = 0;
        }
        if next && !self.next_held {
            self.stepped = (self.stepped + 1) % count;
        }
        self.next_held = next;
        (picked + self.stepped) % count
    }
}

/// Minutes, seconds and hundredths: 1:02.50.
fn clock(seconds: f64) -> String {
    let hundredths = (seconds.max(0.0) * 100.0).round() as u64;
    let (minutes, rest) = (hundredths / 6000, hundredths % 6000);
    format!("{minutes}:{:02}.{:02}", rest / 100, rest % 100)
}

/// A rate without trailing zeros: 30, 29.97.
fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopMode {
    Loop,
    Bounce,
    Once,
}

/// One field's settings, in frames.
#[derive(Clone, Debug)]
struct Controls {
    /// Times the movie's own speed; negative plays backward.
    speed: f64,
    /// The splice playing; a new one starts from Position.
    splice: usize,
    /// The loop, inclusive: In to Out of the splice.
    first: u32,
    last: u32,
    mode: LoopMode,
    scrub: bool,
    /// 0..1 across the loop.
    position: f64,
    /// Pictures per second at most.
    fps: f64,
    /// Locked to the beat clock: how far it has gone, and how fast.
    beat: Option<Beat>,
    restart: bool,
    /// The frame the editor holds the playhead on.
    cue: Option<u32>,
}

impl Controls {
    /// The settings for splice `splice`, which runs from frame `bounds.0`
    /// up to `bounds.1`.
    fn read(ctx: &FieldCtx, io: &FieldIo, frames: u32, splice: usize, bounds: (u32, u32)) -> Self {
        let percent = |i: usize| f64::from(io.param(i) / 100.0).clamp(0.0, 1.0);
        let (first, last) = loop_frames(io.param(IN), io.param(OUT), bounds);
        Self {
            speed: f64::from(io.param(SPEED)),
            splice,
            first,
            last,
            mode: match io.choice(LOOP) {
                0 => LoopMode::Loop,
                1 => LoopMode::Bounce,
                _ => LoopMode::Once,
            },
            scrub: io.choice(MODE) == 1,
            position: percent(POSITION),
            fps: f64::from(io.param(FPS)).max(0.01),
            beat: (io.choice(SYNC) == 1).then(|| Beat {
                loops: ctx.beats * cycles_per_beat(io.choice(LENGTH)),
                hz: beat_hz(ctx.bpm, io.choice(LENGTH)),
            }),
            restart: io.inputs[RESTART] > 0.5,
            cue: io.params.cue().map(|f| (f.max(0.0) as u32).min(frames - 1)),
        }
    }
}

/// A loop locked to the beat clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Beat {
    /// Loops the clock has counted, one per note of the loop's length.
    loops: f64,
    /// Loops a second at the tempo.
    hz: f64,
}

/// What the transport did this field.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Step {
    /// The frame to show.
    frame: u32,
    /// The playhead across the loop, 0..1.
    phase: f32,
    /// The playhead came to the end of the loop: wrapped, turned or stopped.
    end: bool,
    /// The way the playhead goes from here, for the decoder to read ahead.
    order: Order,
}

/// The playhead.
#[derive(Default)]
struct Transport {
    /// In frames from the start of the movie, fractional.
    pos: f64,
    /// Bounce: on the way back.
    turned: bool,
    /// Once: at the end.
    stopped: bool,
    started: bool,
    /// The splice it's playing.
    splice: Option<usize>,
    restart_held: bool,
    /// Loops run on the beat grid, for End.
    cycle: Option<i64>,
    /// The frame on screen, and the frame-rate tick that picked it.
    shown: u32,
    tick: Option<i64>,
}

impl Transport {
    /// Moves the playhead on by `dt` seconds; `seconds` is the engine's clock.
    fn step(&mut self, c: &Controls, movie_fps: f64, dt: f64, seconds: f64) -> Step {
        let a = f64::from(c.first);
        let b = f64::from(c.last) + 1.0;
        let len = b - a;
        // Just inside the loop's end, so the last frame is still in it.
        let top = b - 1e-6;
        let backward = c.speed < 0.0;
        let restart = c.restart && !self.restart_held;
        self.restart_held = c.restart;
        let mut end = false;
        // Nothing carries over from a loop mode once it's left: Once stops
        // only while it sits at its end, and only Bounce turns round.
        let was_stopped = std::mem::take(&mut self.stopped);
        if c.mode != LoopMode::Bounce {
            self.turned = false;
        }

        if let Some(frame) = c.cue {
            // Held in the editor: that frame now, even outside the loop.
            // Play goes on from there once it's let go.
            self.pos = (f64::from(frame) + 0.5).clamp(a, top);
            self.started = true;
            self.splice = Some(c.splice);
            self.tick = None;
            self.shown = frame;
            return Step {
                frame,
                phase: ((self.pos - a) / len) as f32,
                end: false,
                order: Order::Around,
            };
        }

        // A jump shows where it lands before the playhead moves on.
        let jumped = !self.started || restart || self.splice != Some(c.splice);
        if jumped {
            // Position counts from the start of the loop in the way it plays.
            let offset = c.position * len;
            self.pos = if backward { top - offset } else { a + offset };
            self.turned = false;
            self.started = true;
            self.splice = Some(c.splice);
            self.tick = None;
        }

        let mut forward = !backward;
        match (c.scrub, c.beat, c.mode) {
            (true, _, _) => self.pos = a + c.position * len,
            (false, Some(beat), LoopMode::Loop | LoopMode::Bounce) => {
                // On the beat: the loop's phase comes from the beat clock.
                let phase = beat.loops * c.speed + c.position;
                let cycle = phase.floor() as i64;
                end = self.cycle.is_some_and(|last| last != cycle);
                self.cycle = Some(cycle);
                let along = if c.mode == LoopMode::Loop {
                    phase.rem_euclid(1.0)
                } else {
                    let leg = phase.rem_euclid(2.0);
                    // The second half of each period comes back down.
                    forward = (leg < 1.0) != backward;
                    if leg < 1.0 { leg } else { 2.0 - leg }
                };
                self.pos = a + along * len;
            }
            (false, beat, mode) => {
                self.cycle = None;
                let rate = match beat {
                    // Once at the tempo: the loop lasts one note.
                    Some(beat) => c.speed * len * beat.hz,
                    None => c.speed * movie_fps,
                };
                forward = (rate >= 0.0) != self.turned;
                if !jumped {
                    self.pos += if self.turned { -rate } else { rate } * dt;
                }
                match mode {
                    LoopMode::Loop => {
                        if self.pos >= b || self.pos < a {
                            self.pos = a + (self.pos - a).rem_euclid(len);
                            end = true;
                        }
                    }
                    LoopMode::Bounce => {
                        // Reflect off whichever end it went past.
                        let reflected = if self.pos >= b {
                            Some(2.0 * b - self.pos)
                        } else if self.pos < a {
                            Some(2.0 * a - self.pos)
                        } else {
                            None
                        };
                        if let Some(pos) = reflected {
                            self.pos = pos;
                            self.turned = !self.turned;
                            forward = !forward;
                            end = true;
                        }
                    }
                    LoopMode::Once => {
                        // Held at the end it's heading for. It goes again if
                        // that end moves away or the speed turns round.
                        self.stopped = if forward { self.pos >= top } else { self.pos <= a };
                        end = self.stopped && !was_stopped;
                    }
                }
            }
        }
        self.pos = self.pos.clamp(a, top);

        // A new picture only on the frame-rate grid. The nudge keeps a rate
        // equal to the field rate from landing two fields in one tick.
        let target = (self.pos.floor() as u32).clamp(c.first, c.last);
        let tick = (seconds * c.fps + 1e-3).floor() as i64;
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.shown = target;
        }
        self.shown = self.shown.clamp(c.first, c.last);

        let still = c.scrub || self.stopped || c.speed.abs() < STILL;
        let order = match c.mode {
            _ if still => Order::Around,
            LoopMode::Loop => Order::Loop { forward },
            LoopMode::Bounce => Order::Bounce { forward },
            LoopMode::Once => Order::Once { forward },
        };
        Step {
            frame: self.shown,
            phase: ((self.pos - a) / len) as f32,
            end,
            order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD: f64 = 1.0 / 60.0;

    fn controls(first: u32, last: u32, mode: LoopMode, speed: f64) -> Controls {
        Controls {
            speed,
            splice: 0,
            first,
            last,
            mode,
            scrub: false,
            position: 0.0,
            fps: 60.0,
            beat: None,
            restart: false,
            cue: None,
        }
    }

    /// Runs `fields` fields of a 30 fps movie, returning the frames shown.
    fn play(t: &mut Transport, c: &Controls, fields: usize) -> Vec<u32> {
        (0..fields)
            .map(|i| t.step(c, 30.0, FIELD, i as f64 * FIELD).frame)
            .collect()
    }

    /// Runs `fields` more fields starting at field `from`.
    fn play_from(t: &mut Transport, c: &Controls, from: usize, fields: usize) -> Vec<u32> {
        (from..from + fields)
            .map(|i| t.step(c, 30.0, FIELD, i as f64 * FIELD).frame)
            .collect()
    }

    #[test]
    fn times_read_as_a_clock() {
        assert_eq!(clock(62.5), "1:02.50");
        assert_eq!(clock(0.004), "0:00.00");
        assert_eq!(trim(30.0), "30");
        assert_eq!(trim(29.97003), "29.97");
    }

    #[test]
    fn loops_between_in_and_out() {
        let mut t = Transport::default();
        let c = controls(10, 13, LoopMode::Loop, 1.0);
        // Two fields a frame at 60 fields per second.
        let shown = play(&mut t, &c, 10);
        assert_eq!(shown, [10, 10, 11, 11, 12, 12, 13, 13, 10, 10]);
    }

    #[test]
    fn negative_speed_plays_backward_from_out() {
        let mut t = Transport::default();
        let c = controls(10, 13, LoopMode::Loop, -2.0);
        assert_eq!(play(&mut t, &c, 6), [13, 12, 11, 10, 13, 12]);
    }

    #[test]
    fn bounce_turns_at_the_ends() {
        let mut t = Transport::default();
        let c = controls(0, 3, LoopMode::Bounce, 2.0);
        let steps: Vec<Step> = (0..8)
            .map(|i| t.step(&c, 30.0, FIELD, i as f64 * FIELD))
            .collect();
        let frames: Vec<u32> = steps.iter().map(|s| s.frame).collect();
        assert_eq!(frames, [0, 1, 2, 3, 3, 2, 1, 0]);
        assert_eq!(steps[5].order, Order::Bounce { forward: false });
        assert!(steps[4].end, "turned at the top");
    }

    #[test]
    fn once_stops_at_the_end_until_restarted() {
        let mut t = Transport::default();
        let mut c = controls(0, 3, LoopMode::Once, 2.0);
        let steps: Vec<Step> = (0..6)
            .map(|i| t.step(&c, 30.0, FIELD, i as f64 * FIELD))
            .collect();
        let frames: Vec<u32> = steps.iter().map(|s| s.frame).collect();
        assert_eq!(frames, [0, 1, 2, 3, 3, 3]);
        assert_eq!(steps.iter().filter(|s| s.end).count(), 1);
        assert_eq!(steps[5].order, Order::Around);
        c.restart = true;
        assert_eq!(t.step(&c, 30.0, FIELD, 6.0 * FIELD).frame, 0);
    }

    #[test]
    fn once_lets_go_when_left_or_turned_round() {
        let mut t = Transport::default();
        let mut c = controls(0, 3, LoopMode::Once, 2.0);
        play(&mut t, &c, 8);
        // Back to Loop: it plays on from the start.
        c.mode = LoopMode::Loop;
        let shown: Vec<u32> = (8..12)
            .map(|i| t.step(&c, 30.0, FIELD, i as f64 * FIELD).frame)
            .collect();
        assert_eq!(shown, [0, 1, 2, 3]);

        // Stopped at the end, then the speed turns round: it plays back.
        let mut t = Transport::default();
        c.mode = LoopMode::Once;
        play(&mut t, &c, 8);
        c.speed = -2.0;
        let shown: Vec<u32> = (8..12)
            .map(|i| t.step(&c, 30.0, FIELD, i as f64 * FIELD).frame)
            .collect();
        assert_eq!(shown, [2, 1, 0, 0]);
    }

    #[test]
    fn loop_after_bounce_plays_the_way_speed_says() {
        let mut t = Transport::default();
        let mut c = controls(0, 9, LoopMode::Bounce, 2.0);
        // Up to the top and partway back down.
        play(&mut t, &c, 14);
        c.mode = LoopMode::Loop;
        let a = t.step(&c, 30.0, FIELD, 14.0 * FIELD).frame;
        let b = t.step(&c, 30.0, FIELD, 15.0 * FIELD).frame;
        assert_eq!(b, a + 1, "forward at positive speed");
    }

    #[test]
    fn a_mark_set_on_a_frame_stays_on_it() {
        // Long splices too, where a percentage in f32 is coarse.
        for bounds in [(0, 7), (100, 688), (5_000, 200_000)] {
            let len = bounds.1 - bounds.0;
            let percent = |f: u32| ((f - bounds.0) as f64 / len as f64 * 100.0) as f32;
            for f in (bounds.0..bounds.1).step_by((len / 300).max(1) as usize) {
                assert_eq!(loop_frames(percent(f), 100.0, bounds).0, f);
                assert_eq!(loop_frames(0.0, percent(f + 1), bounds).1, f);
            }
        }
        assert_eq!(loop_frames(80.0, 20.0, (0, 10)), (2, 7), "either way round");
        assert_eq!(loop_frames(50.0, 50.0, (0, 10)), (5, 5), "at least a frame");
    }

    #[test]
    fn splices_start_at_their_marks() {
        // 24 fps: 1 s is frame 24. Marks past the end, at the start or on
        // the same frame as another count once or not at all.
        let starts = splice_starts(&[2.0, 1.0, 0.0, 1.01, 100.0], 24.0, 240);
        assert_eq!(starts, [0, 24, 48]);
        assert_eq!(splice_starts(&[], 24.0, 240), [0]);
    }

    #[test]
    fn organize_picks_and_next_steps_on() {
        let mut s = Splicer::default();
        assert_eq!(s.pick(0.0, false, 4), 0);
        assert_eq!(s.pick(0.6, false, 4), 2);
        assert_eq!(s.pick(1.0, false, 4), 3, "the top of the knob is the last");
        // Next steps on once per trigger, round to the first.
        assert_eq!(s.pick(1.0, true, 4), 0);
        assert_eq!(s.pick(1.0, true, 4), 0, "held high, it steps once");
        assert_eq!(s.pick(1.0, false, 4), 0);
        assert_eq!(s.pick(1.0, true, 4), 1);
        // Organize picking another splice starts from there.
        assert_eq!(s.pick(0.3, false, 4), 1);
        assert_eq!(s.pick(0.0, false, 4), 0);
    }

    #[test]
    fn another_splice_starts_from_position() {
        let mut t = Transport::default();
        let mut c = controls(0, 99, LoopMode::Loop, 1.0);
        play(&mut t, &c, 20);
        c.splice = 1;
        c.first = 200;
        c.last = 299;
        assert_eq!(t.step(&c, 30.0, FIELD, 20.0 * FIELD).frame, 200);
        assert_eq!(t.step(&c, 30.0, FIELD, 21.0 * FIELD).frame, 200);
        assert_eq!(t.step(&c, 30.0, FIELD, 22.0 * FIELD).frame, 201);
    }

    #[test]
    fn a_held_cue_shows_any_frame_then_plays_on() {
        let mut t = Transport::default();
        let mut c = controls(100, 199, LoopMode::Loop, 1.0);
        play(&mut t, &c, 4);
        // Outside the loop: shown as asked, read for around it.
        c.cue = Some(500);
        let held = t.step(&c, 30.0, FIELD, 4.0 * FIELD);
        assert_eq!((held.frame, held.order), (500, Order::Around));
        c.cue = Some(150);
        assert_eq!(t.step(&c, 30.0, FIELD, 5.0 * FIELD).frame, 150);
        assert_eq!(t.step(&c, 30.0, FIELD, 6.0 * FIELD).frame, 150);
        // Let go: it plays on from there.
        c.cue = None;
        let shown = play_from(&mut t, &c, 7, 4);
        assert_eq!(shown, [151, 151, 152, 152]);
    }

    #[test]
    fn scrub_follows_position() {
        let mut t = Transport::default();
        let mut c = controls(100, 199, LoopMode::Loop, 1.0);
        c.scrub = true;
        c.position = 0.25;
        let step = t.step(&c, 30.0, FIELD, 0.0);
        assert_eq!(step.frame, 125);
        assert_eq!(step.order, Order::Around);
    }

    #[test]
    fn frame_rate_holds_pictures() {
        let mut t = Transport::default();
        let mut c = controls(0, 999, LoopMode::Loop, 2.0);
        c.fps = 15.0;
        // A new frame every field at 2×, but only every fourth gets shown.
        let shown = play(&mut t, &c, 9);
        assert_eq!(shown, [0, 0, 0, 0, 4, 4, 4, 4, 8]);
    }

    #[test]
    fn beat_sync_fits_the_loop_to_the_note() {
        let mut t = Transport::default();
        let mut c = controls(0, 59, LoopMode::Loop, 1.0);
        // One loop per beat at 120 BPM: half a second.
        let mut at = |t: &mut Transport, s: f64| {
            c.beat = Some(Beat {
                loops: s * 2.0,
                hz: 2.0,
            });
            t.step(&c, 30.0, FIELD, s)
        };
        assert_eq!(at(&mut t, 0.0).frame, 0);
        assert_eq!(at(&mut t, 0.25).frame, 30);
        let wrapped = at(&mut t, 0.5);
        assert_eq!(wrapped.frame, 0);
        assert!(wrapped.end);
    }

    #[test]
    fn restart_jumps_to_position() {
        let mut t = Transport::default();
        let mut c = controls(0, 99, LoopMode::Loop, 1.0);
        play(&mut t, &c, 30);
        c.position = 0.5;
        c.restart = true;
        assert_eq!(t.step(&c, 30.0, FIELD, 1.0).frame, 50);
        // Held high, it doesn't restart again.
        let next = t.step(&c, 30.0, FIELD, 1.0 + FIELD).frame;
        assert!(next >= 50);
    }
}
