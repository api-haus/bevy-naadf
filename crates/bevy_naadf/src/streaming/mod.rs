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
pub mod windowed_slot_map;

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
    noise_terrain_layout_descriptor, queue_noise_terrain_pipeline,
    queue_noise_terrain_pipeline_with_handle, seed_noise_terrain_shader,
    clear_streaming_bound_slots, upload_window_indirection, NoiseTerrainParams,
    StreamingExtractRender, StreamingShaderHandle, NOISE_TERRAIN_SHADER_PATH,
    NOISE_TERRAIN_SHADER_SRC,
};
pub use residency::{
    assert_vram_budget_sufficient, compute_slab_total_mib, residency_driver,
    segment_to_voxel_origin, target_origin_for_camera_seg, world_voxel_to_segment,
    Residency, SlotIndex, WorldSegmentPos, SEGMENT_CHUNKS, SEGMENT_VOXELS,
};
pub use windowed_slot_map::{WindowedSlotMap, EMPTY_SLOT};

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
        // Main-world residency driver. `PreUpdate` so the per-frame
        // admissions/evictions are visible to the render extract that follows.
        app.add_systems(PreUpdate, residency_driver);
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
