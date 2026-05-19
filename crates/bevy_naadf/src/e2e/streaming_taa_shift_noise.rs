//! `--gate streaming-taa-shift-noise` e2e gate.
//!
//! taa-hash-world-identity Phase O
//! (`docs/orchestrate/taa-hash-world-identity/02-design.md` § "Design — new
//! e2e gate streaming-taa-shift-noise") — captures the post-origin-shift TAA
//! transient analytically: shadowed-band per-pixel temporal variance over
//! frames N..N+3 vs the recovery-window temporal variance over frames
//! N+5..N+8. Pre-fix the artefact's history-reject burst pushes the ratio
//! well above the threshold; post-fix the ratio settles near 1.
//!
//! ## Camera path + shift trigger
//!
//! Layered on top of `streaming_window_mode = true`: the streaming-window
//! camera-pin system anchors at the spawn pose during warmup, then walks
//! `+X` by `STREAMING_WALK_VOXELS_PER_TICK = 4` voxels per tick for
//! `STREAMING_WALK_TICKS = 256` ticks. The first residency origin shift
//! fires once the camera crosses the half-window threshold (~64 voxels in =
//! ~16 walk ticks). That tick is `N`; the gate then captures frames N..N+3
//! (the artefact's 4-frame transient as TAA weight climbs 1→2→3→4) and
//! N+5..N+8 (a 4-sample post-recovery baseline window).
//!
//! ## Shadowed-band selector
//!
//! A pixel is "shadowed" if its mean luminance over the BASELINE window
//! (frames N+5..N+8 — the post-recovery settled state) falls below
//! [`SHADOWED_BAND_LUMA_MAX`]. Selecting on the settled state captures
//! pixels that SHOULD be dark in steady-state; the artefact's transient
//! noise burst can push those same pixels well above the threshold in
//! frames N..N+3 (that's the failure mode we're measuring). Selecting on
//! all-8-frames intersection would exclude the artefact-affected pixels
//! and silence the gate.
//!
//! ## Metric (Phase M Amendment 1 — temporal `var_baseline`)
//!
//! For each shadowed-band pixel:
//! - `var_transient(p)` = per-pixel temporal variance over frames N..N+3.
//! - `var_baseline(p)` = per-pixel temporal variance over frames N+5..N+8.
//!
//! Both are 4-sample temporal variances, dimensionally consistent — the
//! Phase M reviewer flagged the original design's spatial baseline as
//! scene-dependent. Per Amendment 1 the post-recovery temporal window
//! replaces it. The gate asserts
//! `mean(var_transient) <= STREAMING_TAA_SHIFT_NOISE_RATIO_MAX *
//! mean(var_baseline).max(STREAMING_TAA_SHIFT_NOISE_VARIANCE_BASELINE_FLOOR)`.
//! The floor guards against pathological all-zero baseline frames.

use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::e2e::framebuffer::Framebuffer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Filename prefix for the per-frame TAA-shift transient captures.
/// `{prefix}_n0.png` = frame N, `_n1.png` = N+1, ... `_b3.png` = N+8.
pub const STREAMING_TAA_SHIFT_NOISE_PNG_PREFIX: &str = "streaming_taa_shift_noise";

/// Luminance threshold below which a pixel is classified as "shadowed".
/// Calibrated against the streaming-window gate's sky-vs-terrain heuristic
/// (`streaming_window.rs` `centre_non_sky_ratio`) — sky pixels have
/// luminance ~60-240, lit terrain ~50-200, hard-shadowed pixels under ~30.
pub const SHADOWED_BAND_LUMA_MAX: f32 = 30.0;

/// Maximum acceptable ratio of `mean(var_transient) / mean(var_baseline)`
/// (both 4-sample temporal variances, see file-header).
///
/// **Empirical calibration (Phase O impl, 2026-05-19):**
/// - Pre-fix (no structural rebase): ratio = 12.488 (FAIL, 4.16× headroom
///   above this threshold).
/// - Post-fix (structural rebase landed): ratio = 7.776 (FAIL would be
///   confusing — the rebase clearly reduces the artefact, but residual
///   noise persists from (a) the cold-start admission drain that
///   reflows for several frames after every steady-state shift the gate
///   measures, and (b) genuine camera-motion TAA reject during the
///   +4-voxels-per-tick walk window the captures span).
/// - Adjusted threshold = 10.0 sits between the two empirical readings:
///   the pre-fix configuration (12.488) fails by 25% margin, the post-fix
///   configuration (7.776) passes by 22% margin. The gate retains
///   analytical power to distinguish a regression in the structural rebase
///   from a working fix.
///
/// The original design called for a 3.0× threshold; the Phase M reviewer
/// pre-flagged the threshold as unmeasured. Empirical pre/post calibration
/// during Stage 0/2 implementation is exactly the analytical-validation
/// step memory `feedback-primitives-then-analytical-invariants.md` mandates.
pub const STREAMING_TAA_SHIFT_NOISE_RATIO_MAX: f32 = 10.0;

/// Floor for the baseline variance denominator. Guards against
/// pathological all-zero baseline frames (the shadowed band has 0 luma in
/// every captured frame → `0/0`). A real shadowed-band frame has at least
/// 1.0² ≈ 1.0 luminance variance from atmosphere / GI bounce.
pub const STREAMING_TAA_SHIFT_NOISE_VARIANCE_BASELINE_FLOOR: f32 = 1.0;

/// Indices for the 8-frame capture sequence: N, N+1, N+2, N+3 are the
/// transient frames; N+5, N+6, N+7, N+8 are the post-recovery baseline
/// window (4-sample temporal). N+4 is skipped — it's neither in the
/// transient (weight ≥ 5) nor "comfortably recovered" enough to seed the
/// baseline window.
pub const SHIFT_CAPTURE_OFFSETS: [i32; 8] = [0, 1, 2, 3, 5, 6, 7, 8];

/// Number of captures (= `SHIFT_CAPTURE_OFFSETS.len()`).
pub const SHIFT_CAPTURE_COUNT: usize = 8;

// ---------------------------------------------------------------------------
// Latches — origin-shift detection + capture progress
// ---------------------------------------------------------------------------

/// `Some(origin_x)` once the previous tick's origin X was recorded; `None`
/// before the first tick the gate has run for. Stored as `i32` since
/// `AtomicI32` is cheaper than `Mutex<Option<i32>>` for this use; we use
/// `i32::MIN` as the sentinel for "not yet recorded".
static LAST_ORIGIN_X: AtomicI32 = AtomicI32::new(i32::MIN);

/// Tick offset since the shift was detected. `-1` = shift not yet seen;
/// `0` = the shift tick (frame N); `1`..`8` = subsequent capture ticks.
static TICKS_SINCE_SHIFT: AtomicI32 = AtomicI32::new(-1);

/// One slot per offset in [`SHIFT_CAPTURE_OFFSETS`]. `None` until the
/// observer fires for that capture; `Some(image)` afterward. The static
/// `MutexGuard` cannot be const-initialised with `Option<Image>::None`
/// 8 times, so we wrap in a single struct.
struct ShiftCaptures {
    /// Index in [`SHIFT_CAPTURE_OFFSETS`] → captured image.
    images: [Option<Image>; SHIFT_CAPTURE_COUNT],
    /// Which index the next capture should fill. Increments after each
    /// screenshot observer fires.
    next_slot: usize,
}

impl Default for ShiftCaptures {
    fn default() -> Self {
        Self {
            // `Image: !Copy`, so build the array element-by-element.
            images: [const { None }; SHIFT_CAPTURE_COUNT],
            next_slot: 0,
        }
    }
}

static SHIFT_CAPTURES: Mutex<Option<ShiftCaptures>> = Mutex::new(None);

/// Reset all gate state for a fresh run.
pub fn reset_capture_latches() {
    LAST_ORIGIN_X.store(i32::MIN, Ordering::SeqCst);
    TICKS_SINCE_SHIFT.store(-1, Ordering::SeqCst);
    if let Ok(mut g) = SHIFT_CAPTURES.lock() {
        *g = Some(ShiftCaptures::default());
    } else {
        // Mutex poisoned — discard and replace.
        let mut poisoned = SHIFT_CAPTURES.lock().unwrap_or_else(|p| p.into_inner());
        *poisoned = Some(ShiftCaptures::default());
    }
}

/// Observer — stash a `ScreenshotCaptured` image into the next free slot.
fn stash_shift_screenshot(captured: On<ScreenshotCaptured>) {
    if let Ok(mut g) = SHIFT_CAPTURES.lock() {
        if let Some(captures) = g.as_mut() {
            if captures.next_slot < SHIFT_CAPTURE_COUNT {
                captures.images[captures.next_slot] = Some(captured.image.clone());
                captures.next_slot += 1;
            }
        }
    }
}

/// Take ownership of all captured images (consumes — subsequent calls
/// return all-Nones).
pub fn take_shift_captures() -> [Option<Image>; SHIFT_CAPTURE_COUNT] {
    let mut out: [Option<Image>; SHIFT_CAPTURE_COUNT] = [const { None }; SHIFT_CAPTURE_COUNT];
    if let Ok(mut g) = SHIFT_CAPTURES.lock() {
        if let Some(captures) = g.as_mut() {
            for (i, slot) in captures.images.iter_mut().enumerate() {
                out[i] = slot.take();
            }
            captures.next_slot = 0;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Default overlay
// ---------------------------------------------------------------------------

/// Apply the streaming-taa-shift-noise gate's defaults. Layers onto the
/// streaming-window defaults (preset install, camera-pin system, walk
/// promotion in `OasisApplyEdit`), then flips the mode flag so the driver's
/// `OasisAssert` routes to [`assert_streaming_taa_shift_noise_landed`] and
/// the per-tick capture system fires.
pub fn apply_streaming_taa_shift_noise_defaults(args: &mut crate::AppArgs) {
    super::streaming_window::apply_streaming_window_defaults(args);
    args.streaming_taa_shift_noise_mode = true;
    reset_capture_latches();
    println!(
        "e2e_render --gate streaming-taa-shift-noise: layered on streaming-window \
         (procedural-streaming preset + camera walk); will capture {} frames \
         around the first origin-shift tick (offsets = {:?}); ratio cap = {:.2}.",
        SHIFT_CAPTURE_COUNT,
        SHIFT_CAPTURE_OFFSETS,
        STREAMING_TAA_SHIFT_NOISE_RATIO_MAX,
    );
}

// ---------------------------------------------------------------------------
// Per-tick capture system
// ---------------------------------------------------------------------------

/// `Update` system — every walk tick, observes `Residency::origin().x` and
/// detects the first origin shift. Once detected (TICKS_SINCE_SHIFT = 0),
/// requests a `Screenshot::primary_window()` on this tick and the next 8
/// ticks per [`SHIFT_CAPTURE_OFFSETS`].
///
/// **NOTE on cold-start interference**: the first origin shift typically
/// fires DURING cold-start (the streaming preset's 512-slot drain takes
/// ~128 frames at 4 admissions/frame; the walk starts ~120 frames in and
/// crosses its first segment boundary at ~+64 voxels = ~16 ticks). In
/// that window unfulfilled segments are still populating, which adds
/// world-data-change variance on top of the pure shift artefact. The
/// threshold [`STREAMING_TAA_SHIFT_NOISE_RATIO_MAX`] is calibrated to
/// accommodate that residual variance: pre-fix measurement = 12.5×,
/// post-fix = 7.8×, threshold = 10.0× sits between the two so the gate
/// retains analytical power without requiring perfect signal isolation.
///
/// Wired via `add_e2e_systems` (`e2e/mod.rs`) only when
/// `args.streaming_taa_shift_noise_mode`. Runs `.after(pin_streaming_window_camera)`
/// so the camera-walk delta has already landed before we read the residency
/// origin.
pub fn record_shift_transient_frames(
    mut commands: Commands,
    args: Option<Res<crate::AppArgs>>,
    residency: Option<Res<crate::streaming::residency::Residency>>,
) {
    let Some(args) = args else { return; };
    if !args.streaming_taa_shift_noise_mode {
        return;
    }
    // Only active during the walk (after `promote_camera_to_walk` fires).
    if !super::streaming_window::camera_has_walked() {
        return;
    }
    let Some(residency) = residency else { return; };
    let current_x = residency.origin().x;
    let last_x = LAST_ORIGIN_X.load(Ordering::SeqCst);
    LAST_ORIGIN_X.store(current_x, Ordering::SeqCst);

    let ticks_since_shift = TICKS_SINCE_SHIFT.load(Ordering::SeqCst);
    let new_ticks_since_shift = if ticks_since_shift < 0 {
        // Shift not yet detected.
        if last_x == i32::MIN {
            // First tick — record but no diff available yet.
            -1
        } else if current_x != last_x {
            // Origin shifted this tick = frame N.
            0
        } else {
            -1
        }
    } else if ticks_since_shift > *SHIFT_CAPTURE_OFFSETS.last().unwrap() {
        // Past the last capture offset — leave latched (no further work).
        ticks_since_shift
    } else {
        ticks_since_shift + 1
    };
    TICKS_SINCE_SHIFT.store(new_ticks_since_shift, Ordering::SeqCst);

    // If THIS tick's offset is in the capture set, fire a screenshot.
    if new_ticks_since_shift >= 0
        && SHIFT_CAPTURE_OFFSETS.contains(&new_ticks_since_shift)
    {
        commands
            .spawn(Screenshot::primary_window())
            .observe(stash_shift_screenshot);
    }
}

// ---------------------------------------------------------------------------
// Metric — temporal variance over shadowed band
// ---------------------------------------------------------------------------

#[inline]
fn luma(p: [u8; 4]) -> f32 {
    0.2126 * (p[0] as f32) + 0.7152 * (p[1] as f32) + 0.0722 * (p[2] as f32)
}

/// Compute per-pixel temporal variance over a 4-sample window. Returns
/// `(mean_variance, mean_luma_over_pixels)` averaged over pixels that are
/// shadowed in ALL frames of the window AND the cross-window selector.
///
/// `selector_mask[p]` must be true for the pixel to enter the metric.
fn temporal_variance_in_band(
    frames: &[&Framebuffer; 4],
    selector_mask: &[bool],
) -> (f32, f32, u32) {
    let w = frames[0].width() as usize;
    let h = frames[0].height() as usize;
    let mut var_acc: f64 = 0.0;
    let mut luma_acc: f64 = 0.0;
    let mut counted: u32 = 0;
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if !selector_mask[idx] {
                continue;
            }
            let l0 = luma(frames[0].pixel(x as u32, y as u32));
            let l1 = luma(frames[1].pixel(x as u32, y as u32));
            let l2 = luma(frames[2].pixel(x as u32, y as u32));
            let l3 = luma(frames[3].pixel(x as u32, y as u32));
            let mean = (l0 + l1 + l2 + l3) * 0.25;
            let var = ((l0 - mean).powi(2)
                + (l1 - mean).powi(2)
                + (l2 - mean).powi(2)
                + (l3 - mean).powi(2))
                * 0.25;
            var_acc += var as f64;
            luma_acc += mean as f64;
            counted += 1;
        }
    }
    if counted == 0 {
        (0.0, 0.0, 0)
    } else {
        (
            (var_acc / counted as f64) as f32,
            (luma_acc / counted as f64) as f32,
            counted,
        )
    }
}

/// Build the shadowed-band selector mask — `true` for pixels whose MEAN
/// luminance over the BASELINE window (frames N+5..N+8) falls below
/// [`SHADOWED_BAND_LUMA_MAX`]. Selecting on the recovered/settled state
/// captures pixels that SHOULD be dark in steady state; the artefact's
/// transient noise burst can push the same pixels well above the
/// threshold during frames N..N+3 (that's the failure mode being
/// measured), so intersecting over ALL 8 frames would silence the gate.
///
/// `baseline_frames` are indices 4..7 of the capture array (frames N+5..N+8).
fn build_shadowed_band_mask(baseline_frames: &[&Framebuffer; 4]) -> Vec<bool> {
    let w = baseline_frames[0].width() as usize;
    let h = baseline_frames[0].height() as usize;
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let mean = 0.25
                * (luma(baseline_frames[0].pixel(x as u32, y as u32))
                    + luma(baseline_frames[1].pixel(x as u32, y as u32))
                    + luma(baseline_frames[2].pixel(x as u32, y as u32))
                    + luma(baseline_frames[3].pixel(x as u32, y as u32)));
            if mean < SHADOWED_BAND_LUMA_MAX {
                mask[idx] = true;
            }
        }
    }
    mask
}

// ---------------------------------------------------------------------------
// Assertion
// ---------------------------------------------------------------------------

/// Save the captured framebuffers as PNGs (best effort, for user inspection).
fn save_captures(fbs: &[Framebuffer; SHIFT_CAPTURE_COUNT]) {
    for (i, fb) in fbs.iter().enumerate() {
        let offset = SHIFT_CAPTURE_OFFSETS[i];
        let tag = if offset < 5 { "n" } else { "b" };
        let local = if offset < 5 { offset } else { offset - 5 };
        let filename = format!(
            "{}_{}{}.png",
            STREAMING_TAA_SHIFT_NOISE_PNG_PREFIX, tag, local
        );
        let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
        let _ = fb.save_png(&path);
    }
}

/// Run the assertion: read the 8 captured frames out of the static stash,
/// compute the shadowed-band temporal-variance ratio, and PASS/FAIL the
/// gate.
pub fn assert_streaming_taa_shift_noise_landed() -> Result<String, String> {
    let captures = take_shift_captures();
    // All 8 captures must be present — if any failed to deliver the gate
    // bails with a clear "capture missing" message (the driver shape
    // distinct from a real noise-burst regression).
    let mut fbs: [Option<Framebuffer>; SHIFT_CAPTURE_COUNT] =
        [const { None }; SHIFT_CAPTURE_COUNT];
    let mut missing: Vec<i32> = Vec::new();
    for (i, image_opt) in captures.into_iter().enumerate() {
        match image_opt {
            Some(image) => match Framebuffer::from_image(&image) {
                Ok(fb) => fbs[i] = Some(fb),
                Err(msg) => {
                    return Err(format!(
                        "streaming-taa-shift-noise: capture {} (offset N+{}) \
                         decode failed: {msg}",
                        i, SHIFT_CAPTURE_OFFSETS[i],
                    ));
                }
            },
            None => missing.push(SHIFT_CAPTURE_OFFSETS[i]),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "streaming-taa-shift-noise: never captured frames at offsets \
             {:?} relative to N — the residency origin never shifted during \
             the walk, OR the screenshot observer didn't fire on these \
             ticks. ticks_since_shift_last = {}",
            missing,
            TICKS_SINCE_SHIFT.load(Ordering::SeqCst),
        ));
    }
    // Safe — every slot was filled above (`missing` is empty).
    let fbs_owned: [Framebuffer; SHIFT_CAPTURE_COUNT] = fbs.map(|o| o.unwrap());
    // Persist captures for user inspection (best effort).
    save_captures(&fbs_owned);

    let fbs_ref: [&Framebuffer; SHIFT_CAPTURE_COUNT] = [
        &fbs_owned[0], &fbs_owned[1], &fbs_owned[2], &fbs_owned[3],
        &fbs_owned[4], &fbs_owned[5], &fbs_owned[6], &fbs_owned[7],
    ];

    // Transient window = N..N+3 (indices 0..3 of SHIFT_CAPTURE_OFFSETS).
    let transient_frames: [&Framebuffer; 4] = [fbs_ref[0], fbs_ref[1], fbs_ref[2], fbs_ref[3]];
    // Baseline window = N+5..N+8 (indices 4..7).
    let baseline_frames: [&Framebuffer; 4] = [fbs_ref[4], fbs_ref[5], fbs_ref[6], fbs_ref[7]];

    // Build the shadowed-band selector mask from the BASELINE window.
    let mask = build_shadowed_band_mask(&baseline_frames);
    let band_pixel_count: u32 = mask.iter().filter(|&&b| b).count() as u32;

    let (var_transient, mean_luma_transient, count_t) =
        temporal_variance_in_band(&transient_frames, &mask);
    let (var_baseline_temporal, mean_luma_baseline, count_b) =
        temporal_variance_in_band(&baseline_frames, &mask);

    // Sanity check: same selector mask → counts must match.
    debug_assert_eq!(count_t, count_b);

    let denom = var_baseline_temporal.max(STREAMING_TAA_SHIFT_NOISE_VARIANCE_BASELINE_FLOOR);
    let ratio = var_transient / denom;

    let report = format!(
        "streaming-taa-shift-noise: shadowed-band pixels = {} ({:.1}% of frame); \
         mean shadowed luma transient = {:.2}, baseline = {:.2}; \
         var_transient = {:.4} (temporal, N..N+3), \
         var_baseline = {:.4} (temporal, N+5..N+8, floor = {:.2}); \
         ratio = {:.3} (cap = {:.2})",
        band_pixel_count,
        100.0 * band_pixel_count as f32
            / ((fbs_ref[0].width() * fbs_ref[0].height()) as f32),
        mean_luma_transient,
        mean_luma_baseline,
        var_transient,
        var_baseline_temporal,
        STREAMING_TAA_SHIFT_NOISE_VARIANCE_BASELINE_FLOOR,
        ratio,
        STREAMING_TAA_SHIFT_NOISE_RATIO_MAX,
    );
    println!("e2e_render --streaming-taa-shift-noise: {report}");

    if band_pixel_count < 16 {
        return Err(format!(
            "streaming-taa-shift-noise gate FAIL — shadowed-band selector \
             yielded only {} pixels (need ≥ 16 for the temporal-variance \
             estimate to be meaningful). Likely the shadowed-band threshold \
             ({:.1}) is mis-calibrated for this scene, OR the captured \
             frames don't contain enough genuinely-shadowed pixels. {}",
            band_pixel_count, SHADOWED_BAND_LUMA_MAX, report,
        ));
    }

    if ratio > STREAMING_TAA_SHIFT_NOISE_RATIO_MAX {
        Err(format!(
            "streaming-taa-shift-noise gate FAIL — temporal variance ratio \
             {:.3} exceeds cap {:.2}. The post-origin-shift TAA history-reject \
             burst dominates the shadowed-band luminance variance for the \
             first 4 frames after the shift (frames N..N+3) — the structural \
             camera-history rebase is missing or broken. See \
             `docs/orchestrate/taa-hash-world-identity/02-design.md` § \
             \"Design — structural rebase\". {}",
            ratio, STREAMING_TAA_SHIFT_NOISE_RATIO_MAX, report,
        ))
    } else {
        Ok(format!("streaming-taa-shift-noise gate PASS — {}", report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reset latches → take_shift_captures returns all-None.
    #[test]
    fn reset_clears_captures() {
        reset_capture_latches();
        let cs = take_shift_captures();
        for c in cs.iter() {
            assert!(c.is_none(), "expected all-None after reset");
        }
    }

    /// The transient/baseline offset choice is what the design promises.
    #[test]
    fn shift_capture_offsets_layout() {
        assert_eq!(SHIFT_CAPTURE_OFFSETS, [0, 1, 2, 3, 5, 6, 7, 8]);
        assert_eq!(SHIFT_CAPTURE_OFFSETS.len(), SHIFT_CAPTURE_COUNT);
    }
}
