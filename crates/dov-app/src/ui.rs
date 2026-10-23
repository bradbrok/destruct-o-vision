//! The rack window: menu bar, the rack itself, a monitor panel with the
//! output and probe previews, engine telemetry, and the status line.

use std::path::{Path, PathBuf};

use dov_core::{ModuleId, RasterStd};
use dov_engine::{EngineStats, IntervalSummary, TempoSync};
use egui::load::SizedTexture;
use egui::{Color32, Key, KeyboardShortcut, Modifiers, Stroke, Ui};

use crate::art;
use crate::rack::Rack;
use crate::rack_view::{self, ProbeView, RackView};

pub const PATCH_DIR: &str = "patches";
pub const PATCH_EXT: &str = "dov";

/// Where patches and captures go: the working directory when run from a
/// terminal, or ~/Documents/Destruct-O-Vision for the app bundle, which macOS
/// starts in `/`. `DOV_HOME` overrides both.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DOV_HOME") {
        return dir.into();
    }
    let bundled =
        std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS"));
    match std::env::var_os("HOME") {
        Some(home) if bundled => Path::new(&home).join("Documents").join("Destruct-O-Vision"),
        _ => PathBuf::new(),
    }
}

pub fn patch_dir() -> PathBuf {
    data_dir().join(PATCH_DIR)
}

pub enum UiAction {
    SetRaster(RasterStd),
    ShowOutput,
    Fullscreen(usize),
    Windowed,
    Snapshot,
    /// Record the output window to a movie, or stop.
    StartMovie(dov_io::Codec),
    StopMovie,
    /// The global tempo, in beats a minute.
    SetTempo(f64),
    SetSync(TempoSync),
    NewPatch,
    LoadShowcase,
    LoadDemo,
    LoadCameraDemo,
    LoadAudioDemo,
    LoadAnalogDemo,
    LoadTimeDemo,
    LoadLabDemo,
    OpenModulesFolder,
    ReloadModules,
    NewModule,
    Open(PathBuf),
    Save,
    SaveAs(String),
    /// Ask for a movie file for this Movie module.
    ChooseMovie(ModuleId),
}

pub struct FrameData<'a> {
    pub stats: EngineStats,
    pub output_preview: egui::TextureId,
    pub probe: ProbeView,
    pub probing: bool,
    pub monitors: &'a [String],
    pub output_shown: bool,
    pub ui_frames: IntervalSummary,
    pub status: Option<&'a str>,
    pub modules: ModulesView<'a>,
    /// How long the movie being recorded has run.
    pub movie: Option<std::time::Duration>,
}

/// The user's modules folder, for the Modules menu.
pub struct ModulesView<'a> {
    pub dir: &'a Path,
    pub count: usize,
    /// Files and packages that failed to load, with what's wrong.
    pub errors: &'a [(PathBuf, String)],
}

#[derive(Default)]
pub struct AppUi {
    /// Simulated UI work per frame, in milliseconds.
    pub load_ms: f32,
    pub rack_view: RackView,
    /// Name being typed in the Save As dialog.
    pub save_as: Option<String>,
    /// The tempo being dragged or typed, until it's let go.
    pub tempo_edit: Option<f64>,
    /// Recent taps of Tap, for tap tempo.
    pub taps: Vec<std::time::Instant>,
}

/// Taps further apart than this start a new tempo.
const TAP_GAP: std::time::Duration = std::time::Duration::from_secs(2);
/// Taps the tempo is averaged over.
const TAPS: usize = 5;

impl AppUi {
    /// Draws the window. Returns the module output to probe.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rack: &mut Rack,
        f: &FrameData,
        actions: &mut Vec<UiAction>,
    ) -> Option<(ModuleId, usize)> {
        self.shortcuts(ui, rack, f.movie, actions);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, rack, f, actions));
        egui::Panel::bottom("status").show(ui, |ui| status_line(ui, rack, f));
        egui::Panel::right("monitor").default_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.monitor_panel(ui, f));
        });
        let mut probe = None;
        egui::CentralPanel::no_frame().show(ui, |ui| probe = self.rack_view.show(ui, rack, &f.probe));
        if let Some(id) = self.rack_view.choose_movie.take() {
            actions.push(UiAction::ChooseMovie(id));
        }
        self.save_as_window(ui, actions);
        probe
    }

    fn shortcuts(
        &mut self,
        ui: &mut Ui,
        rack: &mut Rack,
        movie: Option<std::time::Duration>,
        actions: &mut Vec<UiAction>,
    ) {
        let shortcut =
            |ui: &mut Ui, mods, key| ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key)));
        if shortcut(ui, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
            rack.redo();
        } else if shortcut(ui, Modifiers::COMMAND, Key::Z) {
            rack.undo();
        }
        if shortcut(ui, Modifiers::COMMAND, Key::S) {
            actions.push(UiAction::Save);
        }
        if shortcut(ui, Modifiers::COMMAND, Key::R) {
            actions.push(match movie {
                Some(_) => UiAction::StopMovie,
                None => UiAction::StartMovie(dov_io::Codec::H264),
            });
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing
            && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
            && let Some(id) = self.rack_view.hovered_module
        {
            rack.remove_module(id);
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui, rack: &mut Rack, f: &FrameData, actions: &mut Vec<UiAction>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    actions.push(UiAction::NewPatch);
                    ui.close();
                }
                if ui.button("Showcase").clicked() {
                    actions.push(UiAction::LoadShowcase);
                    ui.close();
                }
                if ui.button("Feedback demo").clicked() {
                    actions.push(UiAction::LoadDemo);
                    ui.close();
                }
                if ui.button("Analog demo").clicked() {
                    actions.push(UiAction::LoadAnalogDemo);
                    ui.close();
                }
                if ui.button("Delay & key demo").clicked() {
                    actions.push(UiAction::LoadTimeDemo);
                    ui.close();
                }
                if ui.button("3D & text demo").clicked() {
                    actions.push(UiAction::LoadLabDemo);
                    ui.close();
                }
                if ui.button("Camera demo").clicked() {
                    actions.push(UiAction::LoadCameraDemo);
                    ui.close();
                }
                if ui.button("Audio demo").clicked() {
                    actions.push(UiAction::LoadAudioDemo);
                    ui.close();
                }
                ui.menu_button("Open", |ui| {
                    let patches = list_patches();
                    if patches.is_empty() {
                        ui.label(format!("No patches in {} yet", patch_dir().display()));
                    }
                    for path in patches {
                        let name = path
                            .file_stem()
                            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
                        if ui.button(name).clicked() {
                            actions.push(UiAction::Open(path));
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button("Save        Cmd+S").clicked() {
                    actions.push(UiAction::Save);
                    ui.close();
                }
                if ui.button("Save as…").clicked() {
                    self.save_as = Some(patch_name(rack));
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(rack.can_undo(), egui::Button::new("Undo        Cmd+Z"))
                    .clicked()
                {
                    rack.undo();
                    ui.close();
                }
                if ui
                    .add_enabled(rack.can_redo(), egui::Button::new("Redo  Shift+Cmd+Z"))
                    .clicked()
                {
                    rack.redo();
                    ui.close();
                }
                ui.separator();
                if ui.button("Fit rack to window").clicked() {
                    self.rack_view.fit();
                    ui.close();
                }
            });
            ui.menu_button("Add", |ui| {
                if let Some(ty) = rack_view::add_module_menu(ui) {
                    let row = rack.modules.iter().map(|m| m.pos.0).max().unwrap_or(0);
                    let col = rack.row_end(row);
                    rack.add_module(ty, row, col);
                }
            });
            let m = &f.modules;
            let title = match m.errors.len() {
                0 => "Modules".to_string(),
                n => format!("Modules ⚠ {n}"),
            };
            ui.menu_button(title, |ui| {
                ui.label(format!("{} from {}", plural(m.count, "module"), m.dir.display()));
                let mut art_on = art::enabled();
                let art_label = format!("Panel art ({})", plural(art::count(), "panel"));
                if ui
                    .checkbox(&mut art_on, art_label)
                    .on_hover_text(
                        "SVG panels from the modules folder: panels/<module-id>.svg restyles any module",
                    )
                    .changed()
                {
                    art::set_enabled(art_on);
                    rack.reflow();
                }
                if ui
                    .button("New module…")
                    .on_hover_text("Write a starter module file into the folder")
                    .clicked()
                {
                    actions.push(UiAction::NewModule);
                    ui.close();
                }
                if ui.button("Open modules folder").clicked() {
                    actions.push(UiAction::OpenModulesFolder);
                    ui.close();
                }
                if ui
                    .button("Reload")
                    .on_hover_text("Files reload by themselves when they change")
                    .clicked()
                {
                    actions.push(UiAction::ReloadModules);
                    ui.close();
                }
                if !m.errors.is_empty() {
                    ui.separator();
                    for (path, error) in m.errors {
                        let name = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        let first = error.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 90), format!("{name}: {first}"))
                            .on_hover_text(egui::RichText::new(error).monospace());
                    }
                }
            });
            ui.menu_button("Raster", |ui| {
                for std in RasterStd::ALL {
                    if ui.selectable_label(f.stats.raster == std, std.name).clicked() {
                        actions.push(UiAction::SetRaster(std));
                        ui.close();
                    }
                }
            });
            ui.menu_button("Output", |ui| {
                if ui
                    .add_enabled(!f.output_shown, egui::Button::new("Show output window"))
                    .clicked()
                {
                    actions.push(UiAction::ShowOutput);
                    ui.close();
                }
                if f.monitors.is_empty() {
                    ui.label("No active displays (asleep or locked?)");
                }
                for (i, name) in f.monitors.iter().enumerate() {
                    if ui.button(format!("Fullscreen on {name}")).clicked() {
                        actions.push(UiAction::Fullscreen(i));
                        ui.close();
                    }
                }
                if ui.button("Windowed").clicked() {
                    actions.push(UiAction::Windowed);
                    ui.close();
                }
                ui.separator();
                if ui.button("Snapshot").clicked() {
                    actions.push(UiAction::Snapshot);
                    ui.close();
                }
                match f.movie {
                    Some(running) => {
                        let stop = egui::Button::new(format!("Stop recording ({})", clock(running)))
                            .shortcut_text(
                                ui.ctx()
                                    .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::R)),
                            );
                        if ui.add(stop).clicked() {
                            actions.push(UiAction::StopMovie);
                            ui.close();
                        }
                    }
                    None => {
                        let h264 = egui::Button::new("Record movie (H.264)").shortcut_text(
                            ui.ctx()
                                .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::R)),
                        );
                        if ui
                            .add(h264)
                            .on_hover_text(
                                "A small MP4 that plays anywhere, of the output window as it shows",
                            )
                            .clicked()
                        {
                            actions.push(UiAction::StartMovie(dov_io::Codec::H264));
                            ui.close();
                        }
                        if ui
                            .button("Record movie (ProRes)")
                            .on_hover_text("A big QuickTime movie that keeps every scan line, for editing")
                            .clicked()
                        {
                            actions.push(UiAction::StartMovie(dov_io::Codec::ProRes));
                            ui.close();
                        }
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.tempo_bar(ui, rack, f, actions);
            });
        });
    }

    /// The global tempo: where it comes from, the beat, the tempo itself and
    /// Tap, right to left.
    fn tempo_bar(&mut self, ui: &mut Ui, rack: &Rack, f: &FrameData, actions: &mut Vec<UiAction>) {
        let tempo = f.stats.tempo;
        match tempo.sync {
            TempoSync::Internal => {}
            TempoSync::Link => {
                ui.weak(match tempo.peers {
                    0 => "no peers".to_string(),
                    1 => "1 peer".to_string(),
                    n => format!("{n} peers"),
                });
            }
            TempoSync::MidiClock => {
                ui.weak(if tempo.receiving {
                    "receiving"
                } else {
                    "waiting for clock"
                });
            }
        }
        let name = |s: TempoSync| match s {
            TempoSync::Internal => "Internal",
            TempoSync::Link => "Ableton Link",
            TempoSync::MidiClock => "MIDI clock",
        };
        egui::ComboBox::from_id_salt("tempo sync")
            .width(110.0)
            .selected_text(name(tempo.sync))
            .show_ui(ui, |ui| {
                for sync in [TempoSync::Internal, TempoSync::Link, TempoSync::MidiClock] {
                    if ui.selectable_label(tempo.sync == sync, name(sync)).clicked() && tempo.sync != sync {
                        actions.push(UiAction::SetSync(sync));
                    }
                }
            })
            .response
            .on_hover_text(
                "Where the tempo comes from: this patch, an Ableton Link session (Live, or any Link app \
                 on this Mac or the network), or MIDI clock from any device",
            );
        beat_lights(ui, tempo.beats);

        // MIDI clock sets the tempo; otherwise it's ours (and Link's).
        let editable = tempo.sync != TempoSync::MidiClock;
        let shown = match tempo.sync {
            TempoSync::Internal => rack.tempo(),
            _ => tempo.bpm,
        };
        let mut bpm = self.tempo_edit.unwrap_or(shown);
        let drag = ui.add_enabled(
            editable,
            egui::DragValue::new(&mut bpm)
                .range(20.0..=300.0)
                .speed(0.1)
                .fixed_decimals(1)
                .suffix(" BPM"),
        );
        if drag.changed() {
            actions.push(UiAction::SetTempo(bpm));
        }
        self.tempo_edit = (drag.dragged() || drag.has_focus()).then_some(bpm);

        let tap = ui
            .add_enabled(editable, egui::Button::new("Tap"))
            .on_hover_text("Tap the beat to set the tempo");
        if tap.clicked() {
            let now = std::time::Instant::now();
            if self.taps.last().is_some_and(|&t| now - t > TAP_GAP) {
                self.taps.clear();
            }
            self.taps.push(now);
            if self.taps.len() > TAPS {
                self.taps.remove(0);
            }
            if let (Some(&first), Some(&last)) = (self.taps.first(), self.taps.last())
                && self.taps.len() > 1
            {
                let beat = (last - first).as_secs_f64() / (self.taps.len() - 1) as f64;
                actions.push(UiAction::SetTempo(((60.0 / beat) * 10.0).round() / 10.0));
            }
        }
    }

    fn monitor_panel(&mut self, ui: &mut Ui, f: &FrameData) {
        let width = ui.available_width();
        let aspect = f.probe.aspect;
        ui.strong("Output");
        let size = egui::vec2(width, width / aspect);
        ui.add(egui::Image::from_texture(SizedTexture::new(f.output_preview, size)).fit_to_exact_size(size));
        ui.add_space(6.0);
        ui.strong("Probe");
        if f.probing {
            ui.add(
                egui::Image::from_texture(SizedTexture::new(f.probe.texture, size)).fit_to_exact_size(size),
            );
        } else {
            ui.small("Hover a jack to see its signal here.");
        }

        if !f.stats.warnings.is_empty() {
            ui.add_space(6.0);
            ui.colored_label(Color32::from_rgb(255, 140, 90), "Patch warnings");
            for w in &f.stats.warnings {
                ui.small(w);
            }
        }

        ui.add_space(6.0);
        egui::CollapsingHeader::new("Engine")
            .default_open(false)
            .show(ui, |ui| {
                let s = &f.stats;
                ui.label(format!(
                    "{} · f_H {:.4} kHz · f_V {:.4} Hz",
                    s.raster.name,
                    s.raster.line_rate_hz() / 1000.0,
                    s.raster.field_rate_hz()
                ));
                egui::Grid::new("pacing")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        row(ui, "Modules / passes", format!("{} / {}", s.modules, s.passes));
                        row(ui, "Field", s.field.to_string());
                        row(
                            ui,
                            "Rendered / dropped",
                            format!("{} / {}", s.fields_rendered, s.fields_dropped),
                        );
                        row(ui, "Repeated", s.fields_repeated.to_string());
                        row(
                            ui,
                            "Presenting",
                            if s.presenting {
                                "yes (vsync)"
                            } else {
                                "no (field clock)"
                            }
                            .into(),
                        );
                        row(ui, "Field interval", interval(&s.field_interval));
                        row(ui, "Present interval", interval(&s.present_interval));
                        row(ui, "Engine encode", format!("{:.3} ms", s.encode_ms));
                        row(ui, "Acquire wait", interval(&s.acquire_wait));
                        row(ui, "Display (flywheel)", format!("{:.3} Hz", s.display_hz));
                        row(
                            ui,
                            "Occluded / timeouts",
                            format!("{} / {}", s.acquire_occluded, s.acquire_timeout),
                        );
                        row(ui, "UI frame interval", interval(&f.ui_frames));
                    });
                ui.small("Present intervals (dashed: one field period)");
                sparkline(ui, &s.recent_present_ms, 1000.0 / s.raster.field_rate_hz() as f32);
                ui.add(egui::Slider::new(&mut self.load_ms, 0.0..=100.0).text("UI stress ms"));
                ui.small("Slows the UI thread. The output window should stay smooth.");
            });
    }

    fn save_as_window(&mut self, ui: &mut Ui, actions: &mut Vec<UiAction>) {
        let Some(name) = &mut self.save_as else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Save patch as")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label(format!("Name (saved in {}):", patch_dir().display()));
                let field = ui.text_edit_singleline(name);
                if !field.has_focus() && !field.lost_focus() {
                    field.request_focus();
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Save").clicked() || enter {
                    save = true;
                }
            });
        if save && !name.trim().is_empty() {
            actions.push(UiAction::SaveAs(name.trim().to_string()));
            self.save_as = None;
        } else if !open {
            self.save_as = None;
        }
    }
}

pub fn patch_name(rack: &Rack) -> String {
    rack.path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map_or_else(|| "untitled".into(), |s| s.to_string_lossy().into_owned())
}

fn list_patches() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(patch_dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == PATCH_EXT))
        .collect();
    paths.sort();
    paths
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn status_line(ui: &mut Ui, rack: &Rack, f: &FrameData) {
    ui.horizontal(|ui| {
        let dirty = if rack.modified { " •" } else { "" };
        ui.strong(format!("{}{dirty}", patch_name(rack)));
        ui.label(format!(
            "{} modules · {} cables",
            rack.modules.len(),
            rack.cables.len()
        ));
        ui.separator();
        if let Some(running) = f.movie {
            // A drawn dot: the UI font has no circle.
            let red = egui::Color32::from_rgb(235, 70, 60);
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.5, red);
            ui.colored_label(red, format!("REC {}", clock(running)));
            ui.separator();
        }
        match f.status {
            Some(status) => ui.label(status),
            None => ui.weak(
                "Right-click the rack to add modules · drag between jacks to patch · drag a plugged input to \
                 move its cable · Cmd+scroll or pinch to zoom · Delete removes the hovered module",
            ),
        };
    });
}

/// Four lights for the beats of the bar: the one on is lit, the first of
/// the bar in a color of its own.
fn beat_lights(ui: &mut Ui, beats: f64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 12.0), egui::Sense::hover());
    let on = beats.rem_euclid(4.0).floor() as usize;
    // Bright at the start of the beat, fading through it.
    let fade = 1.0 - beats.rem_euclid(1.0) as f32 * 0.6;
    // Laid out left to right, though the bar around them runs right to left.
    for k in 0..4 {
        let center = egui::pos2(rect.left() + 6.0 + k as f32 * 13.0, rect.center().y);
        let lit = if k == 0 {
            Color32::from_rgb(255, 170, 60)
        } else {
            Color32::from_rgb(120, 220, 140)
        };
        let color = if k == on {
            lit.gamma_multiply(fade)
        } else {
            Color32::from_gray(60)
        };
        ui.painter().circle_filled(center, 4.0, color);
    }
}

/// Minutes and seconds.
fn clock(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn row(ui: &mut Ui, label: &str, value: String) {
    ui.label(label);
    ui.monospace(value);
    ui.end_row();
}

fn interval(s: &IntervalSummary) -> String {
    format!(
        "{:.2} ±{:.2} [{:.1}–{:.1}]",
        s.mean_ms, s.jitter_ms, s.min_ms, s.max_ms
    )
}

fn sparkline(ui: &mut Ui, samples_ms: &[f32], expected_ms: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let top = samples_ms.iter().copied().fold(expected_ms * 2.0, f32::max);
    let y = |ms: f32| rect.bottom() - (ms / top) * rect.height();
    painter.add(egui::Shape::dashed_line(
        &[
            egui::pos2(rect.left(), y(expected_ms)),
            egui::pos2(rect.right(), y(expected_ms)),
        ],
        Stroke::new(1.0, Color32::GRAY),
        4.0,
        4.0,
    ));
    if samples_ms.len() < 2 {
        return;
    }
    let step = rect.width() / (samples_ms.len() - 1) as f32;
    let points = samples_ms
        .iter()
        .enumerate()
        .map(|(i, &ms)| egui::pos2(rect.left() + i as f32 * step, y(ms)))
        .collect();
    painter.add(egui::Shape::line(
        points,
        Stroke::new(1.5, Color32::from_rgb(255, 120, 60)),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_shortcut_and_clock_read_right() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let shortcut = ctx.format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::R));
        let font = egui::FontId::proportional(14.0);
        for c in shortcut.chars() {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&font, c)),
                "{c} in {shortcut} has no glyph"
            );
        }
        assert_eq!(clock(std::time::Duration::from_secs(75)), "1:15");
    }
}
