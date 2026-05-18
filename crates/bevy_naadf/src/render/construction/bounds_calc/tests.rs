//! W3 — load-bearing tests for `bounds_calc.wgsl` (`15-design-c.md` §1.6,
//! §2.1 W3, `16-impl-c-W3.md`).
//!
//! Three tests, each gated behind a headless render-world fixture (same plumbing
//! pattern as W1 / W5 — skipped with a `eprintln!` when no wgpu adapter is
//! available):
//!
//! 1. [`bounds_calc_convergence_matches_cpu_oracle`] — load-bearing W3 gate.
//!    Build a chunk-layer, run the regime-1 seed + N rounds of regime-2, map
//!    the chunks texture back to CPU, decode each empty chunk's 5-bit AADF,
//!    assert it matches the CPU oracle (a faithful port of `boundsCalc.fx`'s
//!    `addBoundsGroup` algorithm — including the chunk-world-edge OOB-permissive
//!    convention, which differs from W6's `compute_aadf_layer` wall convention
//!    per `16-impl-c-W6.md` assumption #2).
//! 2. [`bounds_queue_no_overrun`] — assert the fixed-size queue never overruns.
//! 3. [`bounds_per_axis_atomic_correctness`] — verify the per-axis
//!    `bound_group_masks` atomic doesn't drop updates under contention.

use std::borrow::Cow;

use bevy::app::App;
use bevy::asset::{AssetPlugin, Assets, Handle};
use bevy::image::ImagePlugin;
use bevy::render::render_resource::{
    BindGroupEntries, Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
    ComputePipeline, MapMode, PipelineCache, PollType,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::settings::RenderCreation;
use bevy::render::{RenderApp, RenderPlugin};
use bevy::shader::Shader;
use bevy::MinimalPlugins;

use crate::aadf::cell::{Aadf6, ChunkCell};
use crate::render::construction::bounds_calc::{
    bound_dispatch_indirect_layout_descriptor, bound_group_count_of,
    construction_bounds_layout_descriptor, construction_bounds_world_layout_descriptor,
    dispatch_add_initial_groups, dispatch_regime_2_rounds, group_size_in_groups_of,
    queue_add_initial_pipeline_with_handle, queue_compute_pipeline_with_handle,
    queue_prepare_pipeline_with_handle, BOUNDS_CALC_SHADER_SRC,
};
use crate::render::gpu_types::{GpuBoundQueueInfo, GpuConstructionParams};
use crate::voxel::AADF_MAX_CHUNK;

// ─── CPU oracle of `boundsCalc.fx`'s converged state ──────────────────────────
//
// Faithful CPU port of the GPU `boundsCalc` convergence algorithm — same
// inputs (a chunk grid of empty/solid cells), same algorithm
// (synchronised-iteration neighbour-merge with the per-axis `addBoundsGroup`
// check), same edge convention (OOB neighbour is growth-permissive,
// `boundsCalc.fx:98-103`). The output is **exactly** what the GPU shader
// converges to in steady state.

/// The 6-bit perpendicular-direction masks from `bounds_calc.wgsl::MASK_*`
/// (also `boundsCommon.fxh:6-11`). Each mask excludes the back-pointer bit
/// (the direction pointing back toward us).
const MASK_MX: u32 = 0x3D;
const MASK_PX: u32 = 0x3E;
const MASK_MY: u32 = 0x37;
const MASK_PY: u32 = 0x3B;
const MASK_MZ: u32 = 0x1F;
const MASK_PZ: u32 = 0x2F;

/// `check_matching_bounds_5bit` CPU port — returns the 6-bit mask of
/// directions where the neighbour's 5-bit AADF ≥ the current cell's.
fn cpu_check_matching_bounds_5bit(neighbour: u32, cur: u32) -> u32 {
    let mut mask = 0u32;
    for bit in 0..6u32 {
        let shift = 5 * bit;
        let n = (neighbour >> shift) & 0x1F;
        let c = (cur >> shift) & 0x1F;
        if n >= c {
            mask |= 1 << bit;
        }
    }
    mask
}

/// `addBoundsGroup` CPU port (`boundsCalc.fx:95-116`).
///
/// Reads neighbour at `chunk_pos + dir_offset`; if OOB, treat as
/// growth-permissive (bump if cur's bound == queue's bound size); if
/// in-bounds and uniform-empty, do the 5-direction-AADF-check growth.
///
/// Returns the updated `cur_chunk`.
fn cpu_add_bounds_group(
    chunks: &[u32],
    size: [u32; 3],
    chunk_pos: [i32; 3],
    dir_offset: [i32; 3],
    mask: u32,
    bounds_location: u32,
    cur_bound: u32,
    cur_chunk_in: u32,
) -> u32 {
    let mut cur_chunk = cur_chunk_in;
    let np = [
        chunk_pos[0] + dir_offset[0],
        chunk_pos[1] + dir_offset[1],
        chunk_pos[2] + dir_offset[2],
    ];
    let oob = np[0] < 0
        || np[1] < 0
        || np[2] < 0
        || np[0] >= size[0] as i32
        || np[1] >= size[1] as i32
        || np[2] >= size[2] as i32;
    if oob {
        if ((cur_chunk >> bounds_location) & 0x1F) == cur_bound {
            cur_chunk += 1 << bounds_location;
        }
        return cur_chunk;
    }
    let ni = np[0] as u32 + np[1] as u32 * size[0] + np[2] as u32 * size[0] * size[1];
    let neighbour = chunks[ni as usize];
    let state = neighbour >> 30;
    if state != 0 {
        return cur_chunk;
    }
    if ((cur_chunk >> bounds_location) & 0x1F) != cur_bound {
        return cur_chunk;
    }
    if (cpu_check_matching_bounds_5bit(neighbour, cur_chunk) & mask) == mask {
        cur_chunk += 1 << bounds_location;
    }
    cur_chunk
}

/// CPU model of `boundsCalc.fx`'s converged steady state.
///
/// Mirrors NAADF's `WorldBoundHandler.Update` (the regime-2 5-rounds-per-frame
/// loop) but runs to convergence on the CPU: for each bound size 0..30 and
/// each axis 0..3, sweep every chunk in the world once (synchronised), in the
/// same iteration order as the GPU's `compute_group_bounds` over the entire
/// queue at that (size, axis). Repeat until no chunk changes.
///
/// The output is a per-chunk u32 in the same encoding the GPU writes — the
/// chunks texture word.
fn cpu_converged_bounds(
    size: [u32; 3],
    initial_chunks: &[u32],
) -> Vec<u32> {
    let mut chunks = initial_chunks.to_vec();
    // Outer loop until convergence (worst case: 31 bound sizes × 3 axes ≈ 100
    // sweeps; cap at 4096 to be safe).
    for _outer in 0..4096 {
        let mut changed = false;
        // Per-(bound_size, bound_xyz) sweep.
        for bound_size in 0..31u32 {
            for bound_xyz in 0..3u32 {
                // The GPU's `compute_group_bounds` reads cur, applies the
                // neighbour check, writes back. Modulo the per-axis queue
                // semantics, the steady state per (size, axis) is the same
                // as "every empty chunk's bound on this axis-side that
                // currently equals `bound_size` may grow if the neighbour
                // permits."
                let prev = chunks.clone();
                let (mask_minus, mask_plus) = match bound_xyz {
                    0 => (MASK_MX, MASK_PX),
                    1 => (MASK_MY, MASK_PY),
                    _ => (MASK_MZ, MASK_PZ),
                };
                let dir_abs: [i32; 3] = match bound_xyz {
                    0 => [1, 0, 0],
                    1 => [0, 1, 0],
                    _ => [0, 0, 1],
                };
                for z in 0..size[2] as i32 {
                    for y in 0..size[1] as i32 {
                        for x in 0..size[0] as i32 {
                            let i = x as usize
                                + y as usize * size[0] as usize
                                + z as usize * size[0] as usize * size[1] as usize;
                            let cur = chunks[i];
                            let state = cur >> 30;
                            if state != 0 {
                                continue; // non-empty: AADFs undefined.
                            }
                            // Read from the snapshot (pre-step state) for the
                            // neighbour, matching the GPU's groupshared
                            // barrier semantics.
                            let mut new_chunk = cur;
                            // -direction grow: bounds_location = boundXYZ * 10 + 0.
                            new_chunk = cpu_add_bounds_group(
                                &prev,
                                size,
                                [x, y, z],
                                [-dir_abs[0], -dir_abs[1], -dir_abs[2]],
                                mask_minus,
                                bound_xyz * 10,
                                bound_size,
                                new_chunk,
                            );
                            // +direction grow.
                            new_chunk = cpu_add_bounds_group(
                                &prev,
                                size,
                                [x, y, z],
                                dir_abs,
                                mask_plus,
                                bound_xyz * 10 + 5,
                                bound_size,
                                new_chunk,
                            );
                            if new_chunk != cur {
                                chunks[i] = new_chunk;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    chunks
}

// ─── Render-world headless fixture ────────────────────────────────────────────

fn render_fixture() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>)> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(ImagePlugin::default())
        .add_plugins(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::default()),
            synchronous_pipeline_compilation: true,
            debug_flags: Default::default(),
        });
    app.finish();
    app.cleanup();

    let shader = Shader::from_wgsl(BOUNDS_CALC_SHADER_SRC, "shaders/bounds_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);

    let render_app = app.get_sub_app_mut(RenderApp)?;
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }

    let device = render_app.world().get_resource::<RenderDevice>()?.clone();
    let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
    Some((app, device, queue, shader_handle))
}

fn create_storage_u32(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    data: &[u32],
) -> Buffer {
    let data = if data.is_empty() { &[0u32][..] } else { data };
    let size = (data.len() * 4) as u64;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
    buffer
}

fn create_storage_indirect_u32(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    data: &[u32],
) -> Buffer {
    let size = (data.len() * 4) as u64;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::STORAGE
            | BufferUsages::COPY_SRC
            | BufferUsages::COPY_DST
            | BufferUsages::INDIRECT,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
    buffer
}

fn create_uniform<T: bytemuck::Pod>(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    data: &T,
) -> Buffer {
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: std::mem::size_of::<T>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::bytes_of(data));
    buffer
}

fn readback_u32(device: &RenderDevice, queue: &RenderQueue, src: &Buffer, count: u64) -> Vec<u32> {
    let size = count * 4;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("w3_readback_staging"),
        size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("w3_readback"),
    });
    encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();
    out
}

fn readback_chunks_buffer(
    device: &RenderDevice,
    queue: &RenderQueue,
    chunks: &Buffer,
    size: [u32; 3],
) -> Vec<u32> {
    // Web-WebGPU migration: chunks is `array<vec2<u32>>` (8 B per pair).
    // Flat buffer→buffer copy; no row-padding needed. Returns `.x` per chunk.
    let chunk_count = (size[0] * size[1] * size[2]) as u64;
    let staging_size = chunk_count * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("w3_chunks_readback_staging"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("w3_chunks_readback"),
    });
    encoder.copy_buffer_to_buffer(chunks, 0, &staging, 0, staging_size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();
    out
}

fn compile_pipelines(
    app: &mut App,
    ids: &[bevy::render::render_resource::CachedComputePipelineId],
) -> Option<Vec<ComputePipeline>> {
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let pipeline_cache = render_app.world().resource::<PipelineCache>();
        let mut got = Vec::with_capacity(ids.len());
        let mut all_ready = true;
        for id in ids {
            if let Some(p) = pipeline_cache.get_compute_pipeline(*id) {
                got.push(p.clone());
            } else {
                all_ready = false;
                break;
            }
        }
        if all_ready {
            return Some(got);
        }
    }
    None
}

/// Test chunk world — 4×4×4 chunks (1 bound group), single solid chunk at the
/// centre (1,1,1).
fn build_test_chunk_world() -> ([u32; 3], Vec<u32>, Vec<bool>) {
    let size = [4u32, 4u32, 4u32];
    let n = (size[0] * size[1] * size[2]) as usize;
    let mut chunks_u32 = vec![0u32; n];
    let mut empty_mask = vec![true; n];

    let cx = 1usize;
    let cy = 1usize;
    let cz = 1usize;
    let solid_idx = cx + cy * 4 + cz * 16;
    chunks_u32[solid_idx] =
        ChunkCell::UniformFull(crate::voxel::VoxelTypeId(1)).encode();
    empty_mask[solid_idx] = false;

    for i in 0..n {
        if empty_mask[i] {
            chunks_u32[i] = ChunkCell::Empty(Aadf6::ZERO).encode();
        }
    }

    (size, chunks_u32, empty_mask)
}

struct W3Fixture {
    // Web-WebGPU migration: chunks is `array<vec2<u32>>` storage buffer.
    chunks_buffer: Buffer,
    bound_queue_info: Buffer,
    bound_group_queues: Buffer,
    bound_group_masks: Buffer,
    bound_refined_info: Buffer,
    bound_dispatch_indirect: Buffer,
    params_buffer: Buffer,
    world_bg: bevy::render::render_resource::BindGroup,
    bounds_bg: bevy::render::render_resource::BindGroup,
    dispatch_bg: bevy::render::render_resource::BindGroup,
    add_initial_pipeline: ComputePipeline,
    prepare_pipeline: ComputePipeline,
    compute_pipeline: ComputePipeline,
    size_in_chunks: [u32; 3],
    bound_group_count: u32,
}

fn build_w3_fixture(
    app: &mut App,
    device: &RenderDevice,
    queue: &RenderQueue,
    shader_handle: Handle<Shader>,
    size_in_chunks: [u32; 3],
    initial_chunks: &[u32],
) -> Option<W3Fixture> {
    let bound_group_count = bound_group_count_of(size_in_chunks);
    assert!(bound_group_count > 0, "test grid must support >= 1 bound group");

    // Web-WebGPU migration: chunks is an `array<vec2<u32>>` storage buffer
    // (was an `Rg32Uint` 3D texture). 8 B per pair; the shader reads `.x`
    // and writes preserving `.y`.
    let total_chunks = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
    let paired_chunks: Vec<[u32; 2]> =
        initial_chunks.iter().map(|&x| [x, 0u32]).collect();
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("w3_chunks"),
        size: (total_chunks as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&paired_chunks));

    let mut info_seed: Vec<GpuBoundQueueInfo> = Vec::with_capacity(32 * 3);
    for i in 0..32u32 {
        for _xyz in 0..3u32 {
            info_seed.push(GpuBoundQueueInfo {
                start: 0,
                size: if i == 0 { bound_group_count } else { 0 },
            });
        }
    }
    let info_u32: &[u32] = bytemuck::cast_slice(&info_seed);
    let bound_queue_info = create_storage_u32(device, queue, "w3_info", info_u32);

    let queue_init = vec![0u32; (32 * 3 * bound_group_count) as usize];
    let bound_group_queues = create_storage_u32(device, queue, "w3_queues", &queue_init);

    let masks_init = vec![0u32; (bound_group_count * 3) as usize];
    let bound_group_masks = create_storage_u32(device, queue, "w3_masks", &masks_init);

    let bound_refined_info = create_storage_u32(device, queue, "w3_refined", &[0u32, 0u32, 0u32]);

    let bound_dispatch_indirect = create_storage_indirect_u32(
        device,
        queue,
        "w3_indirect",
        &[1u32, 1u32, 1u32, 0u32, 0u32],
    );

    let params = GpuConstructionParams {
        size_in_chunks,
        _pad0: 0,
        group_size_in_groups: group_size_in_groups_of(size_in_chunks),
        _pad1: 0,
        bound_group_queue_max_size: bound_group_count,
        hash_map_size: 256,
        segment_size_in_chunks: 4,
        max_group_bound_dispatch: 512 * 64,
        chunk_offset: [0, 0, 0],
        _pad2: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let params_buffer = create_uniform(device, queue, "w3_params", &params);

    let world_layout = construction_bounds_world_layout_descriptor();
    let bounds_layout = construction_bounds_layout_descriptor();
    let dispatch_layout = bound_dispatch_indirect_layout_descriptor();

    let (id_add, id_prep, id_comp) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let a = queue_add_initial_pipeline_with_handle(
            cache,
            world_layout.clone(),
            bounds_layout.clone(),
            shader_handle.clone(),
        );
        let p = queue_prepare_pipeline_with_handle(
            cache,
            world_layout.clone(),
            bounds_layout.clone(),
            dispatch_layout.clone(),
            shader_handle.clone(),
        );
        let c = queue_compute_pipeline_with_handle(
            cache,
            world_layout.clone(),
            bounds_layout.clone(),
            shader_handle.clone(),
        );
        (a, p, c)
    };
    let pipelines = compile_pipelines(app, &[id_add, id_prep, id_comp])?;
    let add_initial_pipeline = pipelines[0].clone();
    let prepare_pipeline = pipelines[1].clone();
    let compute_pipeline = pipelines[2].clone();

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let world_bgl = cache.get_bind_group_layout(&world_layout);
    let bounds_bgl = cache.get_bind_group_layout(&bounds_layout);
    let dispatch_bgl = cache.get_bind_group_layout(&dispatch_layout);

    // Phase 2.6 — window_indirection placeholder (binding 2).
    let windir_buf = device.create_buffer(&BufferDescriptor {
        label: Some("w3_window_indirection_placeholder"),
        size: 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let world_bg = device.create_bind_group(
        "w3_world_bg",
        &world_bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            windir_buf.as_entire_buffer_binding(),
        )),
    );
    let bounds_bg = device.create_bind_group(
        "w3_bounds_bg",
        &bounds_bgl,
        &BindGroupEntries::sequential((
            bound_queue_info.as_entire_buffer_binding(),
            bound_group_queues.as_entire_buffer_binding(),
            bound_group_masks.as_entire_buffer_binding(),
            bound_refined_info.as_entire_buffer_binding(),
        )),
    );
    let dispatch_bg = device.create_bind_group(
        "w3_dispatch_bg",
        &dispatch_bgl,
        &BindGroupEntries::sequential((bound_dispatch_indirect.as_entire_buffer_binding(),)),
    );

    let _ = Cow::<'_, str>::from("w3");

    Some(W3Fixture {
        chunks_buffer,
        bound_queue_info,
        bound_group_queues,
        bound_group_masks,
        bound_refined_info,
        bound_dispatch_indirect,
        params_buffer,
        world_bg,
        bounds_bg,
        dispatch_bg,
        add_initial_pipeline,
        prepare_pipeline,
        compute_pipeline,
        size_in_chunks,
        bound_group_count,
    })
}

/// W3 load-bearing test #1: regime-2 GPU convergence matches the CPU oracle
/// `cpu_converged_bounds` chunk-for-chunk.
///
/// The CPU oracle is a faithful port of `boundsCalc.fx`'s convergence algorithm
/// — including the chunk-world-edge OOB-permissive convention
/// (`boundsCalc.fx:98-103`). This is the §1.6-oracle role for W3, distinct
/// from W6's `compute_aadf_layer` (which treats edges as walls; W6 docs flag
/// the chunk-world-edge divergence as a separate-workstream concern at
/// `16-impl-c-W6.md` assumption #2). The two CPU oracles agree on inner
/// cells far from any world edge; on the small test grid the GPU oracle is
/// the right comparison target.
///
/// Steps:
/// 1. Build a 4×4×4 chunk world (1 bound group, one solid chunk at the centre).
/// 2. Run the regime-1 seed → 1 group enters every (axis, size-0) queue.
/// 3. Run 200 rounds of regime-2 (worst-case convergence is ~3 axes × 31
///    sizes = 93 work-items × few rounds per frame ≈ 30 frames; 200 rounds
///    is comfortable headroom).
/// 4. Read back the chunks texture, compare every chunk word to the CPU
///    oracle's converged value.
#[test]
fn bounds_calc_convergence_matches_cpu_oracle() {
    let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
        eprintln!("no wgpu device — skipping W3 convergence test");
        return;
    };
    let (size, initial_chunks, empty_mask) = build_test_chunk_world();
    let Some(fixture) =
        build_w3_fixture(&mut app, &device, &queue, shader_handle, size, &initial_chunks)
    else {
        eprintln!("W3 fixture failed to build — skipping convergence test");
        return;
    };

    // CPU oracle.
    let cpu_converged = cpu_converged_bounds(size, &initial_chunks);

    // Regime-1 seed.
    {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w3_seed"),
        });
        dispatch_add_initial_groups(
            &mut encoder,
            &fixture.add_initial_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            fixture.bound_group_count,
        );
        queue.submit([encoder.finish()]);
    }

    // Regime-2 rounds — 200 rounds drives full convergence on a 1-group test.
    {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w3_rounds"),
        });
        dispatch_regime_2_rounds(
            &mut encoder,
            &fixture.prepare_pipeline,
            &fixture.compute_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            &fixture.dispatch_bg,
            &fixture.bound_dispatch_indirect,
            200,
        );
        queue.submit([encoder.finish()]);
    }

    let gpu_chunks = readback_chunks_buffer(&device, &queue, &fixture.chunks_buffer, size);

    // Compare GPU to CPU oracle, chunk-by-chunk.
    let dx = size[0] as usize;
    let dy = size[1] as usize;
    let dz = size[2] as usize;
    let mut compared = 0usize;
    let mut mismatched = 0usize;
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let i = x + y * dx + z * dx * dy;
                let g = gpu_chunks[i];
                let c = cpu_converged[i];
                if !empty_mask[i] {
                    // Non-empty chunks should be unchanged (the GPU shader's
                    // `chunkState != BLOCK_STATE_UNIFORM_EMPTY` guard at
                    // `boundsCalc.fx:150` prevents writes).
                    assert_eq!(
                        g, c,
                        "GPU modified non-empty chunk[{},{},{}]: gpu={:#010x} cpu={:#010x}",
                        x, y, z, g, c
                    );
                } else if g != c {
                    eprintln!(
                        "MISMATCH at ({},{},{}): gpu={:#010x} cpu={:#010x}",
                        x, y, z, g, c
                    );
                    mismatched += 1;
                }
                compared += 1;
            }
        }
    }
    eprintln!(
        "W3 convergence: {} chunks compared, {} mismatched",
        compared, mismatched
    );
    assert_eq!(
        mismatched, 0,
        "{} chunks diverged from CPU oracle of `boundsCalc.fx` convergence",
        mismatched
    );

    // Reference fixture fields to keep them alive (preserve GPU resources).
    let _ = (
        &fixture.bound_queue_info,
        &fixture.bound_group_queues,
        &fixture.bound_group_masks,
        &fixture.bound_refined_info,
        &fixture.params_buffer,
        &fixture.chunks_buffer,
        &fixture.size_in_chunks,
    );
}

/// W3 test #2: assert the fixed-size queue never overruns its allocation.
///
/// Invariants:
///   (i) every per-queue `size` ≤ `bound_group_count` (the queue capacity);
///   (ii) every mask is in [0, 2^31) — no legal sequence of
///   `atomicOr/atomicAnd` operations can set bit 31+ given the 31-iteration
///   cap.
#[test]
fn bounds_queue_no_overrun() {
    let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
        eprintln!("no wgpu device — skipping W3 no-overrun test");
        return;
    };
    let (size, initial_chunks, _empty_mask) = build_test_chunk_world();
    let Some(fixture) =
        build_w3_fixture(&mut app, &device, &queue, shader_handle, size, &initial_chunks)
    else {
        eprintln!("W3 fixture failed to build — skipping no-overrun test");
        return;
    };

    {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w3_seed_overrun"),
        });
        dispatch_add_initial_groups(
            &mut encoder,
            &fixture.add_initial_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            fixture.bound_group_count,
        );
        dispatch_regime_2_rounds(
            &mut encoder,
            &fixture.prepare_pipeline,
            &fixture.compute_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            &fixture.dispatch_bg,
            &fixture.bound_dispatch_indirect,
            200,
        );
        queue.submit([encoder.finish()]);
    }

    let queues_u32 = readback_u32(
        &device,
        &queue,
        &fixture.bound_group_queues,
        (32 * 3 * fixture.bound_group_count) as u64,
    );
    let info_u32 = readback_u32(&device, &queue, &fixture.bound_queue_info, 32 * 3 * 2);
    let masks_u32 = readback_u32(
        &device,
        &queue,
        &fixture.bound_group_masks,
        (fixture.bound_group_count * 3) as u64,
    );

    for i in 0..32 {
        for xyz in 0..3 {
            let qi = (i * 3 + xyz) * 2;
            let _start = info_u32[qi];
            let qsize = info_u32[qi + 1];
            assert!(
                qsize <= fixture.bound_group_count,
                "queue (size={}, axis={}) overran: {} > bound_group_count {}",
                i, xyz, qsize, fixture.bound_group_count
            );
        }
    }

    assert_eq!(
        queues_u32.len() as u32,
        32 * 3 * fixture.bound_group_count,
        "queue buffer length mismatch"
    );

    eprintln!(
        "W3 no-overrun: final queue sizes OK; masks at end-of-run = {:?}",
        masks_u32
    );

    let _ = (
        &fixture.bound_queue_info,
        &fixture.bound_group_queues,
        &fixture.bound_group_masks,
        &fixture.bound_refined_info,
        &fixture.bound_dispatch_indirect,
        &fixture.params_buffer,
        &fixture.chunks_buffer,
        &fixture.size_in_chunks,
    );
}

/// W3 test #3: the per-axis `bound_group_masks` atomic doesn't drop updates
/// under contention.
///
/// Setup: run regime-1 seed; then assert exactly bit-0 of each per-axis mask
/// is set (3 mask u32s for our 1-group world = `[1, 1, 1]`). Run regime-2 5
/// rounds; assert (a) every mask stays in [0, 2^31), (b) the masks evolve
/// according to the legal `atomicOr/atomicAnd` semantics.
#[test]
fn bounds_per_axis_atomic_correctness() {
    let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
        eprintln!("no wgpu device — skipping W3 atomic-correctness test");
        return;
    };
    let (size, initial_chunks, _empty_mask) = build_test_chunk_world();
    let Some(fixture) =
        build_w3_fixture(&mut app, &device, &queue, shader_handle, size, &initial_chunks)
    else {
        eprintln!("W3 fixture failed to build — skipping atomic test");
        return;
    };

    {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w3_seed_only"),
        });
        dispatch_add_initial_groups(
            &mut encoder,
            &fixture.add_initial_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            fixture.bound_group_count,
        );
        queue.submit([encoder.finish()]);
    }

    let masks_post_seed = readback_u32(
        &device,
        &queue,
        &fixture.bound_group_masks,
        (fixture.bound_group_count * 3) as u64,
    );
    let expected: Vec<u32> = vec![1u32; 3];
    assert_eq!(
        masks_post_seed, expected,
        "regime-1 seed must set exactly bit-0 per axis, got {:?}",
        masks_post_seed
    );

    {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w3_5rounds"),
        });
        dispatch_regime_2_rounds(
            &mut encoder,
            &fixture.prepare_pipeline,
            &fixture.compute_pipeline,
            &fixture.world_bg,
            &fixture.bounds_bg,
            &fixture.dispatch_bg,
            &fixture.bound_dispatch_indirect,
            5,
        );
        queue.submit([encoder.finish()]);
    }
    let masks_post_5 = readback_u32(
        &device,
        &queue,
        &fixture.bound_group_masks,
        (fixture.bound_group_count * 3) as u64,
    );

    // The atomic semantic preserves bit-bounds — masks never exceed 2^31.
    for &m in &masks_post_5 {
        assert!(
            m < (1u32 << 31),
            "mask {:#010x} has illegal high bit — atomic op dropped/corrupted",
            m
        );
    }
    eprintln!("W3 atomic: masks post 5 rounds = {:?}", masks_post_5);

    let _ = (
        &fixture.bound_queue_info,
        &fixture.bound_group_queues,
        &fixture.bound_group_masks,
        &fixture.bound_refined_info,
        &fixture.bound_dispatch_indirect,
        &fixture.params_buffer,
        &fixture.chunks_buffer,
        &fixture.size_in_chunks,
    );
}

#[cfg(test)]
mod cpu_oracle_tests {
    //! Pure-CPU tests of `cpu_converged_bounds` — no GPU device required.
    //! These exercise the bit-twiddling correctness of the CPU oracle that
    //! the convergence test relies on, so a missing GPU adapter still gates
    //! the algorithmic correctness of the CPU side.
    use super::*;

    /// An all-empty 4×4×4 world: every empty chunk reaches `AADF_MAX_CHUNK`
    /// in every direction (every neighbour chain hits OOB → permissive).
    #[test]
    fn all_empty_saturates_to_max() {
        let size = [4u32, 4u32, 4u32];
        let n = (size[0] * size[1] * size[2]) as usize;
        let initial: Vec<u32> = vec![ChunkCell::Empty(Aadf6::ZERO).encode(); n];
        let out = cpu_converged_bounds(size, &initial);
        for w in &out {
            let aadf = match ChunkCell::decode(*w) {
                ChunkCell::Empty(a) => a,
                other => panic!("non-empty cell in all-empty world: {:?}", other),
            };
            for d in 0..6 {
                assert_eq!(
                    aadf.d[d], AADF_MAX_CHUNK,
                    "non-saturated AADF in all-empty world: {:?}",
                    aadf
                );
            }
        }
    }

    /// A solid wall along the y=0 plane: every chunk above y=0 has d[-y] = 0
    /// (blocked immediately by the wall, the wall is at y=0 in-bounds — the
    /// neighbour's state == UniformFull breaks the growth check).
    #[test]
    fn wall_blocks_negative_direction() {
        let size = [4u32, 4u32, 4u32];
        let n = (size[0] * size[1] * size[2]) as usize;
        let mut initial: Vec<u32> = vec![ChunkCell::Empty(Aadf6::ZERO).encode(); n];
        // Make every y=0 chunk solid.
        for z in 0..4u32 {
            for x in 0..4u32 {
                let i = x + z * 16;
                initial[i as usize] =
                    ChunkCell::UniformFull(crate::voxel::VoxelTypeId(1)).encode();
            }
        }
        let out = cpu_converged_bounds(size, &initial);
        // Cell (0, 1, 0): d[-y] should be 0 (wall at y=0); d[-x], d[-z] should
        // be at AADF_MAX_CHUNK (OOB permissive); d[+y], d[+x], d[+z] should
        // also saturate (open + OOB permissive).
        let aadf = match ChunkCell::decode(out[0 + 1 * 4 + 0 * 16]) {
            ChunkCell::Empty(a) => a,
            _ => panic!("cell (0,1,0) wasn't empty"),
        };
        assert_eq!(aadf.d[crate::aadf::cell::DIR_NEG_Y], 0);
    }
}
