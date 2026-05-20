//! `WorldPlugin` — wires the voxel-world resources (`03-design.md` §4).
//!
//! - [`data`] — the `WorldData` / `VoxelTypes` main-world resources (the three
//!   CPU mirrors + sizes + the voxel-type palette).
//! - [`buffer`] — the `GrowableBuffer<T>` abstraction.
//! - [`oracle`] (crate-internal) — diagnostic-only edit oracles. Extracted
//!   from `WorldData`'s public API by D1 (`/delegate` codebase-tightening,
//!   Finding 1) — production code paths never call into this module.
//!
//! The GPU-side resources (`WorldGpu` / `FrameGpu`) and the render passes live
//! in [`crate::render`], wired by `NaadfRenderPlugin`. `WorldPlugin` only owns
//! the main-world CPU side; `voxel::grid::setup_test_grid` (a `Startup` system,
//! added in `main`) builds it.

pub mod buffer;
pub mod data;
pub(crate) mod oracle;

use bevy::prelude::*;

/// Plugin: registers the main-world voxel-world resources.
///
/// The resources are *inserted* by `voxel::grid::setup_test_grid` at startup
/// (it needs to build them, not just default them), so this plugin has nothing
/// to add at build time beyond marking the module's ownership — it is kept as
/// the wiring seam the design names (`03-design.md` §1, §4) and as the slot
/// Phase A-2 / B grow into.
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, _app: &mut App) {
        // `WorldData` + `VoxelTypes` are inserted by `setup_test_grid`
        // (Startup) — see `voxel::grid`. Nothing to register here in Phase A.
    }
}
