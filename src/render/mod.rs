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

pub mod extract;
pub mod gpu_types;
pub mod graph;
pub mod pipelines;
pub mod prepare;

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::Core3dSystems;
use bevy::prelude::*;
use bevy::render::{
    ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

use extract::{extract_camera, extract_world, ExtractedCameraData, ExtractedWorld};
use graph::{naadf_final_blit_node, naadf_first_hit_node};
use pipelines::{prepare_blit_pipeline, NaadfPipelines};
use prepare::{prepare_frame_gpu, prepare_world_gpu};

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
            // Pipelines + bind-group layouts — `FromWorld`, built once in
            // `RenderStartup` (after the render device exists).
            .init_gpu_resource::<NaadfPipelines>()
            // Extract: main world -> render world mirror.
            .add_systems(ExtractSchedule, (extract_world, extract_camera))
            // Prepare: create + upload GPU resources, build bind groups,
            // queue the per-target-format blit pipeline variant.
            .add_systems(
                Render,
                (prepare_world_gpu, prepare_blit_pipeline)
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_frame_gpu.in_set(RenderSystems::PrepareBindGroups),
            )
            // Render graph: first-hit compute -> final-blit fullscreen, both
            // in PostProcess (the first-hit pass raytraces independently of
            // the main 3D pass) and before tonemapping so the HUD draws over.
            .add_systems(
                Core3d,
                (naadf_first_hit_node, naadf_final_blit_node)
                    .chain()
                    .in_set(Core3dSystems::PostProcess)
                    .before(tonemapping),
            );
    }
}
