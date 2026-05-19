//! `streaming` — procedural-noise generation + sliding-window residency layer
//! for the streaming-world feature (`docs/orchestrate/streaming-world`).
//!
//! ## Phase 1 deliverables
//!
//! - [`noise_fastnoiselite`] — WGSL FastNoiseLite port + GPU oracle runner.
//! - [`noise_fastnoiselite_cpu_oracle`] — Rust port of the same GLSL functions,
//!   used as the CPU reference for the `--wgsl-noise-oracle` e2e gate.
//!
//! ## Phase 2 deliverables
//!
//! - [`residency`] — the sliding-window residency manager (slot table +
//!   per-frame admission/eviction driver). Per `02-design.md` §§ A.1–A.5 + the
//!   v1 carryover documented in `02b-design-plan-b.md` § D.
//! - [`chunk_source`] — the `ChunkSource` trait forward-compat seam (§ K) +
//!   the Phase-2 [`chunk_source::NoiseChunkSource`] impl.
//! - [`noise_dispatch`] — the WGSL noise → segment_voxel_buffer GPU dispatch
//!   wiring (params struct + bind-group layout + pipeline queue + the
//!   ExtractSchedule mirror).
//! - [`StreamingPlugin`] — registers the residency driver + the extract system.
//!
//! The per-frame W5 producer-node branch that consumes [`StreamingExtractRender`]
//! lives in `render/construction/mod.rs`'s `naadf_gpu_producer_node` (a third
//! arm of the existing `model_data.is_some()` ladder at `:2384-2566`).

pub mod camera;
pub mod chunk_source;
pub mod noise_dispatch;
pub mod noise_fastnoiselite;
pub mod noise_fastnoiselite_cpu_oracle;
pub mod residency;
pub mod sliding_window;
pub mod windowed_slot_map;

// streaming-world Phase 2.14.e — composition tests exercise the now-isolated
// primitives (`WindowedSlotMap` atomic API + `compute_window_delta` +
// `Residency` dispatch-ACK tracking + `StreamingDiagnostics`) together
// against synthetic camera-walk traces. No Bevy `App`, no GPU, no render
// world. See `04e-impl-composition-tests.md`.
#[cfg(test)]
mod composition_tests;

use bevy::prelude::*;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderSystems};

pub use camera::{
    install_streaming_camera_position, track_and_pin_camera, CameraAbsolutePosition,
};
pub use chunk_source::{
    ChunkSource, NoiseChunkSource, ProceduralStaticActive, SegmentSourceKind,
};
pub use noise_dispatch::{
    build_noise_terrain_params, build_noise_terrain_shader_src,
    create_noise_terrain_params_buffer, extract_streaming_state,
    noise_terrain_layout_descriptor, pending_clear_on_bind_count,
    pending_dispatch_ack_count, push_dispatched_once_ack,
    queue_noise_terrain_pipeline, queue_noise_terrain_pipeline_with_handle,
    seed_noise_terrain_shader, clear_streaming_bound_slots,
    upload_window_indirection, NoiseTerrainParams, StreamingExtractRender,
    StreamingShaderHandle, NOISE_TERRAIN_SHADER_PATH, NOISE_TERRAIN_SHADER_SRC,
};
pub use residency::{
    apply_dispatch_acks, assert_vram_budget_sufficient, compute_slab_total_mib,
    residency_driver, segment_to_voxel_origin, target_origin_for_camera_seg,
    world_voxel_to_segment, Residency, SlotIndex, StreamingDiagnostics,
    WorldSegmentPos, SEGMENT_CHUNKS, SEGMENT_VOXELS,
};
pub use sliding_window::{compute_window_delta, WindowDelta};
pub use windowed_slot_map::{WindowedSlotMap, EMPTY_SLOT};

// ---------------------------------------------------------------------------
// streaming-world Phase 2.14.f — periodic-diagnostics logging cadence.
//
// The per-shift `info!` line at `residency.rs:~670` fires only on origin
// shifts — i.e. zero log output during cold-start (the camera holds
// segment, the origin never shifts after the first-tick init). The
// periodic logger below covers that window plus the steady-state.
//
// Cadence:
// - Pre cold-start: every COLD_START_LOG_INTERVAL_FRAMES (cheap because
//   diagnostics() is O(window_size) = O(512) — < 10us per call).
// - Post cold-start: every STEADY_LOG_INTERVAL_FRAMES (the user does
//   not need a chatter stream once converged).
// - One-shot warn at COLD_START_WARN_THRESHOLD_FRAMES if unfulfilled > 0
//   — the analytical version of "user sees a sky-coloured hole at startup".
//
// Threshold sizing — with window 16×2×16 = 512 and admit quota 4, the
// perfect-case cold-start budget is ceil(512 / 4) = 128 frames + 1 ACK
// drain frame = 129 frames. We pick 500 (~4x budget) as the
// "something is wrong" threshold; the composition tests
// (`streaming/composition_tests.rs` T1) already prove convergence in
// 130 frames under synthetic ack=4 conditions.
// ---------------------------------------------------------------------------

/// Periodic-diagnostics cadence pre cold-start (in frames).
///
/// During the cold-start admission burst (the first ~128 frames at
/// `max_segments_per_frame = 4`) the user wants fine-grained visibility
/// into the fill rate. Log every 10 frames — ~6 logs/sec at 60fps.
const COLD_START_LOG_INTERVAL_FRAMES: u64 = 10;

/// Periodic-diagnostics cadence post cold-start (in frames).
///
/// At steady state nothing interesting happens between origin shifts
/// (the per-shift `info!` line at `residency.rs:~670` handles those).
/// Log every 300 frames (~5 sec at 60fps) as a heartbeat that confirms
/// the streaming layer is still alive without flooding the console.
const STEADY_LOG_INTERVAL_FRAMES: u64 = 300;

/// One-shot warn threshold (in frames) — if `diag.unfulfilled > 0` at
/// this frame, emit a single `warn!` listing the first few unfulfilled
/// segments. Past the natural cold-start budget by ~4×; non-zero
/// unfulfilled here is the analytical version of "user sees a
/// sky-coloured hole at startup".
const COLD_START_WARN_THRESHOLD_FRAMES: u64 = 500;

/// Number of unfulfilled segments to include in the warn log line —
/// enough to be diagnostic without flooding a single log entry.
const WARN_UNFULFILLED_TRUNCATE: usize = 10;

/// Pure-data predicate: should the periodic logger emit at this frame?
///
/// Extracted as a free function so it can be unit-tested without
/// constructing a Bevy `App`. The system function below calls this on
/// every tick and short-circuits on `false`.
///
/// Returns `true` on frames 0, 10, 20, … during cold-start, and on
/// frames N, N+300, N+600, … in steady state (where N is whichever
/// frame `cold_start_complete` first becomes true).
fn should_log_at_frame(frame: u64, cold_start_complete: bool) -> bool {
    let interval = if cold_start_complete {
        STEADY_LOG_INTERVAL_FRAMES
    } else {
        COLD_START_LOG_INTERVAL_FRAMES
    };
    frame.is_multiple_of(interval)
}

/// State carried by the periodic-diagnostics system between ticks.
/// Tracks the cold-start transition (so we can emit a one-shot
/// "cold-start complete at frame N" line) and the warn-threshold
/// latch (so the warn fires at most once even if `unfulfilled > 0`
/// persists past the threshold).
#[derive(Resource, Default)]
struct StreamingDiagnosticsLoggerState {
    /// True after the first frame where `diag.cold_start_complete`
    /// flipped from `false` to `true`. Used to gate the one-shot
    /// transition log.
    cold_start_seen_complete: bool,
    /// True after the warn at `COLD_START_WARN_THRESHOLD_FRAMES`
    /// fired. Prevents repeat firings.
    warn_threshold_fired: bool,
}

/// `Last`-stage system — fires after `residency_driver` (PreUpdate) has
/// processed the frame's shifts/admissions, so `Residency::frame_counter`
/// and `dispatched_once` are fully up to date.
///
/// Three log channels:
/// - **Periodic heartbeat** `info!` at the cadence picked by
///   [`should_log_at_frame`]. Logs the full diagnostics snapshot.
/// - **Cold-start transition** one-shot `info!` on the frame where
///   `cold_start_complete` first becomes `true`.
/// - **Cold-start warn** one-shot `warn!` at
///   [`COLD_START_WARN_THRESHOLD_FRAMES`] if `unfulfilled > 0`.
///
/// Early-returns when the `Residency` resource is missing
/// (non-streaming presets).
fn log_streaming_diagnostics(
    residency: Option<Res<Residency>>,
    mut state: ResMut<StreamingDiagnosticsLoggerState>,
) {
    let Some(residency) = residency else {
        return;
    };
    // Snapshot once per frame — `diagnostics()` is O(window_size) = O(512),
    // negligible.
    let diag = residency.diagnostics();
    let frame = diag.frame_counter;

    // (1) Cold-start completion transition — fires exactly once on the
    // frame where `cold_start_complete` flips from `false` to `true`.
    if diag.cold_start_complete && !state.cold_start_seen_complete {
        state.cold_start_seen_complete = true;
        bevy::log::info!(
            "streaming-world: cold-start complete at frame {} (unfulfilled={}, \
             bound={}, dispatched_once={})",
            frame,
            diag.camera_window_segments_unfulfilled,
            diag.bound_slots,
            diag.dispatched_once_slots,
        );
    }

    // (2) Cold-start warn — one-shot at the threshold frame if the
    // streamer still has holes.
    if !state.warn_threshold_fired
        && frame >= COLD_START_WARN_THRESHOLD_FRAMES
        && diag.camera_window_segments_unfulfilled > 0
    {
        state.warn_threshold_fired = true;
        let truncated: Vec<_> = diag
            .unfulfilled_camera_window_segments
            .iter()
            .take(WARN_UNFULFILLED_TRUNCATE)
            .copied()
            .collect();
        bevy::log::warn!(
            "streaming-world: cold-start gap detected at frame {} — \
             {} unfulfilled camera-window segments after \
             COLD_START_WARN_THRESHOLD_FRAMES={}. First {} segments: {:?}",
            frame,
            diag.camera_window_segments_unfulfilled,
            COLD_START_WARN_THRESHOLD_FRAMES,
            truncated.len(),
            truncated,
        );
    }

    // (3) Periodic heartbeat — gated on the cadence predicate.
    if !should_log_at_frame(frame, diag.cold_start_complete) {
        return;
    }
    bevy::log::info!(
        "streaming-world: f={} | free={} bound={} dispatched_once={} | \
         generating={} in_flight={} | cold_start={} unfulfilled={} | \
         pending_clear={} pending_acks={}",
        frame,
        diag.free_slots,
        diag.bound_slots,
        diag.dispatched_once_slots,
        diag.generating_slots,
        diag.in_flight_slots,
        diag.cold_start_complete,
        diag.camera_window_segments_unfulfilled,
        diag.pending_clear_on_bind,
        diag.pending_dispatch_acks,
    );
}

/// Phase-2 `StreamingPlugin` — wires:
/// - The main-world `PreUpdate` `residency_driver` system.
/// - The render-world `ExtractSchedule` `extract_streaming_state` system.
/// - The `StreamingExtractRender` resource on the render world.
///
/// The plugin is registered unconditionally — when no `Residency` /
/// `NoiseChunkSource` resource exists (i.e. the user isn't running the
/// `ProceduralStreaming` preset), both systems early-return cheaply.
pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        // Register the inlined `noise_terrain_combined` shader as an asset at
        // startup so the render-world `prepare_construction` can pick up the
        // handle (via the extract) and queue the noise_terrain pipeline
        // lazily once streaming is active.
        app.add_systems(Startup, seed_noise_terrain_shader);
        // streaming-world Phase 2.13
        // (`docs/orchestrate/streaming-world/03r-diagnosis-cold-start-gap.md`
        // MUST-1) — drain the render→main ACK accumulator BEFORE the
        // residency driver picks the frame's admissions. Slots that were
        // dispatched by the previous frame's render-world producer enter
        // `Residency::dispatched_once` here, and the filter at
        // `residency.rs:502` then correctly excludes them from re-pick.
        // Sequencing: `apply_dispatch_acks.before(residency_driver)`.
        // Main-world residency driver. `PreUpdate` so the per-frame
        // admissions/evictions are visible to the render extract that follows.
        app.add_systems(
            PreUpdate,
            (
                apply_dispatch_acks,
                residency_driver.after(apply_dispatch_acks),
            ),
        );
        // Production-side camera-position tracker (`03j` Phase 2.9 fix):
        // re-derives `Transform.translation` to window-local each tick from
        // a separately-tracked absolute world position, so the additive
        // `FreeCamera` controller can't drive the residency driver into an
        // endless reposition loop. Runs `.before(sync_position_split)` so
        // the consumer's `PositionSplit::pos_int` lands in window-local
        // coords. Early-returns when the `Residency` /
        // `CameraAbsolutePosition` resources are absent (non-streaming
        // presets keep the original Transform-is-absolute behaviour).
        app.add_systems(
            Update,
            track_and_pin_camera
                .before(crate::camera::sync_position_split)
                // Run AFTER the e2e camera-pin systems (when present) so any
                // gate-driven Transform writes are folded into
                // `CameraAbsolutePosition` before re-pin. The e2e streaming
                // gate's `pin_streaming_window_camera` is the load-bearing
                // upstream — it applies per-tick additive Transform writes
                // during the walk phase, and `track_and_pin_camera` must
                // observe those deltas before re-pinning to window-local.
                // `ambiguous_with` over the other gates' pin systems is
                // safe — only one gate runs per harness invocation.
                .after(crate::e2e::streaming_window::pin_streaming_window_camera),
        );
        // Phase 2.6 (`02c-design-windowed-slot-map.md` § G.4 + D4): the
        // explicit `Generating → Resident` `Last`-stage system from Phase 2.5
        // is GONE — slot lifecycle is now implicit (bound ∩
        // admissions_this_frame ⇒ generating; bound \ admissions_this_frame ⇒
        // resident). Phase 2.6's `WindowedSlotMap` invariants make the
        // transition unnecessary: the driver clears
        // `admissions_this_frame` at the next `PreUpdate` entry, which IS
        // the Generating→Resident transition (the slot is still in
        // world_to_slot but no longer in admissions_this_frame).

        // streaming-world Phase 2.14.f — periodic analytical diagnostics
        // logger. Runs in `Last` so `residency_driver`'s PreUpdate pass
        // and any cross-world ACK drains have already landed for the
        // frame. The system early-returns when `Residency` is absent
        // (non-streaming presets), so the resource init below is the
        // only mandatory overhead for non-streaming runs (a single
        // zero-sized resource).
        app.init_resource::<StreamingDiagnosticsLoggerState>()
            .add_systems(Last, log_streaming_diagnostics);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<StreamingExtractRender>()
            .add_systems(ExtractSchedule, extract_streaming_state)
            // Phase 2.6 — upload the WindowedSlotMap indirection buffer
            // to the GPU each frame the streaming preset is active. Runs in
            // `Render::Queue` (after the ExtractSchedule populates
            // `StreamingExtractRender.window_indirection`, before the producer
            // node consumes the renderer's chunks bind group).
            .add_systems(
                Render,
                upload_window_indirection.in_set(RenderSystems::Queue),
            )
            // streaming-world Phase 2.12 (`02e-design-phase-2-12.md` § B,
            // MUST-1) — zero `chunks_buffer` slot regions the same frame
            // their indirection-table entry rebound. Runs in
            // `Render::Queue` alongside `upload_window_indirection`; both
            // must complete before the `naadf_gpu_producer_node` (in
            // `Core3d::PostProcess`) consumes the world bind group.
            // Forecloses the ghost-of-old-terrain bug at the indirection
            // race level (`03p-diagnosis-remaining-bugs.md` § Bug 1).
            .add_systems(
                Render,
                clear_streaming_bound_slots.in_set(RenderSystems::Queue),
            );
    }
}

// ---------------------------------------------------------------------------
// streaming-world Phase 2.14.f — periodic-logger cadence + state tests.
//
// Unit tests for the periodic-diagnostics logger added in this phase.
//
// Test shape A (cadence predicate) — pure-data unit tests on
// `should_log_at_frame`. Doesn't exercise the actual `info!` call, but
// proves the cadence math is right.
//
// Test shape B (system function) — drives `log_streaming_diagnostics`
// against a hand-built `Residency` + `StreamingDiagnosticsLoggerState`
// and asserts state-machine transitions on the logger state. Does not
// assert log content (would require a tracing subscriber, which would
// add a dev-dep we don't otherwise need).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod diagnostics_logger_tests {
    use super::*;
    use crate::WORLD_SIZE_IN_SEGMENTS;

    /// Helper — build a `Residency` with the camera window fully bound
    /// + dispatched. Mirrors the construction in
    /// `residency::tests::diagnostics_fully_fulfilled_reports_none_unfulfilled`.
    fn fully_fulfilled_residency(max_segments_per_frame: u32) -> Residency {
        let mut residency = Residency::empty(max_segments_per_frame);
        for sz in 0..WORLD_SIZE_IN_SEGMENTS.z as i32 {
            for sy in 0..WORLD_SIZE_IN_SEGMENTS.y as i32 {
                for sx in 0..WORLD_SIZE_IN_SEGMENTS.x as i32 {
                    let w = WorldSegmentPos(IVec3::new(sx, sy, sz));
                    let slot = residency
                        .window
                        .allocate_and_bind(w)
                        .expect("bind window segment");
                    residency.dispatched_once.insert(slot);
                }
            }
        }
        residency
    }

    /// Cadence — pre cold-start the logger emits every 10 frames.
    /// (Frames 0, 10, 20, 30, … return true; 1..9, 11..19, … return
    /// false.)
    #[test]
    fn cadence_pre_cold_start_emits_every_10_frames() {
        // Spot-check the first three cadence intervals.
        for frame in 0..3 * COLD_START_LOG_INTERVAL_FRAMES {
            let expected = frame.is_multiple_of(COLD_START_LOG_INTERVAL_FRAMES);
            assert_eq!(
                should_log_at_frame(frame, false),
                expected,
                "pre cold-start: frame {} expected log={}",
                frame,
                expected,
            );
        }
        // Pin the constant explicitly so a future change to the
        // constant trips this test.
        assert_eq!(COLD_START_LOG_INTERVAL_FRAMES, 10);
    }

    /// Cadence — post cold-start the logger emits every 300 frames.
    /// (Frames 0, 300, 600, … return true; 1..299, 301..599, …
    /// return false.)
    #[test]
    fn cadence_post_cold_start_emits_every_300_frames() {
        // Spot-check the boundaries: frame 0 fires, mid-interval
        // frames don't, the next multiple fires.
        assert!(should_log_at_frame(0, true), "frame 0 must fire");
        assert!(
            !should_log_at_frame(1, true),
            "frame 1 must NOT fire (mid-interval)"
        );
        assert!(
            !should_log_at_frame(STEADY_LOG_INTERVAL_FRAMES - 1, true),
            "frame STEADY-1 must NOT fire"
        );
        assert!(
            should_log_at_frame(STEADY_LOG_INTERVAL_FRAMES, true),
            "frame STEADY must fire"
        );
        assert!(
            should_log_at_frame(2 * STEADY_LOG_INTERVAL_FRAMES, true),
            "frame 2*STEADY must fire"
        );
        // Frames in the middle of the second interval must NOT fire.
        assert!(
            !should_log_at_frame(STEADY_LOG_INTERVAL_FRAMES + 1, true),
            "STEADY+1 must NOT fire"
        );
        // Pin the constant explicitly.
        assert_eq!(STEADY_LOG_INTERVAL_FRAMES, 300);
    }

    /// Cadence — under cold-start state the cadence is the 10-frame
    /// (NOT 300-frame) interval. Regression catcher for an accidental
    /// inversion of the bool in `should_log_at_frame`.
    #[test]
    fn cadence_cold_start_state_uses_short_interval() {
        // Frame 10 is a multiple of COLD_START (10) but NOT a multiple
        // of STEADY (300) — under cold-start state we expect log=true.
        // (Catches a bool inversion.)
        assert!(
            should_log_at_frame(10, false),
            "frame 10 must fire under cold-start state (10 % 10 == 0)"
        );
        // Frame 10 in steady state must NOT fire (10 % 300 != 0).
        assert!(
            !should_log_at_frame(10, true),
            "frame 10 must NOT fire in steady state (10 % 300 != 0)"
        );
    }

    /// Phase 2.14.f — system-level smoke: run
    /// `log_streaming_diagnostics` against an empty residency at the
    /// warn-threshold frame and assert the warn-latch flips. Asserts
    /// the state-machine transitions; does NOT assert log content
    /// (would need a tracing test subscriber).
    ///
    /// Single-shot semantics: a second invocation at the same frame
    /// MUST NOT re-flip the latch (the warn fires once).
    #[test]
    fn warn_fires_once_at_threshold_when_unfulfilled_nonzero() {
        // World resource: build a Bevy `App` minimal enough to host
        // the system, with `Residency` empty (every window cell
        // unfulfilled) and `frame_counter` pinned to the warn threshold.
        let mut app = App::new();
        let mut residency = Residency::empty(4);
        residency.frame_counter = COLD_START_WARN_THRESHOLD_FRAMES;
        app.insert_resource(residency)
            .init_resource::<StreamingDiagnosticsLoggerState>()
            .add_systems(Update, log_streaming_diagnostics);

        // Before the first run: the warn latch is clear.
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(
                !state.warn_threshold_fired,
                "warn latch must start clear"
            );
        }

        // Run once — the warn must fire (latch flips to true).
        app.update();
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(
                state.warn_threshold_fired,
                "warn latch must flip after first run at threshold frame \
                 with unfulfilled > 0"
            );
        }

        // Run again at the same frame — the latch must stay flipped
        // (one-shot semantics; we don't spam the same warning every
        // tick after the threshold).
        app.update();
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(
                state.warn_threshold_fired,
                "warn latch must remain flipped after a second run \
                 (one-shot semantics — no repeat firings)"
            );
        }
    }

    /// Phase 2.14.f — system-level smoke for the cold-start
    /// transition log. When a Residency goes from `cold_start_complete
    /// = false` to `true`, the logger MUST flip the
    /// `cold_start_seen_complete` latch (the one-shot transition log
    /// path).
    #[test]
    fn cold_start_transition_latch_flips_on_completion() {
        let mut app = App::new();
        let residency = fully_fulfilled_residency(4);
        // Sanity — the helper actually constructs a fully-fulfilled
        // residency. (Cheap to verify here so a future regression in
        // the helper doesn't silently invalidate this test.)
        assert!(residency.is_cold_start_complete());
        app.insert_resource(residency)
            .init_resource::<StreamingDiagnosticsLoggerState>()
            .add_systems(Update, log_streaming_diagnostics);

        // Latch starts clear.
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(!state.cold_start_seen_complete);
        }

        // First run — completion latch flips.
        app.update();
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(
                state.cold_start_seen_complete,
                "cold_start_seen_complete must flip on the first run \
                 where diag.cold_start_complete == true"
            );
        }

        // Second run — latch stays flipped (one-shot transition).
        app.update();
        {
            let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
            assert!(
                state.cold_start_seen_complete,
                "cold_start_seen_complete must remain set after a \
                 second run (no flip back)"
            );
        }
    }

    /// Phase 2.14.f — system safety: `log_streaming_diagnostics` must
    /// early-return when `Residency` is absent (non-streaming
    /// presets). Asserts the system runs without panicking and
    /// leaves the latch state untouched.
    #[test]
    fn logger_early_returns_when_residency_absent() {
        let mut app = App::new();
        app.init_resource::<StreamingDiagnosticsLoggerState>()
            .add_systems(Update, log_streaming_diagnostics);
        // No Residency inserted — the optional `Res<Residency>`
        // parameter resolves to None and the system early-returns.
        app.update();
        let state = app.world().resource::<StreamingDiagnosticsLoggerState>();
        assert!(
            !state.warn_threshold_fired,
            "absent Residency must not trip any latch"
        );
        assert!(
            !state.cold_start_seen_complete,
            "absent Residency must not flip the cold-start-complete latch"
        );
    }
}
