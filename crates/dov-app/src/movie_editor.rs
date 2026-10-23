//! The movie editor: a window for one Movie module, opened by clicking its
//! picture. Its timeline zooms in to single frames, to set In and Out
//! exactly and to cut the movie into splices, as on a Morphagene.

use std::path::Path;

use dov_core::{Category, ModuleId, ParamBank, Patch};
use dov_modules::{Timeline, loop_frames, splice_starts};
use egui::{
    Align2, Color32, FontId, Id, Key, Modifiers, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2,
    vec2,
};

use crate::panel;
use crate::rack::Rack;
use crate::rack_view::ProbeView;

/// Size of the picture in the editor.
const PREVIEW: Vec2 = vec2(480.0, 360.0);
/// The timeline's parts, top to bottom: splice numbers, the track, the ruler.
const LANE_H: f32 = 16.0;
const TRACK_H: f32 = 34.0;
const RULER_H: f32 = 20.0;
/// How near the pointer has to be to pick up a mark, in points.
const GRAB: f32 = 6.0;
/// The fewest frames the timeline zooms in to.
const MIN_SPAN: f64 = 12.0;
/// Ruler labels at least this far apart, in points.
const LABEL_GAP: f64 = 64.0;

const MARK: Color32 = Color32::from_rgb(255, 200, 120);
const PLAYHEAD: Color32 = Color32::from_rgb(235, 87, 87);

pub struct MovieEditor {
    pub id: ModuleId,
    /// Where it first opens.
    at: Pos2,
    /// The frames the timeline shows: the first, and how many. None shows
    /// the whole movie.
    view: Option<(f64, f64)>,
    /// The frame the playhead is held on, while it's held.
    held: Option<u32>,
    drag: Option<Drag>,
    /// A splice picked here that the movie hasn't got to yet.
    picked: Option<usize>,
    /// The pointer was over the window last frame, so keys go to it.
    hovered: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Grab {
    In,
    Out,
    /// The start of splice `k` (never the first).
    Splice(usize),
    Playhead,
}

struct Drag {
    grab: Grab,
    /// Where it's got to, in frames.
    at: f64,
    /// The pointer's x as of the last frame.
    x: f32,
    /// An undo step was taken for it.
    undone: bool,
}

/// Keys pressed for the editor this frame.
#[derive(Default)]
struct Keys {
    hold: bool,
    step: i64,
    set_in: bool,
    set_out: bool,
    splice: bool,
}

impl Keys {
    fn read(i: &mut egui::InputState) -> Self {
        let mut keys = Self {
            hold: i.consume_key(Modifiers::NONE, Key::Space),
            set_in: i.consume_key(Modifiers::NONE, Key::I),
            set_out: i.consume_key(Modifiers::NONE, Key::O),
            splice: i.consume_key(Modifiers::NONE, Key::S),
            ..Self::default()
        };
        for (key, dir) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
            // Shift first: a plain key also matches with shift held.
            if i.consume_key(Modifiers::SHIFT, key) {
                keys.step += 10 * dir;
            } else if i.consume_key(Modifiers::NONE, key) {
                keys.step += dir;
            }
        }
        keys
    }
}

/// The movie module the editor works on, as of this frame.
struct Reel<'a> {
    params: &'a ParamBank,
    tl: Timeline,
    /// The first frame of each splice.
    starts: Vec<u32>,
    /// The splice playing, or just picked.
    splice: usize,
    in_slot: usize,
    out_slot: usize,
    organize_slot: usize,
}

impl Reel<'_> {
    fn splice_of(&self, frame: u32) -> usize {
        self.starts.partition_point(|&s| s <= frame).saturating_sub(1)
    }

    /// Splice `k`'s frames: its first, and the one after its last.
    fn bounds(&self, k: usize) -> (u32, u32) {
        let end = self.starts.get(k + 1).copied().unwrap_or(self.tl.frames);
        (self.starts[k], end)
    }

    /// In and Out as set, on splice `k`: the loop's first frame, and the
    /// frame after its last.
    fn in_out(&self, k: usize) -> (u32, u32) {
        let (first, last) = loop_frames(
            self.params.get(self.in_slot),
            self.params.get(self.out_slot),
            self.bounds(k),
        );
        (first, last + 1)
    }

    fn percent_of(&self, k: usize, frame: u32) -> f32 {
        let (start, end) = self.bounds(k);
        (f64::from(frame - start) / f64::from(end - start) * 100.0) as f32
    }

    /// Starts the loop on `frame`, moving Out to the end of the splice if
    /// it would come first.
    fn set_in(&self, k: usize, frame: u32) {
        let (start, end) = self.bounds(k);
        let frame = frame.clamp(start, end - 1);
        if frame >= self.in_out(k).1 {
            self.params.set(self.out_slot, 100.0);
        }
        self.params.set(self.in_slot, self.percent_of(k, frame));
    }

    /// Ends the loop just before `end`, moving In to the start of the
    /// splice if it would come after.
    fn set_out(&self, k: usize, end: u32) {
        let (start, stop) = self.bounds(k);
        let end = end.clamp(start + 1, stop);
        if end <= self.in_out(k).0 {
            self.params.set(self.in_slot, 0.0);
        }
        self.params.set(self.out_slot, self.percent_of(k, end));
    }

    /// Turns Organize to splice `k` of `count`.
    fn select(&self, k: usize, count: usize) {
        let organize = if count <= 1 {
            0.0
        } else {
            (k as f32 + 0.5) / count as f32
        };
        self.params.set(self.organize_slot, organize);
    }

    /// Cuts the movie at `starts` (the first is frame 0).
    fn set_starts(&self, starts: &[u32]) {
        let fps = self.tl.fps;
        self.params
            .set_marks(starts[1..].iter().map(|&f| f64::from(f) / fps).collect());
    }

    /// The frame on screen: the one held, or the one playing.
    fn playhead(&self) -> u32 {
        self.params
            .cue()
            .map_or(self.tl.frame, |f| (f.max(0.0) as u32).min(self.tl.frames - 1))
    }
}

/// Frames to points along the timeline, for the part of the movie in view.
struct Scale {
    rect: Rect,
    from: f64,
    span: f64,
}

impl Scale {
    fn x(&self, frame: f64) -> f32 {
        self.rect.left() + ((frame - self.from) / self.span) as f32 * self.rect.width()
    }

    fn frame(&self, x: f32) -> f64 {
        self.from + f64::from((x - self.rect.left()) / self.rect.width()) * self.span
    }

    fn frames_per_point(&self) -> f64 {
        self.span / f64::from(self.rect.width())
    }
}

/// Takes one undo step for edits made this frame.
fn checkpoint(rack: &Rack, before: &mut Option<Patch>) {
    if before.is_none() {
        *before = Some(rack.to_patch());
    }
}

impl MovieEditor {
    /// An editor for module `id`, opening at `at`.
    pub fn new(id: ModuleId, at: Pos2) -> Self {
        Self {
            id,
            at,
            view: None,
            held: None,
            drag: None,
            picked: None,
            hovered: false,
        }
    }

    /// Lets go of the playhead, for when the editor closes.
    pub fn close(&self, rack: &Rack) {
        if let Some(m) = rack.module(self.id) {
            m.params.set_cue(None);
        }
    }

    /// Draws the window. False once it's closed or its module is gone.
    /// `choose_movie` is set to ask for a movie file.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        rack: &mut Rack,
        probe: &ProbeView,
        choose_movie: &mut Option<ModuleId>,
    ) -> bool {
        let Some(m) = rack.module(self.id) else {
            return false;
        };
        let (params, ty) = (m.params.clone(), m.ty);
        let desc = &ty.desc;
        let slot = |id| desc.param_index(id);
        let (Some(in_slot), Some(out_slot), Some(organize_slot)) =
            (slot("in_point"), slot("out_point"), slot("organize"))
        else {
            return false;
        };
        let path = params.text();
        let name = Path::new(&path)
            .file_name()
            .map_or_else(|| "no movie".into(), |n| n.to_string_lossy().into_owned());
        if self.drag.is_none() {
            // Undo makes new params; hold on to those too.
            params.set_cue(self.held.map(|f| f as f32));
        }

        let mut open = true;
        let mut before = None;
        let shown = egui::Window::new(format!("Movie · {name}"))
            .id(Id::unique(("movie editor", self.id)))
            .default_pos(self.at)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| match probe.timelines.get(&self.id) {
                Some(&tl) => {
                    let starts = splice_starts(&params.marks(), tl.fps, tl.frames);
                    if self.picked == Some(tl.splice) {
                        self.picked = None;
                    }
                    let reel = Reel {
                        params: &params,
                        tl,
                        splice: self.picked.unwrap_or(tl.splice).min(starts.len() - 1),
                        starts,
                        in_slot,
                        out_slot,
                        organize_slot,
                    };
                    let screen = probe.screens.get(&self.id).copied();
                    self.contents(ui, &reel, screen, rack, &mut before);
                }
                None => {
                    ui.set_width(PREVIEW.x);
                    ui.label(if path.is_empty() {
                        "No movie yet."
                    } else {
                        "Opening the movie… If it doesn't open, the warnings say why."
                    });
                    if ui.button("Choose a movie…").clicked() {
                        *choose_movie = Some(self.id);
                    }
                }
            });
        self.hovered = shown.is_some_and(|r| r.response.contains_pointer());
        if let Some(patch) = before {
            rack.push_undo(patch);
        }
        if !open {
            self.close(rack);
        }
        open
    }

    fn contents(
        &mut self,
        ui: &mut Ui,
        reel: &Reel,
        screen: Option<egui::TextureId>,
        rack: &Rack,
        before: &mut Option<Patch>,
    ) {
        let keys = if self.hovered && !ui.ctx().egui_wants_keyboard_input() {
            ui.input_mut(Keys::read)
        } else {
            Keys::default()
        };
        let (tl, count) = (reel.tl, reel.starts.len());

        // The picture. Clicking it holds it still.
        let (rect, response) = ui.allocate_exact_size(PREVIEW, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::BLACK);
        if let Some(texture) = screen {
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            painter.image(texture, rect, uv, Color32::WHITE);
        }
        if self.held.is_some() {
            let tag = Rect::from_min_size(rect.right_top() + vec2(-62.0, 8.0), vec2(54.0, 18.0));
            painter.rect_filled(tag, 3.0, Color32::from_black_alpha(170));
            painter.text(
                tag.center(),
                Align2::CENTER_CENTER,
                "HELD",
                FontId::proportional(11.0),
                MARK,
            );
        }
        if response.clicked() || keys.hold {
            self.toggle_hold(reel);
        }
        response.on_hover_text("Click to hold the picture still (space); again to play on");

        ui.add_space(6.0);
        self.timeline(ui, reel, rack, before);
        ui.add_space(6.0);

        let playhead = reel.playhead();
        let k = reel.splice;
        let mut step = keys.step;
        ui.horizontal(|ui| {
            if icon_button(ui, Icon::PreviousSplice, "Previous splice").clicked() {
                self.go_to_splice(reel, (k + count - 1) % count, rack, before);
            }
            if icon_button(ui, Icon::Back, "Back a frame (left arrow; with shift, 10)").clicked() {
                step -= 1;
            }
            let hold = if self.held.is_some() { "Play" } else { "Hold" };
            if ui
                .add_sized([44.0, 20.0], egui::Button::new(hold))
                .on_hover_text("Hold the picture still, or play on (space)")
                .clicked()
            {
                self.toggle_hold(reel);
            }
            if icon_button(ui, Icon::On, "On a frame (right arrow; with shift, 10)").clicked() {
                step += 1;
            }
            if icon_button(ui, Icon::NextSplice, "Next splice").clicked() {
                self.go_to_splice(reel, (k + 1) % count, rack, before);
            }
            ui.separator();
            if ui
                .button("Set In")
                .on_hover_text("Start the loop here (I)")
                .clicked()
                || keys.set_in
            {
                self.set_in_or_out(reel, playhead, true, rack, before);
            }
            if ui
                .button("Set Out")
                .on_hover_text("End the loop after this frame (O)")
                .clicked()
                || keys.set_out
            {
                self.set_in_or_out(reel, playhead, false, rack, before);
            }
            let whole = reel.in_out(k) != reel.bounds(k);
            if ui
                .add_enabled(whole, egui::Button::new("Whole splice"))
                .on_hover_text("In and Out at the ends of the splice")
                .clicked()
            {
                checkpoint(rack, before);
                reel.params.set(reel.in_slot, 0.0);
                reel.params.set(reel.out_slot, 100.0);
            }
        });
        if step != 0 {
            self.step(reel, step, rack, before);
        }

        ui.horizontal(|ui| {
            let cut = playhead > 0 && !reel.starts.contains(&playhead);
            if ui
                .add_enabled(cut, egui::Button::new("Splice here"))
                .on_hover_text("Cut the movie at this frame (S)")
                .clicked()
                || (keys.splice && cut)
            {
                self.cut(reel, playhead, rack, before);
            }
            if ui
                .add_enabled(count > 1, egui::Button::new("Join"))
                .on_hover_text("Join this splice to the next one")
                .clicked()
            {
                self.join(reel, rack, before);
            }
            if ui
                .add_enabled(count > 1, egui::Button::new("Clear splices"))
                .on_hover_text("One splice: the whole movie")
                .clicked()
            {
                checkpoint(rack, before);
                reel.set_starts(&[0]);
                reel.select(0, 1);
                self.picked = Some(0);
            }
            ui.separator();
            if ui
                .add_enabled(self.view.is_some(), egui::Button::new("Show all"))
                .on_hover_text("Zoom out to the whole movie")
                .clicked()
            {
                self.view = None;
            }
        });

        ui.horizontal(|ui| {
            ui.monospace(format!(
                "{} · frame {playhead}",
                clock(f64::from(playhead) / tl.fps)
            ));
            ui.separator();
            ui.label(format!("Splice {} of {count}", k + 1));
            ui.separator();
            let (first, end) = reel.in_out(k);
            let (start, stop) = reel.bounds(k);
            let mut v = first;
            ui.label("In");
            let edit = ui.add(egui::DragValue::new(&mut v).range(start..=end - 1).speed(0.25));
            if edit.changed() {
                checkpoint(rack, before);
                reel.set_in(k, v);
            }
            let mut v = end - 1;
            ui.label("Out");
            let edit = ui.add(egui::DragValue::new(&mut v).range(first..=stop - 1).speed(0.25));
            if edit.changed() {
                checkpoint(rack, before);
                reel.set_out(k, v + 1);
            }
            ui.label(format!("Loop {}", clock(f64::from(end - first) / tl.fps)));
        });
        ui.small(
            "Drag the In and Out brackets or the splice marks (shift: fine). Drag anywhere else \
             to scrub, scroll to zoom. In and Out trim whichever splice plays.",
        );
    }

    /// The timeline: splices, the loop, the marks, the playhead, a ruler.
    fn timeline(&mut self, ui: &mut Ui, reel: &Reel, rack: &Rack, before: &mut Option<Patch>) {
        let (_, rect) = ui.allocate_space(vec2(PREVIEW.x, LANE_H + TRACK_H + RULER_H));
        let response = ui.interact(rect, timeline_id(self.id), Sense::click_and_drag());
        let frames = f64::from(reel.tl.frames);

        // Scroll to zoom about the pointer; sideways scrolling pans.
        let (mut from, mut span) = self.view.unwrap_or((0.0, frames));
        span = span.min(frames);
        if response.hovered() {
            let (scroll, zoom, pointer) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = (-f64::from(scroll.y) * 0.005).exp() / f64::from(zoom);
            if factor != 1.0 {
                let scale = Scale { rect, from, span };
                let anchor = pointer.map_or(from + span / 2.0, |p| scale.frame(p.x));
                let zoomed = (span * factor).clamp(MIN_SPAN.min(frames), frames);
                from = anchor - (anchor - from) * zoomed / span;
                span = zoomed;
            }
            from -= f64::from(scroll.x) * span / f64::from(rect.width());
        }
        from = from.clamp(0.0, frames - span);
        self.view = (span < frames).then_some((from, span));
        let scale = Scale { rect, from, span };

        let (pressed, down, pointer, shift) = ui.input(|i| {
            let p = &i.pointer;
            (
                p.primary_pressed(),
                p.primary_down(),
                p.interact_pos(),
                i.modifiers.shift,
            )
        });
        if pressed
            && response.hovered()
            && let Some(p) = pointer
        {
            self.drag = self.grab(reel, &scale, p, rack, before);
        }
        if self.drag.is_some() {
            match pointer.filter(|_| down) {
                Some(p) => self.drag_to(reel, &scale, p.x, shift, rack, before),
                None => {
                    self.drag = None;
                    if let Some(f) = self.held.as_mut() {
                        *f = reel.playhead();
                    }
                    reel.params.set_cue(self.held.map(|f| f as f32));
                }
            }
        }
        self.draw_timeline(ui, reel, &scale, response.hover_pos());
    }

    /// What a press at `p` picks up: a mark near it, or the playhead. A
    /// press on a splice's number picks that splice instead.
    fn grab(
        &mut self,
        reel: &Reel,
        scale: &Scale,
        p: Pos2,
        rack: &Rack,
        before: &mut Option<Patch>,
    ) -> Option<Drag> {
        let lane = p.y < scale.rect.top() + LANE_H;
        let ruler = p.y > scale.rect.top() + LANE_H + TRACK_H;
        let (first, end) = reel.in_out(reel.splice);
        let mut marks = Vec::new();
        if !lane && !ruler {
            marks.extend([(Grab::In, first), (Grab::Out, end)]);
        }
        if !ruler {
            marks.extend((1..reel.starts.len()).map(|k| (Grab::Splice(k), reel.starts[k])));
        }
        // In and Out come first, so they can be picked up off a splice mark.
        let near = marks
            .into_iter()
            .map(|(grab, frame)| (grab, frame, (scale.x(f64::from(frame)) - p.x).abs()))
            .filter(|&(_, _, d)| d <= GRAB)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let (grab, at) = match near {
            Some((grab, frame, _)) => (grab, f64::from(frame)),
            None if lane => {
                let frame = scale.frame(p.x).clamp(0.0, f64::from(reel.tl.frames - 1));
                self.go_to_splice(reel, reel.splice_of(frame as u32), rack, before);
                return None;
            }
            None => (Grab::Playhead, scale.frame(p.x)),
        };
        let mut drag = Drag {
            grab,
            at,
            x: p.x,
            undone: false,
        };
        // A press on the track shows that frame right away.
        if grab == Grab::Playhead {
            self.apply(reel, &mut drag, rack, before);
        }
        Some(drag)
    }

    fn drag_to(
        &mut self,
        reel: &Reel,
        scale: &Scale,
        x: f32,
        fine: bool,
        rack: &Rack,
        before: &mut Option<Patch>,
    ) {
        let Some(mut drag) = self.drag.take() else { return };
        let moved = f64::from(x - drag.x) * scale.frames_per_point();
        drag.x = x;
        drag.at = match drag.grab {
            Grab::Playhead if !fine => scale.frame(x),
            _ => drag.at + moved * if fine { 0.1 } else { 1.0 },
        };
        self.apply(reel, &mut drag, rack, before);
        self.drag = Some(drag);
    }

    /// Puts what's being dragged where the drag has got to, and shows the
    /// frame it's on.
    fn apply(&mut self, reel: &Reel, drag: &mut Drag, rack: &Rack, before: &mut Option<Patch>) {
        let frames = reel.tl.frames;
        let k = reel.splice;
        let (first, end) = reel.in_out(k);
        let mut edit = |rack: &Rack, before: &mut Option<Patch>| {
            if !drag.undone {
                checkpoint(rack, before);
                drag.undone = true;
            }
        };
        let cue = match drag.grab {
            Grab::Playhead => {
                let frame = drag.at.floor().clamp(0.0, f64::from(frames - 1)) as u32;
                // The splice under the playhead plays when it's let go.
                self.pick_splice(reel, reel.splice_of(frame), rack, before);
                frame
            }
            Grab::In => {
                let frame = (drag.at.round().max(0.0) as u32).clamp(reel.bounds(k).0, end - 1);
                if frame != first {
                    edit(rack, before);
                    reel.set_in(k, frame);
                }
                frame
            }
            Grab::Out => {
                let stop = (drag.at.round().max(0.0) as u32).clamp(first + 1, reel.bounds(k).1);
                if stop != end {
                    edit(rack, before);
                    reel.set_out(k, stop);
                }
                stop - 1
            }
            Grab::Splice(s) => {
                let lo = reel.starts[s - 1] + 1;
                let hi = reel.starts.get(s + 1).copied().unwrap_or(frames) - 1;
                let frame = (drag.at.round().max(0.0) as u32).clamp(lo, hi.max(lo));
                if frame != reel.starts[s] {
                    edit(rack, before);
                    let mut starts = reel.starts.clone();
                    starts[s] = frame;
                    reel.set_starts(&starts);
                }
                frame
            }
        };
        reel.params.set_cue(Some(cue as f32));
    }

    fn toggle_hold(&mut self, reel: &Reel) {
        self.held = match self.held {
            Some(_) => None,
            None => Some(reel.playhead()),
        };
        reel.params.set_cue(self.held.map(|f| f as f32));
    }

    /// Holds the playhead `by` frames on from where it is.
    fn step(&mut self, reel: &Reel, by: i64, rack: &Rack, before: &mut Option<Patch>) {
        let last = i64::from(reel.tl.frames) - 1;
        let frame = (i64::from(reel.playhead()) + by).clamp(0, last) as u32;
        self.held = Some(frame);
        reel.params.set_cue(Some(frame as f32));
        self.reveal(frame);
        self.pick_splice(reel, reel.splice_of(frame), rack, before);
    }

    /// Plays splice `k`. False if it's already playing.
    fn pick_splice(&mut self, reel: &Reel, k: usize, rack: &Rack, before: &mut Option<Patch>) -> bool {
        if k == reel.splice {
            return false;
        }
        checkpoint(rack, before);
        reel.select(k, reel.starts.len());
        self.picked = Some(k);
        true
    }

    /// Plays splice `k`; a held picture moves to the start of its loop.
    fn go_to_splice(&mut self, reel: &Reel, k: usize, rack: &Rack, before: &mut Option<Patch>) {
        if self.pick_splice(reel, k, rack, before) && self.held.is_some() {
            let frame = reel.in_out(k).0;
            self.held = Some(frame);
            reel.params.set_cue(Some(frame as f32));
            self.reveal(frame);
        }
    }

    /// In (or Out, just after it) on the playhead, in the splice it's in.
    fn set_in_or_out(
        &mut self,
        reel: &Reel,
        playhead: u32,
        is_in: bool,
        rack: &Rack,
        before: &mut Option<Patch>,
    ) {
        checkpoint(rack, before);
        let k = reel.splice_of(playhead);
        if k != reel.splice {
            reel.select(k, reel.starts.len());
            self.picked = Some(k);
        }
        if is_in {
            reel.set_in(k, playhead);
        } else {
            reel.set_out(k, playhead + 1);
        }
    }

    /// A new splice from `frame` on, which then plays.
    fn cut(&mut self, reel: &Reel, frame: u32, rack: &Rack, before: &mut Option<Patch>) {
        checkpoint(rack, before);
        let mut starts = reel.starts.clone();
        let k = starts.partition_point(|&s| s < frame);
        starts.insert(k, frame);
        reel.set_starts(&starts);
        reel.select(k, starts.len());
        self.picked = Some(k);
    }

    /// Joins the splice playing to the next one (the last, to the one before).
    fn join(&mut self, reel: &Reel, rack: &Rack, before: &mut Option<Patch>) {
        checkpoint(rack, before);
        let mut starts = reel.starts.clone();
        let mut k = reel.splice;
        if k + 1 < starts.len() {
            starts.remove(k + 1);
        } else {
            starts.remove(k);
            k -= 1;
        }
        reel.set_starts(&starts);
        reel.select(k, starts.len());
        self.picked = Some(k);
    }

    /// Scrolls the timeline, if it's zoomed in, to show `frame`.
    fn reveal(&mut self, frame: u32) {
        if let Some((from, span)) = &mut self.view {
            let f = f64::from(frame);
            if f < *from || f + 1.0 > *from + *span {
                *from = f - *span / 2.0;
            }
        }
    }

    fn draw_timeline(&self, ui: &Ui, reel: &Reel, scale: &Scale, hover: Option<Pos2>) {
        let rect = scale.rect;
        let painter = ui.painter_at(rect);
        let lane = Rect::from_min_size(rect.min, vec2(rect.width(), LANE_H));
        let track = Rect::from_min_size(lane.left_bottom(), vec2(rect.width(), TRACK_H));
        let ruler = Rect::from_min_max(track.left_bottom(), rect.max);
        let (tl, k) = (reel.tl, reel.splice);
        let x = |frame: u32| scale.x(f64::from(frame));
        let small = FontId::proportional(10.0);
        painter.rect_filled(lane.union(track), 2.0, Color32::from_gray(14));

        // Splices, the one playing lighter, numbered when there's room.
        for (s, &start) in reel.starts.iter().enumerate() {
            let (x0, x1) = (x(start), x(reel.bounds(s).1));
            let (x0, x1) = (x0.max(rect.left()), x1.min(rect.right()));
            if x1 <= x0 {
                continue;
            }
            let shade = match s {
                _ if s == k => 46,
                _ if s % 2 == 0 => 28,
                _ => 21,
            };
            painter.rect_filled(
                Rect::from_x_y_ranges(x0..=x1, track.y_range()),
                0.0,
                Color32::from_gray(shade),
            );
            if x1 - x0 > 14.0 && reel.starts.len() > 1 {
                let color = if s == k { MARK } else { Color32::from_gray(150) };
                painter.text(
                    pos2((x0 + x1) / 2.0, lane.center().y),
                    Align2::CENTER_CENTER,
                    format!("{}", s + 1),
                    small.clone(),
                    color,
                );
            }
        }

        // The loop as it plays, CV and all.
        let accent = panel::category_color(Category::Io);
        let band = track.shrink2(vec2(0.0, 7.0));
        painter.rect_filled(
            Rect::from_x_y_ranges(x(tl.first)..=x(tl.last + 1), band.y_range()),
            0.0,
            accent.gamma_multiply(0.6),
        );

        // Frame by frame, once frames are wide enough to see.
        let frame_w = 1.0 / scale.frames_per_point();
        if frame_w >= 6.0 {
            let first = scale.from.floor() as u32;
            let last = ((scale.from + scale.span).ceil() as u32).min(tl.frames);
            for f in first..=last {
                let fx = x(f);
                painter.line_segment(
                    [pos2(fx, track.bottom() - 5.0), pos2(fx, track.bottom())],
                    Stroke::new(1.0, Color32::from_gray(90)),
                );
            }
        }

        for &start in &reel.starts[1..] {
            let sx = x(start);
            painter.line_segment(
                [pos2(sx, lane.top()), pos2(sx, track.bottom())],
                Stroke::new(1.5, Color32::from_gray(225)),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(sx - 4.0, lane.top()),
                    pos2(sx + 4.0, lane.top()),
                    pos2(sx, lane.top() + 6.0),
                ],
                Color32::from_gray(225),
                Stroke::NONE,
            ));
        }

        let (first, end) = reel.in_out(k);
        bracket(&painter, x(first), track, true);
        bracket(&painter, x(end), track, false);

        // The ruler: times at a round step that leaves room for them.
        let fps = tl.fps;
        let steps = [1.0, 2.0, 5.0, 10.0].into_iter().chain(
            [
                0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
            ]
            .into_iter()
            .map(|s| s * fps)
            .filter(|&f| f > 10.0),
        );
        let step = steps
            .into_iter()
            .find(|&f| f / scale.frames_per_point() >= LABEL_GAP)
            .unwrap_or(3600.0 * fps);
        let mut tick = (scale.from / step).ceil() * step;
        while tick <= scale.from + scale.span {
            let tx = scale.x(tick);
            painter.line_segment(
                [pos2(tx, ruler.top()), pos2(tx, ruler.top() + 4.0)],
                Stroke::new(1.0, Color32::from_gray(120)),
            );
            painter.text(
                pos2(tx + 3.0, ruler.top() + 3.0),
                Align2::LEFT_TOP,
                clock(tick / fps),
                small.clone(),
                Color32::from_gray(160),
            );
            tick += step;
        }

        // The playhead, over the whole frame when frames are wide.
        let playhead = reel.playhead();
        let px = x(playhead);
        if frame_w >= 3.0 {
            painter.rect_filled(
                Rect::from_x_y_ranges(px..=x(playhead + 1), track.y_range()),
                0.0,
                PLAYHEAD.gamma_multiply(0.35),
            );
        }
        painter.line_segment(
            [pos2(px, lane.top() + 2.0), pos2(px, track.bottom())],
            Stroke::new(2.0, PLAYHEAD),
        );

        // Which frame is under the pointer.
        if let Some(p) = hover.filter(|p| rect.contains(*p) && self.drag.is_none()) {
            let f = scale.frame(p.x).floor().clamp(0.0, f64::from(tl.frames - 1));
            let hx = scale.x(f);
            painter.line_segment(
                [pos2(hx, lane.top()), pos2(hx, track.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(70)),
            );
            let text = format!("{} · {}", f as u32, clock(f / fps));
            let galley = painter.layout_no_wrap(text, small, Color32::WHITE);
            let at = pos2(
                (hx + 4.0).min(rect.right() - galley.size().x - 2.0),
                ruler.bottom() - galley.size().y - 1.0,
            );
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(2.0),
                2.0,
                Color32::from_gray(30),
            );
            painter.galley(at, galley, Color32::WHITE);
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    PreviousSplice,
    Back,
    On,
    NextSplice,
}

/// A button with a transport icon drawn on it. The UI font has no such
/// symbols.
fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> egui::Response {
    let response = ui.add_sized([26.0, 20.0], egui::Button::new(""));
    let color = ui.visuals().widgets.style(&response).fg_stroke.color;
    let c = response.rect.center();
    let left = |x: f32| {
        let points = vec![
            pos2(x - 4.0, c.y),
            pos2(x + 4.0, c.y - 5.0),
            pos2(x + 4.0, c.y + 5.0),
        ];
        Shape::convex_polygon(points, color, Stroke::NONE)
    };
    let right = |x: f32| {
        let points = vec![
            pos2(x - 4.0, c.y - 5.0),
            pos2(x + 4.0, c.y),
            pos2(x - 4.0, c.y + 5.0),
        ];
        Shape::convex_polygon(points, color, Stroke::NONE)
    };
    let bar = |x: f32| Shape::rect_filled(Rect::from_center_size(pos2(x, c.y), vec2(2.0, 10.0)), 0.0, color);
    let shapes = match icon {
        Icon::PreviousSplice => vec![bar(c.x - 5.0), left(c.x + 1.5)],
        Icon::Back => vec![left(c.x)],
        Icon::On => vec![right(c.x)],
        Icon::NextSplice => vec![right(c.x - 1.5), bar(c.x + 5.0)],
    };
    ui.painter().extend(shapes);
    response.on_hover_text(tip)
}

/// The timeline of the editor of module `id`.
pub fn timeline_id(id: ModuleId) -> Id {
    Id::unique(("movie timeline", id))
}

/// A loop mark: a bracket opening into the loop.
fn bracket(painter: &Painter, x: f32, track: Rect, opens_right: bool) {
    let foot = if opens_right { 5.0 } else { -5.0 };
    let stroke = Stroke::new(2.0, MARK);
    painter.line_segment(
        [pos2(x, track.top() + 2.0), pos2(x, track.bottom() - 2.0)],
        stroke,
    );
    for y in [track.top() + 2.0, track.bottom() - 2.0] {
        painter.line_segment([pos2(x, y), pos2(x + foot, y)], stroke);
    }
}

/// The strip along the bottom of a Movie panel's picture: splices, the
/// loop, the playhead.
pub fn draw_strip(painter: &Painter, screen: Rect, tl: &Timeline, marks: &[f64]) {
    let strip = Rect::from_min_max(pos2(screen.left(), screen.bottom() - 5.0), screen.max);
    let frames = tl.frames as f32;
    let x = |frame: u32| strip.left() + frame as f32 / frames * strip.width();
    painter.rect_filled(strip, 0.0, Color32::from_black_alpha(190));
    painter.rect_filled(
        Rect::from_x_y_ranges(x(tl.first)..=x(tl.last + 1), strip.y_range()),
        0.0,
        panel::category_color(Category::Io).gamma_multiply(0.8),
    );
    for &start in &splice_starts(marks, tl.fps, tl.frames)[1..] {
        painter.line_segment(
            [pos2(x(start), strip.top()), pos2(x(start), strip.bottom())],
            Stroke::new(1.0, Color32::from_gray(225)),
        );
    }
    let px = x(tl.frame);
    painter.line_segment(
        [pos2(px, strip.top() - 1.0), pos2(px, strip.bottom())],
        Stroke::new(1.5, PLAYHEAD),
    );
}

/// Minutes, seconds and hundredths: 1:02.50.
fn clock(seconds: f64) -> String {
    let hundredths = (seconds.max(0.0) * 100.0).round() as u64;
    let (minutes, rest) = (hundredths / 6000, hundredths % 6000);
    format!("{minutes}:{:02}.{:02}", rest / 100, rest % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_symbols_are_in_the_font() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let font = FontId::proportional(14.0);
        for c in "·…".chars() {
            assert!(ctx.fonts_mut(|f| f.has_glyph(&font, c)), "{c} has no glyph");
        }
    }

    #[test]
    fn times_read_as_a_clock() {
        assert_eq!(clock(62.5), "1:02.50");
        assert_eq!(clock(11.4583), "0:11.46");
    }
}
