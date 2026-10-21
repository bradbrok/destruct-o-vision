//! Video and control I/O. Each source runs on its own thread and hands the
//! engine its latest data without blocking it.

pub mod camera;

pub use camera::{CameraSource, CameraStatus, Frame};
