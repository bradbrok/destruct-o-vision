//! Video and control I/O: cameras, movie files, audio and MIDI. Each source
//! runs on its own thread and hands the engine its latest data without
//! blocking it.

mod access;
pub mod audio;
pub mod camera;
pub mod midi;
pub mod movie;

pub use audio::{AudioCursor, AudioSource, AudioStatus, InputDevice};
pub use camera::{CameraSource, CameraStatus, Frame};
pub use midi::{MidiMessage, MidiSource, MidiStatus};
pub use movie::{Movie, MovieInfo, MovieStatus, Order, Want};
