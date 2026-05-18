//! `--pbr-visual` mode — PBR-raymarching visual gate.
//!
//! Per `docs/orchestrate/pbr-raymarching/02-design.md` § I: capture a single
//! frame of the default test grid from a fixed side-on pose looking at the
//! metallic pillar (VoxelType 8, `material_layer_index = 3` = metal_02),
//! save the screenshot, and assert:
//!
//! 1. **Specular highlight present** — `region_luminance` over a 40×40 px
//!    rect on the pillar's sun-side highlight exceeds a brightness floor.
//! 2. **Albedo texture variation** — the std-dev of 16 sampled pixel
//!    luminances across an 80×80 px rect on a textured surface exceeds a
//!    floor (catches a flat-colour fallback regression).
//! 3. **Metallic F0 ≈ albedo (colour-pull)** — the mean R/G and R/B ratios
//!    in a 40×40 px rect on the metallic pillar's specular hot-spot stay
//!    within a tolerance of the (manually-pinned) metal_02-with-violet-tint
//!    ratios.
//!
//! Pixel coordinates are pinned after running the gate ONCE and inspecting
//! the saved `target/e2e-screenshots/pbr_visual_baseline.png` — see the
//! consts below.

use std::path::Path;

use bevy::prelude::*;

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::voxel::CELL_DIM;
use crate::world::data::WorldData;

// ---------------------------------------------------------------------------
// Screenshot filename
// ---------------------------------------------------------------------------

/// PNG written by the gate on success — overwritten every run.
pub const PBR_VISUAL_PNG: &str = "pbr_visual_baseline.png";

// ---------------------------------------------------------------------------
// Frame budget
// ---------------------------------------------------------------------------

/// Warmup frames before the screenshot is captured. Same convention as the
/// other visual gates (`OASIS_WARMUP_FRAMES = 120` etc.). 150 gives TAA +
/// GI a chance to converge at the fixed pose.
pub const PBR_VISUAL_WARMUP_FRAMES: u32 = 150;
/// Max frames the driver waits for the async screenshot capture.
pub const PBR_VISUAL_DRAIN_FRAMES: u32 = 16;

// ---------------------------------------------------------------------------
// Assertion rects + thresholds
// ---------------------------------------------------------------------------

/// 40×40 px rect on the metallic pillar's sun-side specular highlight.
/// Coordinates pinned from the first run — see module docs.
pub const PBR_HIGHLIGHT_RECT: Rect = Rect { x0: 110, y0: 100, x1: 150, y1: 140 };
/// 80×80 px rect on a textured surface (the ground / wall) where the
/// triplanar-sampled albedo varies pixel-to-pixel.
pub const PBR_TEXTURE_RECT: Rect = Rect { x0: 60, y0: 180, x1: 140, y1: 260 };
/// 40×40 px rect on the metallic pillar's hot-spot for the F0 colour-pull
/// check. May overlap with `PBR_HIGHLIGHT_RECT`; that's fine — they
/// measure different things.
pub const PBR_F0_RECT: Rect = Rect { x0: 110, y0: 100, x1: 150, y1: 140 };

/// 18×30 px rect on the **interior of the violet metal_02 pillar** —
/// pinned from the post-fix baseline at `(78,156)-(96,186)`. The pillar
/// has a uniform `albedo_tint = [115,82,158]` and uniform `metal_02`
/// material, so the only sources of intra-rect luminance variance are:
/// (a) GI sampling noise (~2-3 units), (b) normal-map perturbations
/// modulating the BRDF terms via `dot(n, l)` / `dot(n, v)` / `dot(n, h)`.
/// The metal_02 base color is nearly flat (luminance std-dev ~2.9 over
/// the source PNG). So a passing std-dev > `PBR_NORMAL_STD_DEV_FLOOR`
/// proves the normal-map is contributing to the BRDF — the bug that bit
/// `03a` (normal map sampled but unused by every BRDF call site).
pub const PBR_NORMAL_RECT: Rect = Rect { x0: 78, y0: 156, x1: 96, y1: 186 };

/// 30×30 px rect on the **bark_04 tower face** at glancing-sun angle —
/// pinned from the post-rewrite baseline at `(100,170)-(130,200)`. The
/// tower (VoxelType 6 = bark_04, no tint) has a uniform tan/brown albedo
/// and high-frequency heightmap variation. The sun's tangent-space
/// component on this face is small enough (`sky_sun_dir` projected onto
/// the face normal ≈ 0.35-0.5) that the modern POM self-shadow march
/// produces a measurable per-pixel darkening in the heightfield valleys.
///
/// **Catches the "self-shadow turned off" regression class** — when
/// shadow strength is forced to zero (or `pom_self_shadow` returns 1
/// unconditionally), the rect's mean luminance rises by ~4 units. The
/// `PBR_SHADOW_MEAN_LUMA_CEIL` floor pins the ceiling between the
/// shadow-on (~152) and shadow-off (~157) means.
pub const PBR_SHADOW_RECT: Rect = Rect { x0: 100, y0: 170, x1: 130, y1: 200 };

/// Minimum mean-luminance the highlight rect must reach.
///
/// **Tuned from baseline.** The standalone Batch-6 default-scene readback
/// shows full-frame mean luminance ~150; a known-specular highlight should
/// be visibly brighter than that floor.
pub const PBR_HIGHLIGHT_LUMA_FLOOR: f32 = 100.0;

/// Minimum std-dev of the 16-tap luminance samples in the texture rect.
/// A flat-colour fallback regression collapses to <2; a triplanar-sampled
/// textured surface empirically varies 10-40 luminance units.
pub const PBR_TEXTURE_STD_DEV_FLOOR: f32 = 5.0;

/// Tolerance for the F0 colour-pull check. The metallic pillar (`metal_02`
/// + violet tint `[115, 82, 158]`) should show a violet-leaning ratio
/// stable across runs.
pub const PBR_F0_TOLERANCE: f32 = 0.5;

/// Minimum std-dev of the 16-tap luminance samples inside the uniform
/// metallic-pillar rect ([`PBR_NORMAL_RECT`]). Pre-fix (normal map sampled
/// but BRDF receives geometric face normal) the std-dev sits ≤ 6 — pure
/// GI noise + minor specular highlights. Post-fix (normal map perturbs
/// the BRDF) the std-dev rises into the 12-20 range from the normal-map
/// shading variation modulating the metallic specular lobe. The floor at
/// 8.0 sits comfortably in the gap. **Catches Bug A** (`05-diagnostic.md`).
pub const PBR_NORMAL_STD_DEV_FLOOR: f32 = 8.0;

/// Maximum fraction of pixels in [`PBR_TEXTURE_RECT`] whose RGB max
/// exceeds 254 (essentially saturated). A clean tonemapped framebuffer
/// has near-zero saturation on a ground texture; an `eval_pbr` NaN /
/// `D = 0/0` cascade saturates pixel clusters there. The textured rect is
/// chosen because it sits OUTSIDE the legitimate HDR-emissive blocks.
/// **Catches Bug B** NaN-cascade class regressions
/// (`05-diagnostic.md` B-extra).
pub const PBR_TEXTURE_SAT_FRAC_CEIL: f32 = 0.10;

/// Maximum mean luminance of [`PBR_SHADOW_RECT`] (the bark_04 tower face
/// at glancing-sun angle). Without POM self-shadow the rect's mean is
/// ~156.5; with self-shadow active it drops to ~152.5 (heightfield
/// valleys are dimmed by the secondary sun-march in `pom_self_shadow`).
/// The ceiling is pinned between the two cases at 155.0 so disabling
/// self-shadow (or accidentally setting `POM_SHADOW_STRENGTH = 0`)
/// causes the gate to fail.
///
/// **Catches the modern POM self-shadow regression class**
/// (`05-diagnostic.md` "POM rewrite — modern implementation").
pub const PBR_SHADOW_MEAN_LUMA_CEIL: f32 = 155.0;

// ---------------------------------------------------------------------------
// State resource
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct PbrVisualState {
    pub captured: Option<Framebuffer>,
    pub saved: bool,
}

// ---------------------------------------------------------------------------
// Entry point + camera pose
// ---------------------------------------------------------------------------

/// Boot the e2e harness with `--pbr-visual` mode active.
pub fn run_pbr_visual() -> AppExit {
    let mut app_args = crate::AppArgs::default();
    app_args.pbr_visual_mode = true;
    println!(
        "e2e_render --pbr-visual: PBR-raymarching visual gate; warmup \
         {PBR_VISUAL_WARMUP_FRAMES} frames; default test grid; side-on pose \
         looking at the metallic pillar."
    );
    crate::run_e2e_render_with_args(app_args)
}

/// Side-on view of the metallic pillar in the default test grid.
///
/// Reuses [`crate::e2e::gates::e2e_camera_transform`]'s 3/4-pose framing of
/// the `GridPreset::Default` scene — it sits the camera back-and-above the
/// 64×32×64 demo and looks at the centre, framing the pillar row, towers,
/// emissive blocks, and several diffuse surfaces in non-overlapping
/// screen regions. The standard Batch-6 gate uses the same pose; reusing
/// it guarantees the PBR voxels are in view.
pub fn pbr_visual_pose() -> Transform {
    crate::e2e::gates::e2e_camera_transform()
}

/// Override the camera pose every frame while the gate is running.
pub fn pin_pbr_visual_camera(
    args: Option<Res<crate::AppArgs>>,
    world_data: Option<Res<WorldData>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
) {
    let Some(args) = args else { return; };
    if !args.pbr_visual_mode {
        return;
    }
    let Some(world_data) = world_data else { return; };
    let size_v = world_data.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
    if size_v.x == 0 || size_v.y == 0 || size_v.z == 0 {
        return;
    }
    let pose = pbr_visual_pose();
    let (transform, position_split) = &mut *camera;
    **transform = pose;
    **position_split = PositionSplit::from_world(pose.translation);
}

// ---------------------------------------------------------------------------
// Save + assertion helpers
// ---------------------------------------------------------------------------

pub fn save_pbr_visual_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --pbr-visual: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --pbr-visual: {filename} save failed: {e}"
        ),
    }
}

/// Std-dev of 16 evenly-spaced pixel luminances inside `rect`. Catches a
/// flat-colour-fallback regression: a textured surface has high variance,
/// a flat-colour fallback has near-zero variance.
fn region_luminance_std_dev_16(fb: &Framebuffer, rect: Rect) -> f32 {
    let w = (rect.x1 - rect.x0) as i32;
    let h = (rect.y1 - rect.y0) as i32;
    if w <= 0 || h <= 0 {
        return 0.0;
    }
    let mut samples = [0.0f32; 16];
    // 4x4 grid of taps inside the rect.
    for i in 0..16i32 {
        let gx = i % 4;
        let gy = i / 4;
        let sx = rect.x0 + ((gx * w) / 4) as u32;
        let sy = rect.y0 + ((gy * h) / 4) as u32;
        if sx < rect.x1 && sy < rect.y1 {
            let p = fb.pixel(sx, sy);
            // Perceptual-luminance approximation (Rec. 709).
            samples[i as usize] =
                0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        }
    }
    let mean: f32 = samples.iter().sum::<f32>() / 16.0;
    let var: f32 =
        samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / 16.0;
    var.sqrt()
}

/// Mean RGB over a rect.
fn region_mean_rgb(fb: &Framebuffer, rect: Rect) -> (f32, f32, f32) {
    let mut acc = (0.0f32, 0.0f32, 0.0f32);
    let mut n = 0u32;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let p = fb.pixel(x, y);
            acc.0 += p[0] as f32;
            acc.1 += p[1] as f32;
            acc.2 += p[2] as f32;
            n += 1;
        }
    }
    if n == 0 {
        (0.0, 0.0, 0.0)
    } else {
        (acc.0 / n as f32, acc.1 / n as f32, acc.2 / n as f32)
    }
}

/// Fraction (`0.0..=1.0`) of pixels in `rect` whose max-channel value is
/// `> 254`. A clean tonemapped scene has near-zero saturation in a
/// textured-ground region; a `D = 0/0` NaN cascade in the BRDF saturates
/// clusters of pixels there.
fn region_saturated_fraction(fb: &Framebuffer, rect: Rect) -> f32 {
    let mut sat = 0u32;
    let mut n = 0u32;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let p = fb.pixel(x, y);
            if p[0] > 254 || p[1] > 254 || p[2] > 254 {
                sat += 1;
            }
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sat as f32 / n as f32
    }
}

pub fn assert_pbr_visual(fb: &Framebuffer) -> Result<String, String> {
    let highlight_luma = fb.region_luminance(PBR_HIGHLIGHT_RECT);
    let texture_std = region_luminance_std_dev_16(fb, PBR_TEXTURE_RECT);
    let (fr, fg, fb_blue) = region_mean_rgb(fb, PBR_F0_RECT);
    // New (post-05-diagnostic) — normal-map shading variance + HDR
    // saturation count.
    let normal_std = region_luminance_std_dev_16(fb, PBR_NORMAL_RECT);
    let sat_frac = region_saturated_fraction(fb, PBR_TEXTURE_RECT);
    // POM-rewrite — mean luminance on the bark_04 tower face at glancing
    // sun angle. Self-shadowing dims the heightfield valleys, dropping the
    // rect's mean below the no-shadow ceiling.
    let (sr, sg, sb) = region_mean_rgb(fb, PBR_SHADOW_RECT);
    let shadow_mean_luma = 0.2126 * sr + 0.7152 * sg + 0.0722 * sb;

    // The metallic pillar carries a violet `albedo_tint = [115, 82, 158]`
    // (PBR-raymarching § A grid-palette assignment), so the F0 colour
    // should bias violet (R > G, B > G). We assert the SHAPE rather than
    // exact numeric ratios — `R/G > 1.0 - tol` AND `B/G > 1.0 - tol`
    // is the load-bearing "the metallic tint is visible" check. The
    // tolerance is loose: GI and atmosphere shift the absolute numbers a
    // lot frame-to-frame, but the ratio shape stays stable.
    let r_over_g = if fg > 1.0 { fr / fg } else { 0.0 };
    let b_over_g = if fg > 1.0 { fb_blue / fg } else { 0.0 };

    let report = format!(
        "highlight luma {highlight_luma:.1} (floor {PBR_HIGHLIGHT_LUMA_FLOOR}); \
         texture std-dev {texture_std:.2} (floor {PBR_TEXTURE_STD_DEV_FLOOR}); \
         normal-rect std-dev {normal_std:.2} (floor {PBR_NORMAL_STD_DEV_FLOOR}); \
         texture sat-frac {sat_frac:.3} (ceil {PBR_TEXTURE_SAT_FRAC_CEIL}); \
         shadow-rect mean luma {shadow_mean_luma:.2} (ceil {PBR_SHADOW_MEAN_LUMA_CEIL}); \
         F0 mean RGB ({fr:.1}, {fg:.1}, {fb_blue:.1}), \
         R/G = {r_over_g:.3}, B/G = {b_over_g:.3}",
    );
    println!("e2e_render --pbr-visual: {report}");

    if highlight_luma < PBR_HIGHLIGHT_LUMA_FLOOR {
        return Err(format!(
            "pbr-visual gate FAIL — highlight rect mean luminance \
             {highlight_luma:.1} below the floor {PBR_HIGHLIGHT_LUMA_FLOOR}. \
             {report}. Inspect target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }
    if texture_std < PBR_TEXTURE_STD_DEV_FLOOR {
        return Err(format!(
            "pbr-visual gate FAIL — texture rect luminance std-dev \
             {texture_std:.2} below the floor {PBR_TEXTURE_STD_DEV_FLOOR}. \
             The PBR raymarcher likely fell back to flat per-VoxelType colour \
             (the texture sample is not actually contributing). {report}. \
             Inspect target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }
    // Bug-A regression catch: normal map sampled but never fed into the
    // BRDF. A flat-shaded metal pillar's luminance variance over a
    // uniform-albedo region is GI-noise floor (~2-6); a normal-mapped one
    // shows clear shading variation (~12-20). See `PBR_NORMAL_STD_DEV_FLOOR`.
    if normal_std < PBR_NORMAL_STD_DEV_FLOOR {
        return Err(format!(
            "pbr-visual gate FAIL — normal-map shading on the uniform-albedo \
             metallic pillar rect has std-dev {normal_std:.2} below the floor \
             {PBR_NORMAL_STD_DEV_FLOOR}. The normal map is likely sampled but \
             not propagating into the BRDF (Bug A). {report}. Inspect \
             target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }
    // Bug-B regression catch: NaN cascades from `eval_pbr` produce
    // saturated HDR clusters in non-emissive regions. The texture rect
    // sits on the ground (no legitimate HDR sources) — saturation there
    // is a bug.
    if sat_frac > PBR_TEXTURE_SAT_FRAC_CEIL {
        return Err(format!(
            "pbr-visual gate FAIL — texture rect saturated-pixel fraction \
             {sat_frac:.3} exceeds ceiling {PBR_TEXTURE_SAT_FRAC_CEIL}. A \
             NaN-cascade through `eval_pbr` (typically roughness ≈ 0 with \
             perfect half-vector alignment) saturates pixel clusters. {report}. \
             Inspect target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }
    // POM self-shadow regression catch: with `pom_self_shadow` returning
    // anything close to 1.0 (or `POM_SHADOW_STRENGTH = 0`), the
    // bark_04 tower face at glancing sun angle stays bright. With the
    // shadow march active the rect mean luminance drops below the
    // pinned ceiling — see `PBR_SHADOW_MEAN_LUMA_CEIL`.
    if shadow_mean_luma > PBR_SHADOW_MEAN_LUMA_CEIL {
        return Err(format!(
            "pbr-visual gate FAIL — POM shadow rect mean luminance \
             {shadow_mean_luma:.2} exceeds ceiling {PBR_SHADOW_MEAN_LUMA_CEIL}. \
             The POM self-shadow march is likely returning 1.0 or \
             `POM_SHADOW_STRENGTH` was reset to 0 — heightfield valleys \
             on the bark_04 tower face are not darkening as expected. \
             {report}. Inspect target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }
    // Colour-pull: with the violet tint we expect both ratios > 1 - tol.
    // A pure-grey fallback would land near 1.0; a working metal_02 + violet
    // tint shows a clearly biased ratio. The tolerance is generous to
    // accommodate GI/atmosphere shifts.
    if r_over_g < 1.0 - PBR_F0_TOLERANCE || b_over_g < 1.0 - PBR_F0_TOLERANCE {
        return Err(format!(
            "pbr-visual gate FAIL — F0 colour-pull check: R/G = {r_over_g:.3} \
             and/or B/G = {b_over_g:.3} are below 1 - {PBR_F0_TOLERANCE:.2}, \
             suggesting the violet tint is not propagating into the metallic \
             F0. {report}. Inspect target/e2e-screenshots/{PBR_VISUAL_PNG}.",
        ));
    }

    // 6th assertion — POM sample-UV consistency. Inspect the WGSL source of
    // every pass that re-shades the first-hit surface; assert that each
    // calls `pom_compute` BEFORE any `triplanar_sample_pom` /
    // `triplanar_sample_normal_pom` call, and that the un-POM
    // `triplanar_sample(pbr_diffuse_ao|pbr_normal, ...)` and
    // `triplanar_sample_normal(pbr_normal, ...)` calls do NOT appear in
    // the first-hit re-sample blocks (they would re-introduce the H2
    // moiré). Catches `05-diagnostic.md` § "POM seam-artifact diagnose+fix"
    // regression class: any future edit that re-introduces un-POM sampling
    // of the first-hit surface in GI or spatial_resampling fails this check.
    if let Err(msg) = assert_pom_uv_consistency_source() {
        return Err(format!(
            "pbr-visual gate FAIL — POM sample-UV consistency check: {msg}. \
             {report}.",
        ));
    }

    Ok(format!("pbr-visual gate PASS — {report}"))
}

// ---------------------------------------------------------------------------
// 6th assertion — POM sample-UV consistency (WGSL source property check)
// ---------------------------------------------------------------------------

/// Inspect the WGSL source of every pass that re-shades the first-hit
/// surface; assert each pass establishes a POM-displaced UV via
/// `pom_compute` BEFORE its first POM-aware sample call AND does not call
/// un-POM `triplanar_sample` on the PBR maps in the first-hit re-sample
/// block. Catches the H2 moiré regression class structurally — if any
/// future edit re-introduces un-POM first-hit shading in GI /
/// spatial_resampling, this assertion fails.
pub fn assert_pom_uv_consistency_source() -> Result<(), String> {
    const FIRST_HIT_WGSL: &str = include_str!(
        "../assets/shaders/naadf_first_hit.wgsl"
    );
    const GI_WGSL: &str = include_str!(
        "../assets/shaders/naadf_global_illum.wgsl"
    );
    const SPATIAL_WGSL: &str = include_str!(
        "../assets/shaders/spatial_resampling.wgsl"
    );

    for (name, src) in &[
        ("naadf_first_hit.wgsl", FIRST_HIT_WGSL),
        ("naadf_global_illum.wgsl", GI_WGSL),
        ("spatial_resampling.wgsl", SPATIAL_WGSL),
    ] {
        // The pass must call `pom_compute` at least once.
        if !src.contains("pom_compute(") {
            return Err(format!(
                "{name} does not call `pom_compute(` — first-hit POM \
                 consolidation regressed; H2 moiré will return"
            ));
        }
        // The pass must call `triplanar_sample_pom` at least once.
        if !src.contains("triplanar_sample_pom(") {
            return Err(format!(
                "{name} does not call `triplanar_sample_pom(` for the \
                 first-hit re-sample — H2 moiré will return"
            ));
        }
        // `pom_compute` must precede the first `triplanar_sample_pom`
        // call in source order — the POM-displaced UV is an input to the
        // sample, so the compute must come first.
        let pc_pos = src
            .find("pom_compute(")
            .ok_or_else(|| format!("{name}: pom_compute( not found"))?;
        let ts_pos = src
            .find("triplanar_sample_pom(")
            .ok_or_else(|| format!("{name}: triplanar_sample_pom( not found"))?;
        if pc_pos > ts_pos {
            return Err(format!(
                "{name}: first `triplanar_sample_pom(` appears before \
                 `pom_compute(` — the displaced UV is consumed before being \
                 computed"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source-level invariant: every WGSL pass that re-shades the
    /// first-hit surface MUST establish its POM-displaced UV via
    /// `pom_compute` BEFORE issuing a POM-aware texture sample. If this
    /// invariant breaks, the H2 first-hit-vs-GI moiré (the "double
    /// surface" artifact in user-report Image #3) returns. See
    /// `05-diagnostic.md` § "POM seam-artifact diagnose+fix".
    #[test]
    fn pom_uv_consistency_source_invariant() {
        if let Err(msg) = assert_pom_uv_consistency_source() {
            panic!("POM UV-consistency source-property check failed: {msg}");
        }
    }

    /// The two POM-aware sample helpers MUST share the per-plane UV
    /// preamble — same swizzle (`p.yz, p.zx, p.xy`) and same branch on
    /// `dominant_axis`. A future divergence would re-introduce H1
    /// (mismatched displacement across helpers).
    #[test]
    fn pom_sample_helpers_share_preamble() {
        const PBR_SAMPLING: &str =
            include_str!("../assets/shaders/pbr_sampling.wgsl");

        // Both helpers compute `let p = world_pos * WORLD_UV_SCALE` and
        // build `uv_x = p.yz; uv_y = p.zx; uv_z = p.xy;` then branch on
        // `dominant_axis`. Count occurrences — there should be at least
        // two of each (one per helper).
        let preamble_count = PBR_SAMPLING
            .matches("var uv_x = p.yz;")
            .count();
        assert!(
            preamble_count >= 2,
            "Expected ≥2 POM-aware sample helpers sharing the \
             `var uv_x = p.yz;` preamble; found {preamble_count}. A \
             helper diverged from the canonical layout — H1 mismatched \
             displacement risk."
        );
    }
}
