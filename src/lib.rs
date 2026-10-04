//! `mviz`, a music visualizer: audio sources, analysis, rendering and offline output. The
//! analysis and the renderer are plain functions of their input, so both are tested without
//! audio devices or a window.

pub mod analysis;
pub mod audio;
pub mod config;
pub mod demo;
pub mod offline;
pub mod render;
