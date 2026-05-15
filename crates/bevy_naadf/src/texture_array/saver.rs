//! [`TextureArrayBasisSaver`] — the saver half of the texture-array
//! `AssetProcessor` (run by the `bake` binary): a baked 2D-array [`Image`] → a
//! Basis Universal supercompressed `.basis` file under `imported_assets/`.
//!
//! The saver feeds each array layer to basis-universal as its own "image", so
//! the result is a *multi-image* `.basis`. Bevy's runtime image loader
//! (`bevy_image`, `basis-universal` feature) reads a multi-image `.basis` back
//! as a `depth_or_array_layers = N` 2D texture and transcodes it per-GPU at
//! load — BC7 on desktop, ETC2/ASTC where available.
//!
//! Native-only: the basis-universal *encoder* is a native C++ build and the
//! `AssetProcessor` only ever runs at build time (see [`crate::texture_array`]
//! for why Basis is native-only). The encode itself lives in the pure
//! [`compress_array_to_basis`] so it is unit-testable without the `AssetSaver` /
//! `Writer` plumbing.

use basis_universal::{
    BasisTextureFormat, ColorSpace, Compressor, CompressorErrorCode, CompressorParams,
    UASTC_QUALITY_DEFAULT,
};
use bevy::asset::saver::{AssetSaver, SavedAsset};
use bevy::asset::{io::Writer, AssetPath, AsyncWriteExt};
use bevy::image::{Image, ImageFormat, ImageFormatSetting, ImageLoader, ImageLoaderSettings};
use bevy::reflect::TypePath;

/// The `AssetSaver` half of the texture-array `AssetProcessor`. Input asset:
/// [`Image`] (the 2D-array baked by [`crate::texture_array::TextureArrayLoader`]);
/// output: a Basis Universal `.basis` file, loaded back at runtime by
/// [`ImageLoader`].
#[derive(TypePath)]
pub struct TextureArrayBasisSaver;

/// Everything that can go wrong Basis-compressing a baked texture array.
#[derive(Debug, thiserror::Error)]
pub enum TextureArrayBasisSaverError {
    /// Failed to write the `.basis` bytes to the processed-asset writer.
    #[error("could not write the baked .basis: {0}")]
    Io(#[from] std::io::Error),
    /// The baked `Image` had no CPU-side pixel data to compress.
    #[error("cannot compress a texture array with uninitialized pixel data")]
    UninitializedImage,
    /// basis-universal rejected the encode.
    #[error("basis-universal compression failed: {0:?}")]
    Compression(CompressorErrorCode),
}

impl AssetSaver for TextureArrayBasisSaver {
    type Asset = Image;
    type Settings = ();
    type OutputLoader = ImageLoader;
    type Error = TextureArrayBasisSaverError;

    async fn save(
        &self,
        writer: &mut Writer,
        image: SavedAsset<'_, '_, Image>,
        _settings: &(),
        _asset_path: AssetPath<'_>,
    ) -> Result<ImageLoaderSettings, Self::Error> {
        let is_srgb = image.texture_descriptor.format.is_srgb();
        let basis = compress_array_to_basis(&image)?;
        writer.write_all(&basis).await?;

        // Tell the runtime `ImageLoader` the bytes are a Basis container; it
        // transcodes per-GPU. The multi-image `.basis` already carries the
        // layer count, so `array_layout` stays `None`.
        Ok(ImageLoaderSettings {
            format: ImageFormatSetting::Format(ImageFormat::Basis),
            is_srgb,
            sampler: image.sampler.clone(),
            asset_usage: image.asset_usage,
            texture_format: None,
            array_layout: None,
        })
    }
}

/// Basis-compress a baked 2D-array [`Image`] into multi-image `.basis` bytes.
///
/// One basis "image" is emitted per array layer (in layer order), at UASTC4x4
/// quality. Mipmaps are *not* generated — per-layer mip generation for arrays
/// is a follow-up; a single-mip array keeps the layer/byte layout unambiguous.
///
/// Expects `image` to be a `TextureDimension::D2` texture with an `Rgba8*`
/// format and initialized [`data`](Image::data) — exactly what
/// [`crate::texture_array::bake_texture_array`] produces.
pub fn compress_array_to_basis(image: &Image) -> Result<Vec<u8>, TextureArrayBasisSaverError> {
    let size = image.texture_descriptor.size;
    let width = size.width;
    let height = size.height;
    let layers = size.depth_or_array_layers;
    let data = image
        .data
        .as_ref()
        .ok_or(TextureArrayBasisSaverError::UninitializedImage)?;
    let layer_len = width as usize * height as usize * 4;

    let mut params = CompressorParams::new();
    params.set_basis_format(BasisTextureFormat::UASTC4x4);
    params.set_uastc_quality_level(UASTC_QUALITY_DEFAULT);
    params.set_color_space(if image.texture_descriptor.format.is_srgb() {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    });
    params.set_generate_mipmaps(false);

    params.resize_source_image_list(layers);
    for layer in 0..layers {
        let start = layer as usize * layer_len;
        let layer_data = &data[start..start + layer_len];
        params
            .source_image_mut(layer)
            .init(layer_data, width, height, 4);
    }

    let mut compressor = Compressor::new(4);
    // SAFETY: `params` is fully initialized above — a non-empty source image
    // list, a basis format, and a quality level — mirroring `bevy_image`'s own
    // `CompressedImageSaver`. The basis-universal bindings note only that
    // *invalid* params are UB.
    #[allow(unsafe_code)]
    unsafe {
        compressor.init(&params);
        compressor
            .process()
            .map_err(TextureArrayBasisSaverError::Compression)?;
    }
    Ok(compressor.basis_file().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use basis_universal::Transcoder;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    #[test]
    fn compresses_each_array_layer_as_its_own_basis_image() {
        let (width, height, layers) = (16u32, 16u32, 3u32);
        // Three visually distinct layers so the round-trip is not vacuous.
        let mut data = Vec::new();
        for layer in 0..layers {
            let value = 40 + layer as u8 * 70;
            data.extend(std::iter::repeat(value).take((width * height * 4) as usize));
        }
        let image = Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::all(),
        );

        let basis = compress_array_to_basis(&image).unwrap();

        let transcoder = Transcoder::new();
        assert!(
            transcoder.validate_header(&basis),
            "saver output is not a valid Basis container"
        );
        assert_eq!(
            transcoder.image_count(&basis),
            layers,
            "every array layer should become one basis image"
        );
        let info = transcoder.image_info(&basis, 0).unwrap();
        assert_eq!(info.m_orig_width, width);
        assert_eq!(info.m_orig_height, height);
    }

    #[test]
    fn rejects_uninitialized_image() {
        let image = Image::new_uninit(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::all(),
        );
        assert!(matches!(
            compress_array_to_basis(&image),
            Err(TextureArrayBasisSaverError::UninitializedImage)
        ));
    }
}
