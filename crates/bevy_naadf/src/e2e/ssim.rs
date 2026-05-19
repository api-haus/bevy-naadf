//! Shared SSIM (Structural Similarity Index) helpers.
//!
//! web-vox-async-loading 2026-05-18 follow-up Step 8 + Step 9 — factored
//! out of `vox_gpu_oracle.rs` so the new `--vox-web-parity` gate (Q5),
//! the `--ssim-compare` flag (Q6, Playwright integration), AND the
//! existing `--vox-gpu-oracle` gate all call the same SSIM impl. Per
//! Decision 4 of `01-context.md`: **zero metric drift** between native
//! and Playwright gates.
//!
//! ## API
//!
//! - [`ssim_compare_framebuffers`] — the load-bearing single function. Takes
//!   two [`Framebuffer`] handles, returns the `Algorithm::MSSIMSimple` score
//!   or an error.
//! - [`load_png_as_framebuffer`] — PNG → `Framebuffer` helper.
//! - [`framebuffer_to_rgb_image`] — `Framebuffer` → `image::RgbImage`.

use std::path::Path;

use crate::e2e::framebuffer::Framebuffer;

/// Compute the SSIM score between two framebuffers via
/// `image_compare::rgb_similarity_structure(MSSIMSimple, …)`. Returns the
/// score (0.0..=1.0 where 1 = identical) or an error string on dimension
/// mismatch / internal compare failure.
///
/// Same impl used by `--vox-gpu-oracle`, `--vox-web-parity`, and
/// `--ssim-compare` (Playwright integration) — Decision 4.
pub fn ssim_compare_framebuffers(
    a: &Framebuffer,
    b: &Framebuffer,
) -> Result<f64, String> {
    if a.width() != b.width() || a.height() != b.height() {
        return Err(format!(
            "frame dimensions differ: {}×{} vs {}×{} — cannot SSIM-compare \
             different-sized images.",
            a.width(),
            a.height(),
            b.width(),
            b.height(),
        ));
    }
    let a_rgb = framebuffer_to_rgb_image(a);
    let b_rgb = framebuffer_to_rgb_image(b);
    let result = image_compare::rgb_similarity_structure(
        &image_compare::Algorithm::MSSIMSimple,
        &a_rgb,
        &b_rgb,
    );
    match result {
        Ok(sim) => Ok(sim.score),
        Err(e) => Err(format!(
            "SSIM computation failed: {e:?}. Dims {}×{} vs {}×{}.",
            a.width(),
            a.height(),
            b.width(),
            b.height(),
        )),
    }
}

/// Convert a [`Framebuffer`] (RGBA8) into an `image::RgbImage` (RGB8) for
/// `image_compare::rgb_similarity_structure`. Drops the alpha channel
/// (both PNGs are written as fully-opaque captures).
pub fn framebuffer_to_rgb_image(fb: &Framebuffer) -> image::RgbImage {
    let mut img = image::RgbImage::new(fb.width(), fb.height());
    for y in 0..fb.height() {
        for x in 0..fb.width() {
            let p = fb.pixel(x, y);
            img.put_pixel(x, y, image::Rgb([p[0], p[1], p[2]]));
        }
    }
    img
}

/// Load a PNG from disk back into a [`Framebuffer`]. Used by the
/// `--ssim-compare` short-circuit flag in `bin/e2e_render.rs` + by the
/// `--vox-web-parity` and `--vox-gpu-oracle` compare phases.
pub fn load_png_as_framebuffer(path: &Path) -> Result<Framebuffer, String> {
    let img = image::open(path)
        .map_err(|e| format!("image::open failed for {}: {e}", path.display()))?;
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let mut data: Vec<[u8; 4]> = Vec::with_capacity((width * height) as usize);
    for px in rgba.pixels() {
        data.push([px[0], px[1], px[2], px[3]]);
    }
    Ok(Framebuffer::from_raw_rgba(data, width, height))
}

// ---------------------------------------------------------------------------
// `--ssim-compare` flag — short-circuit CLI mode (Step 9 / Q6)
// ---------------------------------------------------------------------------

/// Arguments for the `--ssim-compare` short-circuit flag. Built by
/// `bin/e2e_render.rs`'s flag parser.
#[derive(Debug, Clone)]
pub struct SsimArgs {
    /// First PNG path (positional argument 1).
    pub a: std::path::PathBuf,
    /// Second PNG path (positional argument 2).
    pub b: std::path::PathBuf,
    /// `--ssim-max <f64>` — if set, exit 1 when SSIM `>= max`. Used by
    /// `--vox-web-parity` and the Playwright SSIM dissimilarity gate.
    pub max: Option<f64>,
    /// `--ssim-min <f64>` — if set, exit 1 when SSIM `< min`. Used by the
    /// `--vox-gpu-oracle` similarity gate.
    pub min: Option<f64>,
}

/// `--ssim-compare` command body. Loads both PNGs, computes SSIM,
/// optionally asserts the score is within the `[min, max)` band, prints
/// diagnostics, and returns the exit code.
///
/// Exit codes (per `03-architecture.md` § Q6):
/// - `0` — gate passed (SSIM in asserted range).
/// - `1` — gate failed (SSIM out of asserted range).
/// - `2` — internal error (file not found, decode error, dimension
///   mismatch, image-compare internal failure).
pub fn ssim_compare_command(args: &SsimArgs) -> u8 {
    let a_fb = match load_png_as_framebuffer(&args.a) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!("e2e_render --ssim-compare: load A failed: {e}");
            return 2;
        }
    };
    let b_fb = match load_png_as_framebuffer(&args.b) {
        Ok(fb) => fb,
        Err(e) => {
            eprintln!("e2e_render --ssim-compare: load B failed: {e}");
            return 2;
        }
    };
    let score = match ssim_compare_framebuffers(&a_fb, &b_fb) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("e2e_render --ssim-compare: {e}");
            return 2;
        }
    };
    println!("SSIM={score:.6}");
    println!("WIDTH={}", a_fb.width());
    println!("HEIGHT={}", a_fb.height());
    if let Some(max) = args.max {
        if score >= max {
            eprintln!(
                "e2e_render --ssim-compare: FAIL — SSIM {score:.6} >= --ssim-max \
                 {max:.6}"
            );
            return 1;
        }
    }
    if let Some(min) = args.min {
        if score < min {
            eprintln!(
                "e2e_render --ssim-compare: FAIL — SSIM {score:.6} < --ssim-min \
                 {min:.6}"
            );
            return 1;
        }
    }
    println!("e2e_render --ssim-compare: PASS (SSIM={score:.6})");
    0
}

/// Parse the `--ssim-compare` argument slice. Returns `Ok(args)` on
/// success or `Err(message)` if the args are malformed.
///
/// Expected shape: `--ssim-compare <a.png> <b.png> [--ssim-max <f64>] [--ssim-min <f64>]`
///
/// The first two positionals after `--ssim-compare` are the PNG paths;
/// optional `--ssim-max` / `--ssim-min` may appear in any order after
/// them. Anything else is rejected.
pub fn parse_ssim_compare_args(raw: &[String]) -> Result<SsimArgs, String> {
    let mut iter = raw.iter();
    // Find the `--ssim-compare` token; the two positionals come right
    // after it.
    let mut found = false;
    while let Some(t) = iter.next() {
        if t == "--ssim-compare" {
            found = true;
            break;
        }
    }
    if !found {
        return Err("--ssim-compare flag not present".to_string());
    }
    let a = iter
        .next()
        .ok_or_else(|| "expected first PNG path after --ssim-compare".to_string())?;
    let b = iter
        .next()
        .ok_or_else(|| "expected second PNG path after --ssim-compare <a>".to_string())?;
    let mut max: Option<f64> = None;
    let mut min: Option<f64> = None;
    while let Some(t) = iter.next() {
        match t.as_str() {
            "--ssim-max" => {
                let v = iter
                    .next()
                    .ok_or_else(|| "expected float after --ssim-max".to_string())?;
                max = Some(v.parse::<f64>().map_err(|e| {
                    format!("--ssim-max: failed to parse '{v}' as f64: {e}")
                })?);
            }
            "--ssim-min" => {
                let v = iter
                    .next()
                    .ok_or_else(|| "expected float after --ssim-min".to_string())?;
                min = Some(v.parse::<f64>().map_err(|e| {
                    format!("--ssim-min: failed to parse '{v}' as f64: {e}")
                })?);
            }
            other => {
                return Err(format!(
                    "unrecognised flag in --ssim-compare args: '{other}'"
                ));
            }
        }
    }
    Ok(SsimArgs {
        a: std::path::PathBuf::from(a),
        b: std::path::PathBuf::from(b),
        max,
        min,
    })
}
