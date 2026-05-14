//! `NaadfRenderPlugin` — registers the Phase-A render pipelines, bind-group
//! layouts, render-world resources, and render-graph nodes (`03-design.md` §5).
//!
//! - [`extract`] — `ExtractSchedule`: `WorldData` / camera → render-world mirror.
//! - [`prepare`] — `Prepare`: upload buffers, build bind groups, camera uniforms.
//! - [`graph`] — render-graph node systems + edges.
//! - [`pipelines`] — compute / render pipeline descriptors + bind-group layouts.
//! - [`gpu_types`] — `#[repr(C)]` structs mirroring every WGSL struct / uniform.
//!
//! The Phase-A render graph is two passes (`03-design.md` §5.1): a first-hit
//! compute pass, then a fullscreen final-blit pass. Both run in the `Core3d`
//! schedule's `PostProcess` set (the first-hit pass does its own raytracing —
//! it does not depend on the main 3D pass output) and before `tonemapping`
//! (the HUD's UI pass then draws on top).

pub mod atmosphere;
pub mod color_compression;
pub mod extract;
pub mod gpu_types;
pub mod graph;
pub mod graph_b;
pub mod pipelines;
pub mod prepare;
pub mod taa;

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::Core3dSystems;
use bevy::prelude::*;
use bevy::render::{
    ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

use atmosphere::prepare_atmosphere;
use extract::{
    extract_camera, extract_camera_history, extract_taa_config, extract_world,
    ExtractedCameraData, ExtractedCameraHistory, ExtractedTaaConfig, ExtractedWorld,
};
// `naadf_taa_reproject_node` stays defined in `graph.rs` but is OUT of the
// render-graph chain in Batch 2 (`09-design-b.md` §11 Batch 2 step 8 — the
// `base/` TAA rewire is Batch 6); not imported here so the chain stays honest.
use graph::{naadf_final_blit_node, naadf_first_hit_node};
use graph_b::naadf_atmosphere_node;
use pipelines::{prepare_blit_pipeline, NaadfPipelines};
use prepare::{prepare_frame_gpu, prepare_world_gpu};
use taa::prepare_taa;

/// Plugin: wires the Phase-A NAADF render path into the render sub-app.
pub struct NaadfRenderPlugin;

impl Plugin for NaadfRenderPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<ExtractedWorld>()
            .init_resource::<ExtractedCameraData>()
            .init_resource::<ExtractedCameraHistory>()
            .init_resource::<ExtractedTaaConfig>()
            // Pipelines + bind-group layouts — `FromWorld`, built once in
            // `RenderStartup` (after the render device exists).
            .init_gpu_resource::<NaadfPipelines>()
            // Extract: main world -> render world mirror.
            .add_systems(
                ExtractSchedule,
                (
                    extract_world,
                    extract_camera,
                    extract_camera_history,
                    extract_taa_config,
                ),
            )
            // Prepare: create + upload GPU resources, build bind groups,
            // queue the per-target-format blit pipeline variant. `prepare_taa`
            // creates `TaaGpu` here in `PrepareResources` so it exists before
            // `prepare_frame_gpu` (`PrepareBindGroups`) binds `taa_sample_accum`
            // (`06-design-a2.md` §5.5, §9.4).
            // `prepare_atmosphere` (Phase B) creates `AtmosphereGpu` in
            // `PrepareResources` alongside `prepare_world_gpu` / `prepare_taa`
            // — its bind group is self-contained (no `FrameGpu` / `TaaGpu`
            // dependency), so it does not need the `PrepareBindGroups` split.
            .add_systems(
                Render,
                (prepare_world_gpu, prepare_taa, prepare_atmosphere, prepare_blit_pipeline)
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_frame_gpu.in_set(RenderSystems::PrepareBindGroups),
            )
            // Render graph — Phase B Batch 2 (`09-design-b.md` §11 Batch 2
            // step 8): atmosphere precompute -> 4-plane first-hit -> final-blit
            // fullscreen, all in PostProcess (the first-hit pass raytraces
            // independently of the main 3D pass) and before tonemapping so the
            // HUD draws over. `.chain()` gives the render-graph edges and
            // wgpu's automatic buffer barriers serialise the shared-buffer
            // accesses (`atmosphere_comp`, `first_hit_data`, `final_color`).
            //
            // `naadf_atmosphere_node` runs first — NAADF's dispatch order runs
            // the atmosphere precompute before the first-hit pass
            // (`WorldRenderBase.cs:205-228`, `09-design-b.md` §4.2). Batch 2
            // wires its output into the first-hit pass (`@group(2)`).
            //
            // `naadf_taa_reproject_node` is DELIBERATELY OUT of the chain this
            // batch: the `base/` first-hit no longer writes `taa_sample_accum`
            // / `taa_samples`, so the A-2 reproject + the `taa_sample_accum`
            // blit source are temporarily broken. Batch 2's minimal fix points
            // the final blit at `final_color` directly (the 4-plane first-hit
            // result). Batch 6 wires the proper `base/` TAA path
            // (`ReprojectOld` + `CalcNewTaaSample`) and reverts the blit
            // source — the node + the `*_taa_reproject*` plumbing stay in the
            // tree (`graph.rs`, `prepare.rs`) so Batch 6 only re-adds them to
            // the chain.
            .add_systems(
                Core3d,
                (
                    naadf_atmosphere_node,
                    naadf_first_hit_node,
                    naadf_final_blit_node,
                )
                    .chain()
                    .in_set(Core3dSystems::PostProcess)
                    .before(tonemapping),
            );
    }
}
