//! Panel art from the modules folder (see `dov_format::panel`). An SVG named
//! for a module id restyles that module's panel, built-ins included. Art is
//! looked for in three places; for each module, the first one found wins:
//!
//! 1. `panels/<module-id>.svg` in the modules folder: your own skins
//! 2. `<package>/panels/<module-id>.svg`: art that ships with a package, or
//!    a skin pack (a package with only panels)
//! 3. `<file>.svg` next to a loose `<file>.wgsl` module
//!
//! The registry is global because panel layout, which needs it, is worked
//! out wherever a module's width matters.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use dov_core::{ModuleDesc, ParamKind};
use dov_format::panel::{self, PanelArt, Place, component_id};
use dov_format::usvg::fontdb;

/// Parsed art and where it came from.
pub struct LoadedArt {
    pub art: PanelArt,
    pub path: PathBuf,
    /// Changes whenever art is reloaded, so drawings and layouts refresh.
    pub generation: u64,
}

struct Registry {
    by_id: HashMap<String, Arc<LoadedArt>>,
    enabled: bool,
    generation: u64,
}

fn registry() -> &'static RwLock<Registry> {
    static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        RwLock::new(Registry {
            by_id: HashMap::new(),
            enabled: true,
            generation: 1,
        })
    })
}

/// System fonts, for text in art. Loaded the first time art needs them.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// The art for a module, unless art is switched off.
pub fn art_for(id: &str) -> Option<Arc<LoadedArt>> {
    let r = registry().read().unwrap_or_else(|e| e.into_inner());
    r.enabled.then(|| r.by_id.get(id).cloned()).flatten()
}

pub fn enabled() -> bool {
    registry().read().unwrap_or_else(|e| e.into_inner()).enabled
}

pub fn set_enabled(on: bool) {
    let mut r = registry().write().unwrap_or_else(|e| e.into_inner());
    r.enabled = on;
    r.generation += 1;
}

/// Changes whenever the art in use changes.
pub fn generation() -> u64 {
    registry().read().unwrap_or_else(|e| e.into_inner()).generation
}

pub fn count() -> usize {
    registry().read().unwrap_or_else(|e| e.into_inner()).by_id.len()
}

/// Components full art must place for a module: the same controls its
/// panel would have.
pub fn required(desc: &ModuleDesc) -> Vec<String> {
    let mut ids = Vec::new();
    if desc.text.is_some() {
        ids.push(component_id(Place::Text));
    }
    if desc.screen {
        ids.push(component_id(Place::Screen));
    }
    ids.extend(
        desc.params
            .iter()
            .filter(|p| !p.in_editor)
            .map(|p| component_id(Place::Param(p.id))),
    );
    ids.extend(
        desc.cv_params()
            .map(|i| component_id(Place::Cv(desc.params[i].id))),
    );
    ids.extend(
        desc.inputs
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| component_id(Place::Input(p.id))),
    );
    ids.extend(
        desc.outputs
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| component_id(Place::Output(p.id))),
    );
    ids
}

/// Whether a param is drawn as a switch (its component marks the switch).
pub fn is_switch(desc: &ModuleDesc, param: usize) -> bool {
    matches!(desc.params[param].kind, ParamKind::Choice { .. })
}

fn svgs_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "svg"))
        .collect();
    paths.sort();
    paths
}

/// Finds and parses all panel art for the modules folder `dir`. Returns
/// the files that failed, with what's wrong.
pub fn reload(dir: &Path) -> Vec<(PathBuf, String)> {
    // (module id, file), best first.
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let named = |path: &PathBuf| {
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        (id, path.clone())
    };
    candidates.extend(svgs_in(&dir.join("panels")).iter().map(named));
    let mut packages: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(dov_format::PACKAGE_FILE).is_file())
        .collect();
    packages.sort();
    for package in packages {
        candidates.extend(svgs_in(&package.join("panels")).iter().map(named));
    }
    for ty in dov_modules::all() {
        if let dov_modules::Origin::File { path, package: None } = &ty.origin {
            let svg = path.with_extension("svg");
            if svg.is_file() {
                candidates.push((ty.desc.id.to_string(), svg));
            }
        }
    }

    let generation = generation() + 1;
    let mut by_id = HashMap::new();
    let mut errors = Vec::new();
    for (id, path) in candidates {
        if by_id.contains_key(&id) {
            continue;
        }
        let art = match load_file(&path, &id) {
            Ok(art) => art,
            Err(e) => {
                errors.push((path, e));
                continue;
            }
        };
        by_id.insert(
            id,
            Arc::new(LoadedArt {
                art,
                path,
                generation,
            }),
        );
    }
    if !by_id.is_empty() {
        log::info!("loaded panel art for {} module(s)", by_id.len());
    }
    let mut r = registry().write().unwrap_or_else(|e| e.into_inner());
    r.by_id = by_id;
    r.generation = generation;
    errors
}

/// Parses one art file for module `id`. Full art must place exactly the
/// controls of that module, if it's installed.
fn load_file(path: &Path, id: &str) -> Result<PanelArt, String> {
    if !dov_format::module::is_module_id(id) {
        return Err("name panel art for the module it styles, like `dov.vco.svg`".into());
    }
    let art = panel::parse_art(&read(path)?, fonts()).map_err(|e| e.to_string())?;
    if let (Some(components), Some(ty)) = (&art.components, dov_modules::find(id)) {
        panel::check_components(components, &required(&ty.desc)).map_err(|e| e.to_string())?;
    }
    Ok(art)
}

/// Checks an art file named `<module-id>.svg`, for `module check`.
/// Describes what it is when it's fine.
pub fn check_file(path: &Path) -> Result<String, String> {
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let art = load_file(path, &id)?;
    let kind = match (&art.components, dov_modules::find(&id)) {
        (None, _) => "background art",
        (Some(_), Some(_)) => "full art",
        (Some(_), None) => "full art, not checked: module not installed",
    };
    Ok(format!("{id} ({kind}, {} HP)", art.hp()))
}

/// SVG files in a folder's `panels/` folder and in its packages' ones.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = svgs_in(&dir.join("panels"));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path().join(dov_format::PACKAGE_FILE).is_file() {
            files.extend(svgs_in(&entry.path().join("panels")));
        }
    }
    files
}

fn read(path: &Path) -> Result<String, String> {
    dov_format::read_text_file(path, panel::MAX_ART_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{self, Element};

    /// The registry is global, so this one test does everything with art,
    /// and only for example modules no other test uses.
    #[test]
    fn art_places_controls_or_sits_behind_the_layout() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/modules");
        for ty in dov_modules::load_package(&examples.join("example.starter")).loaded {
            dov_modules::install(ty);
        }
        let dir = std::env::temp_dir().join(format!("dov-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("panels")).unwrap();
        let full = examples.join("example.starter/panels/example.tear.svg");
        std::fs::copy(&full, dir.join("panels/example.tear.svg")).unwrap();
        std::fs::copy(
            examples.join("example.skins/panels/dov.mixer.svg"),
            dir.join("panels/example.ghosts.svg"),
        )
        .unwrap();
        // Line Tear's art on Bleed: it places controls Bleed doesn't have.
        std::fs::copy(&full, dir.join("panels/example.bleed.svg")).unwrap();

        let errors = reload(&dir);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0]
                .1
                .contains("`param-amount`, which this module doesn't have"),
            "{errors:?}"
        );

        let tear = &dov_modules::find("example.tear").unwrap().desc;
        let l = panel::layout(tear);
        assert!(l.art.is_some() && !l.labels);
        assert_eq!(l.hp, 8);
        let amount = tear.param_index("amount").unwrap();
        let knob = l
            .elements
            .iter()
            .find(|p| matches!(p.element, Element::Knob { param } if param == amount))
            .unwrap();
        assert_eq!((knob.cell.center().x, knob.cell.top() + 22.0), (60.0, 82.0));
        let input = l
            .elements
            .iter()
            .find(|p| matches!(p.element, Element::Input { .. }))
            .unwrap();
        assert_eq!(input.jack_center().unwrap(), egui::vec2(34.0, 336.0));

        let ghosts = panel::layout(&dov_modules::find("example.ghosts").unwrap().desc);
        assert!(
            ghosts.art.is_some() && ghosts.labels,
            "background art keeps the automatic layout"
        );
        let bleed = panel::layout(&dov_modules::find("example.bleed").unwrap().desc);
        assert!(bleed.art.is_none());

        set_enabled(false);
        assert!(panel::layout(tear).art.is_none(), "switched off");
        set_enabled(true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
