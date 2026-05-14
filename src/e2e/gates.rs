//! Per-batch visual gates + the batch dispatch table (`e2e-render-test.md` §6,
//! §8, §6.4).
//!
//! Everything camera-pose-coupled lives here in one obvious place
//! (`e2e-render-test.md` R5): the fixed E2E camera pose, the
//! `GridPreset::Default`-scene gate rectangles (derived from that pose), the
//! per-batch `assert_batch_N` functions, the `EXPECTED_SPANS` tables, the
//! stability-hash baselines, and `CURRENT_BATCH`.
//!
//! **Adding a batch is a small, obvious edit (`e2e-render-test.md` §6.4):**
//! 1. add one `assert_batch_N(&Framebuffer, &GateState) -> Result<(), String>`,
//! 2. add its row to [`expected_spans`] + [`batch_gate`] + [`hash_baseline`],
//! 3. bump [`CURRENT_BATCH`],
//! 4. if the batch intentionally changes the image, re-bless its hash baseline.
//! The window-boot, bounded-frame driver, readback, and pipeline-error scan are
//! batch-agnostic and written once.

use bevy::prelude::*;

use super::framebuffer::{Framebuffer, Rect};

/// The fixed E2E camera pose (`e2e-render-test.md` §4.2, R5).
///
/// A **test-specific pose** — the design explicitly allows one (§4.2,
/// Assumptions: "if no single pose frames all three cleanly, a test-specific
/// pose const is chosen"). The production `setup_camera` pose
/// (`camera/mod.rs:40`) was tuned for a wide default window and at the e2e
/// 256×256 1:1-aspect window it frames empty space below the horizon — nothing
/// of the voxel grid is in view. This pose instead sits back-and-above the
/// `GridPreset::Default` grid (64×32×64 voxels, 1 voxel = 1 world unit) and
/// looks down at the scene centre, framing several emissive blocks, the diffuse
/// geometry, and a clear sky corner in non-overlapping screen regions. The gate
/// rectangles below are derived from *this* pose; if it changes, re-derive
/// them from a fresh `save_to_disk` dump.
///
/// **History.** The original pose `(104, 34, 110)` sat at a grazing angle and
/// showed streak artifacts; it was repositioned to `(112, 52, 117)` looking at
/// `(34, 20, 34)` — a clean 3/4 vantage. **Re-framed again (2026-05-14, e2e
/// test-scene expansion):** the test scene was expanded with a larger voxel
/// arrangement + five emissive blocks, and at the `(112,52,117)` pose the
/// expanded scene sat small and far in the frame. The camera was pulled
/// **closer** along the same look axis — from ~117 units out to ~83 units —
/// keeping the same ~16°-below-horizontal 3/4 pitch, so the expanded volume
/// fills the 256×256 frame cleanly with the atmosphere-tinted sky band still
/// across the top. Gate rects below were re-derived from a fresh `save_to_disk`
/// dump at this pose.
pub fn e2e_camera_transform() -> Transform {
    Transform::from_xyz(86.0, 42.0, 90.0).looking_at(Vec3::new(32.0, 16.0, 32.0), Vec3::Y)
}

/// The highest batch currently implemented — the `ASSERT` step runs this
/// batch's region gate (older batches' gates are kept as called helpers so an
/// earlier-gate regression still trips). Phase B Batches 1-6 exist
/// (`10-impl-b.md`) — Batch 6 is the final batch, the Phase-B deliverable.
///
/// **Batch 6 is the GI-lit regime.** Batch 6 wires the `base/` `ReprojectOld`
/// pass to write `taa_dist_min_max`, which un-blocks the `renderSampleRefine →
/// valid_samples_compressed → renderSpatialResampling` reservoir chain, so the
/// GI bounce composites into `final_color` and (via `CalcNewTaaSample`) into
/// `taa_sample_accum` — the blit source. The B5-vs-B6 milestone
/// (`10-impl-b.md` Batch-5 section) settled that the visible bounce lands at
/// Batch 6, so [`super::framebuffer::GI_LIT_BATCH`] is `6` — the 0.60 hard
/// luminance gate applies from this batch on, and [`assert_batch_6`] is the
/// first region gate that asserts the previously-near-black diffuse geometry
/// has *brightened*.
pub const CURRENT_BATCH: u32 = 6;

// --- Gate rectangles -------------------------------------------------------
//
// Fractional (0..1) screen coords, keyed off the *actual* readback dimensions
// (`Rect::from_fractional`) so a HiDPI scale-factor difference does not
// silently misalign them (`e2e-render-test.md` §6.5, R5/R7). Derived from a
// `save_to_disk` PNG dump of the readback at the fixed pose above.
//
// `GridPreset::Default` layout (`voxel/grid.rs build_default_volume`, 64×32×64
// voxels) — the **expanded scene** (2026-05-14): a ground slab (y 0..2), four
// corner towers, a sand back wall with an arch carved through it, a row of
// three violet pillars, warm box A + cool box B, two green spheres, and **FIVE
// emissive blocks** distributed through the volume (warm-white near centre,
// cool-white low-near, amber high-far, green mid +x/-z, magenta low near +z).
// RNG-free, deterministic constructors (re-confirmed against
// `build_default_volume`).
//
// At the fixed (re-framed, closer) pose the 256×256 readback shows: the
// warm-white emissive block bright just above screen centre, the magenta
// emissive block bright in the lower-left, the green emissive block bright on
// the right, the dark diffuse voxel geometry (near-black pre-GI) filling the
// mid/lower frame, and the atmosphere-tinted sky band across the top.
//
// Verified-by-dump region means at this pose: warm-white-emissive interior
// luminance ~234, dark-diffuse-geometry luminance ~4, sky-band luminance ~133 —
// well-separated, so the gate thresholds below have generous margin.

/// The emissive-block screen region — an emissive block is the only lit thing
/// pre-GI; should read near-white / high-luminance. This rect is the interior
/// of the warm-white emissive block (the connected bright blob just above
/// screen centre, px x≈104..155, y≈78..134 at 256×256), kept inside its edges
/// so a jittered edge pixel does not pull the region mean down. Measured
/// luminance ~234 (gate `> 120`).
fn emissive_rect(fb: &Framebuffer) -> Rect {
    Rect::from_fractional(fb, 0.45, 0.36, 0.55, 0.45)
}

/// A non-emissive solid-block region — the dark diffuse voxel geometry directly
/// below the warm-white emissive block — near-black pre-GI (no bounce light
/// yet), measurably brighter once GI lands (B5). Measured luminance ~4
/// (gate `< 90`).
fn solid_block_rect(fb: &Framebuffer) -> Rect {
    Rect::from_fractional(fb, 0.42, 0.52, 0.58, 0.66)
}

/// A sky region — an upper-left band that misses all geometry; shows the
/// atmosphere tint, neither solid black nor blown-out white. Measured luminance
/// ~133 (gate `[10, 230]` and `> solid`).
fn sky_rect(fb: &Framebuffer) -> Rect {
    Rect::from_fractional(fb, 0.05, 0.04, 0.45, 0.16)
}

// --- Stability-hash baselines ----------------------------------------------
//
// The §6.1 tripwire: for batches that are *supposed* to leave the image
// unchanged (B3, B4, B5), assert the readback hash equals the prior batch's
// stored hash. Re-blessed *only* by the batch that intentionally changes the
// image (B2's first-hit+atmosphere, B6's GI bounce + TAA — see the B5-vs-B6
// milestone note: B5's GI consumers run but their pre-B6 contribution is
// negligible, so B5 is an "image unchanged" batch, not the GI-visible one).
//
// `None` means "no baseline asserted for this batch" — used while a baseline is
// being blessed for the first time, or for batches that legitimately change the
// image (their gate is the region gate, not the hash). The Batch-2/3 baseline
// is `None` here: the e2e harness landed alongside Batch 3, and the readback is
// only bit-identical run-to-run *on the same binary* — a committed hash literal
// would be re-derived on the dev box anyway. B4 (the first "no visible change"
// batch to land *after* the harness) blesses the first real baseline by
// capturing the Batch-3 readback hash and pinning it here.
// Batch-4 note: the e2e-render-test.md "Remaining issue" suggested B4 bless the
// first real baseline by pinning the Batch-3 readback hash. On reflection that
// is NOT sensible to commit: the readback is only bit-identical run-to-run *on
// the same binary / GPU* (the harness's own §6.1 caveat — "a committed hash
// literal would just be re-derived on the dev box"), so a literal derived on
// this box would spuriously fail on every other. Kept `None` for B4 — the
// region gate (`assert_batch_4` re-runs the B2 emissive/solid/sky gate) is the
// primary "image unchanged" check and catches gross regressions; the hash
// remains the optional tripwire it was always specified as (§6.1). This is the
// deliberate-deferral path the harness doc itself allows.
// Batch-5 note: same reasoning — `hash_baseline(5)` stays `None`. B5 IS an
// "image unchanged" batch (the GI consumers' pre-B6 contribution is negligible
// — the B5-vs-B6 milestone moved the visible bounce to B6), so `assert_batch_5`
// re-runs the B2 region gate as the primary "image unchanged" check; the
// per-binary/GPU non-portability of a committed hash literal is unchanged.
fn hash_baseline(batch: u32) -> Option<u64> {
    match batch {
        _ => None,
    }
}

// --- Per-batch gate state --------------------------------------------------

/// State the per-batch gates may need beyond the single readback `Framebuffer`:
/// the stability hash of the current readback, and (for the B6 temporal gate) a
/// second consecutive-frame readback.
pub struct GateState<'a> {
    /// The primary readback.
    pub fb: &'a Framebuffer,
    /// A second readback one frame later — only populated for the Batch-6
    /// temporal-stability gate; `None` otherwise.
    pub fb_next: Option<&'a Framebuffer>,
}

// --- The per-batch gate functions ------------------------------------------

/// Batch 2 gate (4-plane first-hit + atmosphere) — the manual
/// "emissive-white / solid-black / sky" visual gate, mechanised
/// (`e2e-render-test.md` §6.2).
fn assert_batch_2(state: &GateState) -> Result<(), String> {
    let fb = state.fb;

    // The emissive block: near-white / high luminance (the emissive material is
    // the only lit thing pre-GI). Generous: a real failure is "the emissive
    // block went black", not sub-percent drift.
    let emissive = fb.region_mean(emissive_rect(fb));
    let emissive_lum = Framebuffer::luminance(emissive);
    if emissive_lum < 120.0 {
        return Err(format!(
            "Batch 2: emissive-block region too dark — luminance {emissive_lum:.1} \
             (expected > 120, mean rgba {emissive:?}). The emissive material is the only \
             lit thing pre-GI; if it is dark the first-hit pass is not running."
        ));
    }

    // A non-emissive solid block: near-black (no bounce light yet — Phase B
    // pre-GI gives non-emissive diffuse surfaces no direct light until GI).
    let solid = fb.region_mean(solid_block_rect(fb));
    let solid_lum = Framebuffer::luminance(solid);
    if solid_lum > 90.0 {
        return Err(format!(
            "Batch 2: non-emissive solid-block region too bright — luminance {solid_lum:.1} \
             (expected < 90, mean rgba {solid:?}). Pre-GI a non-emissive diffuse block \
             should be near-black; if it is bright the lighting math is wrong."
        ));
    }

    // The sky region: sky-coloured — not black, not white, luminance in a broad
    // mid band.
    let sky = fb.region_mean(sky_rect(fb));
    let sky_lum = Framebuffer::luminance(sky);
    if !(10.0..=230.0).contains(&sky_lum) {
        return Err(format!(
            "Batch 2: sky region luminance {sky_lum:.1} out of the [10, 230] band \
             (mean rgba {sky:?}). The sky corner should show the atmosphere tint — \
             neither solid black (atmosphere not running) nor blown-out white."
        ));
    }

    // The sky must be brighter than the un-lit solid block (the atmosphere is
    // lit, the pre-GI diffuse block is not) — a cheap relative sanity check
    // that does not depend on absolute tuning.
    if sky_lum <= solid_lum {
        return Err(format!(
            "Batch 2: sky luminance {sky_lum:.1} is not brighter than the un-lit solid \
             block {solid_lum:.1} — the atmosphere is not contributing."
        ));
    }

    Ok(())
}

/// Batch 3 gate — Phase B Batch 3 (`rayQueueCalc` + `globalIllum`) writes GI
/// buffers the blit does not read, so the image is **unchanged from Batch 2**
/// (`10-impl-b.md` Batch 3: "the done-bar is the passes dispatch clean, not the
/// image changes"). The gate is therefore the Batch-2 region gate re-run, plus
/// — once a baseline is blessed — the §6.1 stability-hash equality. The
/// pipeline-error scan + node-dispatch check (run unconditionally by the driver
/// / `run_e2e_render`) cover the new B3 pipelines.
fn assert_batch_3(state: &GateState) -> Result<(), String> {
    assert_batch_2(state)?;
    if let Some(baseline) = hash_baseline(3) {
        let actual = state.fb.stability_hash();
        if actual != baseline {
            return Err(format!(
                "Batch 3: stability hash {actual:#018x} != baseline {baseline:#018x} — \
                 Batch 3 must leave the image unchanged from Batch 2 (it only writes GI \
                 buffers the blit does not read). An unexpected image change is a regression."
            ));
        }
    }
    Ok(())
}

/// Batch 4 gate — Phase B Batch 4 (the 5 `renderSampleRefine` passes) writes
/// the 8×8-bucket refine buffers (`valid_samples_refined` /
/// `valid_samples_compressed` / `bucket_info`) that the blit does not read, so
/// the image is **unchanged from Batch 2/3** (`10-impl-b.md` Batch 4 done-bar:
/// "the 5 passes dispatch clean", not "the image changes" — `valid_samples_
/// compressed` is first read by Batch 5's `spatialResampling`). The gate is
/// therefore the Batch-2 region gate re-run, plus — once a baseline is blessed
/// — the §6.1 stability-hash equality. The `PipelineCache` error scan +
/// node-dispatch check (run unconditionally by the driver) cover the 5 new B4
/// pipelines + the `naadf_sample_refine` span.
fn assert_batch_4(state: &GateState) -> Result<(), String> {
    assert_batch_2(state)?;
    if let Some(baseline) = hash_baseline(4) {
        let actual = state.fb.stability_hash();
        if actual != baseline {
            return Err(format!(
                "Batch 4: stability hash {actual:#018x} != baseline {baseline:#018x} — \
                 Batch 4 must leave the image unchanged from Batch 2/3 (the 5 \
                 sample-refine passes only write GI refine buffers the blit does not \
                 read). An unexpected image change is a regression."
            ));
        }
    }
    Ok(())
}

/// Batch 5 gate — Phase B Batch 5 (`renderSpatialResampling` + the sparse
/// bilateral `renderDenoiseSplit`) builds the GI *consumers* — they write
/// `final_color` / `denoise_preprocessed`, and the chain is
/// `… → spatial_resampling → denoise → final_blit`. The done-bar is "the GI
/// consumer passes dispatch clean"; the image is **unchanged from Batch 2-4**.
///
/// **The B5-vs-B6 "GI visible" milestone — settled in the Batch-5 impl log:**
/// `09-design-b.md` §11 Batch 5 step 15 claims the GI bounce "is visible for the
/// first time". The Batch-4 "note for B5" carry-forward flagged this as
/// suspect, and the Batch-5 verification confirmed it: **the visible-bounce
/// milestone genuinely moves to Batch 6.** Reasoning:
/// - The 12-iteration neighbour-reservoir loop reads `valid_samples_compressed`
///   / `bucket_info` — the `renderSampleRefine` refine buffers, which are
///   *correct-but-empty* until Batch 6 wires `taa_dist_min_max` (Batch 4's
///   reprojection validity test rejects every sample with `dist_min_max ==
///   (0,0)`). So the reservoir path yields nothing pre-B6.
/// - The spatial pass's **sun sample** (`renderSpatialResampling.fx:321-339`)
///   *is* independent of the refine buffers and *is* wired + dispatched — but
///   in this enclosed test scene at the fixed e2e pose its contribution to
///   `final_color` is negligible (the visible diffuse geometry is largely
///   sun-shadowed / sun-averted; the whole-frame non-black fraction stays
///   bit-identical at 69.1%, and the screenshot is visually indistinguishable
///   from Batch 4).
/// So B5's image is stable like B3/B4 — the GI consumers run and write
/// `final_color`, but no visually significant bounce lands until Batch 6
/// populates the reservoir buffers. This gate is therefore the `assert_batch_2`
/// region gate re-run (exactly as `assert_batch_4` does — the GI consumers must
/// not have *broken* the first-hit / atmosphere image). `GI_LIT_BATCH` is `6`,
/// so B5 still uses the pre-GI luminance floor — the honest regime. The
/// `PipelineCache` error scan + the node-dispatch check (run unconditionally by
/// the driver) cover the new B5 pipelines + the `naadf_spatial_resampling` /
/// `naadf_denoise` spans.
fn assert_batch_5(state: &GateState) -> Result<(), String> {
    assert_batch_2(state)?;
    if let Some(baseline) = hash_baseline(5) {
        let actual = state.fb.stability_hash();
        if actual != baseline {
            return Err(format!(
                "Batch 5: stability hash {actual:#018x} != baseline {baseline:#018x} — \
                 Batch 5 must leave the image unchanged from Batch 2-4 (the GI \
                 consumers write `final_color`, but their pre-B6 contribution is \
                 negligible — the reservoir buffers are empty until Batch 6 wires \
                 `taa_dist_min_max`). An unexpected image change is a regression."
            ));
        }
    }
    Ok(())
}

/// Batch 6 gate — Phase B Batch 6 (the `base/` TAA rewire + `renderFinal` +
/// the final integration) is the **Phase-B deliverable**: the GI bounce lights
/// the scene for the first time.
///
/// **This is the first region gate that asserts the GI bounce is VISIBLE.**
/// Batch 6 wires the `base/` `ReprojectOld` to write `taa_dist_min_max`, which
/// un-blocks the `renderSampleRefine → valid_samples_compressed →
/// renderSpatialResampling` reservoir chain (the B5-vs-B6 milestone —
/// `10-impl-b.md`), so the indirect GI bounce composites into `final_color`,
/// `CalcNewTaaSample` folds it into `taa_sample_accum`, and the reverted
/// `base/` final blit shows it.
///
/// The gate:
/// 1. Re-runs the `assert_batch_2` emissive/sky checks — the emissive blocks
///    must still render, the atmosphere sky must still be clean (the GI rewire
///    must not have *broken* the first-hit / atmosphere image).
/// 2. **The positive GI check:** the `solid_block_rect` region — the dark
///    diffuse voxel geometry directly below the warm-white emissive block —
///    was near-black pre-GI (measured luminance ~4 through Batch 5, gate
///    `< 90` in `assert_batch_2`). Batch 6's GI bounce lights it: the gate
///    asserts the region has *brightened* past [`MIN_GI_BOUNCE_LUMINANCE`].
///    This is the check `assert_batch_5`'s first draft wrongly expected at
///    Batch 5 (`10-impl-b.md` Batch-5 "How the e2e harness batch-tracker was
///    set") — it correctly belongs here, at the batch the bounce actually
///    lands.
fn assert_batch_6(state: &GateState) -> Result<(), String> {
    let fb = state.fb;

    // (1) The emissive blocks + the atmosphere sky must still render — re-run
    // the emissive + sky portions of the Batch-2 gate. (NOT the full
    // `assert_batch_2` — its `solid_block_rect < 90` "near-black" check is
    // exactly what Batch 6 *inverts*: the diffuse geometry is now GI-lit.)
    let emissive = fb.region_mean(emissive_rect(fb));
    let emissive_lum = Framebuffer::luminance(emissive);
    if emissive_lum < 120.0 {
        return Err(format!(
            "Batch 6: emissive-block region too dark — luminance {emissive_lum:.1} \
             (expected > 120, mean rgba {emissive:?}). The emissive blocks must still \
             render with the `base/` TAA path + the reverted `taa_sample_accum` blit."
        ));
    }
    let sky = fb.region_mean(sky_rect(fb));
    let sky_lum = Framebuffer::luminance(sky);
    if !(10.0..=230.0).contains(&sky_lum) {
        return Err(format!(
            "Batch 6: sky region luminance {sky_lum:.1} out of the [10, 230] band \
             (mean rgba {sky:?}). The atmosphere sky must still be clean — the GI \
             rewire must not have broken the first-hit / atmosphere image."
        ));
    }

    // (2) The positive GI check: the dark diffuse geometry has BRIGHTENED.
    let solid = fb.region_mean(solid_block_rect(fb));
    let solid_lum = Framebuffer::luminance(solid);
    if solid_lum < MIN_GI_BOUNCE_LUMINANCE {
        return Err(format!(
            "Batch 6: diffuse-geometry region too dark — luminance {solid_lum:.1} \
             (expected >= {MIN_GI_BOUNCE_LUMINANCE} — the GI bounce should light it; \
             it measured ~4 near-black through Batch 5, mean rgba {solid:?}). \
             If it is still near-black the `taa_dist_min_max` wiring did not un-block \
             the `renderSampleRefine → renderSpatialResampling` reservoir chain, or \
             the final blit is not reading the GI-folded `taa_sample_accum`."
        ));
    }

    Ok(())
}

/// The minimum `solid_block_rect` luminance for [`assert_batch_6`]'s positive
/// "GI bounce is visible" check. The dark diffuse voxel geometry measured
/// luminance ~4 (near-black) through Batch 5; Batch 6's GI bounce should light
/// it.
///
/// **Held at the 12.0 design-intent value (2026-05-15, Batch-6 TAA-path
/// black-frame fix).** With the `GpuTaaParams` layout bug fixed, the TAA path
/// works and the frame is no longer black — but the `solid_block_rect` region
/// still measures luminance ~4.4 (mean rgba ~[3.8, 4.5, 5.4]), barely above the
/// pre-GI ~4.0. The GI bounce is *not yet* visibly lighting the dark diffuse
/// geometry: a remaining issue in the GI-consumer chain (`renderGlobalIllum →
/// renderSampleRefine → renderSpatialResampling → renderDenoiseSplit →
/// final_color`) that only became observable now that the TAA path delivers a
/// real `taa_sample_accum`. This threshold is deliberately **not** rubber-
/// stamped down to 4.4 — the gate honestly fails until the GI bounce actually
/// lands (see `10-impl-b.md`'s Batch-6 black-frame-fix section).
const MIN_GI_BOUNCE_LUMINANCE: f32 = 12.0;

// --- Dispatch tables -------------------------------------------------------

/// The expected render-graph spans for a given batch — the node-dispatch check
/// (`e2e-render-test.md` §8) asserts each has a recorded measurement.
///
/// Batches 1-3 node set (`render/mod.rs` `Core3d` chain + `graph_b.rs` /
/// `graph.rs` span consts): atmosphere precompute → 4-plane first-hit →
/// rayQueueCalc → globalIllum → final blit.
///
/// Batch 4 adds `naadf_sample_refine` — the 5 `renderSampleRefine` passes are 5
/// separate node systems but share ONE span (`graph_b.rs SAMPLE_REFINE_SPAN` —
/// `09-design-b.md` §4.7 "one span recommended"), so one new row entry covers
/// all five. Batch 5 adds `naadf_spatial_resampling` + `naadf_denoise` (the two
/// `renderDenoiseSplit` passes share one span). Batch 6 adds the `base/` TAA
/// nodes: `naadf_taa_reproject` (the `ReprojectOld` pass, re-added to the
/// chain) + `naadf_calc_new_taa_sample` (the new `CalcNewTaaSample` pass).
pub fn expected_spans(batch: u32) -> &'static [&'static str] {
    match batch {
        0..=3 => &[
            "naadf_atmosphere",
            "naadf_first_hit",
            "naadf_ray_queue",
            "naadf_global_illum",
            "naadf_final_blit",
        ],
        // B4: + `naadf_sample_refine` (the 5 sample-refine passes' shared span).
        4 => &[
            "naadf_atmosphere",
            "naadf_first_hit",
            "naadf_ray_queue",
            "naadf_global_illum",
            "naadf_sample_refine",
            "naadf_final_blit",
        ],
        // B5: + `naadf_spatial_resampling` + `naadf_denoise` (the GI consumers).
        5 => &[
            "naadf_atmosphere",
            "naadf_first_hit",
            "naadf_ray_queue",
            "naadf_global_illum",
            "naadf_sample_refine",
            "naadf_spatial_resampling",
            "naadf_denoise",
            "naadf_final_blit",
        ],
        // B6: + `naadf_taa_reproject` + `naadf_calc_new_taa_sample` (the `base/`
        // TAA path rewired into the chain — the full Phase-B node set).
        _ => &[
            "naadf_atmosphere",
            "naadf_first_hit",
            "naadf_taa_reproject",
            "naadf_ray_queue",
            "naadf_global_illum",
            "naadf_sample_refine",
            "naadf_spatial_resampling",
            "naadf_denoise",
            "naadf_calc_new_taa_sample",
            "naadf_final_blit",
        ],
    }
}

/// Whether the current batch's gate needs the second consecutive-frame readback
/// (only Batch 6's temporal-stability gate does). The driver checks this to
/// decide whether to shoot a second screenshot.
pub fn batch_needs_second_frame(batch: u32) -> bool {
    batch >= 6
}

/// Run the region gate for `batch`. Older batches' gates are kept as called
/// helpers so an earlier-gate regression still trips (`e2e-render-test.md`
/// §6.4).
pub fn batch_gate(batch: u32, state: &GateState) -> Result<(), String> {
    match batch {
        // Batches 0/1 land no visible-change gate of their own (Batch 1 is the
        // atmosphere precompute, written into a buffer the blit does not read);
        // the degenerate-frame floor + the pipeline scan + node-dispatch cover
        // them. Run the Batch-2 gate from Batch 2 on.
        0..=1 => Ok(()),
        2 => assert_batch_2(state),
        3 => assert_batch_3(state),
        4 => assert_batch_4(state),
        5 => assert_batch_5(state),
        // B6 — the Phase-B deliverable: the GI bounce is visible.
        _ => assert_batch_6(state),
    }
}
