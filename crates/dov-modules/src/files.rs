//! Module files: turning a checked file into a module type, scanning a
//! modules folder (loose files and packages), and stand-ins for modules a
//! patch uses that aren't installed.
//!
//! Loaded descriptors are leaked so they can be `&'static` like the
//! built-ins. Each reload of an edited file leaks its previous descriptor (a
//! few kilobytes), which is fine at editing speed.

use std::path::{Path, PathBuf};

use dov_core::{
    Category, Curve, Kernel, ModuleDesc, ParamDesc, ParamKind, PortDesc, PortKind, RingSpec, TextSpec,
};
use dov_format as f;

use crate::{ModuleType, Origin, PackageRef};

/// Module files bigger than this are refused unread.
const MAX_FILE_BYTES: u64 = 1 << 20;

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_slice<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

/// Checks a module file and turns it into a module type. `path` labels
/// errors. Only built-ins may use the `dov` namespace.
pub fn load(source: &str, path: &str, origin: Origin) -> Result<&'static ModuleType, f::Error> {
    let module = f::check(source, path)?;
    let builtin = origin == Origin::BuiltIn;
    if builtin != (module.namespace() == "dov") {
        let message = if builtin {
            "built-in modules use the `dov` namespace"
        } else {
            "the `dov` namespace is reserved for built-in modules"
        };
        return Err(f::Error {
            messages: vec![message.into()],
        });
    }
    Ok(Box::leak(Box::new(ModuleType {
        desc: desc(module),
        behavior: None,
        origin,
    })))
}

fn port_kind(kind: f::PortKind) -> PortKind {
    match kind {
        f::PortKind::Mono => PortKind::Mono,
        f::PortKind::Rgb => PortKind::Rgb,
        f::PortKind::Control => PortKind::Control,
    }
}

fn desc(m: f::Module) -> ModuleDesc {
    let port = |p: f::Port| PortDesc {
        id: leak(p.id),
        name: leak(p.name),
        kind: port_kind(p.kind),
        default: p.default,
        hidden: p.hidden,
    };
    let iface = m.interface;
    let params = iface
        .params
        .into_iter()
        .map(|p| ParamDesc {
            id: leak(p.id),
            name: leak(p.name),
            kind: match p.kind {
                f::ParamKind::Knob {
                    min,
                    max,
                    default,
                    curve,
                    unit,
                } => ParamKind::Knob {
                    min,
                    max,
                    default,
                    curve: match curve {
                        f::Curve::Linear => Curve::Linear,
                        f::Curve::Exp => Curve::Exp,
                    },
                    unit: leak(unit),
                },
                f::ParamKind::Choice { options, default } => ParamKind::Choice {
                    options: leak_slice(options.into_iter().map(leak).collect()),
                    default,
                },
            },
            cv: p.cv.map(|cv| match cv {
                f::Cv::Video => PortKind::Mono,
                f::Cv::Control => PortKind::Control,
            }),
            live: None,
            in_editor: false,
        })
        .collect();
    let source = leak(m.source);
    ModuleDesc {
        id: leak(m.id),
        name: leak(m.name),
        category: match m.category {
            f::Category::Source => Category::Source,
            f::Category::Process => Category::Process,
            f::Category::Color => Category::Color,
            f::Category::Space => Category::Space,
            f::Category::Control => Category::Control,
            f::Category::Io => Category::Io,
        },
        description: leak(m.description),
        inputs: leak_slice(iface.inputs.into_iter().map(port).collect()),
        outputs: leak_slice(iface.outputs.into_iter().map(port).collect()),
        params: leak_slice(params),
        screen: m.screen,
        ring: iface.history.map(|h| RingSpec {
            output: h.output,
            freeze_param: h.freeze,
            fields: h.fields,
        }),
        text: m.text_default.map(|t| TextSpec {
            default: leak(t),
            live: None,
        }),
        kernel: match iface.kernel {
            f::KernelKind::Pointwise => Kernel::Pointwise(source),
            f::KernelKind::LineScan => Kernel::LineScan(source),
        },
    }
}

/// What a folder scan found.
#[derive(Default)]
pub struct Scan {
    pub loaded: Vec<&'static ModuleType>,
    /// Files and packages that failed, with what's wrong.
    pub errors: Vec<(PathBuf, String)>,
}

/// Loads every module in `dir`: loose `*.wgsl` files, and packages
/// (folders with a `package.toml`). Ids must be unique across them.
pub fn load_dir(dir: &Path) -> Scan {
    let mut scan = Scan::default();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let manifest = path.join(f::PACKAGE_FILE);
            if manifest.is_file() {
                add_package(&path, &manifest, &mut scan);
            }
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            load_file(&path, None, &mut scan);
        }
    }
    scan
}

fn read(path: &Path) -> Result<String, String> {
    f::read_text_file(path, MAX_FILE_BYTES)
}

/// Loads one package folder.
pub fn load_package(dir: &Path) -> Scan {
    let mut scan = Scan::default();
    add_package(dir, &dir.join(f::PACKAGE_FILE), &mut scan);
    scan
}

/// Loads individual module files, as loose modules.
pub fn load_files(paths: &[PathBuf]) -> Scan {
    let mut scan = Scan::default();
    for path in paths {
        load_file(path, None, &mut scan);
    }
    scan
}

fn add_package(dir: &Path, manifest: &Path, scan: &mut Scan) {
    let package = match read(manifest).and_then(|text| f::parse_package(&text).map_err(|e| e.to_string())) {
        Ok(p) => p,
        Err(e) => {
            scan.errors.push((manifest.to_path_buf(), e));
            return;
        }
    };
    let files = package.module_files(dir);
    // A package with only panel art is a skin pack.
    if files.is_empty() && !dir.join("panels").is_dir() {
        scan.errors.push((
            manifest.to_path_buf(),
            "the package's patterns match no module files, and it has no panels folder".into(),
        ));
    }
    for file in files {
        load_file(&file, Some(&package), scan);
    }
}

fn load_file(path: &Path, package: Option<&f::Package>, scan: &mut Scan) {
    let origin = Origin::File {
        path: path.to_path_buf(),
        package: package.map(|p| PackageRef {
            id: p.id.clone(),
            version: p.version.clone(),
        }),
    };
    let loaded = read(path)
        .and_then(|source| load(&source, &path.display().to_string(), origin).map_err(|e| e.to_string()));
    match loaded {
        Ok(ty) => {
            let namespace = ty.desc.id.split('.').next().unwrap_or_default();
            if let Some(p) = package
                && namespace != p.namespace()
            {
                scan.errors.push((
                    path.to_path_buf(),
                    format!(
                        "module {} isn't in its package's namespace `{}`",
                        ty.desc.id,
                        p.namespace()
                    ),
                ));
            } else if scan.loaded.iter().any(|t| t.desc.id == ty.desc.id) {
                scan.errors.push((
                    path.to_path_buf(),
                    format!("another file already defines {}", ty.desc.id),
                ));
            } else {
                scan.loaded.push(ty);
            }
        }
        Err(e) => scan.errors.push((path.to_path_buf(), e)),
    }
}

/// A stand-in for a module type that isn't installed, shaped by what the
/// patch knows: the params it saved, and the ports its cables use (with the
/// kinds of whatever is at their other ends). It runs nothing; its outputs
/// stay black.
pub fn missing(
    kind: &str,
    params: &[(String, f32)],
    inputs: &[(String, PortKind)],
    outputs: &[(String, PortKind)],
    text: Option<&str>,
) -> &'static ModuleType {
    let port = |(id, kind): &(String, PortKind)| PortDesc::new(leak(id.clone()), leak(id.clone()), *kind);
    let params = params
        .iter()
        .map(|(id, v)| {
            let (min, max) = (v.min(0.0) - 1.0, v.max(1.0) + 1.0);
            ParamDesc::knob(leak(id.clone()), leak(id.clone()), min, max, *v)
        })
        .collect();
    Box::leak(Box::new(ModuleType {
        desc: ModuleDesc {
            id: leak(kind.to_string()),
            name: "Missing",
            category: Category::Io,
            description: leak(format!(
                "{kind} isn't installed. Its settings and cables are kept, and saving the patch keeps them."
            )),
            inputs: leak_slice(inputs.iter().map(port).collect()),
            outputs: leak_slice(outputs.iter().map(port).collect()),
            params: leak_slice(params),
            screen: false,
            ring: None,
            text: text.map(|t| TextSpec {
                default: leak(t.to_string()),
                live: None,
            }),
            kernel: Kernel::Control,
        },
        behavior: None,
        origin: Origin::Missing,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_package_loads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/modules/example.starter");
        let scan = load_package(&dir);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        let mut ids: Vec<_> = scan.loaded.iter().map(|t| t.desc.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["example.bleed", "example.ghosts", "example.tear"]);
        let Origin::File { package: Some(p), .. } = &scan.loaded[0].origin else {
            panic!("package origin");
        };
        assert_eq!((p.id.as_str(), p.version.as_str()), ("example.starter", "0.1.0"));
    }

    #[test]
    fn only_built_ins_use_the_dov_namespace() {
        let source = include_str!("../modules/posterize.wgsl");
        let origin = Origin::File {
            path: "posterize.wgsl".into(),
            package: None,
        };
        let e = load(source, "posterize.wgsl", origin).err().unwrap();
        assert!(e.to_string().contains("reserved for built-in modules"));
    }
}
