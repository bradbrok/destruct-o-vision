//! The user's modules folder: loose module files and packages, loaded at
//! startup and reloaded whenever something in the folder changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often the folder is checked for changes.
const POLL: Duration = Duration::from_millis(500);

pub struct UserModules {
    pub dir: PathBuf,
    /// Modules installed from the folder: id and file.
    installed: Vec<(String, PathBuf)>,
    /// Files and packages that failed to load, with what's wrong.
    pub errors: Vec<(PathBuf, String)>,
    stamps: Vec<(PathBuf, SystemTime, u64)>,
    checked: Instant,
}

impl UserModules {
    pub fn new(dir: PathBuf) -> Self {
        let mut m = Self {
            dir,
            installed: Vec::new(),
            errors: Vec::new(),
            stamps: Vec::new(),
            checked: Instant::now(),
        };
        m.reload();
        m
    }

    pub fn count(&self) -> usize {
        self.installed.len()
    }

    /// Reloads when anything in the folder changed since the last look.
    /// Returns true when it reloaded.
    pub fn poll(&mut self) -> bool {
        if self.checked.elapsed() < POLL {
            return false;
        }
        self.checked = Instant::now();
        if stamps(&self.dir) == self.stamps {
            return false;
        }
        self.reload();
        true
    }

    /// Loads everything in the folder again. A module whose file now fails
    /// keeps its last good version, so an edit in progress doesn't pull it
    /// out of the rack.
    pub fn reload(&mut self) {
        self.stamps = stamps(&self.dir);
        let scan = dov_modules::load_dir(&self.dir);
        let failed = |path: &Path| {
            scan.errors.iter().any(|(e, _)| {
                e == path
                    || (e.file_name().is_some_and(|n| n == dov_format::PACKAGE_FILE)
                        && e.parent().is_some_and(|dir| path.starts_with(dir)))
            })
        };
        let mut installed = Vec::new();
        for (id, path) in std::mem::take(&mut self.installed) {
            if scan.loaded.iter().any(|t| t.desc.id == id) {
                continue;
            }
            if path.exists() && failed(&path) {
                installed.push((id, path));
            } else {
                dov_modules::uninstall(&id);
            }
        }
        for ty in &scan.loaded {
            dov_modules::install(ty);
            if let dov_modules::Origin::File { path, .. } = &ty.origin {
                installed.push((ty.desc.id.to_string(), path.clone()));
            }
        }
        for (path, e) in &scan.errors {
            log::warn!("module {}: {e}", path.display());
        }
        if !scan.loaded.is_empty() {
            log::info!(
                "loaded {} module(s) from {}",
                scan.loaded.len(),
                self.dir.display()
            );
        }
        self.installed = installed;
        self.errors = scan.errors;
        // Art after modules: full art is checked against the module it styles.
        for (path, e) in crate::art::reload(&self.dir) {
            log::warn!("panel art {}: {e}", path.display());
            self.errors.push((path, e));
        }
    }
}

/// Size and modification time of everything that defines modules or their
/// panels: module files, package manifests and SVGs, three folders deep.
fn stamps(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 3 {
                    walk(&path, depth + 1, out);
                }
            } else if path.extension().is_some_and(|e| e == "wgsl" || e == "svg")
                || path.file_name().is_some_and(|n| n == dov_format::PACKAGE_FILE)
            {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, modified, meta.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out
}

/// A new module file to start from. `id` is `<namespace>.<name>`.
pub fn template(id: &str, linescan: bool) -> String {
    let name = id.split('.').nth(1).unwrap_or(id);
    let title: String = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let (kernel, body) = if linescan {
        (
            "linescan",
            r#"// Runs left to right along each line, carrying `State` from sample to
// sample like an analog circuit: here, a smear that slews towards the input.
struct State {
    y: vec3<f32>,
}

fn init_state() -> State {
    return State(vec3<f32>(0.0));
}

fn process(ctx: Ctx, inp: Inputs, p: Params, s: ptr<function, State>) -> Outputs {
    (*s).y = mix((*s).y, inp.sig, 1.0 - p.amount * 0.98);
    var o: Outputs;
    o.out = (*s).y;
    return o;
}
"#,
        )
    } else {
        (
            "pointwise",
            r#"// Runs once per sample. `inp` holds the inputs, `p` the params with CV
// already applied; fill in and return the outputs. `ctx` says where you
// are: `ctx.uv` (0..1), `ctx.x`, `ctx.row`, `ctx.field`.
fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = mix(inp.sig, vec3<f32>(1.0) - inp.sig, p.amount);
    return o;
}
"#,
        )
    };
    format!(
        r#"/* dov-module
format = 1
id = "{id}"
name = "{title}"
category = "process"
description = "What it does, in a sentence or two."
kernel = "{kernel}"

[[input]]
id = "sig"
name = "In"
kind = "rgb"

[[output]]
id = "out"
name = "Out"
kind = "rgb"

[[param]]
id = "amount"
name = "Amount"
range = [0.0, 1.0]
default = 0.5
cv = "video"
*/

{body}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid_modules() {
        for linescan in [false, true] {
            let src = template("local.my-module", linescan);
            let m = dov_format::check(&src, "template.wgsl").unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(m.name, "My Module");
        }
    }

    #[test]
    fn a_broken_edit_keeps_the_last_good_version() {
        let _lock = crate::test_lock();
        let dir = std::env::temp_dir().join(format!("dov-user-modules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("glow.wgsl");
        std::fs::write(&file, template("test.glow", false)).unwrap();
        let mut modules = UserModules::new(dir.clone());
        assert_eq!(modules.count(), 1);
        let first = dov_modules::find("test.glow").unwrap();

        std::fs::write(&file, template("test.glow", false).replace("p.amount", "p.nope")).unwrap();
        modules.reload();
        assert_eq!(modules.errors.len(), 1);
        assert!(
            std::ptr::eq(dov_modules::find("test.glow").unwrap(), first),
            "still the good one"
        );

        std::fs::remove_file(&file).unwrap();
        modules.reload();
        assert!(
            dov_modules::find("test.glow").is_none(),
            "deleting the file removes it"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
