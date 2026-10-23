//! The module library: the built-in modules, module files loaded at run
//! time, and the registry the rack and engine look types up in.
//!
//! A module is a [`ModuleDesc`] (ports, params, kernel) plus an optional CPU
//! [`Behavior`] that runs once per field. GPU kernels are small WGSL
//! functions; the engine wraps them in the module format's glue (see
//! `dov_format`). Most built-ins are module files in `modules/`, exactly like
//! community modules; the rest are Rust, where they need CPU code or engine
//! resources.

mod audio;
mod behaviors;
pub mod crt;
mod files;
mod library;
mod midi;
mod movie;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use dov_core::{ModuleDesc, RasterStd};
use dov_io::Frame;

pub use behaviors::{FieldIo, choice_value, param_value};
pub use dov_format::glue::EXTRA_SLOTS;
pub use files::{Scan, load, load_dir, load_files, load_package, missing};
pub use movie::{Timeline, loop_frames, splice_starts};

/// Per-field CPU side of a module.
pub trait Behavior: Send {
    fn field(&mut self, ctx: &FieldCtx, io: &mut FieldIo);

    /// A problem to show the user, such as a device that won't open.
    fn warning(&self) -> Option<String> {
        None
    }

    /// A line or two about what the module is doing, for its panel: a
    /// movie's length and where its loop is.
    fn note(&self) -> Option<String> {
        None
    }

    /// For a [`Kernel::Picture`](dov_core::Kernel::Picture) module: the
    /// picture to show. The engine uploads it when it's a different one.
    fn picture(&self) -> Option<Arc<Frame>> {
        None
    }

    /// Where a module that plays a movie is in it, for its editor.
    fn timeline(&self) -> Option<Timeline> {
        None
    }
}

pub struct FieldCtx<'a> {
    pub raster: &'a RasterStd,
    /// Absolute field index.
    pub field: u64,
    /// Fields elapsed since the previous call (0 on the first).
    pub advanced: u64,
    /// Every field is rendered however long it takes (recording), so
    /// sources wait for their data rather than skip it.
    pub offline: bool,
}

impl FieldCtx<'_> {
    pub fn elapsed_seconds(&self) -> f64 {
        self.advanced as f64 * self.raster.field_period()
    }

    /// Time on the engine's clock: the start of this field, in seconds.
    pub fn seconds(&self) -> f64 {
        self.field as f64 * self.raster.field_period()
    }
}

pub struct ModuleType {
    pub desc: ModuleDesc,
    pub behavior: Option<fn() -> Box<dyn Behavior>>,
    pub origin: Origin,
}

/// Where a module type came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    BuiltIn,
    /// A module file, loose or in a package.
    File {
        path: PathBuf,
        package: Option<PackageRef>,
    },
    /// A patch uses a module that isn't installed: a stand-in that keeps the
    /// module's params and cables so saving the patch loses nothing.
    Missing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackageRef {
    pub id: String,
    pub version: String,
}

/// Every installed module type, built-ins first.
fn registry() -> &'static RwLock<Vec<&'static ModuleType>> {
    static REGISTRY: OnceLock<RwLock<Vec<&'static ModuleType>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(library::builtins()))
}

/// Installed module types, in browser order.
pub fn all() -> Vec<&'static ModuleType> {
    registry().read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn find(id: &str) -> Option<&'static ModuleType> {
    let types = registry().read().unwrap_or_else(|e| e.into_inner());
    types.iter().copied().find(|m| m.desc.id == id)
}

/// Adds a module type, replacing any installed one with the same id.
pub fn install(ty: &'static ModuleType) {
    let mut types = registry().write().unwrap_or_else(|e| e.into_inner());
    match types.iter_mut().find(|m| m.desc.id == ty.desc.id) {
        Some(slot) => *slot = ty,
        None => types.push(ty),
    }
}

/// Removes a module type loaded from a file. Built-ins stay.
pub fn uninstall(id: &str) {
    let mut types = registry().write().unwrap_or_else(|e| e.into_inner());
    types.retain(|m| m.desc.id != id || m.origin == Origin::BuiltIn);
}
