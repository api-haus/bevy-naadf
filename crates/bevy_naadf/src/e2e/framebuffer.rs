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
use std::path::Path;

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

/// The "not pitch black" luminance floor (channels `0.0..=255.0`) for the
/// global liveness gate ([`Framebuffer::check_luminance_alive`]). A pixel whose
/// Rec.709 luminance is at or below this is treated as pitch black. Kept small —
/// it is a "the pixel received *some* light" floor, not a brightness threshold:
/// the atmosphere-tinted sky, the emissive block, and any GI-lit geometry all
/// sit comfortably above it; only the literal clear-colour-black background and
/// fully-unlit pre-GI geometry fall at or below it.
pub const NON_BLACK_EPS: f32 = 2.0;

/// The minimum non-black fraction for the **GI-lit** batches (Batch 5 onward).
///
/// The user asked for "most of the pixels are not pitch black … at least 50%, i
/// guess" — 50% is the floor of that intent. That target describes the *GI-lit*
/// scene: once the GI bounce light reaches the screen (Batch 6 — see
/// [`GI_LIT_BATCH`]; the B5-vs-B6 milestone settled the visible bounce to B6),
/// the dark diffuse geometry is no longer pitch black.
///
/// **Recalibrated (2026-05-14, e2e test-scene expansion):** the test scene was
/// expanded with five emissive blocks + more geometry, and the pre-GI non-black
/// fraction at the re-framed e2e pose rose to ~69.1% (was ~41%).
///
/// **Re-measured (2026-05-15, Batch-6 TAA-path black-frame fix):** with the
/// `GpuTaaParams` WGSL/`#[repr(C)]` layout mismatch fixed, the TAA path executes
/// and the blit reads a real `taa_sample_accum` — the e2e frame is no longer
/// black, measuring **69.3%** non-black (sky + the five emissive blocks; the GI
/// bounce onto dark diffuse geometry was still weak at that point).
///
/// **Re-measured (2026-05-15, GI-bounce visibility fix):** the second
/// `vec3`-then-scalar uniform-layout bug — this one in `GpuGiParams` — was
/// found and fixed (the `sun_color` `vec3` followed by `screen_width` shifted
/// every GI-uniform scalar 4 bytes early, so `bucket_count == 0` ⇒
/// `clear_buckets_and_calc_mask` populated nothing ⇒ the whole `sampleRefine →
/// spatialResampling` reservoir chain was dead). With it fixed (and the e2e
/// frame budget raised to 96 so NAADF's temporal-accumulation GI converges) the
/// GI bounce now visibly lights the diffuse geometry — the e2e frame measures
/// **99.2%** non-black. Set to **0.95** — just below the measured value, a real
/// regression tripwire on the GI-lit fraction (it trips hard if the GI chain
/// regresses to the pre-fix ~69% sky+emissive-only state), not a rubber stamp.
pub const MIN_NON_BLACK_FRACTION_GI: f32 = 0.95;

/// The minimum non-black fraction for the **pre-GI** batches (Batch 1–4).
///
/// Before GI bounce lands, the scene is *correctly* mostly dark: only the
/// atmosphere-tinted sky and the emissive blocks are lit; the non-emissive
/// diffuse voxel geometry is pitch black by design (`10-impl-b.md` Batch 2:
/// "pre-GI a non-emissive diffuse block should be near-black"). Demanding the
/// full GI-lit threshold here would be a false failure. This floor covers
/// **Batch 1-5** ([`GI_LIT_BATCH`] is `6` — the B5-vs-B6 milestone settled the
/// visible bounce to Batch 6; B5's GI consumers run but their pre-B6
/// contribution is negligible, so B5's frame is still pre-GI-like).
///
/// **Recalibrated (2026-05-14, e2e test-scene expansion):** the expanded scene
/// (five emissive blocks distributed through the volume + the atmosphere-tinted
/// sky band) measures **69.1% non-black** at the re-framed e2e pose pre-GI (was
/// ~41% with the old single-emissive scene). This floor is set to **0.50** —
/// comfortably below the measured 69.1% so it is a real "the screen isn't dead,
/// the sky and the five emissive blocks are rendering" liveness check, not a
/// rubber stamp, while still catching the failure where the blit/sky/first-hit
/// node silently dropped a large part of the frame. When Batch 5 lands,
/// [`MIN_NON_BLACK_FRACTION_GI`] takes over.
pub const MIN_NON_BLACK_FRACTION_PRE_GI: f32 = 0.50;

/// The batch at which the GI bounce lights the scene and the full
/// [`MIN_NON_BLACK_FRACTION_GI`] liveness threshold becomes a hard gate. Below
/// it the gate uses [`MIN_NON_BLACK_FRACTION_PRE_GI`].
///
/// **This is `6`, not `5`.** `09-design-b.md` §11 Batch 5 step 15 claimed the
/// GI bounce becomes visible at end-of-B5, but the Batch-5 verification (see
/// `10-impl-b.md` Batch-5 section + `gates::assert_batch_5`) settled the
/// B5-vs-B6 question: the visible-bounce milestone genuinely moves to **Batch
/// 6**. B5's GI consumers (`renderSpatialResampling` / `renderDenoiseSplit`)
/// run and write `final_color`, but the 12-iteration reservoir loop reads the
/// `renderSampleRefine` refine buffers, which are correct-but-empty until B6
/// wires `taa_dist_min_max`; the spatial pass's independent sun sample
/// contributes negligibly in the enclosed e2e test scene (the whole-frame
/// non-black fraction stays bit-identical at 69.1% through B5). So B5 keeps the
/// pre-GI floor — the honest regime — and B6 is the first batch the GI hard
/// gate applies to.
pub const GI_LIT_BATCH: u32 = 6;

/// The non-black-fraction threshold for `batch` — [`MIN_NON_BLACK_FRACTION_GI`]
/// (0.60) from [`GI_LIT_BATCH`] (`6`) on, [`MIN_NON_BLACK_FRACTION_PRE_GI`]
/// (0.50) before it. See [`Framebuffer::check_luminance_alive`].
pub fn min_non_black_fraction(batch: u32) -> f32 {
    if batch >= GI_LIT_BATCH {
        MIN_NON_BLACK_FRACTION_GI
    } else {
        MIN_NON_BLACK_FRACTION_PRE_GI
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

    /// Build a `Framebuffer` directly from a row-major `RGBA u8` byte
    /// vector. Used by oracle / regression gates that load a previously-saved
    /// PNG back from disk for comparison (the standard
    /// [`Framebuffer::from_image`] path expects a Bevy `Image` carrying a
    /// reported texture format, which is overkill when we already have RGBA
    /// bytes in hand).
    pub fn from_raw_rgba(data: Vec<[u8; 4]>, width: u32, height: u32) -> Self {
        debug_assert_eq!(
            data.len(),
            (width as usize) * (height as usize),
            "from_raw_rgba: data length must equal width*height",
        );
        Self {
            data,
            width,
            height,
        }
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

    /// Maximum-of-channel-means over `rect`. Returns the largest of
    /// `(mean_R, mean_G, mean_B)`, each in `0.0..=255.0`. Useful for
    /// "the frame has at least one colored channel above floor X" gates
    /// where Rec.709 luminance is too lossy (a green-only frame has
    /// `lum ≈ 180` but R+B near zero; a colorless dark-blue-gray frame has
    /// `lum ≈ 10`).
    ///
    /// Added by `web-vox-color-divergence` (2026-05-18) Decision 4 to catch
    /// the near-black-but-structurally-correct regression class the
    /// luminance-only gate at `vox_e2e.rs:402-433` and the SSIM-only gate
    /// at `vox_web_parity.rs:117-190` are blind to.
    pub fn region_channel_max(&self, rect: Rect) -> f32 {
        let m = self.region_mean(rect);
        m[0].max(m[1]).max(m[2])
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

    /// Count of pixels within `rect` whose Rec.709 luminance is **strictly
    /// below** `threshold` (channels `0.0..=255.0`). Pass `None` to scan the
    /// whole framebuffer.
    ///
    /// Added by vox-gpu-rewrite W5.3-fix Stage 1.5 for the
    /// `--vox-gpu-construction` gate's near-black-pixel assertion: when the
    /// W5 GPU producer chain mis-handles mixed blocks (CAS collision class
    /// of regression — see `docs/orchestrate/vox-gpu-rewrite/06-diagnostic-inversion.md`),
    /// scattered "hole" pixels appear where solid walls should render. A
    /// correctly-rendered Oasis frame has architectural detail at varying
    /// luminances but very few near-zero "hole" pixels; an inverted frame
    /// has many. `count_pixels_with_luminance_below(fb, None, T) > FLOOR`
    /// catches the inversion class directly.
    pub fn count_pixels_with_luminance_below(&self, rect: Option<Rect>, threshold: f32) -> usize {
        let rect = rect.unwrap_or(Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        });
        let mut count = 0usize;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let p = self.pixel(x, y);
                let lum =
                    Self::luminance([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
                if lum < threshold {
                    count += 1;
                }
            }
        }
        count
    }

    /// Fraction (`0.0..=1.0`) of the *whole* framebuffer whose pixels are **not
    /// pitch black** — luminance strictly above `eps` (`0.0..=255.0`).
    ///
    /// Backs the global "the scene isn't mostly dead" liveness gate
    /// (`e2e-render-test.md` §6 / Implementation log): a frame where almost
    /// every pixel is pitch black means the render graph delivered essentially
    /// nothing. `eps` is a small "not pitch black" floor — anything above it
    /// counts as lit (sky, geometry, the emissive block).
    pub fn non_black_fraction(&self, eps: f32) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let mut lit = 0u64;
        for p in &self.data {
            let lum = Self::luminance([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
            if lum > eps {
                lit += 1;
            }
        }
        lit as f32 / self.data.len() as f32
    }

    /// Write the framebuffer to `path` as a standard 8-bit sRGB **RGB** PNG.
    ///
    /// The bytes are already format-normalised to RGBA channel order by
    /// [`Framebuffer::from_image`] (R7 — the platform surface format, commonly
    /// `Bgra8*` on Vulkan/Linux, is decoded there), so this is a straight
    /// encode. The alpha channel is dropped: on this render path it carries the
    /// blit weight, not coverage — mirroring Bevy's own `save_to_disk`, which
    /// `to_rgb8()`s for exactly that reason — so an RGB PNG renders correctly
    /// when an agent `Read`s it.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        let mut rgb = Vec::with_capacity(self.data.len() * 3);
        for px in &self.data {
            rgb.push(px[0]);
            rgb.push(px[1]);
            rgb.push(px[2]);
        }
        let buf: image::RgbImage = image::ImageBuffer::from_raw(self.width, self.height, rgb)
            .ok_or_else(|| {
                format!(
                    "RGB buffer size mismatch for {}x{} framebuffer",
                    self.width, self.height
                )
            })?;
        buf.save_with_format(path, image::ImageFormat::Png)
            .map_err(|e| format!("could not write PNG to {}: {e}", path.display()))
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

    /// The global "the scene isn't mostly dead" liveness gate
    /// (`e2e-render-test.md` Implementation log — 2026-05-14): assert that a
    /// large fraction of the frame is **not pitch black** (luminance above
    /// [`NON_BLACK_EPS`]).
    ///
    /// The threshold is **batch-aware** ([`min_non_black_fraction`]): the user's
    /// "at least 50%" target describes the GI-lit scene, so 50% is a hard gate
    /// from [`GI_LIT_BATCH`] on; before GI bounce lands the scene is *correctly*
    /// mostly dark (only sky + the emissive block are lit) and the floor is the
    /// lower [`MIN_NON_BLACK_FRACTION_PRE_GI`] — still a real liveness check, not
    /// a rubber stamp.
    ///
    /// This is a global frame check, run alongside the degenerate-frame floor.
    /// Where [`check_not_degenerate`](Self::check_not_degenerate) only catches a
    /// *uniformly* dead frame, this catches the weaker failure where the render
    /// graph produced *something* but most of the screen is still black — a sky
    /// node that silently early-returned, a blit reading the wrong buffer, a
    /// camera framing nothing. Always prints the measured fraction to stdout so
    /// it is visible run-to-run and easy to re-tune.
    pub fn check_luminance_alive(&self, batch: u32) -> Result<(), String> {
        let frac = self.non_black_fraction(NON_BLACK_EPS);
        let threshold = min_non_black_fraction(batch);
        println!(
            "e2e_render: luminance gate (batch {batch}) — {:.1}% of the frame is non-black \
             (luminance > {NON_BLACK_EPS}); threshold {:.0}%",
            frac * 100.0,
            threshold * 100.0
        );
        if frac < threshold {
            return Err(format!(
                "only {:.1}% of the frame is non-black (luminance > {NON_BLACK_EPS}), \
                 expected >= {:.0}% for batch {batch} — most of the screen is pitch black, \
                 the render graph produced almost no light (a sky/blit node likely silently \
                 early-returned, or the camera frames nothing)",
                frac * 100.0,
                threshold * 100.0
            ));
        }
        Ok(())
    }
}
