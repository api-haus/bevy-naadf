//! `Framebuffer` — a format-normalised CPU view over a captured window
//! framebuffer, plus the region/statistic helpers the per-batch gates use
//! (`e2e-render-test.md` §5.3, §6.2, §7).
//!
//! The window surface format is the platform/driver's choice — commonly
//! `Bgra8UnormSrgb` on Vulkan/Linux, not `Rgba8UnormSrgb` (`e2e-render-test.md`
//! R7). [`Framebuffer::from_image`] **branches on `Image.texture_descriptor.
//! format`** and normalises the channel order so the rest of the harness can
//! treat the buffer as a uniform `&[[u8; 4]]` RGBA grid — it must not assume
//! RGBA.

use std::hash::{Hash, Hasher};

use bevy::image::Image;
use bevy::render::render_resource::TextureFormat;

/// A rectangle in the framebuffer, in physical pixels. `[x0, y0]` inclusive,
/// `[x1, y1]` exclusive. Constructed from fractional screen coords keyed off
/// the *actual* readback dimensions so a HiDPI scale-factor difference does not
/// silently misalign the gate rects (`e2e-render-test.md` §6.5, R5/R7).
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    /// Build a `Rect` from fractional (0..1) screen coords against a concrete
    /// framebuffer size. Clamps to the framebuffer bounds.
    pub fn from_fractional(fb: &Framebuffer, fx0: f32, fy0: f32, fx1: f32, fy1: f32) -> Self {
        let w = fb.width() as f32;
        let h = fb.height() as f32;
        let x0 = (fx0 * w) as u32;
        let y0 = (fy0 * h) as u32;
        let x1 = ((fx1 * w) as u32).max(x0 + 1).min(fb.width());
        let y1 = ((fy1 * h) as u32).max(y0 + 1).min(fb.height());
        Self { x0, y0, x1, y1 }
    }
}

/// A captured framebuffer, normalised to RGBA `u8` regardless of the platform
/// surface format.
pub struct Framebuffer {
    /// Row-major `y * width + x`, RGBA channel order, `u8` per channel.
    data: Vec<[u8; 4]>,
    width: u32,
    height: u32,
}

impl Framebuffer {
    /// Build a `Framebuffer` from a captured `Screenshot` `Image`, normalising
    /// the channel order from the reported `texture_descriptor.format`.
    ///
    /// Honours `e2e-render-test.md` R7: branches on the format, handles both
    /// `Rgba8*` and `Bgra8*` (a window surface is commonly BGRA on
    /// Vulkan/Linux). Returns `Err` for an unexpected format or a missing
    /// `data` payload rather than silently producing a channel-swapped buffer.
    pub fn from_image(image: &Image) -> Result<Self, String> {
        let desc = &image.texture_descriptor;
        let width = desc.size.width;
        let height = desc.size.height;
        let Some(bytes) = image.data.as_ref() else {
            return Err("captured screenshot Image has no `data` payload".to_string());
        };
        let expected = (width as usize) * (height as usize) * 4;
        if bytes.len() < expected {
            return Err(format!(
                "captured screenshot too small: {} bytes, expected >= {} for {}x{} RGBA8",
                bytes.len(),
                expected,
                width,
                height
            ));
        }

        // Which channel order do the source bytes use? The window surface is
        // the platform's choice — branch on it, do not assume RGBA (R7).
        let swap_rb = match desc.format {
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => false,
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => true,
            other => {
                return Err(format!(
                    "unexpected window surface format {other:?} — Framebuffer::from_image \
                     only normalises Rgba8*/Bgra8* (e2e-render-test.md assumption); \
                     add a branch for this format"
                ));
            }
        };

        let mut data = Vec::with_capacity((width as usize) * (height as usize));
        for px in bytes[..expected].chunks_exact(4) {
            if swap_rb {
                // BGRA source → RGBA.
                data.push([px[2], px[1], px[0], px[3]]);
            } else {
                data.push([px[0], px[1], px[2], px[3]]);
            }
        }

        Ok(Self {
            data,
            width,
            height,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA pixel at `(x, y)`; `[0,0,0,255]` (opaque black) out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 255];
        }
        self.data[(y * self.width + x) as usize]
    }

    /// The mean RGBA over `rect`, each channel in `0.0..=255.0`.
    pub fn region_mean(&self, rect: Rect) -> [f32; 4] {
        let mut acc = [0.0f64; 4];
        let mut n = 0u64;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let p = self.pixel(x, y);
                for c in 0..4 {
                    acc[c] += p[c] as f64;
                }
                n += 1;
            }
        }
        if n == 0 {
            return [0.0; 4];
        }
        [
            (acc[0] / n as f64) as f32,
            (acc[1] / n as f64) as f32,
            (acc[2] / n as f64) as f32,
            (acc[3] / n as f64) as f32,
        ]
    }

    /// Rec.709-ish luminance of an RGB(A) triple, channels in `0.0..=255.0`.
    pub fn luminance(rgba: [f32; 4]) -> f32 {
        0.2126 * rgba[0] + 0.7152 * rgba[1] + 0.0722 * rgba[2]
    }

    /// Mean luminance over `rect` (`0.0..=255.0`).
    pub fn region_luminance(&self, rect: Rect) -> f32 {
        Self::luminance(self.region_mean(rect))
    }

    /// Fraction (`0.0..=1.0`) of pixels in `rect` whose luminance exceeds
    /// `thresh` (`0.0..=255.0`).
    pub fn fraction_brighter_than(&self, rect: Rect, thresh: f32) -> f32 {
        let mut bright = 0u64;
        let mut n = 0u64;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let p = self.pixel(x, y);
                let lum =
                    Self::luminance([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
                if lum > thresh {
                    bright += 1;
                }
                n += 1;
            }
        }
        if n == 0 {
            0.0
        } else {
            bright as f32 / n as f32
        }
    }

    /// Whether every channel of `a` is within `tol` of `b` (channels in
    /// `0.0..=255.0`).
    pub fn is_near(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
        (0..4).all(|c| (a[c] - b[c]).abs() <= tol)
    }

    /// Mean per-pixel RGB delta between two same-sized framebuffers
    /// (`0.0..=255.0`) — the Batch-6 temporal-stability metric. Returns a large
    /// value if the dimensions differ (a size change *is* a failure).
    pub fn mean_pixel_delta(&self, other: &Framebuffer) -> f32 {
        if self.width != other.width || self.height != other.height {
            return f32::MAX;
        }
        let mut acc = 0.0f64;
        let n = self.data.len().max(1);
        for (a, b) in self.data.iter().zip(other.data.iter()) {
            for c in 0..3 {
                acc += (a[c] as f64 - b[c] as f64).abs();
            }
        }
        (acc / (n as f64 * 3.0)) as f32
    }

    /// A stable hash of the framebuffer bytes — the optional §6.1 stability
    /// tripwire. Stable across runs (`DefaultHasher` over the raw bytes +
    /// dimensions); asserted-equal only where the design says "image
    /// unchanged" (B3, B4).
    pub fn stability_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.width.hash(&mut hasher);
        self.height.hash(&mut hasher);
        for px in &self.data {
            px.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The readback sanity floor (`e2e-render-test.md` §7): the framebuffer
    /// must not be degenerate — not all-identical-pixels (a stuck clear
    /// colour), and it must contain both some dark and some bright pixels
    /// (geometry + sky present). Returns `Err` with a clear message on a
    /// degenerate frame.
    pub fn check_not_degenerate(&self) -> Result<(), String> {
        if self.data.is_empty() {
            return Err("framebuffer is empty — the render graph produced no output".to_string());
        }
        let first = self.data[0];
        let all_same = self.data.iter().all(|p| *p == first);
        if all_same {
            return Err(format!(
                "framebuffer is uniformly {first:?} — the render graph produced no output \
                 (a node likely silently early-returned on a failed pipeline)"
            ));
        }

        // Some dark and some bright pixels must both exist: a scene with
        // geometry (dark, pre-GI) and sky (mid-bright) cannot be all-one-band.
        let mut has_dark = false;
        let mut has_bright = false;
        for p in &self.data {
            let lum = Self::luminance([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
            if lum < 32.0 {
                has_dark = true;
            }
            if lum > 64.0 {
                has_bright = true;
            }
            if has_dark && has_bright {
                return Ok(());
            }
        }
        Err(format!(
            "framebuffer has no contrast (has_dark={has_dark}, has_bright={has_bright}) — \
             expected both dark geometry and a brighter sky"
        ))
    }
}
