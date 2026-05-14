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
/// earlier-gate regression still trips). Phase B Batches 1-4 exist
/// (`10-impl-b.md`); bump this as B5/B6 land.
pub const CURRENT_BATCH: u32 = 4;

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
// unchanged (B3, B4), assert the readback hash equals the prior batch's stored
// hash. Re-blessed *only* by the batch that intentionally changes the image
// (B2's first-hit+atmosphere, B5's GI bounce, B6's TAA).
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
/// all five. B5 adds the denoiser span, B6 the TAA-node spans.
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
        _ => &[
            "naadf_atmosphere",
            "naadf_first_hit",
            "naadf_ray_queue",
            "naadf_global_illum",
            "naadf_sample_refine",
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
        // B5+ : add `5 => assert_batch_5(state)`, etc.
        _ => assert_batch_4(state),
    }
}
