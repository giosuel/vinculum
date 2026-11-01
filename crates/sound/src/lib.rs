//! Sound through PipeWire's PulseAudio protocol (pipewire-pulse), in pure Rust: the outputs and
//! inputs, their volumes and ports, the apps playing and where, and the sound cards' profiles.

mod client;
mod model;

pub use client::{Error, Session};
pub use model::{Card, Device, NORM, Port, Profile, Snapshot, Stream, Volume};
