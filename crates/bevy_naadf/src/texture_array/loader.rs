//! [`TextureArrayLoader`] — bakes a `*.texarray.ron` definition into a 2D-array
//! [`Image`].
//!
//! The loader is registered unconditionally, so it backs *both* paths:
//! * **Loaded** (default / wasm): it is the runtime loader —
//!   `asset_server.load::<Image>("…​.texarray.ron")` runs this and yields an
//!   uncompressed RGBA8 2D-array `Image` directly.
//! * **Processed** (the `bake` binary): the `AssetProcessor` runs it as the
//!   *load* half of the pipeline, then hands the `Image` to
//!   [`crate::texture_array::TextureArrayBasisSaver`].
//!
//! The actual channel-combine + layer-pack work lives in the pure, synchronous
//! [`bake_texture_array`] so it can be unit-tested without an `AssetServer`.

use std::collections::HashMap;

use bevy::asset::{io::Reader, AssetLoader, LoadContext, ReadAssetBytesError, RenderAssetUsages};
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use image::RgbaImage;

use crate::texture_array::def::{TexArrayFormat, TextureArrayDef};

/// The `AssetLoader` for `*.texarray.ron` definitions. Output asset: [`Image`].
#[derive(Default, TypePath)]
pub struct TextureArrayLoader;

/// Everything that can go wrong turning a `*.texarray.ron` into an [`Image`].
#[derive(Debug, thiserror::Error)]
pub enum TextureArrayLoaderError {
    /// Failed to read the definition file's bytes.
    #[error("could not read the texarray definition: {0}")]
    Io(#[from] std::io::Error),
    /// The definition file is not valid RON / does not match the schema.
    #[error("could not parse the texarray definition (RON): {0}")]
    Ron(#[from] ron::error::SpannedError),
    /// A referenced source texture could not be read.
    ///
    /// Inside the `bake` binary this most often means the source PNG is missing
    /// a `Load`-action `.meta` sidecar and got Basis-compressed by the default
    /// PNG processor — see [`crate::texture_array`].
    #[error("could not read source texture: {0}")]
    ReadAsset(#[from] ReadAssetBytesError),
    /// A referenced source texture could not be decoded as PNG/JPEG.
    #[error("could not decode source texture `{path}`: {source}")]
    Decode {
        /// The offending source path.
        path: String,
        /// The underlying `image` crate error.
        source: image::ImageError,
    },
    /// The definition has an empty `elements` list — there is no array to bake.
    #[error("texarray definition has no `elements` (nothing to bake)")]
    NoElements,
    /// Two source textures disagree on dimensions; the baker will not guess a
    /// resize.
    #[error(
        "source `{path}` is {width}x{height}, but the array's first element \
         established {expected_width}x{expected_height} — all sources must match"
    )]
    SizeMismatch {
        /// The offending source path.
        path: String,
        /// Its actual width.
        width: u32,
        /// Its actual height.
        height: u32,
        /// The width every source is required to have.
        expected_width: u32,
        /// The height every source is required to have.
        expected_height: u32,
    },
}

impl AssetLoader for TextureArrayLoader {
    type Asset = Image;
    type Settings = ();
    type Error = TextureArrayLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Image, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let def: TextureArrayDef = ron::de::from_bytes(&bytes)?;

        // Resolve every *distinct* source texture exactly once — a colour map
        // feeding three channels of an element is read a single time.
        // `read_asset_bytes` also registers each source as a process
        // dependency, so the `AssetProcessor` re-bakes when a source changes.
        let mut sources: HashMap<String, RgbaImage> = HashMap::new();
        for element in &def.elements {
            for channel in element.channels() {
                if sources.contains_key(&channel.input) {
                    continue;
                }
                // `read_asset_bytes` wants an owned `AssetPath<'static>` here —
                // a borrowed path would tie the load future to `def`'s lifetime.
                let raw = load_context.read_asset_bytes(channel.input.clone()).await?;
                let decoded = image::load_from_memory(&raw)
                    .map_err(|source| TextureArrayLoaderError::Decode {
                        path: channel.input.clone(),
                        source,
                    })?
                    .to_rgba8();
                sources.insert(channel.input.clone(), decoded);
            }
        }

        bake_texture_array(&def, &sources)
    }

    fn extensions(&self) -> &[&str] {
        &["texarray.ron"]
    }
}

/// Combine + pack a parsed [`TextureArrayDef`] and its already-decoded source
/// textures into a 2D-array [`Image`].
///
/// One [`Element`](crate::texture_array::Element) becomes one array layer; each
/// of the layer's R/G/B/A bytes is sampled from the named source channel and
/// optionally inverted. Pure and synchronous — the `AssetLoader` is a thin
/// async wrapper that just resolves `sources` first.
///
/// `sources` must contain an entry for every `input` path the definition names
/// (the loader guarantees this); a missing entry panics.
pub fn bake_texture_array(
    def: &TextureArrayDef,
    sources: &HashMap<String, RgbaImage>,
) -> Result<Image, TextureArrayLoaderError> {
    let first = def
        .elements
        .first()
        .ok_or(TextureArrayLoaderError::NoElements)?;
    // The first element's red source establishes the dimensions every other
    // source must match.
    let (width, height) = sources[&first.r.input].dimensions();

    let layer_len = width as usize * height as usize * 4;
    let mut data = vec![0u8; layer_len * def.elements.len()];

    for (layer_index, element) in def.elements.iter().enumerate() {
        let layer = &mut data[layer_index * layer_len..(layer_index + 1) * layer_len];
        for (out_channel, source) in element.channels().into_iter().enumerate() {
            let src = &sources[&source.input];
            if src.dimensions() != (width, height) {
                let (w, h) = src.dimensions();
                return Err(TextureArrayLoaderError::SizeMismatch {
                    path: source.input.clone(),
                    width: w,
                    height: h,
                    expected_width: width,
                    expected_height: height,
                });
            }
            let src_channel = source.channel.index();
            for (texel_index, pixel) in src.pixels().enumerate() {
                let value = pixel.0[src_channel];
                layer[texel_index * 4 + out_channel] =
                    if source.invert { 255 - value } else { value };
            }
        }
    }

    let format = match def.format {
        TexArrayFormat::Rgba8UnormSrgb => TextureFormat::Rgba8UnormSrgb,
        TexArrayFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
    };
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: def.elements.len() as u32,
        },
        TextureDimension::D2,
        data,
        format,
        // Keep the pixels CPU-side: the processor's saver reads them back, and
        // consumers may want to inspect the bake.
        RenderAssetUsages::all(),
    );
    // `D2` + `depth_or_array_layers > 1` is Bevy's representation of a 2D array;
    // pin the view dimension so it binds as `texture_2d_array` even for a
    // single-element (depth-1) array.
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..Default::default()
    });
    // Terrain atlases tile, so default to repeat addressing + linear filtering.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_array::def::{ChannelSource, Element, SourceChannel};

    /// A 2x2 RGBA image whose four texels are filled with the given bytes.
    fn img_2x2(texels: [[u8; 4]; 4]) -> RgbaImage {
        RgbaImage::from_raw(2, 2, texels.into_iter().flatten().collect()).unwrap()
    }

    fn channel(input: &str, channel: SourceChannel, invert: bool) -> ChannelSource {
        ChannelSource {
            input: input.to_string(),
            channel,
            invert,
        }
    }

    #[test]
    fn combines_channels_from_distinct_sources_with_invert() {
        // `color` carries RGB, `mask` carries a value we route into alpha and
        // invert (a height-as-depth style packing).
        let color = img_2x2([
            [10, 20, 30, 255],
            [11, 21, 31, 255],
            [12, 22, 32, 255],
            [13, 23, 33, 255],
        ]);
        let mask = img_2x2([[40, 0, 0, 0], [41, 0, 0, 0], [42, 0, 0, 0], [43, 0, 0, 0]]);
        let mut sources = HashMap::new();
        sources.insert("color".to_string(), color);
        sources.insert("mask".to_string(), mask);

        let def = TextureArrayDef {
            format: TexArrayFormat::Rgba8UnormSrgb,
            elements: vec![Element {
                r: channel("color", SourceChannel::R, false),
                g: channel("color", SourceChannel::G, false),
                b: channel("color", SourceChannel::B, false),
                a: channel("mask", SourceChannel::R, true),
            }],
        };

        let image = bake_texture_array(&def, &sources).unwrap();
        assert_eq!(image.texture_descriptor.size.width, 2);
        assert_eq!(image.texture_descriptor.size.height, 2);
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 1);
        assert_eq!(image.texture_descriptor.dimension, TextureDimension::D2);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );

        let data = image.data.as_ref().unwrap();
        // Texel 0: RGB straight from `color`, A = 255 - mask.R = 255 - 40.
        assert_eq!(&data[0..4], &[10, 20, 30, 215]);
        // Texel 3: RGB from `color`, A = 255 - 43.
        assert_eq!(&data[12..16], &[13, 23, 33, 212]);
    }

    #[test]
    fn stacks_one_layer_per_element_in_order() {
        let a = img_2x2([[1, 0, 0, 0]; 4]);
        let b = img_2x2([[2, 0, 0, 0]; 4]);
        let mut sources = HashMap::new();
        sources.insert("a".to_string(), a);
        sources.insert("b".to_string(), b);

        let layer = |input: &str| Element {
            r: channel(input, SourceChannel::R, false),
            g: channel(input, SourceChannel::R, false),
            b: channel(input, SourceChannel::R, false),
            a: channel(input, SourceChannel::R, false),
        };
        let def = TextureArrayDef {
            format: TexArrayFormat::Rgba8Unorm,
            elements: vec![layer("a"), layer("b")],
        };

        let image = bake_texture_array(&def, &sources).unwrap();
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 2);
        let data = image.data.as_ref().unwrap();
        // Layer 0 is all `1`s, layer 1 is all `2`s — contiguous, layer-major.
        assert!(data[..16].iter().all(|&b| b == 1));
        assert!(data[16..].iter().all(|&b| b == 2));
        assert_eq!(
            image.texture_view_descriptor.as_ref().unwrap().dimension,
            Some(TextureViewDimension::D2Array)
        );
    }

    #[test]
    fn rejects_mismatched_source_dimensions() {
        let small = img_2x2([[0; 4]; 4]);
        let big = RgbaImage::from_raw(4, 4, vec![0u8; 4 * 4 * 4]).unwrap();
        let mut sources = HashMap::new();
        sources.insert("small".to_string(), small);
        sources.insert("big".to_string(), big);

        let def = TextureArrayDef {
            format: TexArrayFormat::Rgba8Unorm,
            elements: vec![Element {
                r: channel("small", SourceChannel::R, false),
                g: channel("big", SourceChannel::G, false),
                b: channel("small", SourceChannel::B, false),
                a: channel("small", SourceChannel::A, false),
            }],
        };

        assert!(matches!(
            bake_texture_array(&def, &sources),
            Err(TextureArrayLoaderError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_empty_definition() {
        let def = TextureArrayDef {
            format: TexArrayFormat::Rgba8Unorm,
            elements: vec![],
        };
        assert!(matches!(
            bake_texture_array(&def, &HashMap::new()),
            Err(TextureArrayLoaderError::NoElements)
        ));
    }
}
