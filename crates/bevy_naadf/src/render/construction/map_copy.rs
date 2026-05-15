//! W1 — `map_copy.wgsl` Rust side: layout + pipeline queueing + dispatch
//! helper for the hash-map regrow path (`15-design-c.md` §2.1 W1, §4.4).
//!
//! Two pipelines:
//!   - `map_copy_pipeline_copy` — `copy_map` entry, linear-probe re-hash.
//!   - `map_copy_pipeline_test` — `test_hash` entry, CPU-debug sanity probe.
//!
//! Layout: a single `map_copy_layout` `@group(0)`. The 6 bindings:
//!   0: old_map           storage_buffer<array<HashValueSlot>>  (read-only)
//!   1: new_map           storage_buffer<array<HashValueSlot>>  (rw)
//!   2: params            uniform<MapCopyParams>
//!   3: hash_coefficients storage_buffer<array<u32>>            (read-only)  ← test_hash only
//!   4: voxels_to_hash    storage_buffer<array<u32>>            (read-only)  ← test_hash only
//!   5: result_hash       storage_buffer<array<u32>>            (rw)         ← test_hash only
//!
//! Bindings 3–5 are only consumed by `test_hash`, but the layout declares
//! them so both pipelines bind against the same layout descriptor. The CPU
//! side builds a bind group with placeholder buffers for the unused slots
//! when dispatching `copy_map` (a 1-u32 storage buffer is enough).

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
    },
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoder, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
    ShaderStages,
};
use bevy::shader::Shader;
use bytemuck::{Pod, Zeroable};

/// Asset path of the W1 `map_copy.wgsl` shader.
pub const MAP_COPY_SHADER: &str = "shaders/map_copy.wgsl";

/// Inlined source (test fixture, same pattern as W5's
/// `generator_model.rs::GENERATOR_MODEL_SHADER_SRC`).
pub const MAP_COPY_SHADER_SRC: &str =
    include_str!("../../assets/shaders/map_copy.wgsl");

/// Rust mirror of `map_copy.wgsl::MapCopyParams`. 16 B = 1 × 16-byte row.
///
/// No `vec3`-then-scalar hazard: 4 × u32 in a single row, naturally aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuMapCopyParams {
    pub old_size: u32,
    pub new_size: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}

// Compile-time guards.
const _: () = assert!(std::mem::size_of::<GpuMapCopyParams>() == 16);
const _: () = assert!(std::mem::offset_of!(GpuMapCopyParams, old_size) == 0);
const _: () = assert!(std::mem::offset_of!(GpuMapCopyParams, new_size) == 4);

/// Build the `map_copy_layout` bind-group-layout descriptor (§1.3 / §4.4).
pub fn map_copy_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size = NonZeroU64::new(std::mem::size_of::<GpuMapCopyParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_map_copy_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only_sized(false, None), // old_map
                storage_buffer_sized(false, None),           // new_map
                uniform_buffer_sized(false, Some(params_size)), // params
                storage_buffer_read_only_sized(false, None), // hash_coefficients
                storage_buffer_read_only_sized(false, None), // voxels_to_hash
                storage_buffer_sized(false, None),           // result_hash
            ),
        ),
    )
}

/// Queue the `copy_map` pipeline.
pub fn queue_copy_map_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(MAP_COPY_SHADER);
    queue_copy_map_pipeline_with_handle(pipeline_cache, layout, shader)
}

pub fn queue_copy_map_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_map_copy_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("copy_map")),
        ..default()
    })
}

/// Queue the `test_hash` pipeline (CPU-debug sanity probe; not used in
/// production startup).
pub fn queue_test_hash_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(MAP_COPY_SHADER);
    queue_test_hash_pipeline_with_handle(pipeline_cache, layout, shader)
}

pub fn queue_test_hash_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_map_copy_test_hash_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("test_hash")),
        ..default()
    })
}

/// Dispatch `copy_map` over `old_size` slots (64 threads/group → `ceil(old_size
/// / 64)` workgroups). Mirrors `BlockHashingHandler.cs:193` —
/// `App.graphicsDevice.DispatchCompute((mapSize / 64) + 1, 1, 1)`.
///
/// **Note** — the C# uses `(mapSize / 64) + 1`, which over-dispatches by up to
/// 64 threads past the array bound when `mapSize` is a multiple of 64. The
/// shader's `if (id >= params.old_size) { return; }` guard makes the extras
/// no-ops. Faithful port: same `+1` over-dispatch.
pub fn dispatch_copy_map(
    encoder: &mut CommandEncoder,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    old_size: u32,
) {
    if old_size == 0 {
        return;
    }
    let workgroup_count = (old_size / 64) + 1;
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("naadf_map_copy_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(workgroup_count, 1, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_copy_params_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<GpuMapCopyParams>(), 16);
        assert_eq!(offset_of!(GpuMapCopyParams, old_size), 0);
        assert_eq!(offset_of!(GpuMapCopyParams, new_size), 4);
        assert_eq!(offset_of!(GpuMapCopyParams, _pad0), 8);
        assert_eq!(offset_of!(GpuMapCopyParams, _pad1), 12);
    }
}
