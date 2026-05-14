//! `NaadfRenderPlugin` — registers the Phase-A render pipelines, bind-group
//! layouts, and render-graph nodes (`03-design.md` §5).
//!
//! - [`extract`] — `ExtractSchedule`: `WorldData`/camera → render-world mirror.
//! - [`prepare`] — `Prepare`: upload buffers, build bind groups, camera uniforms.
//! - [`graph`] — render-graph node definitions + edges.
//! - [`pipelines`] — compute/render pipeline descriptors for the WGSL passes.
//! - [`gpu_types`] — `#[repr(C)]` structs mirroring every WGSL struct/uniform.
//!
//! Filled by design §8 steps 8–11 (Batch 2).

pub mod extract;
pub mod gpu_types;
pub mod graph;
pub mod pipelines;
pub mod prepare;
