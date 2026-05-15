//! InstaMAT → Bevy integration.
//!
//! Two faces, split by the `instamat` Cargo feature:
//!
//! * **feature-free** (always compiled): [`baked_material`] — the game-side
//!   `material.ron` asset loader, the [`MaterialRon`] schema, and
//!   [`BakedMaterialPlugin`]. This is all `bevy-naadf` depends on.
//! * **`instamat` feature** (dev-side only): [`instamat`] — the FFI layer over
//!   `libInstaMATNativeInterface.so`, package discovery, the bake driver, the
//!   PBR channel mapper, and the PNG/RON writer. Drives the `instamat_bake`
//!   batch baker (`src/bin/instamat_bake.rs`). The game crate never enables the
//!   feature, so none of this enters a shipped build.

pub mod baked_material;

#[cfg(feature = "instamat")]
pub mod instamat;

// Re-export the game-facing surface at the crate root for ergonomic use:
//   use bevy_instamat::{BakedMaterialPlugin, MaterialRon, MaterialRonLoader};
pub use baked_material::{BakedMaterialPlugin, MaterialRon, MaterialRonError, MaterialRonLoader};
