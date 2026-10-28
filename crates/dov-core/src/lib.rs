//! Core types shared by every Destruct-O-Vision crate. No GPU dependencies.

pub mod module;
pub mod param;
pub mod patch;
pub mod phase;
pub mod raster;

pub use module::{
    Category, CompositeOp, Curve, FONTS, Kernel, Live, ModuleDesc, ParamBank, ParamDesc, ParamKind, PortDesc,
    PortKind, RingSpec, TEXT_MAX_CHARS, TextSpec,
};
pub use param::AtomicF32;
pub use patch::{Cable, ModuleId, ModuleState, Patch, PortAddr};
pub use phase::{PhaseAcc, RasterPhase, SyncReset};
pub use raster::{Composite, RasterStd};
