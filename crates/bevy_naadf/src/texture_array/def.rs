//! The `*.texarray.ron` texture-array definition format.
//!
//! A definition names a target pixel [`format`](TextureArrayDef::format) and a
//! list of [`elements`](TextureArrayDef::elements) — one per array layer. Each
//! element wires its four output channels (R, G, B, A) to a source texture +
//! channel, optionally inverted. So one definition is at once a channel
//! *combiner* (gather four arbitrary source channels into one RGBA texel) and
//! an array *packer* (stack the elements into a 2D-array texture) — exactly
//! what terrain rendering wants to bind as a single `texture_2d_array`.
//!
//! The shape mirrors the sketch in the task brief; the only deviation is that
//! an element's four channel sources are named `r` / `g` / `b` / `a` fields
//! rather than a positional `inputs: [..; 4]` list, so a definition can never
//! silently transpose two channels.
//!
//! ```ron
//! // assets/textures/terrain.texarray.ron
//! (
//!     format: Rgba8UnormSrgb,
//!     elements: [
//!         // layer 0: rock — albedo RGB from one map, height packed into alpha
//!         (
//!             r: (input: "textures/rock_color.png",  channel: R),
//!             g: (input: "textures/rock_color.png",  channel: G),
//!             b: (input: "textures/rock_color.png",  channel: B),
//!             a: (input: "textures/rock_height.png", channel: R, invert: true),
//!         ),
//!     ],
//! )
//! ```

use serde::Deserialize;

/// A parsed `*.texarray.ron` definition — the root the texture-array
/// loader/processor consumes (see [`crate::texture_array`]).
#[derive(Debug, Clone, Deserialize)]
pub struct TextureArrayDef {
    /// Pixel format of the baked array. In the `bake` (processed) pass this is
    /// the format basis-universal compresses *from*; on the loaded path it is
    /// the format of the `Image` directly (see `crate::texture_array::saver`).
    pub format: TexArrayFormat,
    /// One entry per array layer, in layer order. Every channel source of every
    /// element must resolve to the same width/height — the loader errors out on
    /// a mismatch rather than guessing a resize.
    pub elements: Vec<Element>,
}

/// The (uncompressed) pixel format of the baked array.
///
/// The choice is really sRGB-vs-linear: colour maps (albedo, …) want
/// [`Rgba8UnormSrgb`](TexArrayFormat::Rgba8UnormSrgb); data maps (height,
/// roughness, normals, …) want [`Rgba8Unorm`](TexArrayFormat::Rgba8Unorm) so
/// the GPU does not apply a gamma curve to non-colour data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TexArrayFormat {
    /// 8-bit RGBA, sRGB-encoded — colour data.
    Rgba8UnormSrgb,
    /// 8-bit RGBA, linear — non-colour ("data") channels.
    Rgba8Unorm,
}

/// One array layer: an RGBA texel assembled from up to four independent source
/// channels.
#[derive(Debug, Clone, Deserialize)]
pub struct Element {
    /// Source for the baked **red** channel.
    pub r: ChannelSource,
    /// Source for the baked **green** channel.
    pub g: ChannelSource,
    /// Source for the baked **blue** channel.
    pub b: ChannelSource,
    /// Source for the baked **alpha** channel.
    pub a: ChannelSource,
}

impl Element {
    /// The element's four channel sources in baked-channel order (R, G, B, A) —
    /// so callers can iterate `enumerate()` to get `(output_channel_index,
    /// source)`.
    pub fn channels(&self) -> [&ChannelSource; 4] {
        [&self.r, &self.g, &self.b, &self.a]
    }
}

/// Where a single baked channel's value comes from: one channel of one source
/// texture, optionally inverted.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelSource {
    /// Asset path of the source texture, relative to the asset root
    /// (`src/assets/`). Decoded as PNG or JPEG.
    ///
    /// When baked via the `bake` binary the source must opt out of the default
    /// PNG→Basis processor with a `Load`-action `.meta` sidecar — the baker
    /// needs the raw, uncompressed pixels. See [`crate::texture_array`].
    pub input: String,
    /// Which channel of [`input`](ChannelSource::input) to sample.
    pub channel: SourceChannel,
    /// Invert the sampled value (`255 - v`) before writing it — e.g. a height
    /// map authored as depth, or a smoothness map wanted as roughness.
    #[serde(default)]
    pub invert: bool,
}

/// A colour channel of a source texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SourceChannel {
    /// Red.
    R,
    /// Green.
    G,
    /// Blue.
    B,
    /// Alpha.
    A,
}

impl SourceChannel {
    /// Byte offset of this channel within an `Rgba8` texel (`R → 0 … A → 3`).
    pub fn index(self) -> usize {
        match self {
            SourceChannel::R => 0,
            SourceChannel::G => 1,
            SourceChannel::B => 2,
            SourceChannel::A => 3,
        }
    }
}
