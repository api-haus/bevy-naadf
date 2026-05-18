//! `material.ron` → `StandardMaterial` asset loader.
//!
//! Reads a small RON manifest (per-channel PNG filenames + scalar fallbacks)
//! and the sibling PNGs it references, and yields a `StandardMaterial` with
//! sRGB / linear texture formats wired correctly for each PBR channel.

pub mod baked_material;

// Re-export the surface at the crate root for ergonomic use:
//   use bevy_instamat::{BakedMaterialPlugin, MaterialRon, MaterialRonLoader};
pub use baked_material::{BakedMaterialPlugin, MaterialRon, MaterialRonError, MaterialRonLoader};
