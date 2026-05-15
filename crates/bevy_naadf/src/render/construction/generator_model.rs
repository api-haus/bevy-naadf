//! W5 — GPU world generator (`15-design-c.md` §2.1 W5, §4.5).
//!
//! Hosts the `generator_model_pipeline` + `generator_model_layout`, the
//! `GpuGeneratorModelParams` uniform mirror, and the standalone
//! [`dispatch_generator_model`] entry point that drives one regime-1 dispatch
//! of `generator_model.wgsl` against a caller-built bind group.
//!
//! W5 is the FIRST step of NAADF's regime-1 startup construction
//! (`generator → chunk_calc → bounds_init` per `15-design-c.md` §1.2 / §3):
//! the generator writes the segment voxel buffer that `chunk_calc.fx` (W1)
//! then consumes. **Until W1 lands**, the generator's path is exercised only
//! via the W5 unit test (the `generator_model_gpu_vs_cpu_bit_exact` test in
//! `crate::aadf::generator`'s adjacent module — see this file's `tests` mod)
//! or behind the `ConstructionConfig.run_worldgen_only` flag the W5 brief
//! introduces. The production CPU world-build path is untouched.
//!
//! ## Seam contract update (`16-impl-c-W5.md`)
//!
//! W5 adds the FIRST real fields to `ConstructionPipelines` — flipping it
//! from W0's "empty struct, default-derived FromWorld" shell to a real
//! `FromWorld` impl that builds:
//!
//! - `generator_model_layout` — the `@group(0)` bind-group layout
//!   (`chunk_data_rw`, `model_data_chunk_ro`, `model_data_block_ro`,
//!   `model_data_voxel_ro`, `params_uniform`).
//! - `generator_model_pipeline` — the `CachedComputePipelineId` for
//!   `generator_model.wgsl`'s `fill_chunk_data_with_model_data_16` entry point.
//!
//! W1 will later extend the same resource with `chunk_calc_*_pipeline` and
//! `map_copy_*_pipeline` fields plus a `construction_world_layout` that
//! includes the same `segment_voxel_buffer` (read-only, on its side) — the
//! buffer flows generator → chunk_calc within the regime-1 driver.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
    },
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedComputePipelineId, CommandEncoderDescriptor,
    ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache, ShaderStages,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::shader::Shader;
use bytemuck::{Pod, Zeroable};

/// Inlined shader source — `assets/shaders/generator_model.wgsl`. Used by the
/// W5 unit test (which builds a headless render world without an asset loader)
/// + as a build-time sanity check that the asset file's text and what the
/// pipeline-cache sees are the same. The `include_str!` is relative to this
/// `.rs` file, so a typo in the asset path fails to compile.
pub const GENERATOR_MODEL_SHADER_SRC: &str =
    include_str!("../../assets/shaders/generator_model.wgsl");

/// Asset path of the W5 generator-model WGSL shader.
pub const GENERATOR_MODEL_SHADER: &str = "shaders/generator_model.wgsl";

/// `numthreads(4,4,4)` per `generator_model.wgsl` — one workgroup per chunk.
pub const GENERATOR_MODEL_WORKGROUP_SIZE: u32 = 4;

/// u32s emitted by one workgroup of the generator (64 voxels per thread × 64
/// threads ÷ 2 voxels/u32 = 2048 u32s). Matches NAADF's `WorldData.cs:73`.
pub const CHUNK_DATA_U32S: u32 = 2048;

/// Rust mirror of `GeneratorModelParams` in `generator_model.wgsl`.
///
/// Layout: 64 B = 4 × 16-byte rows. Every `vec3<u32>` is followed by explicit
/// padding to keep `15-design-c.md` §1.5's `vec3`-then-scalar hazard out of
/// the WGSL counterpart. The compile-time guards below pin every row boundary
/// + every `vec3` 3-tuple's `% 16 == 0` constraint.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuGeneratorModelParams {
    // Row 0 (offset 0): size_in_voxels (vec3) + pad to 16.
    pub size_in_voxels: [u32; 3],
    pub _pad0: u32,
    // Row 1 (offset 16): model_size_in_chunks (vec3) + pad to 16.
    pub model_size_in_chunks: [u32; 3],
    pub _pad1: u32,
    // Row 2 (offset 32): group_offset_in_chunks (vec3) +
    //                    group_size_in_chunks_x (the X stride for groupIndex).
    pub group_offset_in_chunks: [u32; 3],
    pub group_size_in_chunks_x: u32,
    // Row 3 (offset 48): group_size_in_chunks_y (Y stride for groupIndex) +
    //                    3 pad u32s.
    pub group_size_in_chunks_y: u32,
    pub _pad2: u32,
    pub _pad3: u32,
    pub _pad4: u32,
}

// W5 — `GpuGeneratorModelParams` layout pins (`15-design-c.md` §1.5, §5).
// 64 B = 4 × 16-byte rows; every `vec3<u32>` followed by explicit padding so
// the WGSL `vec3<u32>`-then-scalar hazard cannot recur on the generator's
// per-dispatch uniform. The runtime mirror lives in
// `tests::generator_model_params_layout`.
const _: () = assert!(std::mem::size_of::<GpuGeneratorModelParams>() == 64);
const _: () =
    assert!(std::mem::offset_of!(GpuGeneratorModelParams, size_in_voxels) == 0);
const _: () = assert!(
    std::mem::offset_of!(GpuGeneratorModelParams, model_size_in_chunks) == 16
);
const _: () = assert!(
    std::mem::offset_of!(GpuGeneratorModelParams, group_offset_in_chunks) == 32
);
const _: () = assert!(
    std::mem::offset_of!(GpuGeneratorModelParams, group_size_in_chunks_y) == 48
);
const _: () =
    assert!(std::mem::offset_of!(GpuGeneratorModelParams, size_in_voxels) % 16 == 0);
const _: () = assert!(
    std::mem::offset_of!(GpuGeneratorModelParams, model_size_in_chunks) % 16 == 0
);
const _: () = assert!(
    std::mem::offset_of!(GpuGeneratorModelParams, group_offset_in_chunks) % 16 == 0
);

/// Construct the `generator_model_layout` `BindGroupLayoutDescriptor`
/// (`15-design-c.md` §4.5).
///
/// Bindings (`@group(0)`):
/// - 0: `chunk_data_rw` — the segment voxel buffer (W1 wires the same buffer
///   read-only from `construction_world_layout`).
/// - 1: `model_data_chunk_ro` — `ModelData.dataChunk` host upload.
/// - 2: `model_data_block_ro` — `ModelData.dataBlock` host upload.
/// - 3: `model_data_voxel_ro` — `ModelData.dataVoxel` host upload.
/// - 4: `params` — the [`GpuGeneratorModelParams`] uniform.
pub fn generator_model_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<GpuGeneratorModelParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_generator_model_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_sized(false, None),           // chunk_data_rw
                storage_buffer_read_only_sized(false, None), // model_data_chunk_ro
                storage_buffer_read_only_sized(false, None), // model_data_block_ro
                storage_buffer_read_only_sized(false, None), // model_data_voxel_ro
                uniform_buffer_sized(false, Some(params_size)),
            ),
        ),
    )
}

/// Queue the `generator_model_pipeline` against the given layout. Uses the
/// project's `AssetServer` to resolve the shader handle (the production path).
pub fn queue_generator_model_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(GENERATOR_MODEL_SHADER);
    queue_generator_model_pipeline_with_handle(pipeline_cache, layout, shader)
}

/// Queue the `generator_model_pipeline` against the given layout + an
/// already-resolved shader handle. Used by the W5 unit test (it inserts the
/// shader directly into `Assets<Shader>` rather than going through the
/// `AssetServer`, because the headless test fixture has no working asset
/// loader for `.wgsl` files).
pub fn queue_generator_model_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_generator_model_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("fill_chunk_data_with_model_data_16")),
        ..default()
    })
}

/// Helper: allocate a host-side `Buffer` of `data.len()` u32s, populated with
/// `data`. The buffer is `STORAGE | COPY_DST | COPY_SRC` so the test can map
/// it back. Used by `dispatch_generator_model_test_path`.
pub fn create_storage_buffer_u32(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    data: &[u32],
) -> Buffer {
    // wgpu storage buffers cannot be zero-size — pad to 1 element if empty.
    let data = if data.is_empty() { &[0u32][..] } else { data };
    let size = (data.len() * std::mem::size_of::<u32>()) as u64;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
    buffer
}

/// Helper: allocate a uniform buffer holding one [`GpuGeneratorModelParams`].
pub fn create_params_uniform(
    device: &RenderDevice,
    queue: &RenderQueue,
    params: &GpuGeneratorModelParams,
) -> Buffer {
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("naadf_generator_model_params"),
        size: std::mem::size_of::<GpuGeneratorModelParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::bytes_of(params));
    buffer
}

/// Dispatch `generator_model.wgsl` against a caller-built bind group.
///
/// One workgroup per chunk in the segment (`generator_model.wgsl` dispatch
/// shape, `15-design-c.md` §4.5). The caller owns the bind group (so the
/// production regime-1 driver in W1 can pre-build the bind group once for the
/// whole startup chain, while the W5 unit test builds a one-shot bind group
/// against test buffers).
///
/// This function is W5's seam contract for W1: W1's
/// `run_gpu_construction_startup` body calls this for each segment of the
/// world, then immediately calls W1's `chunk_calc_calc_block_from_raw_data`
/// dispatch over the same `segment_voxel_buffer`.
pub fn dispatch_generator_model(
    device: &RenderDevice,
    queue: &RenderQueue,
    pipeline: &bevy::render::render_resource::ComputePipeline,
    bind_group: &bevy::render::render_resource::BindGroup,
    group_size_in_chunks: [u32; 3],
) {
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("naadf_generator_model_encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_generator_model_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        // Dispatch shape: one workgroup per chunk in the segment.
        pass.dispatch_workgroups(
            group_size_in_chunks[0],
            group_size_in_chunks[1],
            group_size_in_chunks[2],
        );
    }
    queue.submit([encoder.finish()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W5 — runtime mirror of the compile-time `GpuGeneratorModelParams`
    /// layout guards (`15-design-c.md` §1.5, §5). The `const _: () = assert!(...)`
    /// guards above already catch the layout at compile time; this test exists
    /// so a future refactor that strips the const-asserts still has a runtime
    /// failure signal. Same +1-test discipline W0's
    /// `construction_params_layout` test uses.
    #[test]
    fn generator_model_params_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<GpuGeneratorModelParams>(), 64);
        assert_eq!(offset_of!(GpuGeneratorModelParams, size_in_voxels), 0);
        assert_eq!(offset_of!(GpuGeneratorModelParams, model_size_in_chunks), 16);
        assert_eq!(offset_of!(GpuGeneratorModelParams, group_offset_in_chunks), 32);
        assert_eq!(offset_of!(GpuGeneratorModelParams, group_size_in_chunks_x), 44);
        assert_eq!(offset_of!(GpuGeneratorModelParams, group_size_in_chunks_y), 48);
    }
}
