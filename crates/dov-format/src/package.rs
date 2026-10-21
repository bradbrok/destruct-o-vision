//! Packages: the unit that gets published, versioned and installed. A
//! package is a folder with a `package.toml` and its module files.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::module::{Error, is_module_id};

pub const PACKAGE_FILE: &str = "package.toml";

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    /// `<namespace>.<package>`; its modules share the namespace.
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    /// SPDX expression. Optional locally, required to publish.
    pub license: Option<String>,
    pub repository: Option<String>,
    /// Module file patterns, relative to the package folder.
    pub modules: Vec<String>,
}

impl Package {
    pub fn namespace(&self) -> &str {
        self.id.split('.').next().unwrap_or_default()
    }

    /// The module files the patterns name, sorted. `dir` is the package folder.
    pub fn module_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .modules
            .iter()
            .flat_map(|pattern| expand(dir, pattern))
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageFile {
    package: Header,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    authors: Vec<String>,
    license: Option<String>,
    repository: Option<String>,
    modules: Option<Vec<String>>,
}

pub fn parse_package(text: &str) -> Result<Package, Error> {
    let file: PackageFile = toml::from_str(text).map_err(|e| Error {
        messages: vec![format!("{PACKAGE_FILE}: {}", e.message().trim())],
    })?;
    let h = file.package;
    let mut errors = Vec::new();
    if !is_module_id(&h.id) {
        errors.push(format!("package id `{}` must be `<namespace>.<package>`", h.id));
    }
    if h.name.trim().is_empty() || h.name.chars().count() > 48 {
        errors.push("package name must be 1 to 48 characters".into());
    }
    if !is_version(&h.version) {
        errors.push(format!("version `{}` must be semver, like 1.2.0", h.version));
    }
    let modules = h.modules.unwrap_or_else(|| vec!["modules/*".into()]);
    for pattern in &modules {
        let path = Path::new(pattern);
        let escapes = path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
        if escapes
            || pattern.matches('*').count() > 1
            || path.parent().is_some_and(|p| p.to_string_lossy().contains('*'))
        {
            errors.push(format!(
                "module pattern `{pattern}` must be a path inside the package, with at most one `*` in the file name"
            ));
        }
    }
    if !errors.is_empty() {
        return Err(Error { messages: errors });
    }
    Ok(Package {
        id: h.id,
        name: h.name,
        version: h.version,
        description: h.description,
        authors: h.authors,
        license: h.license,
        repository: h.repository,
        modules,
    })
}

fn is_version(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Module files matching `pattern`: a path whose file name may have one `*`.
fn expand(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let path = dir.join(pattern);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    let Some(folder) = path.parent() else {
        return Vec::new();
    };
    let is_module = |p: &Path| p.is_file() && p.extension().is_some_and(|e| e == "wgsl");
    match name.split_once('*') {
        None => [path.clone()].into_iter().filter(|p| is_module(p)).collect(),
        Some((prefix, suffix)) => std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                let n = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                n.len() >= prefix.len() + suffix.len() && n.starts_with(prefix) && n.ends_with(suffix)
            })
            .filter(|p| is_module(p))
            .collect(),
    }
}
