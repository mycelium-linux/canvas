//! Native client for streamed Canvas models.
//!
//! `canvas-model-host` owns mesh baking, GPU rendering, and video encoding.
//! This crate owns viewer-side interaction and presentation. Transport remains
//! a narrow adapter supplied by the embedding application.

pub mod orbit;
pub mod state;
pub mod video;
pub mod view;
mod view_cube;

pub use orbit::{OrbitAction, OrbitController, action_for_key};
