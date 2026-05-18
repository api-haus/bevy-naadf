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

pub mod chunk_source;
pub mod noise_dispatch;
pub mod noise_fastnoiselite;
pub mod noise_fastnoiselite_cpu_oracle;
pub mod residency;

use bevy::prelude::*;
use bevy::render::{ExtractSchedule, RenderApp};

pub use chunk_source::{ChunkSource, NoiseChunkSource, SegmentSourceKind};
pub use noise_dispatch::{
    build_noise_terrain_params, build_noise_terrain_shader_src,
    create_noise_terrain_params_buffer, extract_streaming_state,
    noise_terrain_layout_descriptor, queue_noise_terrain_pipeline,
    queue_noise_terrain_pipeline_with_handle, seed_noise_terrain_shader,
    NoiseTerrainParams, StreamingExtractRender, StreamingShaderHandle,
    NOISE_TERRAIN_SHADER_PATH, NOISE_TERRAIN_SHADER_SRC,
};
pub use residency::{
    assert_vram_budget_sufficient, compute_slab_total_mib, mark_admissions_resident,
    residency_driver, segment_to_voxel_origin, target_origin_for_camera_seg,
    world_voxel_to_segment, Residency, SlotIndex, SlotState, WorldSegmentPos,
    SEGMENT_CHUNKS, SEGMENT_VOXELS,
};

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

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<StreamingExtractRender>()
            .add_systems(ExtractSchedule, extract_streaming_state);
    }
}
