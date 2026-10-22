//! Procedural module panels: a layout derived from the descriptor, and the
//! drawing of panels, knobs, switches, jacks and cables.
//!
//! Geometry follows VCV Rack: 1 HP = 15 px, rows 380 px tall. Panels grow in
//! 4 HP columns until everything fits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use dov_core::{Category, Curve, ModuleDesc, ParamDesc, ParamKind, PortDesc, PortKind};
use dov_format::panel::{Place, component_id};

use crate::art::{self, LoadedArt};
use egui::epaint::CubicBezierShape;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

pub const HP: f32 = 15.0;
pub const ROW_H: f32 = 380.0;
pub const CELL_W: f32 = 60.0;
const CELL_HP: i32 = 4;
const HEADER_H: f32 = 30.0;
const TEXT_H: f32 = 30.0;
const CHOICE_H: f32 = 40.0;
const KNOB_H: f32 = 60.0;
const JACK_H: f32 = 50.0;
const GAP: f32 = 6.0;
const MAX_COLS: usize = 8;

pub const KNOB_R: f32 = 15.0;
pub const ATTEN_R: f32 = 8.0;
pub const JACK_R: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
pub enum Element {
    /// The module's text field, across the panel.
    Text,
    Choice {
        param: usize,
    },
    Knob {
        param: usize,
    },
    /// A CV jack with its attenuverter.
    Cv {
        input: usize,
        param: usize,
        atten: usize,
    },
    Input {
        input: usize,
    },
    Output {
        output: usize,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub element: Element,
    /// Relative to the panel's top-left corner.
    pub cell: Rect,
}

impl Placed {
    /// Where the jack sits, for elements that have one.
    pub fn jack_center(&self) -> Option<Vec2> {
        let c = self.cell.center().to_vec2();
        match self.element {
            Element::Input { .. } | Element::Output { .. } => Some(c - vec2(0.0, 7.0)),
            Element::Cv { .. } => Some(c + vec2(-12.0, -7.0)),
            Element::Text | Element::Choice { .. } | Element::Knob { .. } => None,
        }
    }
}

pub struct PanelLayout {
    pub hp: i32,
    pub elements: Vec<Placed>,
    /// Background box behind the outputs, VCV-style.
    pub output_box: Option<Rect>,
    /// Live picture area for modules with a screen.
    pub screen: Option<Rect>,
    /// Content height, at most one rack row.
    pub height: f32,
    /// Panel art drawn instead of the plain panel.
    pub art: Option<Arc<LoadedArt>>,
    /// Whether the app draws the title and labels. Full art draws its own.
    pub labels: bool,
}

impl PanelLayout {
    pub fn size(&self) -> Vec2 {
        vec2(self.hp as f32 * HP, ROW_H)
    }
}

/// The panel layout for a module type, computed once per type and art: full
/// panel art places the controls itself; otherwise the narrowest panel that
/// fits the row, with the biggest screen that allows it, under any
/// background art.
pub fn layout(desc: &'static ModuleDesc) -> Arc<PanelLayout> {
    type Cache = (u64, HashMap<usize, Arc<PanelLayout>>);
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let generation = art::generation();
    if cache.0 != generation {
        *cache = (generation, HashMap::new());
    }
    cache
        .1
        // By identity, not id: a reloaded module file can change its panel.
        .entry(std::ptr::from_ref(desc) as usize)
        .or_insert_with(|| {
            let art = art::art_for(desc.id);
            if let Some(full) = art.as_ref().and_then(|a| art_layout(desc, a)) {
                return Arc::new(full);
            }
            let screens: &[f32] = if desc.screen { &SCREEN_WIDTHS } else { &[0.0] };
            let min_cols = if desc.screen { 2 } else { 1 };
            let fitting = (min_cols..=MAX_COLS)
                .flat_map(|cols| screens.iter().map(move |&w| (cols, w)))
                .map(|(cols, w)| place(desc, cols, w))
                .find(|l| l.height <= ROW_H);
            let mut layout =
                fitting.unwrap_or_else(|| place(desc, MAX_COLS, *screens.last().unwrap_or(&0.0)));
            // Background art (no components) goes under the automatic layout.
            layout.art = art.filter(|a| a.art.components.is_none());
            Arc::new(layout)
        })
        .clone()
}

/// The layout full art describes: each control centered on its component.
/// `None` if the art doesn't fit the module (missing or unknown components),
/// in which case the panel falls back to the automatic layout.
fn art_layout(desc: &ModuleDesc, loaded: &Arc<LoadedArt>) -> Option<PanelLayout> {
    let components = loaded.art.components.as_ref()?;
    dov_format::panel::check_components(components, &art::required(desc)).ok()?;
    let find = |id: String| components.iter().find(|c| c.id == id);
    let center = |id: String| find(id).map(|c| Pos2::from(c.center()));
    let rect = |id: String| {
        find(id).map(|c| Rect::from_min_max(pos2(c.rect[0], c.rect[1]), pos2(c.rect[2], c.rect[3])))
    };
    // Cells are offset so each control's drawing lands on the center.
    let cell =
        |c: Pos2, h: f32, top: f32| Rect::from_min_size(pos2(c.x - CELL_W / 2.0, c.y - top), vec2(CELL_W, h));
    let jack = |c: Pos2, dx: f32| Rect::from_center_size(pos2(c.x + dx, c.y + 7.0), vec2(CELL_W, JACK_H));

    let mut elements = Vec::new();
    if desc.text.is_some() {
        elements.push(Placed {
            element: Element::Text,
            cell: rect(component_id(Place::Text))?,
        });
    }
    for (i, p) in desc.params.iter().enumerate() {
        let c = center(component_id(Place::Param(p.id)))?;
        let (element, cell) = if art::is_switch(desc, i) {
            (Element::Choice { param: i }, cell(c, CHOICE_H, 25.0))
        } else {
            (Element::Knob { param: i }, cell(c, KNOB_H, 22.0))
        };
        elements.push(Placed { element, cell });
        if let Some((input, atten)) = desc.cv_slot(i) {
            elements.push(Placed {
                element: Element::Cv {
                    input,
                    param: i,
                    atten,
                },
                cell: jack(center(component_id(Place::Cv(p.id)))?, 12.0),
            });
        }
    }
    for (i, p) in desc.inputs.iter().enumerate().filter(|(_, p)| !p.hidden) {
        elements.push(Placed {
            element: Element::Input { input: i },
            cell: jack(center(component_id(Place::Input(p.id)))?, 0.0),
        });
    }
    for (k, p) in desc.outputs.iter().enumerate().filter(|(_, p)| !p.hidden) {
        elements.push(Placed {
            element: Element::Output { output: k },
            cell: jack(center(component_id(Place::Output(p.id)))?, 0.0),
        });
    }
    let screen = if desc.screen {
        Some(rect(component_id(Place::Screen))?)
    } else {
        None
    };
    Some(PanelLayout {
        hp: loaded.art.hp(),
        elements,
        output_box: None,
        screen,
        height: ROW_H,
        art: Some(loaded.clone()),
        labels: false,
    })
}

/// Screen widths to try, largest first.
const SCREEN_WIDTHS: [f32; 3] = [168.0, 120.0, 90.0];

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Text,
    Choices,
    Knobs,
    Jacks,
}

/// Panel rows: the text field, switches, then knobs, then one grid of jacks
/// (CV inputs, inputs, then outputs, which share the last input row when
/// they fit).
fn rows(desc: &ModuleDesc, cols: usize) -> Vec<(Section, Vec<Element>)> {
    let mut choices = Vec::new();
    let mut knobs = Vec::new();
    let mut ins = Vec::new();
    for (i, p) in desc.params.iter().enumerate() {
        match p.kind {
            ParamKind::Choice { .. } => choices.push(Element::Choice { param: i }),
            ParamKind::Knob { .. } => knobs.push(Element::Knob { param: i }),
        }
        if let Some((input, atten)) = desc.cv_slot(i) {
            ins.push(Element::Cv {
                input,
                param: i,
                atten,
            });
        }
    }
    let visible = |p: &&PortDesc| !p.hidden;
    for (i, _) in desc.inputs.iter().enumerate().filter(|(_, p)| visible(p)) {
        ins.push(Element::Input { input: i });
    }
    let outs: Vec<_> = desc
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, p)| visible(p))
        .map(|(i, _)| Element::Output { output: i })
        .collect();

    let mut rows = Vec::new();
    if desc.text.is_some() {
        rows.push((Section::Text, vec![Element::Text]));
    }
    for (section, items) in [(Section::Choices, choices), (Section::Knobs, knobs)] {
        rows.extend(items.chunks(cols).map(|c| (section, c.to_vec())));
    }
    let mut jack_rows: Vec<Vec<Element>> = ins.chunks(cols).map(<[Element]>::to_vec).collect();
    match jack_rows.last_mut() {
        Some(last) if last.len() + outs.len() <= cols => last.extend(outs),
        _ => jack_rows.extend(outs.chunks(cols).map(<[Element]>::to_vec)),
    }
    rows.extend(jack_rows.into_iter().map(|r| (Section::Jacks, r)));
    rows
}

fn place(desc: &ModuleDesc, cols: usize, screen_w: f32) -> PanelLayout {
    let width = cols as f32 * CELL_W;
    let mut y = HEADER_H + GAP;
    let mut screen = None;
    if desc.screen {
        let w = (width - 12.0).min(screen_w);
        let rect = Rect::from_min_size(pos2((width - w) / 2.0, y), vec2(w, w * 0.75));
        y += rect.height() + GAP;
        screen = Some(rect);
    }
    let mut elements = Vec::new();
    let mut output_box: Option<Rect> = None;
    let mut prev = None;
    for (section, row) in rows(desc, cols) {
        if prev.is_some_and(|p| p != section) {
            y += GAP;
        }
        prev = Some(section);
        let h = match section {
            Section::Text => TEXT_H,
            Section::Choices => CHOICE_H,
            Section::Knobs => KNOB_H,
            Section::Jacks => JACK_H,
        };
        let x0 = (width - row.len() as f32 * CELL_W) / 2.0;
        for (c, element) in row.into_iter().enumerate() {
            let cell = match element {
                Element::Text => Rect::from_min_size(pos2(8.0, y), vec2(width - 16.0, h)),
                _ => Rect::from_min_size(pos2(x0 + c as f32 * CELL_W, y), vec2(CELL_W, h)),
            };
            if matches!(element, Element::Output { .. }) {
                let cell_box = cell.shrink2(vec2(2.0, 0.0));
                output_box = Some(output_box.map_or(cell_box, |b| b.union(cell_box)));
            }
            elements.push(Placed { element, cell });
        }
        y += h;
    }
    PanelLayout {
        hp: cols as i32 * CELL_HP,
        elements,
        output_box,
        screen,
        height: y + GAP,
        art: None,
        labels: true,
    }
}

// ------------------------------------------------------------------ values

/// Knob position 0..1 for a param value.
pub fn to_norm(p: &ParamDesc, v: f32) -> f32 {
    match p.kind {
        ParamKind::Knob { min, max, curve, .. } => match curve {
            Curve::Exp if min > 0.0 => (v.max(min) / min).ln() / (max / min).ln(),
            _ => (v - min) / (max - min),
        },
        ParamKind::Choice { options, .. } => v / (options.len().max(2) - 1) as f32,
    }
    .clamp(0.0, 1.0)
}

pub fn from_norm(p: &ParamDesc, n: f32) -> f32 {
    let n = n.clamp(0.0, 1.0);
    match p.kind {
        ParamKind::Knob { min, max, curve, .. } => match curve {
            Curve::Exp if min > 0.0 => min * (max / min).powf(n),
            _ => min + n * (max - min),
        },
        ParamKind::Choice { options, .. } => (n * (options.len() - 1) as f32).round(),
    }
}

pub fn format_value(p: &ParamDesc, v: f32) -> String {
    match p.kind {
        ParamKind::Knob { unit, .. } => {
            let digits = if v.abs() >= 100.0 {
                1
            } else if v.abs() >= 10.0 {
                2
            } else {
                3
            };
            if unit.is_empty() {
                format!("{v:.digits$}")
            } else {
                format!("{v:.digits$} {unit}")
            }
        }
        ParamKind::Choice { .. } => p
            .option_name(v.round().max(0.0) as usize)
            .map_or_else(|| "?".into(), |name| name.into_owned()),
    }
}

// ------------------------------------------------------------------ colors

pub fn category_color(c: Category) -> Color32 {
    match c {
        Category::Source => Color32::from_rgb(232, 131, 58),
        Category::Process => Color32::from_rgb(58, 176, 160),
        Category::Color => Color32::from_rgb(208, 74, 154),
        Category::Space => Color32::from_rgb(86, 128, 222),
        Category::Analog => Color32::from_rgb(120, 210, 120),
        Category::Control => Color32::from_rgb(216, 192, 64),
        Category::Io => Color32::from_rgb(170, 170, 170),
    }
}

pub fn kind_color(k: PortKind) -> Color32 {
    match k {
        PortKind::Mono => Color32::from_gray(205),
        PortKind::Rgb => Color32::from_rgb(232, 178, 80),
        PortKind::Control => Color32::from_rgb(90, 200, 235),
        PortKind::Composite => Color32::from_rgb(110, 235, 140),
    }
}

const CABLE_COLORS: [Color32; 7] = [
    Color32::from_rgb(242, 201, 76),
    Color32::from_rgb(235, 87, 87),
    Color32::from_rgb(111, 207, 151),
    Color32::from_rgb(86, 204, 242),
    Color32::from_rgb(187, 107, 217),
    Color32::from_rgb(242, 153, 74),
    Color32::from_rgb(230, 230, 230),
];

pub fn cable_color(index: u8) -> Color32 {
    CABLE_COLORS[index as usize % CABLE_COLORS.len()]
}

// ----------------------------------------------------------------- drawing

pub fn draw_rack_row(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(24));
    for y in [rect.top(), rect.bottom() - 10.0] {
        let rail = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), 10.0));
        painter.rect_filled(rail, 0.0, Color32::from_gray(52));
        let mut x = rect.left() + HP * 0.5;
        while x < rect.right() {
            painter.circle_filled(pos2(x, y + 5.0), 1.6, Color32::from_gray(30));
            x += HP;
        }
    }
}

/// Draws a module's panel: its art if it has some (`art` is the art's
/// texture), otherwise the plain panel with a category stripe. A stand-in
/// for a module that isn't installed (`missing`) is outlined in red and named
/// by its id.
pub fn draw_panel(
    painter: &Painter,
    rect: Rect,
    desc: &ModuleDesc,
    layout: &PanelLayout,
    lifted: bool,
    missing: bool,
    art: Option<egui::TextureId>,
) {
    if lifted {
        painter.rect_filled(
            rect.translate(vec2(4.0, 6.0)),
            3.0,
            Color32::from_black_alpha(120),
        );
    }
    let warning = Color32::from_rgb(230, 80, 70);
    match art {
        Some(texture) => {
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            painter.image(texture, rect, uv, Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, 3.0, Color32::from_rgb(44, 46, 52));
            painter.rect_stroke(
                rect,
                3.0,
                Stroke::new(1.0, Color32::from_gray(12)),
                StrokeKind::Inside,
            );
            let stripe = Rect::from_min_size(rect.min, vec2(rect.width(), 4.0));
            let color = if missing {
                warning
            } else {
                category_color(desc.category)
            };
            painter.rect_filled(stripe, 3.0, color);
            for corner in [
                pos2(rect.left() + 7.0, rect.top() + 9.0),
                pos2(rect.right() - 7.0, rect.bottom() - 7.0),
            ] {
                painter.circle_filled(corner, 2.5, Color32::from_gray(80));
            }
        }
    }
    if missing {
        painter.rect_stroke(rect, 3.0, Stroke::new(2.0, warning), StrokeKind::Inside);
        painter.text(
            pos2(rect.center().x, rect.top() + 14.0),
            Align2::CENTER_CENTER,
            "Not installed",
            FontId::proportional(12.0),
            warning,
        );
        painter.text(
            pos2(rect.center().x, rect.top() + 27.0),
            Align2::CENTER_CENTER,
            desc.id,
            FontId::monospace(9.0),
            Color32::from_gray(200),
        );
    } else if layout.labels {
        painter.text(
            pos2(rect.center().x, rect.top() + 17.0),
            Align2::CENTER_CENTER,
            desc.name,
            FontId::proportional(13.0),
            Color32::from_gray(235),
        );
    }
    if let Some(b) = layout.output_box {
        painter.rect_filled(b.translate(rect.min.to_vec2()), 4.0, Color32::from_gray(22));
    }
}

/// Angle in radians, 0 = up, clockwise positive.
fn dir(angle: f32) -> Vec2 {
    vec2(angle.sin(), -angle.cos())
}

fn arc(painter: &Painter, center: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = ((a1 - a0).abs() * 12.0).ceil().max(2.0) as usize;
    let points = (0..=n)
        .map(|i| center + dir(a0 + (a1 - a0) * i as f32 / n as f32) * r)
        .collect();
    painter.add(Shape::line(points, stroke));
}

const KNOB_SWEEP: f32 = 135.0 * std::f32::consts::PI / 180.0;

pub fn draw_knob(
    painter: &Painter,
    center: Pos2,
    r: f32,
    norm: f32,
    bipolar: bool,
    accent: Color32,
    hot: bool,
) {
    let a = -KNOB_SWEEP + norm * 2.0 * KNOB_SWEEP;
    arc(
        painter,
        center,
        r + 4.0,
        -KNOB_SWEEP,
        KNOB_SWEEP,
        Stroke::new(2.0, Color32::from_gray(30)),
    );
    let from = if bipolar { 0.0 } else { -KNOB_SWEEP };
    arc(painter, center, r + 4.0, from, a, Stroke::new(2.5, accent));
    painter.circle_filled(center, r, Color32::from_gray(if hot { 70 } else { 58 }));
    painter.circle_stroke(center, r, Stroke::new(1.0, Color32::from_gray(20)));
    painter.line_segment(
        [center + dir(a) * r * 0.25, center + dir(a) * r * 0.9],
        Stroke::new(2.5, Color32::from_gray(240)),
    );
}

pub fn draw_jack(painter: &Painter, center: Pos2, kind: PortKind, highlight: Option<Color32>) {
    painter.circle_filled(center, JACK_R, Color32::from_gray(16));
    painter.circle_stroke(center, JACK_R - 1.0, Stroke::new(2.0, kind_color(kind)));
    painter.circle_filled(center, 4.5, Color32::from_gray(4));
    if let Some(c) = highlight {
        painter.circle_stroke(center, JACK_R + 3.0, Stroke::new(2.0, c));
    }
}

/// A light showing a control signal's level: blue when positive, red when
/// negative, brightest at ±1.
pub fn draw_led(painter: &Painter, center: Pos2, value: f32) {
    let lit = if value >= 0.0 {
        kind_color(PortKind::Control)
    } else {
        Color32::from_rgb(235, 87, 87)
    };
    let color = Color32::from_gray(28).lerp_to_gamma(lit, value.abs().min(1.0));
    painter.circle_filled(center, 3.5, color);
    painter.circle_stroke(center, 3.5, Stroke::new(1.0, Color32::from_gray(8)));
}

pub fn draw_label(painter: &Painter, pos: Pos2, text: &str) {
    painter.text(
        pos,
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(10.0),
        Color32::from_gray(185),
    );
}

pub fn draw_switch(painter: &Painter, rect: Rect, label: &str, value: &str, hot: bool) {
    draw_label(painter, pos2(rect.center().x, rect.top() + 7.0), label);
    let body = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 25.0),
        vec2(rect.width() - 8.0, 18.0),
    );
    painter.rect_filled(body, 3.0, Color32::from_gray(if hot { 34 } else { 22 }));
    painter.rect_stroke(
        body,
        3.0,
        Stroke::new(1.0, Color32::from_gray(70)),
        StrokeKind::Inside,
    );
    painter.text(
        body.center(),
        Align2::CENTER_CENTER,
        value,
        FontId::proportional(11.0),
        Color32::from_rgb(255, 200, 120),
    );
}

/// A box naming something picked from a list, such as a device: the name,
/// cut to fit, and a mark showing that clicking opens the list.
pub fn draw_picker(painter: &Painter, rect: Rect, value: &str, hot: bool) {
    let body = rect.shrink2(vec2(0.0, 4.0));
    painter.rect_filled(body, 3.0, Color32::from_gray(if hot { 34 } else { 22 }));
    painter.rect_stroke(
        body,
        3.0,
        Stroke::new(1.0, Color32::from_gray(70)),
        StrokeKind::Inside,
    );
    let color = Color32::from_rgb(255, 200, 120);
    let mut job =
        egui::text::LayoutJob::simple_singleline(value.to_owned(), FontId::proportional(11.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(body.width() - 26.0);
    let galley = painter.layout_job(job);
    let at = pos2(body.left() + 8.0, body.center().y - galley.size().y / 2.0);
    painter.galley(at, galley, color);
    let mark = pos2(body.right() - 10.0, body.center().y);
    painter.add(Shape::convex_polygon(
        vec![
            mark + vec2(-4.0, -2.0),
            mark + vec2(4.0, -2.0),
            mark + vec2(0.0, 3.0),
        ],
        Color32::from_gray(160),
        Stroke::NONE,
    ));
}

/// A patch cable hanging between two jacks.
pub fn draw_cable(painter: &Painter, a: Pos2, b: Pos2, color: Color32) {
    let sag = (a.distance(b) * 0.35).clamp(25.0, 200.0);
    let points = [a, a + vec2(0.0, sag), b + vec2(0.0, sag), b];
    let shadow = Color32::from_black_alpha(140);
    painter.add(CubicBezierShape::from_points_stroke(
        points,
        false,
        Color32::TRANSPARENT,
        Stroke::new(7.0, shadow),
    ));
    painter.add(CubicBezierShape::from_points_stroke(
        points,
        false,
        Color32::TRANSPARENT,
        Stroke::new(4.5, color.gamma_multiply(0.9)),
    ));
    for end in [a, b] {
        painter.circle_filled(end, 7.0, color);
        painter.circle_filled(end, 3.0, Color32::from_black_alpha(160));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_fits_its_row() {
        for m in dov_modules::all() {
            let l = layout(&m.desc);
            assert!(l.height <= ROW_H, "{} overflows: {}", m.desc.id, l.height);
            for p in &l.elements {
                assert!(p.cell.max.x <= l.hp as f32 * HP + 0.5, "{}", m.desc.id);
            }
        }
    }

    #[test]
    fn knob_mapping_round_trips() {
        let lin = ParamDesc::knob("a", "A", -1.0, 1.0, 0.0);
        let exp = ParamDesc::knob("b", "B", 0.01, 20.0, 1.0).exp();
        for p in [lin, exp] {
            for n in [0.0, 0.25, 0.5, 1.0] {
                assert!((to_norm(&p, from_norm(&p, n)) - n).abs() < 1e-5);
            }
        }
    }
}
