//! The rack as the UI edits it: placed modules, their live params, cables,
//! and undo history. Converts to patch files and to engine snapshots.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use dov_core::{Cable, ModuleId, ModuleState, ParamBank, Patch, PortAddr, PortKind};
use dov_engine::{PatchSnapshot, SnapshotCable, SnapshotModule};
use dov_modules::{ModuleType, Origin};

use crate::panel;

const UNDO_DEPTH: usize = 100;

pub struct RackModule {
    pub id: ModuleId,
    pub ty: &'static ModuleType,
    /// (row, column in HP).
    pub pos: (i32, i32),
    pub params: Arc<ParamBank>,
}

impl RackModule {
    pub fn hp(&self) -> i32 {
        panel::layout(&self.ty.desc).hp
    }
}

#[derive(Default)]
pub struct Rack {
    pub modules: Vec<RackModule>,
    pub cables: Vec<Cable>,
    next_id: u64,
    next_color: u8,
    undo: Vec<Patch>,
    redo: Vec<Patch>,
    /// Structure changed since the engine last got a snapshot.
    dirty: bool,
    /// Where this patch was loaded from or last saved to.
    pub path: Option<PathBuf>,
    /// Unsaved changes (structure or params).
    pub modified: bool,
    /// Packages the loaded patch said it needs, kept for modules that
    /// aren't installed so saving doesn't forget them.
    requires: BTreeMap<String, String>,
    /// The global tempo, if the patch sets one.
    tempo: Option<f64>,
}

/// The tempo of a patch that doesn't set one.
pub const DEFAULT_TEMPO: f64 = 120.0;

/// Modules that had a BPM knob of their own before the global tempo, with
/// the switch that put them on the beat and its setting for that.
const OWN_TEMPO: [(&str, &str, f32); 3] = [
    ("dov.vco", "range", 3.0),
    ("dov.lfo", "mode", 1.0),
    ("dov.movie", "sync", 1.0),
];

/// A patch's tempo: its own, or for a patch from before the global tempo,
/// the first tempo a module was synced to.
fn tempo_of(patch: &Patch) -> Option<f64> {
    patch.tempo.or_else(|| {
        patch.modules.iter().find_map(|m| {
            let (_, switch, on) = OWN_TEMPO.iter().find(|(kind, ..)| *kind == m.kind)?;
            let synced = m.params.get(*switch).is_some_and(|v| (v - on).abs() < 0.5);
            synced.then(|| m.params.get("bpm").map(|&b| f64::from(b)))?
        })
    })
}

/// Output's old display settings. Patches from before the CRT module chose
/// a picture tube with Output's Display switch; they open with a CRT in its
/// place, keeping the settings it shares.
const OLD_DISPLAY: [&str; 6] = ["display", "mask_type", "mask", "scanlines", "glow", "curvature"];

fn migrate(m: &ModuleState) -> Cow<'_, ModuleState> {
    // The global tempo replaced modules' own BPM knobs (see `tempo_of`).
    if OWN_TEMPO.iter().any(|(kind, ..)| *kind == m.kind) && m.params.contains_key("bpm") {
        let mut m = m.clone();
        m.params.retain(|k, _| k != "bpm" && k != "bpm_cv");
        return Cow::Owned(m);
    }
    if m.kind != "dov.output" || !m.params.keys().any(|k| OLD_DISPLAY.contains(&k.as_str())) {
        return Cow::Borrowed(m);
    }
    let mut m = m.clone();
    if m.params.get("display").is_some_and(|&d| d >= 0.5) {
        m.kind = dov_modules::crt::ID.into();
        m.params
            .retain(|k, _| matches!(k.as_str(), "mask_type" | "mask" | "glow" | "curvature"));
    } else {
        m.params.retain(|k, _| !OLD_DISPLAY.contains(&k.as_str()));
    }
    Cow::Owned(m)
}

impl Rack {
    pub fn from_patch(patch: &Patch) -> (Self, Vec<String>) {
        let mut rack = Self::default();
        let warnings = rack.load(patch);
        rack.tempo = tempo_of(patch);
        (rack, warnings)
    }

    /// The global tempo, in beats a minute.
    pub fn tempo(&self) -> f64 {
        self.tempo.unwrap_or(DEFAULT_TEMPO)
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        if self.tempo != Some(bpm) {
            self.tempo = Some(bpm);
            self.modified = true;
        }
    }

    /// Replaces the contents, keeping undo history. Returns problems found.
    fn load(&mut self, patch: &Patch) -> Vec<String> {
        let mut warnings = Vec::new();
        self.modules.clear();
        self.requires = patch.requires.clone();
        for m in &patch.modules {
            let m = &*migrate(m);
            let ty = match dov_modules::find(&m.kind) {
                Some(ty) => ty,
                None => {
                    let namespace = m.kind.split('.').next().unwrap_or_default();
                    let from = patch
                        .requires
                        .iter()
                        .find(|(package, _)| package.split('.').next() == Some(namespace))
                        .map(|(package, version)| format!(" (from package {package} {version})"))
                        .unwrap_or_default();
                    warnings.push(format!(
                        "{} isn't installed{from}; it's kept as a stand-in",
                        m.kind
                    ));
                    stand_in(patch, m)
                }
            };
            let desc = &ty.desc;
            let mut slots = desc.default_slots();
            for (key, &value) in &m.params {
                match slot_index(ty, key) {
                    Some(slot) => slots[slot] = value,
                    None => warnings.push(format!("{}: unknown param {key}", desc.id)),
                }
            }
            let params = ParamBank::new(&slots);
            if let Some(spec) = desc.text {
                params.set_text(m.text.as_deref().unwrap_or(spec.default));
            }
            params.set_marks(m.marks.clone());
            self.modules.push(RackModule {
                id: m.id,
                ty,
                pos: m.pos,
                params: Arc::new(params),
            });
        }
        self.cables = patch
            .cables
            .iter()
            .filter(|c| {
                let ok = self.output_index(&c.from).is_some() && self.input_index(&c.to).is_some();
                // An LFO's own BPM CV went with its BPM knob.
                if !ok && c.to.port == "bpm_cv" {
                    return false;
                }
                if !ok {
                    warnings.push(format!(
                        "dropped cable {}.{} -> {}.{}",
                        c.from.module.0, c.from.port, c.to.module.0, c.to.port
                    ));
                }
                ok
            })
            .cloned()
            .collect();
        self.next_id = self.modules.iter().map(|m| m.id.0 + 1).max().unwrap_or(1);
        self.next_color = self
            .cables
            .iter()
            .map(|c| c.color.wrapping_add(1))
            .max()
            .unwrap_or(0);
        self.dirty = true;
        self.reflow();
        warnings
    }

    pub fn to_patch(&self) -> Patch {
        let modules = self
            .modules
            .iter()
            .map(|m| {
                let desc = &m.ty.desc;
                let values = m.params.snapshot();
                let mut params = BTreeMap::new();
                for (i, p) in desc.params.iter().enumerate() {
                    params.insert(p.id.to_string(), values[i]);
                    if let Some((_, atten)) = desc.cv_slot(i) {
                        params.insert(format!("{}_cv", p.id), values[atten]);
                    }
                }
                ModuleState {
                    id: m.id,
                    kind: desc.id.to_string(),
                    pos: m.pos,
                    params,
                    text: desc.text.map(|_| m.params.text()),
                    marks: m.params.marks(),
                }
            })
            .collect();
        Patch {
            version: Patch::VERSION,
            raster: None,
            tempo: self.tempo,
            requires: self.requires(),
            modules,
            cables: self.cables.clone(),
        }
    }

    /// The packages the rack's modules come from. Packages of modules that
    /// aren't installed are carried over from the loaded patch.
    fn requires(&self) -> BTreeMap<String, String> {
        let mut requires = BTreeMap::new();
        for m in &self.modules {
            match &m.ty.origin {
                Origin::File { package: Some(p), .. } => {
                    requires.insert(p.id.clone(), p.version.clone());
                }
                Origin::Missing => {
                    let namespace = m.ty.desc.id.split('.').next();
                    for (package, version) in &self.requires {
                        if package.split('.').next() == namespace {
                            requires.insert(package.clone(), version.clone());
                        }
                    }
                }
                Origin::File { package: None, .. } | Origin::BuiltIn => {}
            }
        }
        requires
    }

    /// Slides modules right wherever they overlap, keeping their order in
    /// each row: panel art can change a module's width.
    pub fn reflow(&mut self) {
        let mut order: Vec<usize> = (0..self.modules.len()).collect();
        order.sort_by_key(|&i| self.modules[i].pos);
        let mut row_end: Option<(i32, i32)> = None;
        for i in order {
            let (row, col) = self.modules[i].pos;
            let col = match row_end {
                Some((r, end)) if r == row => col.max(end),
                _ => col,
            };
            self.modules[i].pos.1 = col;
            row_end = Some((row, col + self.modules[i].hp()));
        }
    }

    /// Looks every module's type up again, after module files changed:
    /// edited modules pick up their new definition (losing only their
    /// running state), removed ones become stand-ins, and newly installed
    /// ones replace their stand-ins. Undo history and cables are kept.
    pub fn reload_types(&mut self) -> Vec<String> {
        let patch = self.to_patch();
        self.load(&patch)
    }

    /// The engine's view: modules with shared param banks, cables by index.
    pub fn snapshot(&self) -> PatchSnapshot {
        PatchSnapshot {
            modules: self
                .modules
                .iter()
                .map(|m| SnapshotModule {
                    id: m.id,
                    ty: m.ty,
                    params: m.params.clone(),
                })
                .collect(),
            cables: self
                .cables
                .iter()
                .filter_map(|c| {
                    Some(SnapshotCable {
                        from: c.from.module,
                        output: self.output_index(&c.from)?,
                        to: c.to.module,
                        input: self.input_index(&c.to)?,
                    })
                })
                .collect(),
        }
    }

    /// True once after any structural change.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn module(&self, id: ModuleId) -> Option<&RackModule> {
        self.modules.iter().find(|m| m.id == id)
    }

    pub fn output_index(&self, addr: &PortAddr) -> Option<usize> {
        self.module(addr.module)?.ty.desc.output_index(&addr.port)
    }

    pub fn input_index(&self, addr: &PortAddr) -> Option<usize> {
        self.module(addr.module)?.ty.desc.input_index(&addr.port)
    }

    /// Records the current state for undo. Call before each edit.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.to_patch());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.modified = true;
    }

    /// Records a state captured before an in-place edit (knob turns).
    pub fn push_undo(&mut self, before: Patch) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.modified = true;
    }

    /// Forget history, e.g. after building a patch programmatically.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.modified = false;
    }

    /// The column just past the last module in `row`.
    pub fn row_end(&self, row: i32) -> i32 {
        self.modules
            .iter()
            .filter(|m| m.pos.0 == row)
            .map(|m| m.pos.1 + m.hp())
            .max()
            .unwrap_or(0)
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(self.to_patch());
        self.load(&prev);
        self.modified = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(self.to_patch());
        self.load(&next);
        self.modified = true;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn add_module(&mut self, ty: &'static ModuleType, row: i32, col: i32) -> ModuleId {
        self.checkpoint();
        let id = ModuleId(self.next_id.max(1));
        self.next_id = id.0 + 1;
        let hp = panel::layout(&ty.desc).hp;
        let col = self.free_column(row, col, hp, None);
        let params = ParamBank::new(&ty.desc.default_slots());
        if let Some(spec) = ty.desc.text {
            params.set_text(spec.default);
        }
        self.modules.push(RackModule {
            id,
            ty,
            pos: (row, col),
            params: Arc::new(params),
        });
        self.dirty = true;
        id
    }

    pub fn duplicate(&mut self, id: ModuleId) -> Option<ModuleId> {
        let (ty, (row, col), values, text, marks) = {
            let m = self.module(id)?;
            (
                m.ty,
                m.pos,
                m.params.snapshot(),
                m.params.text(),
                m.params.marks(),
            )
        };
        let new = self.add_module(ty, row, col + panel::layout(&ty.desc).hp);
        if let Some(m) = self.modules.iter().find(|m| m.id == new) {
            for (slot, v) in values.into_iter().enumerate() {
                m.params.set(slot, v);
            }
            m.params.set_text(&text);
            m.params.set_marks(marks);
        }
        Some(new)
    }

    pub fn remove_module(&mut self, id: ModuleId) {
        self.checkpoint();
        self.modules.retain(|m| m.id != id);
        self.cables.retain(|c| c.from.module != id && c.to.module != id);
        self.dirty = true;
    }

    pub fn reset_params(&mut self, id: ModuleId) {
        self.checkpoint();
        if let Some(m) = self.module(id) {
            for (slot, v) in m.ty.desc.default_slots().into_iter().enumerate() {
                m.params.set(slot, v);
            }
        }
    }

    /// Moves a module, sliding it sideways to the nearest free space.
    pub fn move_module(&mut self, id: ModuleId, row: i32, col: i32) {
        let Some(hp) = self.module(id).map(RackModule::hp) else {
            return;
        };
        let col = self.free_column(row.max(0), col, hp, Some(id));
        if let Some(m) = self.modules.iter_mut().find(|m| m.id == id)
            && m.pos != (row.max(0), col)
        {
            m.pos = (row.max(0), col);
            self.modified = true;
        }
    }

    /// Connects an output to an input, replacing whatever fed that input.
    pub fn connect(&mut self, from: PortAddr, to: PortAddr, color: Option<u8>) {
        self.checkpoint();
        self.cables.retain(|c| c.to != to);
        let color = color.unwrap_or_else(|| {
            let c = self.next_color;
            self.next_color = self.next_color.wrapping_add(1);
            c
        });
        self.cables.push(Cable { from, to, color });
        self.dirty = true;
    }

    /// Removes the cable feeding `to`, returning it.
    pub fn take_cable(&mut self, to: &PortAddr) -> Option<Cable> {
        let i = self.cables.iter().position(|c| &c.to == to)?;
        self.checkpoint();
        self.dirty = true;
        Some(self.cables.remove(i))
    }

    pub fn disconnect_all(&mut self, port: &PortAddr) {
        if self.cables.iter().any(|c| &c.from == port || &c.to == port) {
            self.checkpoint();
            self.cables.retain(|c| &c.from != port && &c.to != port);
            self.dirty = true;
        }
    }

    pub fn cable_into(&self, to: &PortAddr) -> Option<&Cable> {
        self.cables.iter().find(|c| &c.to == to)
    }

    /// Nearest column to `col` where an `hp`-wide module fits in `row`.
    pub fn free_column(&self, row: i32, col: i32, hp: i32, ignore: Option<ModuleId>) -> i32 {
        let taken: Vec<(i32, i32)> = self
            .modules
            .iter()
            .filter(|m| m.pos.0 == row && Some(m.id) != ignore)
            .map(|m| (m.pos.1, m.pos.1 + m.hp()))
            .collect();
        let fits = |c: i32| c >= 0 && taken.iter().all(|&(a, b)| c + hp <= a || c >= b);
        let col = col.max(0);
        (0..)
            .flat_map(|d| [col + d, col - d])
            .find(|&c| fits(c))
            .unwrap_or(col)
    }
}

/// Param slot for a patch-file key: a param id, or `<param>_cv` for its attenuverter.
fn slot_index(ty: &ModuleType, key: &str) -> Option<usize> {
    let desc = &ty.desc;
    if let Some(i) = desc.param_index(key) {
        return Some(i);
    }
    let param = desc.param_index(key.strip_suffix("_cv")?)?;
    desc.cv_slot(param).map(|(_, atten)| atten)
}

/// A stand-in for a module that isn't installed, with the params the patch
/// saved and the ports its cables use. Port kinds come from the module at
/// the other end of each cable, when that one is known.
fn stand_in(patch: &Patch, m: &ModuleState) -> &'static ModuleType {
    let type_of = |id: ModuleId| {
        let kind = &patch.modules.iter().find(|x| x.id == id)?.kind;
        dov_modules::find(kind)
    };
    let mut inputs: Vec<(String, PortKind)> = Vec::new();
    let mut outputs: Vec<(String, PortKind)> = Vec::new();
    for c in &patch.cables {
        if c.to.module == m.id && !inputs.iter().any(|(p, _)| *p == c.to.port) {
            let kind = type_of(c.from.module)
                .and_then(|t| t.desc.outputs.iter().find(|p| p.id == c.from.port))
                .map_or(PortKind::Mono, |p| p.kind);
            inputs.push((c.to.port.clone(), kind));
        }
        if c.from.module == m.id && !outputs.iter().any(|(p, _)| *p == c.from.port) {
            let kind = type_of(c.to.module)
                .and_then(|t| t.desc.inputs_full().into_iter().find(|p| p.id == c.to.port))
                .map_or(PortKind::Mono, |p| p.kind);
            outputs.push((c.from.port.clone(), kind));
        }
    }
    let params: Vec<(String, f32)> = m.params.iter().map(|(k, v)| (k.clone(), *v)).collect();
    dov_modules::missing(&m.kind, &params, &inputs, &outputs, m.text.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(id: ModuleId, port: &str) -> PortAddr {
        PortAddr {
            module: id,
            port: port.into(),
        }
    }

    #[test]
    fn modules_never_overlap() {
        let mut rack = Rack::default();
        let vco = dov_modules::find("dov.vco").unwrap();
        let a = rack.add_module(vco, 0, 0);
        let b = rack.add_module(vco, 0, 0);
        let (pa, pb) = (rack.module(a).unwrap().pos, rack.module(b).unwrap().pos);
        assert_eq!(pa, (0, 0));
        assert_eq!(pb, (0, rack.module(a).unwrap().hp()));
    }

    #[test]
    fn patch_round_trip_keeps_params_and_cables() {
        let mut rack = Rack::default();
        let vco = rack.add_module(dov_modules::find("dov.vco").unwrap(), 0, 0);
        let out = rack.add_module(dov_modules::find("dov.output").unwrap(), 0, 20);
        rack.connect(addr(vco, "out"), addr(out, "sig"), None);
        let m = rack.module(vco).unwrap();
        m.params.set(1, 13.0);
        let (_, atten) = m.ty.desc.cv_slot(1).unwrap();
        m.params.set(atten, -0.25);

        let text = rack.to_patch().to_ron().unwrap();
        let (loaded, warnings) = Rack::from_patch(&Patch::from_ron(&text).unwrap());
        assert!(warnings.is_empty(), "{warnings:?}");
        let m = loaded.module(vco).unwrap();
        assert_eq!(m.params.get(1), 13.0);
        assert_eq!(m.params.get(atten), -0.25);
        assert_eq!(loaded.cables.len(), 1);
        assert_eq!(loaded.snapshot().cables.len(), 1);
    }

    #[test]
    fn an_old_crt_output_opens_as_a_crt() {
        let patch = Patch::from_ron(
            r#"(modules: [
                (id: (1), kind: "dov.vco", pos: (0, 0)),
                (id: (2), kind: "dov.output", pos: (0, 8),
                 params: {"display": 1.0, "mask_type": 2.0, "glow": 0.45, "scanlines": 0.8}),
                (id: (3), kind: "dov.output", pos: (1, 0), params: {"display": 0.0, "glow": 0.3}),
            ], cables: [(from: (module: (1), port: "out"), to: (module: (2), port: "sig"))])"#,
        )
        .unwrap();
        let (rack, warnings) = Rack::from_patch(&patch);
        assert!(warnings.is_empty(), "{warnings:?}");
        let crt = rack.module(ModuleId(2)).unwrap();
        assert_eq!(crt.ty.desc.id, dov_modules::crt::ID);
        let desc = &crt.ty.desc;
        assert_eq!(crt.params.get(desc.param_index("mask_type").unwrap()), 2.0);
        assert_eq!(crt.params.get(desc.param_index("glow").unwrap()), 0.45);
        assert_eq!(rack.snapshot().cables.len(), 1, "the cable still reaches it");
        assert_eq!(rack.module(ModuleId(3)).unwrap().ty.desc.id, "dov.output");
    }

    #[test]
    fn an_old_patch_takes_its_tempo_from_its_synced_modules() {
        let patch = Patch::from_ron(
            r#"(modules: [
                (id: (1), kind: "dov.lfo", pos: (0, 0), params: {"mode": 0.0, "bpm": 90.0}),
                (id: (2), kind: "dov.lfo", pos: (0, 8), params: {"mode": 1.0, "bpm": 128.0, "bpm_cv": 0.5}),
                (id: (3), kind: "dov.vco", pos: (1, 0), params: {"range": 3.0, "bpm": 100.0}),
            ], cables: [(from: (module: (1), port: "out"), to: (module: (2), port: "bpm_cv"))])"#,
        )
        .unwrap();
        let (rack, warnings) = Rack::from_patch(&patch);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(rack.tempo(), 128.0, "the first module on the beat");
        assert!(rack.cables.is_empty());
        let saved = rack.to_patch();
        assert_eq!(saved.tempo, Some(128.0));
        assert!(saved.modules.iter().all(|m| !m.params.contains_key("bpm")));
        // A new patch, and one that sets its own.
        assert_eq!(Rack::default().tempo(), DEFAULT_TEMPO);
        let (rack, _) = Rack::from_patch(&Patch {
            tempo: Some(87.5),
            ..saved
        });
        assert_eq!(rack.tempo(), 87.5);
    }

    #[test]
    fn text_survives_saving_and_duplicating() {
        let mut rack = Rack::default();
        let text = rack.add_module(dov_modules::find("dov.text").unwrap(), 0, 0);
        assert_eq!(
            rack.module(text).unwrap().params.text(),
            "DESTRUCT-O-VISION",
            "starts with the default"
        );
        rack.module(text).unwrap().params.set_text("hello rack");

        let saved = rack.to_patch().to_ron().unwrap();
        let (loaded, warnings) = Rack::from_patch(&Patch::from_ron(&saved).unwrap());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.module(text).unwrap().params.text(), "hello rack");

        let copy = rack.duplicate(text).unwrap();
        assert_eq!(rack.module(copy).unwrap().params.text(), "hello rack");
        let vco = rack.add_module(dov_modules::find("dov.vco").unwrap(), 1, 0);
        let patch = rack.to_patch();
        let vco_state = patch.modules.iter().find(|m| m.id == vco).unwrap();
        assert_eq!(vco_state.text, None, "modules without a text field save none");
    }

    #[test]
    fn missing_modules_survive_a_round_trip_and_come_back_when_installed() {
        let patch = Patch::from_ron(
            r#"(
                version: 1,
                requires: {"zz.pack": "1.0.0"},
                modules: [
                    (id: (1), kind: "dov.ramps", pos: (0, 0)),
                    (id: (2), kind: "zz.smear", pos: (0, 10), params: {"amount": 0.7, "amount_cv": -0.25}),
                    (id: (3), kind: "dov.output", pos: (0, 20)),
                ],
                cables: [
                    (from: (module: (1), port: "h"), to: (module: (2), port: "sig")),
                    (from: (module: (2), port: "out"), to: (module: (3), port: "sig")),
                ],
            )"#,
        )
        .unwrap();
        let (mut rack, warnings) = Rack::from_patch(&patch);
        assert!(
            warnings[0].contains("zz.smear isn't installed (from package zz.pack 1.0.0)"),
            "{warnings:?}"
        );
        let stand_in = rack.module(ModuleId(2)).unwrap();
        assert_eq!(stand_in.ty.origin, Origin::Missing);
        assert_eq!(rack.cables.len(), 2, "cables to the stand-in are kept");
        let schedule = dov_engine::graph::schedule(&rack.snapshot());
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);

        let saved = rack.to_patch();
        assert_eq!(saved.modules[1].params, patch.modules[1].params);
        assert_eq!(saved.cables, patch.cables);
        assert_eq!(saved.requires, patch.requires);

        // Installing the module brings it back, settings intact.
        let source = crate::modules::template("zz.smear", false);
        let ty = dov_modules::load(
            &source,
            "smear.wgsl",
            Origin::File {
                path: "smear.wgsl".into(),
                package: None,
            },
        )
        .unwrap();
        dov_modules::install(ty);
        assert!(rack.reload_types().is_empty());
        let m = rack.module(ModuleId(2)).unwrap();
        assert!(std::ptr::eq(m.ty, ty));
        assert_eq!(m.params.get(0), 0.7);
        assert_eq!(rack.cables.len(), 2);
        dov_modules::uninstall("zz.smear");
    }

    #[test]
    fn an_input_takes_one_cable_and_undo_restores_it() {
        let mut rack = Rack::default();
        let a = rack.add_module(dov_modules::find("dov.vco").unwrap(), 0, 0);
        let b = rack.add_module(dov_modules::find("dov.ramps").unwrap(), 0, 20);
        let f = rack.add_module(dov_modules::find("dov.fold").unwrap(), 0, 40);
        rack.connect(addr(a, "out"), addr(f, "sig"), None);
        rack.connect(addr(b, "h"), addr(f, "sig"), None);
        assert_eq!(rack.cables.len(), 1);
        assert_eq!(rack.cables[0].from.module, b);
        assert!(rack.undo());
        assert_eq!(rack.cables[0].from.module, a);
        assert!(rack.redo());
        assert_eq!(rack.cables[0].from.module, b);
    }
}
