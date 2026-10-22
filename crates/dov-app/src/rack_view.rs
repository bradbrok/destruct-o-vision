//! The rack canvas: modules on rails, knobs, jacks and cables, with
//! VCV-style patching. Drag from a jack to another to patch; drag a plugged
//! input to pick its cable back up; right-click for menus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dov_core::{Category, ModuleDesc, ModuleId, ParamBank, ParamKind, Patch, PortAddr, PortKind};
use dov_modules::{ModuleType, Origin};
use egui::emath::TSTransform;
use egui::load::SizedTexture;
use egui::{Color32, Id, Painter, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::art::LoadedArt;
use crate::panel::{self, ATTEN_R, Element, HP, JACK_R, KNOB_R, Placed, ROW_H};
use crate::rack::Rack;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortEnd {
    pub module: ModuleId,
    pub side: Side,
    pub index: usize,
}

struct Jack {
    end: PortEnd,
    pos: Pos2,
}

struct CableDrag {
    /// The end that stays put; the other follows the pointer.
    anchor: PortEnd,
    color: Option<u8>,
}

struct ModuleDrag {
    id: ModuleId,
    grab: Vec2,
    origin: Pos2,
}

/// Edits collected while drawing and applied afterwards.
enum Action {
    /// Push a state captured before an in-place param edit.
    Undo(Box<Patch>),
    Move(ModuleId, i32, i32),
    Take(PortAddr),
    Connect(PortAddr, PortAddr, Option<u8>),
    Disconnect(PortAddr),
    Remove(ModuleId),
    Duplicate(ModuleId),
    Reset(ModuleId),
    Add(&'static ModuleType, i32, i32),
}

/// The probe preview the engine renders for the hovered jack.
pub struct ProbeView {
    pub texture: egui::TextureId,
    pub aspect: f32,
    /// Live pictures for modules with a screen.
    pub screens: std::collections::HashMap<ModuleId, egui::TextureId>,
}

pub struct RackView {
    scene_rect: Rect,
    cable_drag: Option<CableDrag>,
    module_drag: Option<ModuleDrag>,
    menu_at: Option<Pos2>,
    /// The module under the pointer, for keyboard shortcuts.
    pub hovered_module: Option<ModuleId>,
    /// Scene-to-screen transform from the last frame.
    to_global: TSTransform,
    /// Panel art, rasterized off the UI thread.
    art: ArtCache,
}

impl Default for RackView {
    fn default() -> Self {
        Self {
            // ZERO makes the scene fit its contents on the first frame.
            scene_rect: Rect::ZERO,
            cable_drag: None,
            module_drag: None,
            menu_at: None,
            hovered_module: None,
            to_global: TSTransform::IDENTITY,
            art: ArtCache::default(),
        }
    }
}

impl RackView {
    /// Zoom to fit the whole rack on the next frame.
    pub fn fit(&mut self) {
        self.scene_rect = Rect::ZERO;
    }

    /// Draws the rack and handles editing. Returns the module output to probe.
    pub fn show(&mut self, ui: &mut Ui, rack: &mut Rack, probe: &ProbeView) -> Option<(ModuleId, usize)> {
        let mut target = None;
        let mut scene_rect = self.scene_rect;
        egui::Scene::new()
            .zoom_range(0.2..=1.5)
            .show(ui, &mut scene_rect, |ui| target = self.contents(ui, rack, probe));
        self.scene_rect = scene_rect;
        target
    }

    fn contents(&mut self, ui: &mut Ui, rack: &mut Rack, probe: &ProbeView) -> Option<(ModuleId, usize)> {
        let mut actions = Vec::new();
        let mut probe_target = None;
        self.hovered_module = None;
        let pointer = pointer_in_scene(ui);
        self.to_global = ui
            .ctx()
            .layer_transform_to_global(ui.layer_id())
            .unwrap_or_default();

        // Rails: the occupied rows plus an empty one, with room to the right.
        let rows = rack.modules.iter().map(|m| m.pos.0 + 1).max().unwrap_or(1) + 1;
        let cols = rack
            .modules
            .iter()
            .map(|m| m.pos.1 + m.hp())
            .max()
            .unwrap_or(0)
            .max(60)
            + 24;
        let world = Rect::from_min_size(Pos2::ZERO, vec2(cols as f32 * HP, rows as f32 * ROW_H));
        let background = ui.allocate_rect(world, Sense::click());
        let painter = ui.painter().clone();
        for r in 0..rows {
            let row = Rect::from_min_size(pos2(0.0, r as f32 * ROW_H), vec2(world.width(), ROW_H));
            panel::draw_rack_row(&painter, row);
        }
        if background.secondary_clicked() {
            self.menu_at = pointer;
        }
        let menu_at = self.menu_at.unwrap_or_default();
        background.context_menu(|ui| {
            if let Some(ty) = add_module_menu(ui) {
                let row = (menu_at.y / ROW_H).floor() as i32;
                let col = (menu_at.x / HP).round() as i32;
                actions.push(Action::Add(ty, row, col));
            }
        });

        // Jack positions first: cables and drops need every module's jacks.
        let origin = |id: ModuleId, pos: (i32, i32), drag: &Option<ModuleDrag>| match drag {
            Some(d) if d.id == id => d.origin,
            _ => pos2(pos.1 as f32 * HP, pos.0 as f32 * ROW_H),
        };
        let mut jacks = Vec::new();
        for m in &rack.modules {
            let at = origin(m.id, m.pos, &self.module_drag);
            for p in &panel::layout(&m.ty.desc).elements {
                let (Some(center), Some((side, index))) = (p.jack_center(), jack_of(p)) else {
                    continue;
                };
                jacks.push(Jack {
                    end: PortEnd {
                        module: m.id,
                        side,
                        index,
                    },
                    pos: at + center,
                });
            }
        }

        // Modules; the one being dragged is drawn last so it floats on top.
        let mut order: Vec<usize> = (0..rack.modules.len()).collect();
        if let Some(d) = &self.module_drag
            && let Some(k) = order.iter().position(|&i| rack.modules[i].id == d.id)
        {
            let i = order.remove(k);
            order.push(i);
        }
        for i in order {
            let m = &rack.modules[i];
            let (id, desc, params) = (m.id, &m.ty.desc, m.params.clone());
            let layout = panel::layout(desc);
            let at = origin(id, m.pos, &self.module_drag);
            let rect = Rect::from_min_size(at, layout.size());
            let response = ui.interact(rect, Id::unique(("panel", id)), Sense::click_and_drag());
            if response.contains_pointer() {
                self.hovered_module = Some(id);
            }
            if response.drag_started()
                && let Some(p) = pointer
            {
                actions.push(Action::Undo(Box::new(rack.to_patch())));
                self.module_drag = Some(ModuleDrag {
                    id,
                    grab: p - at,
                    origin: at,
                });
            }
            if response.dragged()
                && let (Some(d), Some(p)) = (&mut self.module_drag, pointer)
                && d.id == id
            {
                d.origin = p - d.grab;
            }
            if response.drag_stopped()
                && let Some(d) = self.module_drag.take()
            {
                let row = (d.origin.y / ROW_H).round() as i32;
                let col = (d.origin.x / HP).round() as i32;
                actions.push(Action::Move(id, row, col));
            }
            response.context_menu(|ui| {
                ui.strong(desc.name);
                ui.label(desc.description);
                ui.separator();
                if ui.button("Duplicate").clicked() {
                    actions.push(Action::Duplicate(id));
                    ui.close();
                }
                if ui.button("Reset parameters").clicked() {
                    actions.push(Action::Reset(id));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    actions.push(Action::Remove(id));
                    ui.close();
                }
            });

            let lifted = self.module_drag.as_ref().is_some_and(|d| d.id == id);
            let pixels = ui.ctx().pixels_per_point() * self.to_global.scaling;
            let art = layout
                .art
                .as_ref()
                .and_then(|a| self.art.texture(ui.ctx(), a, rect.size(), pixels));
            panel::draw_panel(
                &painter,
                rect,
                desc,
                &layout,
                lifted,
                m.ty.origin == Origin::Missing,
                art,
            );
            if let Some(screen) = layout.screen {
                let screen = screen.translate(at.to_vec2());
                painter.rect_filled(screen.expand(2.0), 2.0, Color32::from_gray(8));
                if let Some(&texture) = probe.screens.get(&id) {
                    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                    painter.image(texture, screen, uv, Color32::WHITE);
                }
            }
            for placed in &layout.elements {
                let cell = placed.cell.translate(at.to_vec2());
                let ctx = ElementCtx {
                    rack,
                    id,
                    desc,
                    params: &params,
                    painter: &painter,
                    probe,
                    labels: layout.labels,
                };
                self.element(ui, &ctx, placed, cell, &mut actions, &mut probe_target);
            }
        }

        let pos_of = |end: PortEnd| jacks.iter().find(|j| j.end == end).map(|j| j.pos);
        for c in &rack.cables {
            let ends = (end_of(rack, &c.from, Side::Out), end_of(rack, &c.to, Side::In));
            if let (Some(a), Some(b)) = (ends.0.and_then(pos_of), ends.1.and_then(pos_of)) {
                panel::draw_cable(&painter, a, b, panel::cable_color(c.color));
            }
        }

        // The cable being patched follows the pointer and snaps to valid jacks.
        if let Some(drag) = &self.cable_drag {
            let target = pointer.and_then(|p| {
                jacks
                    .iter()
                    .filter(|j| compatible(rack, drag.anchor, j.end))
                    .min_by(|a, b| a.pos.distance(p).total_cmp(&b.pos.distance(p)))
                    .filter(|j| j.pos.distance(p) < 2.0 * JACK_R)
            });
            if let (Some(a), Some(p)) = (pos_of(drag.anchor), pointer) {
                let color = drag.color.map_or(Color32::from_gray(230), panel::cable_color);
                panel::draw_cable(&painter, a, target.map_or(p, |j| j.pos), color);
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(target) = target {
                    let (out, inp) = if drag.anchor.side == Side::Out {
                        (drag.anchor, target.end)
                    } else {
                        (target.end, drag.anchor)
                    };
                    if let (Some(from), Some(to)) = (port_addr(rack, out), port_addr(rack, inp)) {
                        actions.push(Action::Connect(from, to, drag.color));
                    }
                }
                self.cable_drag = None;
            }
        }

        for action in actions {
            match action {
                Action::Undo(patch) => rack.push_undo(*patch),
                Action::Move(id, row, col) => rack.move_module(id, row, col),
                Action::Take(addr) => {
                    rack.take_cable(&addr);
                }
                Action::Connect(from, to, color) => rack.connect(from, to, color),
                Action::Disconnect(addr) => rack.disconnect_all(&addr),
                Action::Remove(id) => rack.remove_module(id),
                Action::Duplicate(id) => {
                    rack.duplicate(id);
                }
                Action::Reset(id) => rack.reset_params(id),
                Action::Add(ty, row, col) => {
                    rack.add_module(ty, row.max(0), col.max(0));
                }
            }
        }
        self.art.end_frame();
        probe_target
    }

    fn element(
        &mut self,
        ui: &mut Ui,
        ctx: &ElementCtx,
        placed: &Placed,
        cell: Rect,
        actions: &mut Vec<Action>,
        probe_target: &mut Option<(ModuleId, usize)>,
    ) {
        let (desc, params, painter, id) = (ctx.desc, ctx.params, ctx.painter, ctx.id);
        let labels = ctx.labels;
        let label = |painter: &Painter, at: Pos2, text: &str| {
            if labels {
                panel::draw_label(painter, at, text);
            }
        };
        let accent = panel::category_color(desc.category);
        let inputs = desc.inputs_full();
        match placed.element {
            Element::Text => {
                let mut text = params.text();
                let field = egui::TextEdit::singleline(&mut text)
                    .id(Id::unique(("text", id)))
                    .font(egui::FontId::monospace(13.0))
                    .align(egui::Align2::CENTER_CENTER)
                    .char_limit(80);
                let response = ui.put(cell.shrink2(vec2(0.0, 3.0)), field);
                if response.gained_focus() {
                    actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
                }
                if response.changed() {
                    params.set_text(&text);
                }
                response.on_hover_text("Type here; the text renders live");
            }
            Element::Choice { param } => {
                let p = &desc.params[param];
                let ParamKind::Choice { options, .. } = p.kind else {
                    return;
                };
                let n = options.len();
                let response =
                    ui.interact(cell.shrink(3.0), Id::unique(("param", id, param)), Sense::click());
                let v = (params.get(param).round().max(0.0) as usize).min(n - 1);
                let step = if response.clicked() {
                    Some(1)
                } else if response.secondary_clicked() {
                    Some(n - 1)
                } else {
                    None
                };
                if let Some(step) = step {
                    actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
                    params.set(param, ((v + step) % n) as f32);
                }
                let v = (params.get(param).round().max(0.0) as usize).min(n - 1);
                let label = if ctx.labels { p.name } else { "" };
                panel::draw_switch(painter, cell, label, options[v], response.hovered());
                response.on_hover_text(format!(
                    "{}: {}\nClick: next · right-click: previous",
                    p.name,
                    options.join(" / ")
                ));
            }
            Element::Knob { param } => {
                let p = &desc.params[param];
                let center = pos2(cell.center().x, cell.top() + 22.0);
                let bipolar = matches!(p.kind, ParamKind::Knob { min, max, .. } if min == -max);
                let response = knob(
                    ui,
                    ctx,
                    Id::unique(("param", id, param)),
                    center,
                    KNOB_R,
                    param,
                    |v| panel::to_norm(p, v),
                    |n| panel::from_norm(p, n),
                    KnobSpec::param(p),
                    actions,
                );
                let norm = panel::to_norm(p, params.get(param));
                panel::draw_knob(
                    painter,
                    center,
                    KNOB_R,
                    norm,
                    bipolar,
                    accent,
                    response.hovered() || response.dragged(),
                );
                label(painter, pos2(cell.center().x, cell.bottom() - 10.0), p.name);
                response.on_hover_text(format!(
                    "{}: {}\nDrag (shift: fine) · double-click: reset · right-click: type a value",
                    p.name,
                    panel::format_value(p, params.get(param))
                ));
            }
            Element::Cv { input, param, atten } => {
                let p = &desc.params[param];
                let jack_at = cell.center() + vec2(-12.0, -7.0);
                self.jack(
                    ui,
                    ctx,
                    PortEnd {
                        module: id,
                        side: Side::In,
                        index: input,
                    },
                    jack_at,
                    &inputs[input].name,
                    inputs[input].kind,
                    actions,
                    probe_target,
                );
                let knob_at = cell.center() + vec2(14.0, -7.0);
                let response = knob(
                    ui,
                    ctx,
                    Id::unique(("atten", id, atten)),
                    knob_at,
                    ATTEN_R,
                    atten,
                    |v| (v + 1.0) * 0.5,
                    |n| n * 2.0 - 1.0,
                    KnobSpec {
                        label: format!("{} CV amount", p.name),
                        min: -1.0,
                        max: 1.0,
                        unit: "",
                        default: dov_core::module::DEFAULT_ATTENUVERTER,
                    },
                    actions,
                );
                let norm = (params.get(atten) + 1.0) * 0.5;
                panel::draw_knob(
                    painter,
                    knob_at,
                    ATTEN_R,
                    norm,
                    true,
                    accent,
                    response.hovered() || response.dragged(),
                );
                label(
                    painter,
                    pos2(cell.center().x, cell.bottom() - 10.0),
                    &format!("{} CV", p.name),
                );
                response.on_hover_text(format!("{} CV amount: {:+.2}", p.name, params.get(atten)));
            }
            Element::Input { input } => {
                let at = cell.center() + vec2(0.0, -7.0);
                self.jack(
                    ui,
                    ctx,
                    PortEnd {
                        module: id,
                        side: Side::In,
                        index: input,
                    },
                    at,
                    &inputs[input].name,
                    inputs[input].kind,
                    actions,
                    probe_target,
                );
                label(
                    painter,
                    pos2(cell.center().x, cell.bottom() - 10.0),
                    &inputs[input].name,
                );
            }
            Element::Output { output } => {
                let port = &desc.outputs[output];
                let at = cell.center() + vec2(0.0, -7.0);
                self.jack(
                    ui,
                    ctx,
                    PortEnd {
                        module: id,
                        side: Side::Out,
                        index: output,
                    },
                    at,
                    port.name,
                    port.kind,
                    actions,
                    probe_target,
                );
                label(painter, pos2(cell.center().x, cell.bottom() - 10.0), port.name);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn jack(
        &mut self,
        ui: &mut Ui,
        ctx: &ElementCtx,
        end: PortEnd,
        center: Pos2,
        name: &str,
        kind: PortKind,
        actions: &mut Vec<Action>,
        probe_target: &mut Option<(ModuleId, usize)>,
    ) {
        let rack = ctx.rack;
        let response = ui.interact(
            Rect::from_center_size(center, Vec2::splat(2.0 * JACK_R + 6.0)),
            Id::unique(("jack", end)),
            Sense::click_and_drag(),
        );
        let Some(addr) = port_addr(rack, end) else { return };
        let highlight = match &self.cable_drag {
            Some(d) if compatible(rack, d.anchor, end) => Some(Color32::from_rgb(120, 255, 160)),
            _ if response.hovered() => Some(Color32::from_gray(220)),
            _ => None,
        };
        panel::draw_jack(ctx.painter, center, kind, highlight);

        let plugged = (end.side == Side::In).then(|| rack.cable_into(&addr)).flatten();
        if response.drag_started() {
            self.cable_drag = Some(
                match plugged.and_then(|c| Some((c, end_of(rack, &c.from, Side::Out)?))) {
                    // Grab the plug: move this cable's input end.
                    Some((cable, from)) => {
                        actions.push(Action::Take(addr.clone()));
                        CableDrag {
                            anchor: from,
                            color: Some(cable.color),
                        }
                    }
                    None => CableDrag {
                        anchor: end,
                        color: None,
                    },
                },
            );
        }

        // What this jack carries, for the probe: an output's own signal, or
        // whatever is patched into an input.
        let source = match end.side {
            Side::Out => kind.is_video().then_some((end.module, end.index)),
            Side::In => plugged.and_then(|c| {
                let from = end_of(rack, &c.from, Side::Out)?;
                let out_kind = rack.module(from.module)?.ty.desc.outputs[from.index].kind;
                out_kind.is_video().then_some((from.module, from.index))
            }),
        };
        if response.hovered() && self.cable_drag.is_none() {
            *probe_target = source;
        }

        response.context_menu(|ui| {
            if ui.button("Disconnect").clicked() {
                actions.push(Action::Disconnect(addr.clone()));
                ui.close();
            }
        });
        let kind_name = match kind {
            PortKind::Mono => "mono video",
            PortKind::Rgb => "RGB video",
            PortKind::Control => "control",
            PortKind::Composite => "composite",
        };
        let from_label = plugged.and_then(|c| {
            let m = rack.module(c.from.module)?;
            let out = m.ty.desc.outputs.get(rack.output_index(&c.from)?)?;
            Some(format!("from {} · {}", m.ty.desc.name, out.name))
        });
        let probe = ctx.probe;
        response.on_hover_ui(|ui| {
            ui.strong(format!("{name} ({kind_name})"));
            if let Some(label) = &from_label {
                ui.label(label);
            }
            if source.is_some() {
                let size = vec2(192.0, 192.0 / probe.aspect);
                ui.add(
                    egui::Image::from_texture(SizedTexture::new(probe.texture, size)).fit_to_exact_size(size),
                );
            }
        });
    }
}

struct ElementCtx<'a> {
    rack: &'a Rack,
    id: ModuleId,
    desc: &'static ModuleDesc,
    params: &'a ParamBank,
    painter: &'a Painter,
    probe: &'a ProbeView,
    /// Draw control labels (full panel art draws its own).
    labels: bool,
}

/// What the exact-value editor needs to know about a knob.
struct KnobSpec {
    label: String,
    min: f32,
    max: f32,
    unit: &'static str,
    default: f32,
}

impl KnobSpec {
    fn param(p: &dov_core::ParamDesc) -> Self {
        let (min, max, unit) = match p.kind {
            ParamKind::Knob { min, max, unit, .. } => (min, max, unit),
            ParamKind::Choice { options, .. } => (0.0, options.len().saturating_sub(1) as f32, ""),
        };
        Self {
            label: p.name.to_string(),
            min,
            max,
            unit,
            default: p.default_value(),
        }
    }
}

/// Knob interaction: vertical drag (shift for fine), double-click to reset,
/// right-click to type a value.
#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut Ui,
    ctx: &ElementCtx,
    id: Id,
    center: Pos2,
    radius: f32,
    slot: usize,
    to_norm: impl Fn(f32) -> f32,
    from_norm: impl Fn(f32) -> f32,
    spec: KnobSpec,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let response = ui.interact(
        Rect::from_center_size(center, Vec2::splat(2.0 * radius + 10.0)),
        id,
        Sense::click_and_drag(),
    );
    let params = ctx.params;
    if response.drag_started() {
        actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
    } else if response.dragged() {
        let speed = if ui.input(|i| i.modifiers.shift) {
            1500.0
        } else {
            150.0
        };
        let n = to_norm(params.get(slot)) - response.drag_delta().y / speed;
        params.set(slot, from_norm(n.clamp(0.0, 1.0)));
    }
    if response.double_clicked() {
        actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
        params.set(slot, spec.default);
    }
    // Right-click: type an exact value.
    response.context_menu(|ui| {
        ui.strong(&spec.label);
        let mut v = params.get(slot);
        let speed = (spec.max - spec.min) / 2000.0;
        let suffix = if spec.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", spec.unit)
        };
        let edit = ui.add(
            egui::DragValue::new(&mut v)
                .range(spec.min..=spec.max)
                .speed(speed)
                .min_decimals(2)
                .max_decimals(4)
                .suffix(suffix),
        );
        if edit.drag_started() || (edit.changed() && !edit.dragged()) {
            actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
        }
        if edit.changed() {
            params.set(slot, v);
        }
        if ui.button("Reset").clicked() {
            actions.push(Action::Undo(Box::new(ctx.rack.to_patch())));
            params.set(slot, spec.default);
            ui.close();
        }
    });
    response
}

fn jack_of(p: &Placed) -> Option<(Side, usize)> {
    match p.element {
        Element::Input { input } | Element::Cv { input, .. } => Some((Side::In, input)),
        Element::Output { output } => Some((Side::Out, output)),
        Element::Text | Element::Choice { .. } | Element::Knob { .. } => None,
    }
}

fn port_addr(rack: &Rack, end: PortEnd) -> Option<PortAddr> {
    let desc = &rack.module(end.module)?.ty.desc;
    let port = match end.side {
        Side::Out => desc.outputs.get(end.index)?.id.to_string(),
        Side::In => desc.inputs_full().get(end.index)?.id.to_string(),
    };
    Some(PortAddr {
        module: end.module,
        port,
    })
}

fn end_of(rack: &Rack, addr: &PortAddr, side: Side) -> Option<PortEnd> {
    let index = match side {
        Side::Out => rack.output_index(addr)?,
        Side::In => rack.input_index(addr)?,
    };
    Some(PortEnd {
        module: addr.module,
        side,
        index,
    })
}

fn port_kind(rack: &Rack, end: PortEnd) -> Option<PortKind> {
    let desc = &rack.module(end.module)?.ty.desc;
    Some(match end.side {
        Side::Out => desc.outputs.get(end.index)?.kind,
        Side::In => desc.inputs_full().get(end.index)?.kind,
    })
}

/// Can a cable run between these two jacks?
fn compatible(rack: &Rack, a: PortEnd, b: PortEnd) -> bool {
    let (out, inp) = match (a.side, b.side) {
        (Side::Out, Side::In) => (a, b),
        (Side::In, Side::Out) => (b, a),
        _ => return false,
    };
    matches!((port_kind(rack, out), port_kind(rack, inp)), (Some(o), Some(i)) if o.can_drive(i))
}

fn pointer_in_scene(ui: &Ui) -> Option<Pos2> {
    let p = ui.input(|i| i.pointer.hover_pos())?;
    Some(match ui.ctx().layer_transform_from_global(ui.layer_id()) {
        Some(t) => t * p,
        None => p,
    })
}

/// Category submenus listing every installed module; modules from files
/// show their namespace. Returns the one picked.
pub fn add_module_menu(ui: &mut Ui) -> Option<&'static ModuleType> {
    let mut chosen = None;
    let modules = dov_modules::all();
    for category in Category::ALL {
        ui.menu_button(category.name(), |ui| {
            for m in modules.iter().filter(|m| m.desc.category == category) {
                let (label, hover) = match &m.origin {
                    Origin::File { path, package } => {
                        let namespace = m.desc.id.split('.').next().unwrap_or_default();
                        let from = match package {
                            Some(p) => format!("{} {}", p.id, p.version),
                            None => path.display().to_string(),
                        };
                        (
                            format!("{} · {namespace}", m.desc.name),
                            format!("{}\n\nFrom {from}", m.desc.description),
                        )
                    }
                    Origin::BuiltIn | Origin::Missing => {
                        (m.desc.name.to_string(), m.desc.description.to_string())
                    }
                };
                if ui.button(label).on_hover_text(hover).clicked() {
                    chosen = Some(*m);
                    ui.close();
                }
            }
        });
    }
    chosen
}

/// Rasterized panel art. Rendering an SVG can take a while, so it happens on
/// background threads (a couple at a time) and the panel is drawn plain
/// until its art arrives; when the zoom changes, the old texture stays up
/// until the new one is ready. Textures are kept within a memory budget,
/// dropping the least recently drawn.
#[derive(Default)]
struct ArtCache {
    entries: HashMap<(PathBuf, u64), ArtEntry>,
    frame: u64,
}

#[derive(Default)]
struct ArtEntry {
    /// Width in pixels, and the texture.
    texture: Option<(u32, egui::TextureHandle)>,
    /// A rendering under way: its width, and where it arrives.
    pending: Option<(u32, mpsc::Receiver<egui::ColorImage>)>,
    bytes: usize,
    last_drawn: u64,
}

/// Renderings at once, and the texture memory panel art may use.
const ART_WORKERS: usize = 2;
const ART_BUDGET_BYTES: usize = 256 << 20;

impl ArtCache {
    /// The texture of `art` for a panel `size` (panel units) seen at
    /// `pixels` screen pixels per unit, or `None` until it's been rendered.
    /// Rendered in 1.5x steps of scale, so zooming doesn't re-render it
    /// every frame.
    fn texture(
        &mut self,
        ctx: &egui::Context,
        art: &Arc<LoadedArt>,
        size: Vec2,
        pixels: f32,
    ) -> Option<egui::TextureId> {
        let scale = 1.5f32.powf(pixels.max(0.25).log(1.5).ceil()).min(4.0);
        let (w, h) = (
            (size.x * scale).round().clamp(1.0, 4096.0) as u32,
            (size.y * scale).round().clamp(1.0, 4096.0) as u32,
        );
        let busy = self.entries.values().filter(|e| e.pending.is_some()).count();
        let entry = self
            .entries
            .entry((art.path.clone(), art.generation))
            .or_default();
        entry.last_drawn = self.frame;

        if let Some((pw, rx)) = &entry.pending {
            match rx.try_recv() {
                Ok(image) => {
                    let name = format!("panel art {}", art.path.display());
                    entry.bytes = image.pixels.len() * 4;
                    entry.texture = Some((*pw, ctx.load_texture(name, image, egui::TextureOptions::LINEAR)));
                    entry.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => entry.pending = None,
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(30)),
            }
        }
        let current =
            entry.texture.as_ref().map(|t| t.0) == Some(w) || entry.pending.as_ref().map(|p| p.0) == Some(w);
        if !current && busy < ART_WORKERS {
            let (tx, rx) = mpsc::channel();
            let art = art.clone();
            let started = std::thread::Builder::new()
                .name("panel art".into())
                .spawn(move || {
                    if let Some(image) = rasterize(&art, w, h) {
                        let _ = tx.send(image);
                    }
                });
            if started.is_ok() {
                entry.pending = Some((w, rx));
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        entry.texture.as_ref().map(|t| t.1.id())
    }

    /// Once per frame: keep the textures within budget, dropping the least
    /// recently drawn (art that's been replaced is never drawn again).
    fn end_frame(&mut self) {
        self.frame += 1;
        let mut total: usize = self.entries.values().map(|e| e.bytes).sum();
        if total <= ART_BUDGET_BYTES {
            return;
        }
        let mut by_age: Vec<((PathBuf, u64), u64)> = self
            .entries
            .iter()
            .map(|(k, e)| (k.clone(), e.last_drawn))
            .collect();
        by_age.sort_by_key(|(_, last)| *last);
        for (key, _) in by_age {
            if total <= ART_BUDGET_BYTES {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                total -= e.bytes;
            }
        }
    }
}

fn rasterize(art: &LoadedArt, w: u32, h: u32) -> Option<egui::ColorImage> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let s = art.art.tree.size();
    let fit = resvg::tiny_skia::Transform::from_scale(w as f32 / s.width(), h as f32 / s.height());
    resvg::render(&art.art.tree, fit, &mut pixmap.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        pixmap.data(),
    ))
}

#[cfg(test)]
mod tests {
    //! Drives the rack with simulated pointer input through a real egui
    //! context (no GPU), to check patching gestures end to end.

    use egui::{Event, Modifiers, PointerButton, RawInput};

    use super::*;

    struct Harness {
        ctx: egui::Context,
        view: RackView,
        rack: Rack,
        time: f64,
    }

    impl Harness {
        fn new(rack: Rack) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                view: RackView::default(),
                rack,
                time: 0.0,
            };
            // Let the scene fit itself and the transform settle.
            for _ in 0..3 {
                h.frame(vec![]);
            }
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let probe = ProbeView {
                texture: egui::TextureId::Managed(0),
                aspect: 4.0 / 3.0,
                screens: Default::default(),
            };
            let (view, rack) = (&mut self.view, &mut self.rack);
            let output = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| view.show(ui, rack, &probe));
            });
            output.drop_without_applying_deltas();
        }

        fn press(&mut self, at: Pos2, button: PointerButton, pressed: bool) {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }

        /// Press at `from`, move to `to` in steps, release.
        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.frame(vec![Event::PointerMoved(from)]);
            self.press(from, PointerButton::Primary, true);
            for i in 1..=8 {
                let p = from + (to - from) * (i as f32 / 8.0);
                self.frame(vec![Event::PointerMoved(p)]);
            }
            self.press(to, PointerButton::Primary, false);
            self.frame(vec![]);
        }

        fn jack(&self, module: ModuleId, side: Side, port: &str) -> Pos2 {
            let m = self.rack.module(module).unwrap();
            let desc = &m.ty.desc;
            let index = match side {
                Side::Out => desc.output_index(port),
                Side::In => desc.input_index(port),
            }
            .unwrap();
            let at = pos2(m.pos.1 as f32 * HP, m.pos.0 as f32 * ROW_H);
            let placed = panel::layout(desc)
                .elements
                .iter()
                .find(|p| jack_of(p) == Some((side, index)))
                .copied()
                .unwrap();
            self.view.to_global * (at + placed.jack_center().unwrap())
        }

        fn knob(&self, module: ModuleId, param: &str) -> Pos2 {
            let m = self.rack.module(module).unwrap();
            let index = m.ty.desc.param_index(param).unwrap();
            let at = pos2(m.pos.1 as f32 * HP, m.pos.0 as f32 * ROW_H);
            let placed = panel::layout(&m.ty.desc)
                .elements
                .iter()
                .find(|p| matches!(p.element, Element::Knob { param } if param == index))
                .copied()
                .unwrap();
            let cell = placed.cell.translate(at.to_vec2());
            self.view.to_global * pos2(cell.center().x, cell.top() + 22.0)
        }
    }

    fn two_modules() -> (Rack, ModuleId, ModuleId) {
        let mut rack = Rack::default();
        let vco = rack.add_module(dov_modules::find("dov.vco").unwrap(), 0, 0);
        let out = rack.add_module(dov_modules::find("dov.output").unwrap(), 0, 20);
        rack.clear_history();
        (rack, vco, out)
    }

    #[test]
    fn dragging_between_jacks_patches_a_cable() {
        let (rack, vco, out) = two_modules();
        let mut h = Harness::new(rack);
        let (a, b) = (h.jack(vco, Side::Out, "out"), h.jack(out, Side::In, "sig"));
        h.drag(a, b);
        assert_eq!(h.rack.cables.len(), 1, "cable created");
        let c = &h.rack.cables[0];
        assert_eq!((c.from.module, c.from.port.as_str()), (vco, "out"));
        assert_eq!((c.to.module, c.to.port.as_str()), (out, "sig"));
        assert!(h.rack.take_dirty(), "engine gets a new snapshot");
    }

    #[test]
    fn dragging_from_an_input_also_patches() {
        let (rack, vco, out) = two_modules();
        let mut h = Harness::new(rack);
        let (a, b) = (h.jack(out, Side::In, "sig"), h.jack(vco, Side::Out, "out"));
        h.drag(a, b);
        assert_eq!(h.rack.cables.len(), 1);
        assert_eq!(h.rack.cables[0].from.module, vco);
    }

    #[test]
    fn dragging_a_plugged_input_into_empty_space_unplugs_it() {
        let (rack, vco, out) = two_modules();
        let mut h = Harness::new(rack);
        let (a, b) = (h.jack(vco, Side::Out, "out"), h.jack(out, Side::In, "sig"));
        h.drag(a, b);
        assert_eq!(h.rack.cables.len(), 1);
        let away = b + vec2(0.0, 120.0);
        h.drag(b, away);
        assert!(h.rack.cables.is_empty(), "cable picked up and dropped");
        assert!(h.rack.undo(), "undo brings it back");
        assert_eq!(h.rack.cables.len(), 1);
    }

    #[test]
    fn incompatible_jacks_do_not_connect() {
        let mut rack = Rack::default();
        let encoder = rack.add_module(dov_modules::find("dov.encoder").unwrap(), 0, 0);
        let fold = rack.add_module(dov_modules::find("dov.fold").unwrap(), 0, 20);
        let mut h = Harness::new(rack);
        // A composite waveform only goes into the analog signal path.
        let (a, b) = (h.jack(encoder, Side::Out, "out"), h.jack(fold, Side::In, "sig"));
        h.drag(a, b);
        assert!(h.rack.cables.is_empty());
    }

    #[test]
    fn video_patches_into_control_jacks() {
        let mut rack = Rack::default();
        let vco = rack.add_module(dov_modules::find("dov.vco").unwrap(), 0, 0);
        let lfo = rack.add_module(dov_modules::find("dov.lfo").unwrap(), 0, 20);
        let mut h = Harness::new(rack);
        // The LFO's rate follows the picture's brightness.
        let (a, b) = (h.jack(vco, Side::Out, "out"), h.jack(lfo, Side::In, "rate_cv"));
        h.drag(a, b);
        assert_eq!(h.rack.cables.len(), 1);
    }

    #[test]
    fn dragging_a_knob_up_increases_it() {
        let (rack, vco, _) = two_modules();
        let mut h = Harness::new(rack);
        let slot = h
            .rack
            .module(vco)
            .unwrap()
            .ty
            .desc
            .param_index("multiple")
            .unwrap();
        let before = h.rack.module(vco).unwrap().params.get(slot);
        let k = h.knob(vco, "multiple");
        h.drag(k, k - vec2(0.0, 40.0));
        let after = h.rack.module(vco).unwrap().params.get(slot);
        assert!(after > before, "{before} -> {after}");
        assert!(h.rack.undo());
        assert_eq!(h.rack.module(vco).unwrap().params.get(slot), before);
    }

    #[test]
    fn dragging_a_panel_moves_the_module() {
        let (rack, vco, out) = two_modules();
        let mut h = Harness::new(rack);
        let before = h.rack.module(out).unwrap().pos;
        // Grab the Output panel by its title and drop it one row down.
        let m = h.rack.module(out).unwrap();
        let title = h.view.to_global * pos2(m.pos.1 as f32 * HP + 30.0, m.pos.0 as f32 * ROW_H + 15.0);
        let down = h.view.to_global.scaling * ROW_H;
        h.drag(title, title + vec2(0.0, down));
        let after = h.rack.module(out).unwrap().pos;
        assert_eq!(after, (before.0 + 1, before.1), "moved down a row");
        assert_eq!(h.rack.module(vco).unwrap().pos, (0, 0));
    }
}
