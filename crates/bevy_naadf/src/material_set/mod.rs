//! `MaterialSet` — the four linked texture-arrays (Diffuse+AO, Normal,
//! MRH, Emissive) that the PBR raymarcher samples per voxel-face.
//!
//! Each `Handle<Image>` points at the baked output of one `.texarray.ron`
//! (`assets/materials/*.texarray.ron`). All four arrays SHARE the
//! layer-index space — material N occupies layer N in every array — so the
//! 12-bit `VoxelType.material_layer_index` (`crate::voxel::VoxelType`)
//! selects across the whole set.
//!
//! Built programmatically in [`MaterialSetPlugin::build`] for now; if the
//! project ever ships multiple material sets selectable per-world this
//! resource is the seam to lift into an asset format (`.matset.ron`). See
//! `docs/orchestrate/pbr-raymarching/02-design.md` § C for the design
//! rationale (Resource over Asset).

use bevy::asset::AssetServer;
use bevy::prelude::*;

/// The four linked texture arrays sampled by the unified PBR raymarcher.
///
/// Layer N indexes consistently across all four arrays — diffuse-ao layer N
/// is the same material as normal layer N, mrh layer N, emissive layer N.
/// See `assets/materials/diffuse.texarray.ron` for the canonical layer
/// ordering.
#[derive(Resource, Clone)]
pub struct MaterialSet {
    /// Layer N: RGB = sampled albedo (sRGB-decoded by `Rgba8UnormSrgb`),
    /// A = AO factor in `[0,1]`. Loaded from
    /// `materials/diffuse.texarray.ron`.
    pub diffuse_ao: Handle<Image>,
    /// Layer N: RGB = tangent-space normal (GL convention, Y-up), A unused.
    /// Loaded from `materials/normal.texarray.ron`.
    pub normal: Handle<Image>,
    /// Layer N: R = metallic, G = roughness (perceptual), B = height in
    /// `[0,1]` (POM source), A unused. Loaded from
    /// `materials/mrh.texarray.ron`.
    pub mrh: Handle<Image>,
    /// Layer N: RGB = emissive HDR colour (sRGB-decoded). For PBR voxels
    /// every layer is the black placeholder; only the Emissive fast-path
    /// samples it. Loaded from `materials/emissive.texarray.ron`.
    pub emissive: Handle<Image>,
}

/// Plugin: registers `MaterialSet` as a resource by loading the four
/// `.texarray.ron` definitions from the asset server. See
/// [`MaterialSet`] for the per-array semantics.
pub struct MaterialSetPlugin;

impl Plugin for MaterialSetPlugin {
    fn build(&self, app: &mut App) {
        let asset_server = app.world().resource::<AssetServer>();
        let set = MaterialSet {
            diffuse_ao: asset_server.load("materials/diffuse.texarray.ron"),
            normal: asset_server.load("materials/normal.texarray.ron"),
            mrh: asset_server.load("materials/mrh.texarray.ron"),
            emissive: asset_server.load("materials/emissive.texarray.ron"),
        };
        app.insert_resource(set);
    }
}
