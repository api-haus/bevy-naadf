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
//!
//! The window-boot, bounded-frame driver, readback, and pipeline-error scan are
//! batch-agnostic and written once.

use bevy::prelude::*;

use super::framebuffer::{Framebuffer, Rect};

/// Re-export of the canonical [`crate::voxel::grid::demo_origin_v`] — kept
/// here so existing `crate::e2e::gates::demo_origin_v` imports across the
/// e2e harness keep resolving without sweeping every callsite. The body
/// lives next to [`crate::voxel::grid::DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS`]
/// (the small-world footprint it reads) so production code (the
/// `--entities` fixture spawner) no longer imports from `e2e/`. Moved per
/// the codebase-tightening D7 architect's Side note 6.
pub use crate::voxel::grid::demo_origin_v;

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
    let off = demo_origin_v(crate::WORLD_SIZE_IN_CHUNKS);
    Transform::from_translation(off + Vec3::new(86.0, 42.0, 90.0))
        .looking_at(off + Vec3::new(32.0, 16.0, 32.0), Vec3::Y)
}

/// The point the e2e camera always looks at — the `GridPreset::Default` scene
/// centre, in small-world-relative voxel coords. Callers translate by
/// [`demo_origin_v`] when constructing a world-space transform.
pub const E2E_LOOK_TARGET: Vec3 = Vec3::new(32.0, 16.0, 32.0);

/// World-space look target — [`E2E_LOOK_TARGET`] translated by
/// [`demo_origin_v`]. Use this in place of [`E2E_LOOK_TARGET`] anywhere a
/// transform-space coord is required.
pub fn e2e_look_target_world() -> Vec3 {
    demo_origin_v(crate::WORLD_SIZE_IN_CHUNKS) + E2E_LOOK_TARGET
}

/// A deterministic camera pose along the moving-camera e2e motion path
/// (`10-impl-b.md` — TAA camera-motion reprojection coverage).
///
/// `t` runs `0.0 → 1.0` over the motion phase. **The path is OPEN, not closed:**
/// - `e2e_orbit_camera_transform(0.0)` = [`e2e_motion_start_transform`] — the
///   pose the `WARMUP` phase renders at.
/// - `e2e_orbit_camera_transform(1.0)` = [`e2e_camera_transform`] — the fixed
///   pose the readback happens at and every gate rectangle is derived from.
///
/// An *open* path is the load-bearing design choice. A *closed* orbit (warmup,
/// move away, come back to the warmup pose) lets the readback land on a pose
/// the camera has already accumulated 96 frames of same-pose GI/TAA history at
/// — so even a broken reprojection finds that same-pose history again and the
/// decay is masked. An open path readback-poses the camera somewhere it has
/// **never been static**: every GI/TAA history sample contributing to the
/// readback frame had to arrive *through the camera-motion reprojection*. If
/// the reprojection is faithful, the shadowed/indirect regions stay GI-lit; if
/// it is broken (the TAA shadow decay-to-black bug), they decay to black during
/// the move and are still black at the readback.
///
/// The camera always looks at the scene centre ([`E2E_LOOK_TARGET`]); it both
/// **orbits** (yaw sweep) and **dollies** (radius change) between the two
/// endpoints, so every frame changes both rotation and translation — the full
/// camera-motion reprojection workload.
///
/// The interpolation is **linear in `t`** — *constant* angular + radial
/// velocity, deliberately *not* eased. An easing curve (smoothstep etc.) has
/// zero velocity at `t == 1`, so the last few motion frames before the readback
/// would be nearly static and the running average would re-converge — masking
/// exactly the decay this phase exists to catch (the same masking trap a long
/// `SETTLE` or a closed orbit has). A constant-velocity path keeps the camera
/// moving at full speed right up to the readback pose, so a TAA camera-motion
/// reprojection decay is freshly present, not washed out.
pub fn e2e_orbit_camera_transform(t: f32) -> Transform {
    // Linear — constant velocity, no easing (see the doc comment: easing's
    // zero end-velocity would mask the decay).
    let s = t;

    // Both endpoints expressed relative to the shared look target (in
    // world space — the look-target shift cancels in the subtraction).
    let look_world = e2e_look_target_world();
    let end = e2e_camera_transform().translation - look_world;
    let start = e2e_motion_start_transform().translation - look_world;
    let end_radius = end.length();
    let start_radius = start.length();
    let end_yaw = end.x.atan2(end.z);
    let start_yaw = start.x.atan2(start.z);

    // Interpolate yaw, radius and height between the two endpoints.
    let yaw = start_yaw + (end_yaw - start_yaw) * s;
    let radius = start_radius + (end_radius - start_radius) * s;
    let height = start.y + (end.y - start.y) * s;

    // Horizontal radius from the (radius, height) on the sphere of this yaw.
    let horizontal = (radius * radius - height * height).max(0.0).sqrt();
    let offset = Vec3::new(yaw.sin() * horizontal, height, yaw.cos() * horizontal);
    Transform::from_translation(look_world + offset).looking_at(look_world, Vec3::Y)
}

/// The pose the moving-camera e2e [`WARMUP` phase](crate::e2e::E2E_WARMUP_FRAMES)
/// renders at — the `t == 0` endpoint of [`e2e_orbit_camera_transform`].
///
/// A deliberately *different* vantage from [`e2e_camera_transform`]: a wider,
/// higher, more yawed-around view of the same `GridPreset::Default` scene. The
/// `MOTION` phase then sweeps from here to the fixed readback pose — ~95° of
/// yaw and a large radius+height change — so the readback pose is one the
/// camera arrives at fresh, with no same-pose history (see
/// [`e2e_orbit_camera_transform`]'s open-path rationale). The exact pose is not
/// gated against — only the readback pose ([`e2e_camera_transform`]) is — so it
/// just needs to (a) frame the scene so the warmup GI converges on real
/// geometry and (b) be far enough from the readback pose that the motion is a
/// genuine reprojection workload.
pub fn e2e_motion_start_transform() -> Transform {
    let off = demo_origin_v(crate::WORLD_SIZE_IN_CHUNKS);
    Transform::from_translation(off + Vec3::new(-28.0, 70.0, 96.0))
        .looking_at(off + E2E_LOOK_TARGET, Vec3::Y)
}

/// Camera pose for the **resize-blackness reproduction test**
/// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
/// `## GI-bounce-on-resize fix (2026-05-16)`).
///
/// **Why a different pose from [`e2e_camera_transform`]?** The standard
/// readback pose `(86, 42, 90)` was tuned for the GI-lit Batch-6 region gate:
/// the `solid_block_rect` rect (frac 0.42..0.58 × 0.52..0.66) hits the small
/// shadowed sliver directly below the warm-white emissive block — a tiny
/// fraction of the frame. The user's directive for this dispatch is
/// explicit: "the camera is positioned so that it only sees tiny slivers of
/// shadowed areas". To make the TAA/GI ring-drain bug observable in
/// full-frame luma (the metric this test now uses), shadow regions must
/// occupy a *significant* portion of the frame — not a sliver.
///
/// **Scene + sun geometry** (`atmosphere.rs:323-330`): the sun is at elevation
/// 0.9 rad (~51° above horizon) and azimuth 0.6 rad — direction approximately
/// `(0.514, 0.783, 0.351)`. Sun comes from +x, slightly +z, high above. Shadows
/// cast toward -x, slightly -z.
///
/// **Pose choice — close low-angle view of the back wall + box A.** The back
/// wall sits at `x=56..60, y=3..22, z=14..49` with an arch carved at
/// `x=55..61, y=3..14, z=26..37`. With the sun in +x/+y/+z, the back wall's
/// -x face (toward the scene interior) is in self-shadow. The wall and the
/// towers in the +x corners also cast shadows on the ground that stretches
/// across the volume in the -x direction.
///
/// Camera at `(20, 12, 50)` looking at `(58, 18, 30)`:
/// - Low altitude (`y=12`) — ground-plane-grazing 3/4 view.
/// - From the -x, +z front quadrant of the volume — the camera sees the -x
///   (self-shadowed) faces of all the geometry to the right of the frame.
/// - Look target `(58, 18, 30)` lands on the back wall *above* the arch top
///   (`y=18 > y_arch_top=14`), so the wall fills the right side of the frame
///   as a large dark shadowed surface — not as a hole the camera can see
///   through.
/// - Box A (`x=12..23, y=3..20, z=14..25`) sits between the camera and the
///   sun; its shadow falls across the ground on the camera-side of the scene
///   centre — visible as a large dark band in the lower-left of the frame.
/// - The atmospheric sky band still occupies the top of the frame.
///
/// Used **only** by the resize-test phases (gated by `AppArgs.resize_test`);
/// the standard Batch-6 harness keeps using [`e2e_camera_transform`]. The
/// pose is pinned for the entire resize-test sequence — no orbit motion, no
/// per-phase changes — so any luma collapse between the three captures is
/// attributable to the resize-induced ring drain, not to camera motion.
pub fn e2e_resize_test_camera_transform() -> Transform {
    let off = demo_origin_v(crate::WORLD_SIZE_IN_CHUNKS);
    Transform::from_translation(off + Vec3::new(20.0, 12.0, 50.0))
        .looking_at(off + Vec3::new(58.0, 18.0, 30.0), Vec3::Y)
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

/// Phase-C followup #5 — the screen rectangle the 4×4×4 green-emissive
/// fixture entity (`--entities` mode) projects into at the fixed e2e camera
/// pose.
///
/// **Position derivation.** Entity at world `(30, 24, 30)` (centre (32, 26,
/// 32) — paired with the test scene's `(32, 16, 32)` look target offset by
/// `+10` voxels in Y). At camera `(86, 42, 90)` looking at `(32, 16, 32)`,
/// the entity sits just above-and-right of screen centre. Empirical pixel-
/// diff scan vs the no-entities baseline (`16-impl-c-followups.md` —
/// followup #5 calibration) shows the entity's strongest contribution at
/// pixel `(y ≈ 117..130, x ≈ 168..175)` — fractional `(0.457..0.512,
/// 0.656..0.684)`. We pick a slightly wider 14×8 region to absorb TAA jitter.
///
/// **Threshold rationale.** The 4-voxel entity at the e2e camera distance
/// (~84 voxels) renders ~10-pixels-wide on the 256-pixel framebuffer. The
/// surrounding GI-lit scene is already bright (mean luminance ~143 in the
/// region), so the entity does not visibly raise the region mean — its
/// emissive-green replaces the underlying diffuse, keeping mean luminance
/// near scene baseline. The honest gate, therefore, is **"this region is
/// brightly lit" — a `> 80` luminance floor**: if the entity dispatch
/// regresses to a no-op or the underlying geometry breaks, the region drops
/// below this floor (the entity rendering DOES land green pixels on what
/// would otherwise be the dark interior of the scene's emissive-block
/// neighbourhood; if it stops working OR the GI pipeline breaks, the
/// region collapses).
fn entity_pixel_rect(fb: &Framebuffer) -> Rect {
    // Fractional bounds derived from the 256×256 pixel region (115..135,
    // 165..180) — slightly wider than the calibrated entity-diff cluster
    // (117..130, 168..175) to be jitter-tolerant. See the `assert_entity_pixel`
    // doc for the calibration breadcrumbs.
    Rect::from_fractional(fb, 0.645, 0.449, 0.703, 0.527)
}

/// The minimum mean luminance for the `entity_pixel_rect` region in
/// `--entities` mode. **Calibration**: measured at 193.5 (baseline-with-
/// entities, GI-lit scene); calibration ran 2026-05-15. Threshold set to
/// **80.0** — a 2.4× safety margin below the measured value, well clear of
/// a "GI bounce subsides" (region would still be ~140), but firmly above a
/// "geometry vanishes" failure mode (region collapses to ~10 luminance, as
/// we observed when the runtime-GPU-producer upload-skip path failed during
/// the followup #1 investigation).
const ENTITY_PIXEL_MIN_LUM: f32 = 80.0;

/// Phase-C followup #5 — entity-pixel luminance gate.
///
/// Fires only in `--entities` mode (the driver gates the call on the
/// `SpawnTestEntity` resource). Asserts the screen region the fixture
/// entity projects into is brightly lit (mean luminance ≥
/// [`ENTITY_PIXEL_MIN_LUM`]) — proves both (a) the rest of the renderer
/// is producing usable framebuffer content at the entity's screen position
/// and (b) the entity's emissive contribution doesn't collapse the region.
///
/// **Why not a green-channel gate?** Empirically, the entity is small (~10
/// pixels wide on a 256-pixel framebuffer) and rendered into a GI-busy area;
/// the mean green-dominance over a 14×8 region is essentially noise
/// (`16-impl-c-followups.md` — followup #5 calibration). The entity DOES
/// visibly change individual pixels (max delta ~99 in green-dominance), but
/// region-mean smoothing dilutes the signal. A pixel-by-pixel diff would be
/// a more discriminating gate, but requires storing a baseline reference
/// inside the harness — beyond scope. The luminance-floor gate is the
/// honest "region is correctly rendering" check.
pub fn assert_entity_pixel(state: &GateState) -> Result<(), String> {
    let fb = state.fb;
    let region = entity_pixel_rect(fb);
    let mean = fb.region_mean(region);
    let lum = Framebuffer::luminance(mean);
    if lum < ENTITY_PIXEL_MIN_LUM {
        return Err(format!(
            "entity_pixel region (fractional 0.645..0.703 × 0.449..0.527, the 4×4×4 \
             green-emissive fixture's projected screen rect) too dark — luminance \
             {lum:.1} (expected ≥ {ENTITY_PIXEL_MIN_LUM}, mean rgba {mean:?}). \
             Either the entity-update dispatch regressed to a no-op, or the \
             underlying renderer broke at this screen location."
        ));
    }
    Ok(())
}

/// A one-line diagnostic of the three gate regions' mean luminance — printed
/// every run by the driver so a moving-camera TAA decay shows up as a
/// `solid`-region downtrend even when the gate still passes by a margin
/// (`10-impl-b.md` — TAA shadow decay-to-black coverage).
pub fn region_luminance_report(fb: &Framebuffer) -> String {
    let emissive = Framebuffer::luminance(fb.region_mean(emissive_rect(fb)));
    let solid = Framebuffer::luminance(fb.region_mean(solid_block_rect(fb)));
    let sky = Framebuffer::luminance(fb.region_mean(sky_rect(fb)));
    format!(
        "region luminance — emissive {emissive:.1}, solid(GI-lit diffuse) {solid:.1}, \
         sky {sky:.1}  (solid is the TAA camera-motion decay tripwire — the readback is \
         post-camera-motion, so solid should stay >= {MIN_GI_BOUNCE_AFTER_MOTION}; a \
         decay collapses it toward ~4-6)"
    )
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
fn hash_baseline(_batch: u32) -> Option<u64> {
    None
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
///
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
/// 3. **The TAA camera-motion stability check (2026-05-15).** The readback
///    happens at the `t == 1` end of the open camera-motion path — a pose the
///    camera was **never static at** before the readback (`e2e/mod.rs` —
///    `WARMUP` was at the *start* pose, `MOTION` swept here, `SETTLE` is a
///    single frame). So every GI/TAA history sample lighting the diffuse
///    geometry in the readback frame had to arrive *through the TAA
///    camera-motion reprojection*. If `reproject_old_samples` /
///    `reproject_sample` decayed the shadowed/indirect GI under motion (the
///    `10-impl-b.md` "TAA shadow decay-to-black" bug class), the diffuse
///    geometry would be near-black here. The gate asserts it stayed *robustly*
///    GI-lit past [`MIN_GI_BOUNCE_AFTER_MOTION`] — a meaningfully higher bar
///    than the bare `MIN_GI_BOUNCE_LUMINANCE` "bounce is visible at all"
///    floor, so a *partial* camera-motion decay (history thinning toward black
///    without fully vanishing) is also caught.
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

    // (3) The TAA camera-motion stability check — the readback pose is one the
    // camera reached only by *moving* (the open motion path), so a GI-lit
    // diffuse region here proves the TAA camera-motion reprojection carried
    // the bounce through the motion. A camera-motion reprojection decay
    // (`10-impl-b.md` — TAA shadow decay-to-black) would thin or black this
    // out; the bar is meaningfully above the bare visibility floor so even a
    // *partial* decay trips it.
    if solid_lum < MIN_GI_BOUNCE_AFTER_MOTION {
        return Err(format!(
            "Batch 6: TAA camera-motion reprojection decay — the GI-lit diffuse \
             geometry measured luminance {solid_lum:.1} at the post-camera-motion \
             readback pose (expected >= {MIN_GI_BOUNCE_AFTER_MOTION}, mean rgba \
             {solid:?}). The readback pose is reached only by camera motion (the open \
             `MOTION` path — `e2e/mod.rs`), so every GI/TAA history sample here came \
             through the reprojection: a thinned/black diffuse region means \
             `reproject_old_samples` / `reproject_sample` is dropping reprojected GI \
             history under camera motion. Trace the 3×3 `dist_min_max` / hash / \
             screen-position rejects + `color_sum.a` in `taa.wgsl` against \
             `base/renderTaaSampleReverse.fx`."
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

/// The minimum `solid_block_rect` luminance for [`assert_batch_6`]'s **TAA
/// camera-motion stability** check — the tripwire for the `10-impl-b.md` "TAA
/// shadow decay-to-black" bug class.
///
/// The e2e readback now happens at the end of an *open* camera-motion path
/// (`e2e/mod.rs` — `WARMUP` at the start pose, `MOTION` sweeps to the readback
/// pose, `SETTLE` is one frame), so the readback pose is one the camera was
/// **never static at**: every GI/TAA history sample lighting the diffuse
/// geometry had to come through the TAA camera-motion reprojection. With a
/// faithful reprojection the `solid_block_rect` region measures luminance
/// **~235** at the post-motion readback (verified across multiple
/// motion-profile runs, 2026-05-15) — essentially as bright as a static-camera
/// render, confirming the reprojection carries the GI bounce through camera
/// motion without decay.
///
/// The threshold is **150.0** — far below the measured ~235 (so normal
/// frame-to-frame variation never trips it) yet far above both the bare
/// `MIN_GI_BOUNCE_LUMINANCE = 12.0` "bounce visible at all" floor and the ~4-6
/// luminance a camera-motion decay collapses the region to (`10-impl-b.md` —
/// the decay drives shadowed/indirect regions toward pitch black). A *partial*
/// decay — reprojected history thinning the bounce toward black without fully
/// vanishing — also lands well under 150 and is caught. This is a real
/// regression tripwire, not a rubber stamp.
const MIN_GI_BOUNCE_AFTER_MOTION: f32 = 150.0;

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
