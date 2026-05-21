//! Named camera poses shared between production code paths and e2e gates.
//!
//! Production code (`voxel/grid.rs::install_imported_vox`,
//! `voxel/web_vox::pin_web_horizon_camera`) and the cross-target SSIM gate
//! (`e2e/vox_horizon_parity`) both anchor on these constants so a
//! `just web-static` / `just web` / native release boot lands at the same
//! camera the Playwright gate screenshots. **Production code MUST NOT import
//! from `crate::e2e`** — this module is the canonical home (D3 finding 6).

use bevy::prelude::*;

/// Cross-target horizon-view camera position (voxel units, world coords).
/// User-captured 2026-05-19. See [`crate::e2e::vox_horizon_parity`] module
/// docs for rationale + the corresponding window-resolution / SSIM threshold.
pub const HORIZON_CAMERA_POS: Vec3 = Vec3::new(3880.187, 497.332, 3514.350);

/// Cross-target horizon-view camera rotation. Forward ≈ `(-0.924, -0.241, -0.297)`.
pub const HORIZON_CAMERA_ROT: Quat = Quat::from_xyzw(
    -0.09791362,
    0.5846077,
    0.07135339,
    0.8022191,
);
