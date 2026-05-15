//! Texture-array asset pipeline — bakes `*.texarray.ron` channel/array
//! definitions into 2D-array textures bindable as `texture_2d_array` in WGSL.
//!
//! One definition file is both a **channel combiner** (gather four arbitrary
//! source-texture channels into one RGBA texel, optionally inverting each) and
//! an **array packer** (stack those texels, one definition element per array
//! layer) — see [`def`] for the format. The intended consumer is terrain
//! raymarching, which wants a single 2D-array texture binding rather than a
//! pile of loose maps.
//!
//! # Two paths, one definition
//!
//! The same `*.texarray.ron` drives two paths:
//!
//! * **Loaded** (the default; the *only* path on wasm) — [`TextureArrayLoader`]
//!   is a normal runtime `AssetLoader`:
//!   `asset_server.load::<Image>("…​.texarray.ron")` bakes the definition into an
//!   *uncompressed* RGBA8 2D-array [`Image`] on load. This is the path the
//!   production app and the e2e harness take.
//! * **Processed** (the `bake` binary, native) — `src/bin/bake.rs` runs a
//!   headless `AssetMode::Processed` app; Bevy's `AssetProcessor` runs the same
//!   loader, then [`TextureArrayBasisSaver`] Basis-Universal-supercompresses the
//!   array into a `.basis` file under `imported_assets/`. Bevy's runtime
//!   transcoder (the native-only `bevy/basis-universal` feature) decodes it
//!   per-GPU at load — BC7 on desktop, ETC2/ASTC where available.
//!
//! A Bevy `AssetProcessor` is *app-global* — it would route every asset (all
//! the render shaders included) through `imported_assets/` and race the
//! processor against the render pipeline's shader loads. So it is kept out of
//! the render app entirely and confined to the dedicated `bake` binary
//! (`cargo run --bin bake` / `just bake`); the production app and the e2e
//! harness stay `AssetMode::Unprocessed`.
//!
//! Basis is **native-only**: the `basis-universal` C++ encoder does not
//! cross-compile to the `wasm32-unknown-unknown` web target (see this crate's
//! `Cargo.toml`), so the web build always takes the loaded path. Getting a
//! *compressed* array onto the web would need either a wasm C sysroot or a
//! `bevy_voxel_world`-style dual-format (BC7 + ETC2) pre-transcoding baker — a
//! deliberate follow-up, not wired here.
//!
//! ## Source textures and the `bake` binary
//!
//! Inside `AssetMode::Processed` a loader reads its dependencies through the
//! *processed* asset reader, and Bevy's default `png` processor is
//! `CompressedImageSaver` — so a bare `foo.png` would reach the baker already
//! Basis-compressed, useless for channel-packing. Each source texture a
//! `*.texarray.ron` references must therefore ship a `Load`-action `.meta`
//! sidecar opting it out of compression (the baker needs raw pixels):
//!
//! ```ron
//! // assets/textures/rock_color.png.meta
//! (
//!     meta_format_version: "1.0",
//!     asset: Load(
//!         loader: "bevy_image::image_loader::ImageLoader",
//!         settings: (
//!             format: FromExtension,
//!             is_srgb: true,
//!             sampler: Default,
//!             asset_usage: ("MAIN_WORLD | RENDER_WORLD"),
//!             texture_format: None,
//!             array_layout: None,
//!         ),
//!     ),
//! )
//! ```
//!
//! This is inert in unprocessed mode, so the sidecars are harmless on the
//! default build. See `assets/textures/*.png.meta` for working examples.
//!
//! # Binding to a shader
//!
//! The baked [`Image`] is a `TextureDimension::D2` texture with
//! `depth_or_array_layers = elements.len()` and a `D2Array` texture view — bind
//! it as a 2D-array and index the layer in WGSL:
//!
//! ```ignore
//! #[derive(Asset, AsBindGroup, TypePath, Clone)]
//! struct TerrainMaterial {
//!     #[texture(0, dimension = "2d_array")]
//!     #[sampler(1)]
//!     layers: Handle<Image>, // load("textures/terrain.texarray.ron")
//! }
//! // WGSL: textureSample(layers, layers_sampler, uv, layer_index);
//! ```

mod def;
mod loader;
#[cfg(not(target_arch = "wasm32"))]
mod saver;

pub use def::{ChannelSource, Element, SourceChannel, TexArrayFormat, TextureArrayDef};
pub use loader::{bake_texture_array, TextureArrayLoader, TextureArrayLoaderError};
#[cfg(not(target_arch = "wasm32"))]
pub use saver::{compress_array_to_basis, TextureArrayBasisSaver, TextureArrayBasisSaverError};

use bevy::asset::AssetApp;
use bevy::prelude::*;

/// Registers the `*.texarray.ron` asset loader (every target / both modes) and,
/// on native, the Basis-Universal `AssetProcessor` that runs under
/// `AssetMode::Processed`.
pub struct TextureArrayPlugin;

impl Plugin for TextureArrayPlugin {
    fn build(&self, app: &mut App) {
        // The loader backs the unprocessed runtime path *and* the load half of
        // the processed pipeline, so it is always registered.
        app.register_asset_loader(TextureArrayLoader);

        #[cfg(not(target_arch = "wasm32"))]
        {
            use bevy::asset::processor::LoadTransformAndSave;
            use bevy::asset::transformer::IdentityAssetTransformer;

            // The `Process` impl: run `TextureArrayLoader`, pass the baked
            // `Image` straight through (identity transform), then Basis-compress
            // it with `TextureArrayBasisSaver`.
            type TextureArrayProcessor = LoadTransformAndSave<
                TextureArrayLoader,
                IdentityAssetTransformer<Image>,
                TextureArrayBasisSaver,
            >;

            // Both calls are inert unless the app runs in `AssetMode::Processed`
            // (i.e. the `bake` binary): without the `AssetProcessor` resource
            // they are no-ops, so adding `TextureArrayPlugin` to the render app
            // is harmless — it just registers the loader there.
            app.register_asset_processor::<TextureArrayProcessor>(TextureArrayBasisSaver.into());
            app.set_default_asset_processor::<TextureArrayProcessor>("texarray.ron");
        }
    }
}
