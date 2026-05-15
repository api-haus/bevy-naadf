//! Game-side consumption of baked InstaMAT materials — **feature-free**.
//!
//! This module compiles into the shippable game with **no `instamat` feature**.
//! It contains zero InstaMAT / FFI / `libloading` symbols — only `bevy`,
//! `serde`, `ron`, and `std`.
//!
//! The offline `instamat_bake` tool (`src/bin/instamat_bake.rs`, dev-side only)
//! writes a material as per-channel PNG files plus a `material.ron` manifest
//! under `assets/materials/<name>/`. This module is the *other* side of that
//! contract: [`MaterialRonLoader`] is a stock Bevy [`AssetLoader`] that
//! RON-parses the manifest, loads the sibling PNGs through Bevy's own
//! `ImageLoader`, and yields a `StandardMaterial`. The game does:
//!
//! ```ignore
//! asset_server.load::<StandardMaterial>("materials/steampunk_metal/material.ron")
//! ```
//!
//! and gets a `Handle<StandardMaterial>` straight away — no InstaMAT on this
//! path, no custom wrapper asset.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::image::{Image, ImageLoaderSettings};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

// ---- material.ron schema ------------------------------------------------

/// The `material.ron` schema — the contract between the dev-side bake tool and
/// the game. RON is field-name-keyed; this struct is **structurally identical**
/// to `MaterialRonOut` in `src/instamat/bake_output.rs` (the bake side), so the
/// two round-trip. **Keep the field names in sync with that struct.**
///
/// A `None` filename means the channel was not baked; the loader falls back to
/// the scalar fields for those channels.
#[derive(Serialize, Deserialize, Debug)]
pub struct MaterialRon {
    /// Material name (from the `.imp` file stem; informational).
    pub name: String,
    /// Base-color / albedo PNG, relative to `material.ron`'s own directory.
    pub base_color: Option<String>,
    /// Tangent-space normal-map PNG.
    pub normal: Option<String>,
    /// Packed metallic/roughness PNG — roughness in G, metallic in B.
    pub metallic_roughness: Option<String>,
    /// Ambient-occlusion PNG.
    pub occlusion: Option<String>,
    /// Emissive PNG.
    pub emissive: Option<String>,
    /// Scalar roughness fallback for channels that were not baked (mirrors
    /// `StandardMaterial::perceptual_roughness` default 0.5).
    pub perceptual_roughness: f32,
    /// Scalar metallic fallback (mirrors `StandardMaterial::metallic` default 0.0).
    pub metallic: f32,
    /// `true` iff `emissive` is `Some` — tells the loader to drive the emissive
    /// color at white so the emissive map is actually visible.
    pub emissive_is_textured: bool,
}

// ---- loader error -------------------------------------------------------

/// Errors from [`MaterialRonLoader::load`]. Hand-written (no `thiserror`
/// dependency) — `AssetLoader::Error` only needs `Into<BevyError>`, which any
/// `std::error::Error` satisfies.
#[derive(Debug)]
pub enum MaterialRonError {
    /// The `material.ron` byte stream could not be read.
    Io(std::io::Error),
    /// The `material.ron` text could not be deserialized as a [`MaterialRon`].
    Ron(ron::error::SpannedError),
}

impl std::fmt::Display for MaterialRonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "failed to read material.ron bytes: {e}"),
            Self::Ron(e) => write!(f, "failed to deserialize material.ron (RON): {e}"),
        }
    }
}

impl std::error::Error for MaterialRonError {}

impl From<std::io::Error> for MaterialRonError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ron::error::SpannedError> for MaterialRonError {
    fn from(e: ron::error::SpannedError) -> Self {
        Self::Ron(e)
    }
}

// ---- the loader ---------------------------------------------------------

/// Loads a `material.ron` manifest and the sibling PNGs it references, yielding
/// a `StandardMaterial`. Registered for the `ron` extension — this repo has no
/// other `.ron` assets, and a non-`MaterialRon` `.ron` would simply fail the
/// RON parse here with a clear error.
#[derive(Default, TypePath)]
pub struct MaterialRonLoader;

impl AssetLoader for MaterialRonLoader {
    /// The loader's output asset *is* the material — `StandardMaterial` is
    /// `#[derive(Asset)]`, so no wrapper asset is needed.
    type Asset = StandardMaterial;
    type Settings = ();
    type Error = MaterialRonError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<StandardMaterial, MaterialRonError> {
        // 1. Read + RON-deserialize the manifest.
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let manifest: MaterialRon = ron::de::from_bytes(&bytes)?;

        // 2. Load each present sibling PNG as a dependency `Image`, resolving
        //    the filename against `material.ron`'s own AssetPath. Bevy 0.19's
        //    `LoadContext::load_builder()` returns a `NestedLoadBuilder` whose
        //    `.with_settings(...)` overrides the `ImageLoaderSettings` — that is
        //    how the per-channel sRGB-vs-linear `TextureFormat` is forced:
        //    Bevy's stock `ImageLoader` decodes a PNG as `Rgba8UnormSrgb` when
        //    `is_srgb` is true (correct for base color / emissive) and
        //    `Rgba8Unorm` when false (correct for normal / metallic_roughness /
        //    occlusion — linear data, or PBR shading is visibly wrong).
        let manifest_path = load_context.path().clone_owned();
        let mut load_png = |file: &str, srgb: bool| -> Result<Handle<Image>, MaterialRonError> {
            // `resolve_embed_str` resolves `file` as a sibling of `material.ron`
            // (RFC-1808 embedded semantics — the manifest's filename is dropped
            // before concatenation), so the material directory is relocatable.
            let sibling = manifest_path
                .resolve_embed_str(file)
                .map_err(|e| {
                    MaterialRonError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("invalid sibling texture path `{file}` in material.ron: {e}"),
                    ))
                })?;
            Ok(load_context
                .load_builder()
                .with_settings(move |s: &mut ImageLoaderSettings| {
                    s.is_srgb = srgb;
                })
                .load::<Image>(sibling))
        };

        let base_color = match manifest.base_color.as_deref() {
            Some(f) => Some(load_png(f, true)?),
            None => None,
        };
        let normal_map = match manifest.normal.as_deref() {
            Some(f) => Some(load_png(f, false)?),
            None => None,
        };
        let metallic_roughness = match manifest.metallic_roughness.as_deref() {
            Some(f) => Some(load_png(f, false)?),
            None => None,
        };
        let occlusion = match manifest.occlusion.as_deref() {
            Some(f) => Some(load_png(f, false)?),
            None => None,
        };
        let emissive = match manifest.emissive.as_deref() {
            Some(f) => Some(load_png(f, true)?),
            None => None,
        };

        // 3. Build the StandardMaterial. Texture handles for present channels;
        //    scalar fallbacks for absent metallic/roughness; a white `emissive`
        //    color when an emissive map is present so it is actually visible.
        Ok(StandardMaterial {
            base_color_texture: base_color,
            normal_map_texture: normal_map,
            metallic_roughness_texture: metallic_roughness,
            occlusion_texture: occlusion,
            emissive_texture: emissive,
            perceptual_roughness: manifest.perceptual_roughness,
            metallic: manifest.metallic,
            emissive: if manifest.emissive_is_textured {
                LinearRgba::WHITE
            } else {
                LinearRgba::BLACK
            },
            ..default()
        })
    }

    fn extensions(&self) -> &[&str] {
        &["ron"]
    }
}

// ---- plugin -------------------------------------------------------------

/// Registers [`MaterialRonLoader`] so the game can
/// `asset_server.load::<StandardMaterial>("materials/<name>/material.ron")`.
///
/// `StandardMaterial`'s asset type and `Assets<StandardMaterial>` are already
/// initialized by `DefaultPlugins` (`MaterialPlugin`), so this plugin only needs
/// `register_asset_loader` — no `init_asset`. The plugin is **infrastructure
/// only**: it registers the loader and nothing else — nothing in the scene is
/// spawned or queried here. Wiring a baked material into a renderer is the
/// consumer's job.
pub struct BakedMaterialPlugin;

impl Plugin for BakedMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.register_asset_loader(MaterialRonLoader);
    }
}
