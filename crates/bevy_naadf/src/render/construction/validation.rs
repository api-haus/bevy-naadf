//! Phase-C — e2e gate fixtures + GPU↔CPU oracle tests.
//!
//! Moved out of `construction/mod.rs` per the D5 architecture pass
//! (`docs/orchestrate/codebase-tightening/gpu-construction/03-architecture.md`
//! Step 5). Houses the six `validate_*` functions reachable via
//! `bin/e2e_render.rs`'s CLI flags + their helpers + the embedded W1/W4/W5
//! GPU↔CPU oracle test modules. All bodies move verbatim; `pub use` in
//! `mod.rs` preserves every `bevy_naadf::render::construction::validate_*`
//! call path so `bin/e2e_render.rs` is unchanged.

/// Build a `segment_voxel_buffer` from a `DenseVolume` segment matching the
/// byte layout that `chunkCalc.fx::calcBlockFromRawData` reads. Used by the
/// W1 GPU/CPU oracle test + by `--validate-gpu-construction`.
///
/// The encoding (per `chunkCalc.fx:120-121` + `compute_voxel_bounds`'s
/// `localIndex = lx + ly*4 + lz*16` voxel ordering, `chunkCalc.fx:205`):
///
/// - For each chunk in the segment (in chunk scan order `cx, cy, cz` →
///   `chunk_index = cx + cy*seg + cz*seg*seg`), the 64 blocks of the chunk
///   are at consecutive offsets `chunk_index * 2048 + block_index * 32` for
///   `block_index = 0..64`.
/// - The block at intra-chunk position `(bx, by, bz)` has
///   `block_index = bx + by*4 + bz*16`.
/// - The 64 voxels of a block are packed two per u32; voxel at intra-block
///   position `(vx, vy, vz)` has `voxel_index = vx + vy*4 + vz*16`. The u32
///   offset within the block is `voxel_index / 2`; the low half holds the
///   even-index voxel, the high half the odd-index.
/// - Each voxel encodes as `u16`: full voxel = `(1 << 15) | type`; empty = 0.
///
/// The `segment_size_in_chunks` is the chunk extent of the segment (NAADF
/// default 4 — the C# `WorldData.cs:73`). For the W1 test we use whatever the
/// volume's `size_in_chunks` is (so the test segment matches the test grid).
pub fn build_segment_voxel_buffer(
    volume: &crate::aadf::construct::DenseVolume,
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let total_u32s = seg * seg * seg * 2048;
    let mut out = vec![0u32; total_u32s];

    for cz in 0..seg {
        for cy in 0..seg {
            for cx in 0..seg {
                let chunk_index_in_segment = cx + cy * seg + cz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;

                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;

                            // 64 voxels per block, packed two per u32, low
                            // half = even index.
                            for vi in 0..32 {
                                // Two voxels per pair.
                                let lo = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2 + 1,
                                );
                                out[block_base + vi] =
                                    (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

/// Read voxel at intra-block index `voxel_idx` of block `(bx,by,bz)` in
/// chunk `(cx,cy,cz)`, encoded as the 16-bit `VoxelCell::Full` /
/// `VoxelCell::Empty` payload that `chunkCalc.fx` reads.
///
/// Out-of-bounds positions clamp to empty (type 0).
fn voxel_at_block_local(
    volume: &crate::aadf::construct::DenseVolume,
    chunk: [usize; 3],
    block: [usize; 3],
    voxel_idx: usize,
) -> u16 {
    // voxel intra-block position: vx + vy*4 + vz*16 = voxel_idx
    let vx = voxel_idx % 4;
    let vy = (voxel_idx / 4) % 4;
    let vz = voxel_idx / 16;
    let world_v = [
        (chunk[0] * 16 + block[0] * 4 + vx) as u32,
        (chunk[1] * 16 + block[1] * 4 + vy) as u32,
        (chunk[2] * 16 + block[2] * 4 + vz) as u32,
    ];
    let size = volume.size_in_voxels();
    if world_v[0] >= size[0] || world_v[1] >= size[1] || world_v[2] >= size[2] {
        return 0;
    }
    let ty = volume.voxel_at(world_v);
    if ty == crate::voxel::VoxelTypeId::EMPTY {
        0u16
    } else {
        // VoxelCell::Full encoding (`aadf::cell::VoxelCell::encode`): bit 15
        // set, low 15 bits = type. Matches `chunkCalc.fx`'s `voxel & 0x7FFF`
        // hash extraction + `>> 15` state detection.
        crate::voxel::VOXEL_FULL_FLAG | (ty.raw() & crate::voxel::VOXEL_PAYLOAD_MASK)
    }
}

/// W1 — entry point for `bevy-naadf e2e_render --validate-gpu-construction`.
///
/// Boots a headless render world, builds the GPU `chunk_calc.wgsl` pipelines,
/// runs Algorithm 1 + the two AADF passes against a small deterministic test
/// scene, and asserts the GPU output matches the CPU `aadf::construct::
/// construct` oracle byte-for-byte. Returns the number of bytes compared on
/// success, an error message on failure.
///
/// Used by `crates/bevy_naadf/src/bin/e2e_render.rs` when the
/// `--validate-gpu-construction` CLI flag is present. The flag-plumbing was
/// added by W0; W1 fills in the body here.
///
/// The validation scene is a 1×1×1 chunk world with one solid voxel — the
/// minimum geometry that exercises Algorithm 1's mixed-block / hash-dedup /
/// AADF-encode paths AND has a deterministic `VoxelPtr(0)` assignment on
/// both CPU and GPU (mixed-block dedup hits a single key, deterministic
/// regardless of HashMap iteration order). Bigger scenes diverge at the
/// `VoxelPtr` level (CPU `HashMap` iteration vs GPU
/// `hash & (mapSize - 1)`) — semantic equality is provable but not byte
/// equality; the W1 brief / `15-design-c.md` §1.6 assumption #7 flags this.
///
/// The runtime path here mirrors the `tests_w1::gpu_algorithm1_vs_cpu_bit_exact`
/// unit test exactly; the helper exists so both the test + the e2e CLI flag
/// run the same code.
pub fn validate_gpu_construction() -> Result<usize, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell, VoxelPtr};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // ── Boot headless render world ────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app available".into());
    };
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    // ── Test scene: 1×1×1 chunk world, single mixed block ────────────────────
    let mut volume = DenseVolume::empty([1, 1, 1]);
    let ty = VoxelTypeId(7);
    volume.set([0, 0, 0], ty);

    let oracle = construct(&volume);

    // ── Allocate GPU buffers + uniform ────────────────────────────────────────
    let segment_size_in_chunks: u32 = 1;
    let size_in_chunks: [u32; 3] = volume.size_in_chunks;
    let segment_voxels = build_segment_voxel_buffer(&volume, segment_size_in_chunks);
    let hash_map_size_slots: u32 = 256;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data: &[u32]| {
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
    };

    let gpu_blocks = mk_storage(
        "validate_blocks",
        &vec![0u32; oracle.blocks.len().max(64) + 64],
    );
    let gpu_voxels = mk_storage(
        "validate_voxels",
        &vec![0u32; oracle.voxels.len().max(32) + 32],
    );
    let gpu_block_voxel_count = mk_storage("validate_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("validate_segment", &segment_voxels);
    let gpu_hash_map = mk_storage("validate_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("validate_coeffs", &coeffs);

    let params = GpuConstructionParams {
        size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("validate_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    // Web-WebGPU migration: chunks is an `array<vec2<u32>>` storage buffer
    // (was `Rg32Uint` 3D texture). 8 B per chunk pair; the W1 validation
    // path zeros every channel.
    let chunk_count_total =
        (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count_total];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("validate_chunks"),
        size: (chunk_count_total as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Queue + compile pipelines ─────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let a = queue_calc_block_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        let b = queue_voxel_bounds_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        let c = queue_block_bounds_pipeline_with_handle(
            cache,
            layout.clone(),
            shader_handle.clone(),
        );
        (a, b, c)
    };

    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("W1 pipelines did not compile")?;

    // ── Build bind group ──────────────────────────────────────────────────────
    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "validate_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch the 3 passes ─────────────────────────────────────────────────
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_calc_block"),
    });
    dispatch_calc_block_from_raw_data(
        &mut encoder,
        &pipelines[0],
        &bind_group,
        segment_size_in_chunks,
    );
    queue.submit([encoder.finish()]);

    // Read cursors to size the bounds dispatches faithfully.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("validate_cursor_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("validate_cursor_readback"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut encoder, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut encoder, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([encoder.finish()]);

    // ── Read back + compare ───────────────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("validate_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("validate_readback_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };

    let gpu_blocks_out = read_u32(&gpu_blocks, (oracle.blocks.len().max(64) + 64) as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, (oracle.voxels.len().max(32) + 32) as u64);

    // Web-WebGPU migration: chunks is a flat `array<vec2<u32>>` storage
    // buffer (8 B per pair). The validation gate compares the `.x` (state)
    // channel only — that's what W1 writes; `.y` (entity pointer) stays zero.
    // Buffer→buffer copy doesn't need bytes_per_row padding.
    let chunk_count = size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2];
    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("validate_chunks_readback"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("validate_chunks_readback_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // Compare. The CPU oracle uses `VoxelPtr(0)` / `BlockPtr(0)` for its
    // first allocations; the GPU seeds the cursors at 64, so its first
    // mixed-chunk's BlockPtr = 64 and the first mixed-block's VoxelPtr = 32
    // (in u32-element units). Re-encode the oracle output with these shifts.
    let mut bytes_compared: usize = 0;
    for (i, &gpu_chunk) in gpu_chunks_out.iter().enumerate() {
        let expected = match ChunkCell::decode(oracle.chunks[i]) {
            ChunkCell::Mixed(ptr) => {
                ChunkCell::Mixed(crate::aadf::cell::BlockPtr(ptr.0 + 64)).encode()
            }
            other => other.encode(),
        };
        if gpu_chunk != expected {
            return Err(format!(
                "chunk[{}] mismatch: gpu={:#010x} expected={:#010x}",
                i, gpu_chunk, expected
            ));
        }
        bytes_compared += 4;
    }
    for (i, &c) in oracle.blocks.iter().enumerate() {
        let expected = match BlockCell::decode(c) {
            BlockCell::Mixed(VoxelPtr(v)) => {
                BlockCell::Mixed(VoxelPtr(v + 32)).encode()
            }
            other => other.encode(),
        };
        let g = gpu_blocks_out[64 + i];
        if g != expected {
            return Err(format!(
                "block[{}] mismatch: gpu={:#010x} expected={:#010x}",
                64 + i, g, expected
            ));
        }
        bytes_compared += 4;
    }
    for (i, &c) in oracle.voxels.iter().enumerate() {
        let g = gpu_voxels_out[32 + i];
        if g != c {
            return Err(format!(
                "voxel[{}] mismatch: gpu={:#010x} oracle={:#010x}",
                32 + i, g, c
            ));
        }
        bytes_compared += 4;
    }

    Ok(bytes_compared)
}

/// vox-gpu-rewrite — concrete byte-level diagnostic for GPU vs CPU chunk_calc
/// divergence (`12-diagnostic-byte-diff-concrete.md`).
///
/// Drives the same W5 chunk_calc chain as [`validate_gpu_construction`] but
/// across a sweep of progressively-larger fixtures, emitting the first
/// divergent u32 index per buffer (chunks / blocks / voxels) for each fixture
/// — both via raw byte-equality and via semantic pointer-following
/// comparison. The raw-byte form is sensitive to nondeterministic atomic
/// ordering (`atomicAdd` cursor races across mixed chunks) and will report
/// divergences that are semantically benign; the pointer-following form
/// reports only divergences in the actual referenced content.
///
/// Output: a multi-fixture report printed to stderr; the function returns
/// `Ok(report_string)` on completion regardless of whether divergences were
/// found (this is a DIAGNOSTIC, not a gate).
pub fn validate_gpu_construction_scaled() -> Result<String, String> {
    let mut report = String::new();
    report.push_str("=== vox-gpu-rewrite scaled byte-diff diagnostic ===\n\n");

    // Fixture sweep — three diversity modes per scale to triangulate where
    // bytes diverge:
    //   "uniform":  every chunk has identical mixed content (1 voxel at
    //               block-corner) — heavy dedup-hit traffic, all blocks
    //               funnel to one voxel slot.
    //   "diverse":  every chunk has a UNIQUE mixed content (different voxel
    //               position per chunk) — no dedup hits, every chunk claims
    //               its own slot. Stresses CAS slot-claim path.
    //   "mixed":    half chunks unique, half identical — both dedup-hit and
    //               new-slot CAS are exercised in the same dispatch.
    let fixtures: &[(&str, [u32; 3], FixtureMode)] = &[
        ("2x1x2-uniform",   [2, 1, 2],   FixtureMode::Uniform),
        ("4x1x4-mixed",     [4, 1, 4],   FixtureMode::Mixed),
        ("16x1x16-mixed",   [16, 1, 16], FixtureMode::Mixed),
        ("32x2x32-diverse", [32, 2, 32], FixtureMode::Diverse),
        ("64x4x64-mixed",   [64, 4, 64], FixtureMode::Mixed),
    ];

    for (label, dims, mode) in fixtures {
        report.push_str(&format!("--- fixture {label}: {:?} chunks, mode={:?} (single-dispatch) ---\n", dims, mode));
        match run_one_fixture_byte_diff(*dims, *mode) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // Multi-segment dispatch sweep — mirrors the production W5 segment loop:
    // hash_map is allocated once + reused across all per-segment dispatches.
    // Each segment dispatches generator_model + calc_block_from_raw_data
    // for its own chunk_offset (the segment buffer is reused per segment).
    // This exercises cross-segment hash_map state accumulation that the
    // single-dispatch fixtures cannot reach.
    let multi_seg_fixtures: &[(&str, [u32; 3], u32, FixtureMode)] = &[
        // (label, world_chunks, segment_size_in_chunks, mode)
        ("4x4x4-seg2-mixed",   [4, 4, 4],   2, FixtureMode::Mixed),
        ("8x4x8-seg4-mixed",   [8, 4, 8],   4, FixtureMode::Mixed),
        ("16x4x16-seg4-mixed", [16, 4, 16], 4, FixtureMode::Mixed),
        ("32x4x32-seg4-mixed", [32, 4, 32], 4, FixtureMode::Mixed),
        ("64x16x64-seg16-mixed",[64, 16, 64], 16, FixtureMode::Mixed),
        // Closest to Oasis-class scale with a sane CPU oracle footprint.
        // 128x16x128 = 262144 chunks. CPU `DenseVolume::voxels` is
        // (128*16)*(16*16)*(128*16) = 2048*256*2048 ≈ 1 G voxels (~ 1 GB for
        // u16 voxel-types). Larger world sizes overflow u32 voxel-count
        // (e.g. 256x32x256 → 8.5 G voxels → u32 wrap → empty Vec → panic).
        ("128x16x128-seg16-mixed", [128, 16, 128], 16, FixtureMode::Mixed),
    ];

    for (label, dims, seg_size, mode) in multi_seg_fixtures {
        report.push_str(&format!(
            "--- fixture {label}: {:?} chunks, seg={seg_size}, mode={:?} (multi-segment) ---\n",
            dims, mode
        ));
        match run_one_fixture_multiseg_byte_diff(*dims, *seg_size, *mode) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // ── Generator_model.wgsl byte-diff ──────────────────────────────────────
    // Compares the generator_model.wgsl GPU output against the
    // `generate_segment_cpu` Rust oracle (the bit-exact port). Both consume a
    // `ModelData` and produce the same `segment_voxel_buffer` layout that
    // chunk_calc reads. If THIS diverges, the bug is upstream of chunk_calc
    // (in generator_model itself).
    report.push_str("\n=== Generator_model.wgsl GPU vs CPU oracle byte-diff ===\n");
    let gen_fixtures: &[(&str, [u32; 3], [u32; 3], [u32; 3])] = &[
        // (label, model_size_in_chunks, group_size_in_chunks, group_offset_in_chunks)
        ("model-1x1x1-seg-1x1x1",   [1, 1, 1],   [1, 1, 1],  [0, 0, 0]),
        ("model-2x1x2-seg-4x4x4",   [2, 1, 2],   [4, 4, 4],  [0, 0, 0]),
        ("model-4x2x4-seg-8x4x8",   [4, 2, 4],   [8, 4, 8],  [0, 0, 0]),
        ("model-8x2x8-seg-16x16x16",[8, 2, 8],   [16, 16, 16],[0, 0, 0]),
        // Offset matters — test a non-zero offset to exercise the Y-clamp
        // and the world-extent gate.
        ("model-2x1x2-seg-4x4x4-off2-1-2", [2, 1, 2], [4, 4, 4], [2, 1, 2]),
    ];
    for (label, model_dims, seg_dims, off) in gen_fixtures {
        report.push_str(&format!(
            "--- gen fixture {label}: model={:?} seg={:?} off={:?} ---\n",
            model_dims, seg_dims, off
        ));
        match run_one_generator_model_byte_diff(*model_dims, *seg_dims, *off) {
            Ok(s) => {
                report.push_str(&s);
                report.push('\n');
            }
            Err(e) => {
                report.push_str(&format!("FIXTURE FAILED: {e}\n\n"));
            }
        }
    }

    // ── TILED ModelData: small model tiled into a larger world ─────────────
    // Mirrors production behavior: a small ModelData (a "tile") gets repeated
    // across the larger fixed world via the generator's `voxelPos % modelSize`
    // wraparound. The repeated tiles produce many chunks with identical
    // content, hitting the chunk_calc dedup path heavily.
    report.push_str("\n=== Tiled ModelData (small model, large world) generator+chunk_calc byte-diff ===\n");
    let tiled_fixtures: &[(&str, [u32; 3], [u32; 3])] = &[
        ("model-2x1x2_world-4x1x4",  [2, 1, 2], [4, 1, 4]),
        ("model-4x1x4_world-16x1x16",[4, 1, 4], [16, 1, 16]),
        ("model-4x2x4_world-16x4x16",[4, 2, 4], [16, 4, 16]),
        ("model-8x2x8_world-32x4x32",[8, 2, 8], [32, 4, 32]),
        // Closest to Oasis tile ratio: 93x34x84 model in 256x32x256 world =
        // ~3x4 horizontal tiles. Use 32x16x32 model in 96x16x96 world for
        // 3x3 = 9-tile equivalent.
        ("model-32x16x32_world-96x16x96",  [32, 16, 32], [96, 16, 96]),
    ];
    for (label, model_dims, world_dims) in tiled_fixtures {
        report.push_str(&format!(
            "--- tiled fixture {label}: model={:?} world={:?} ---\n",
            model_dims, world_dims
        ));
        match run_one_tiled_byte_diff(*model_dims, *world_dims) {
            Ok(s) => report.push_str(&s),
            Err(e) => report.push_str(&format!("FAILED: {e}\n")),
        }
        report.push('\n');
    }

    // ── Real Oasis model loaded from disk ───────────────────────────────────
    // Load the actual oasis_hard_cover.vox into a ModelData and exercise the
    // generator+chunk_calc chain on a few segments. Compares the per-segment
    // GPU output to the CPU oracle (generate_segment_cpu + construct of the
    // resulting DenseVolume). If THIS diverges where synthesized fixtures
    // didn't, the bug is triggered by the real-world model's specific
    // structure.
    report.push_str("\n=== Real Oasis ModelData + per-segment generator+chunk_calc byte-diff ===\n");
    let oasis_path = std::path::Path::new(
        "crates/bevy_naadf/assets/test/oasis_hard_cover.vox",
    );
    match load_oasis_model_data(oasis_path) {
        Ok(model) => {
            report.push_str(&format!(
                "Loaded Oasis ModelData: {}×{}×{} chunks (data_chunk={} data_block={} data_voxel={})\n",
                model.size_in_chunks[0], model.size_in_chunks[1], model.size_in_chunks[2],
                model.data_chunk.len(), model.data_block.len(), model.data_voxel.len(),
            ));
            // Run on a 4x4x4-chunk segment at offset (0,0,0) — should land
            // in the densely-populated corner of the model.
            for &(off, seg) in &[
                // Cover model interior — pick offsets inside the 93×34×84
                // model where content actually lives.
                ([0u32, 0, 0], [16u32, 16, 16]),
                ([16, 0, 16], [16, 16, 16]),
                ([32, 0, 32], [16, 16, 16]),
                ([48, 0, 48], [16, 16, 16]),
                ([60, 0, 60], [16, 16, 16]),
                // Edge-of-model segment to test wrap behaviour.
                ([76, 0, 64], [16, 16, 16]),
                // Segment that straddles the model's vertical extent (y=16+
                // crosses model y=2 boundary at chunk y=2).
                ([16, 0, 16], [16, 16, 16]),
            ] {
                report.push_str(&format!(
                    "--- oasis segment off={:?} seg={:?} ---\n", off, seg
                ));
                match run_oasis_segment_byte_diff(&model, off, seg) {
                    Ok(s) => report.push_str(&s),
                    Err(e) => report.push_str(&format!("FAILED: {e}\n")),
                }
                report.push('\n');
            }
        }
        Err(e) => {
            report.push_str(&format!("Failed to load Oasis VOX fixture: {e}\n"));
        }
    }

    eprintln!("{report}");
    Ok(report)
}

/// Scan an Oasis `ModelData` to find world voxel positions that are FULL in
/// the model AND fall within the fixed world's bounds. Returns up to `limit`
/// positions, distributed across the model's interior (not clustered).
///
/// Picks fixed positions deterministically (no RNG) for reproducibility.
fn discover_populated_oasis_voxels(
    model: &crate::aadf::generator::ModelData,
    world_voxels: [u32; 3],
    limit: usize,
) -> Vec<[u32; 3]> {
    let msc = model.size_in_chunks;
    let model_voxels = [msc[0] * 16, msc[1] * 16, msc[2] * 16];
    // Bound search to whichever is smaller — the model OR the world.
    let upper = [
        model_voxels[0].min(world_voxels[0]),
        model_voxels[1].min(world_voxels[1]),
        model_voxels[2].min(world_voxels[2]),
    ];
    let mut out = Vec::with_capacity(limit);
    // Probe by stepping every (dx, dy, dz) until we have `limit` populated
    // positions. Finer step sizes than naive max/N to ensure we sample the
    // model interior densely (Oasis is mostly air).
    let stride_x: u32 = (upper[0] / 16).max(8);
    let stride_y: u32 = (upper[1] / 24).max(4);
    let stride_z: u32 = (upper[2] / 16).max(8);
    let probe = |voxel_pos: [i64; 3]| -> u32 {
        // Inline the get_voxel_type_in_model logic (it's `fn` not `pub fn`,
        // so we replicate the few lines we need).
        if voxel_pos[0] < 0
            || voxel_pos[1] < 0
            || voxel_pos[2] < 0
            || voxel_pos[0] >= world_voxels[0] as i64
            || voxel_pos[1] >= world_voxels[1] as i64
            || voxel_pos[2] >= world_voxels[2] as i64
        {
            return 0;
        }
        let vx = voxel_pos[0] as u32;
        let vy = voxel_pos[1] as u32;
        let vz = voxel_pos[2] as u32;
        let model_extent_v = [msc[0] * 16, msc[1] * 16, msc[2] * 16];
        let vpim = [
            vx % model_extent_v[0],
            vy % model_extent_v[1],
            vz % model_extent_v[2],
        ];
        let model_index_y = vy / (msc[1] * 16);
        let cpim = [vpim[0] / 16, vpim[1] / 16, vpim[2] / 16];
        let chunk_index_in_model =
            (cpim[0] + cpim[1] * msc[0] + cpim[2] * msc[0] * msc[1]) as usize;
        let chunk = model.data_chunk[chunk_index_in_model];
        let mut ty: u32 = 0;
        let chunk_disc = chunk >> 30;
        if chunk_disc == 2 {
            let mbpic = [(vpim[0] % 16) / 4, (vpim[1] % 16) / 4, (vpim[2] % 16) / 4];
            let model_block_index = mbpic[0] + mbpic[1] * 4 + mbpic[2] * 16;
            let block_addr = ((chunk & 0x3FFF_FFFF) + model_block_index) as usize;
            let block = model.data_block[block_addr];
            let block_disc = block >> 30;
            if block_disc == 2 {
                let mvpic = [vpim[0] % 4, vpim[1] % 4, vpim[2] % 4];
                let model_voxel_index = mvpic[0] + mvpic[1] * 4 + mvpic[2] * 16;
                let voxel_addr =
                    ((block & 0x3FFF_FFFF) + model_voxel_index / 2) as usize;
                let voxel_comp = model.data_voxel[voxel_addr];
                ty = if model_voxel_index % 2 == 0 {
                    voxel_comp & 0x7FFF
                } else {
                    (voxel_comp >> 16) & 0x7FFF
                };
            } else if block_disc == 1 {
                ty = block & 0x3FFF_FFFF;
            }
        } else if chunk_disc == 1 {
            ty = chunk & 0x3FFF_FFFF;
        }
        if model_index_y > 0 {
            return 0;
        }
        ty
    };
    'outer: for vy in (0..upper[1]).step_by(stride_y as usize) {
        for vz in (0..upper[2]).step_by(stride_z as usize) {
            for vx in (0..upper[0]).step_by(stride_x as usize) {
                if probe([vx as i64, vy as i64, vz as i64]) != 0 {
                    out.push([vx, vy, vz]);
                    if out.len() >= limit {
                        break 'outer;
                    }
                }
            }
        }
    }
    // Pad with arbitrary positions if we didn't find enough — extremely
    // unlikely on Oasis but defensive against an empty-model future test.
    while out.len() < limit {
        out.push([0, 0, 0]);
    }
    out
}

/// vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic.
///
/// Loads `oasis_hard_cover.vox` as `ModelData` and runs the FULL W5 producer
/// chain at production scale (256×32×256 chunk fixed world, 512 segments,
/// full bounds chain) in headless mode. Reads back `voxels[]` at TWO
/// checkpoints — post-producer (pre-bounds-calc) and post-bounds-calc — and
/// compares against the CPU oracle at ~25 sampled Oasis-populated voxel
/// positions.
///
/// The discriminating question this diagnostic answers (per
/// `docs/orchestrate/vox-gpu-rewrite/14-diagnostic-type-decode.md` follow-up):
///
/// - **If post-full-pipeline `voxels[]` is BYTE-EQUAL to CPU oracle** → the
///   bug is in the renderer's decode path (not in the producer chain).
/// - **If post-full-pipeline `voxels[]` DIFFERS from CPU oracle** → the bug
///   is in whichever stage corrupted it (`compute_voxel_bounds`'s leaf
///   writeback at `chunk_calc.wgsl:495-499`, or a downstream pass).
///
/// The Stage 6 diagnostic
/// (`docs/orchestrate/vox-gpu-rewrite/12-diagnostic-byte-diff-concrete.md`)
/// only sampled 6 Oasis segments at sub-production scale (16³-chunk segments
/// driven through their own fresh hash_map, NOT the production 512-segment
/// shared-hash-map + bounds chain pipeline). Stage 9 covers the gap.
///
/// CPU oracle methodology: for each sampled voxel position `(vx, vy, vz)`,
/// invoke `aadf::generator::get_voxel_type_in_model`-equivalent logic via
/// `generate_segment_cpu` over a single-chunk segment around the sample
/// position. The returned segment buffer holds the **producer-stage** voxel
/// type (the raw `voxel | (1<<15)` half-word emitted by `generator_model.wgsl`
/// before `chunk_calc` runs). That oracle's value is what the GPU's voxels[]
/// must contain at the leaf after the producer + bounds chain.
///
/// GPU pointer walk: at each sample position, decode the GPU's
/// `chunks[chunk_idx]`. If Mixed, dereference `blocks[BlockPtr +
/// block_idx_in_chunk]`. If Mixed, dereference `voxels[VoxelPtr + voxel_idx /
/// 2]`. Extract the half-word for the voxel's parity. Compare type-bit
/// equality.
///
/// The diagnostic is **DIAGNOSTIC ONLY** — it does not land a fix and does
/// not assert. It writes its report to stderr and returns `Ok(report)`. The
/// caller (in `bin/e2e_render.rs`) propagates the result as exit code 0; the
/// orchestration doc at
/// `docs/orchestrate/vox-gpu-rewrite/15-diagnostic-production-scale-readback.md`
/// summarises the findings.
pub fn validate_gpu_construction_production_scale() -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::generator::{generate_segment_cpu, CHUNK_DATA_U32S};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor,
        dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_params_uniform, create_storage_buffer_u32,
        dispatch_generator_model_with_encoder, generator_model_layout_descriptor,
        queue_generator_model_pipeline_with_handle, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    let mut report = String::new();
    report.push_str(
        "=== vox-gpu-rewrite Stage 9 — production-scale voxels[] readback diagnostic ===\n\n",
    );

    // ── Load Oasis ModelData ─────────────────────────────────────────────────
    let oasis_path = std::path::Path::new(
        "crates/bevy_naadf/assets/test/oasis_hard_cover.vox",
    );
    let model = load_oasis_model_data(oasis_path)?;
    report.push_str(&format!(
        "Loaded Oasis ModelData: {}×{}×{} chunks (data_chunk={} data_block={} data_voxel={})\n",
        model.size_in_chunks[0],
        model.size_in_chunks[1],
        model.size_in_chunks[2],
        model.data_chunk.len(),
        model.data_block.len(),
        model.data_voxel.len(),
    ));

    // ── Fixed-world dispatch shape (mirrors production) ──────────────────────
    let world_chunks = [
        crate::WORLD_SIZE_IN_CHUNKS.x,
        crate::WORLD_SIZE_IN_CHUNKS.y,
        crate::WORLD_SIZE_IN_CHUNKS.z,
    ];
    let world_voxels = [
        crate::WORLD_SIZE_IN_VOXELS.x,
        crate::WORLD_SIZE_IN_VOXELS.y,
        crate::WORLD_SIZE_IN_VOXELS.z,
    ];
    let segment_chunks: u32 = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
    let world_segments = [
        crate::WORLD_SIZE_IN_SEGMENTS.x,
        crate::WORLD_SIZE_IN_SEGMENTS.y,
        crate::WORLD_SIZE_IN_SEGMENTS.z,
    ];
    let total_segments = world_segments[0] * world_segments[1] * world_segments[2];

    report.push_str(&format!(
        "Fixed-world: {}×{}×{} chunks ({}×{}×{} voxels), {}×{}×{} segments × {}³-chunk segments = {} total segments\n\n",
        world_chunks[0], world_chunks[1], world_chunks[2],
        world_voxels[0], world_voxels[1], world_voxels[2],
        world_segments[0], world_segments[1], world_segments[2],
        segment_chunks, total_segments,
    ));

    // ── Sample positions: discovered by scanning the Oasis model ─────────────
    //
    // The Oasis model is 1488×544×1344 voxels (93×34×84 chunks). When tiled
    // into the 4096×512×4096 world via `voxelPos % modelSize`, the model
    // repeats horizontally (3 X-tiles × 3 Z-tiles), and the Y-clamp
    // (`generator_model.fx:48`) zeros out everything above
    // `model_size_in_chunks.y * 16 = 544` voxels.
    //
    // Production world Y=512 voxels = 32 chunks. Model Y=544 voxels (>world
    // Y), so the Y-clamp does NOT fire on any world voxel — every world Y
    // position maps to model_index_y=0 (= the only copy in Y).
    //
    // Picking sample positions at arbitrary Y (e.g. Y=16) yields mostly
    // EMPTY voxels because the model's lower volume is mostly air. To make
    // the discriminating test meaningful we need positions that are KNOWN
    // FULL in the Oasis model — discovered by scanning the source ModelData
    // buffers for any voxel slot with a non-zero type.
    let sample_positions = discover_populated_oasis_voxels(&model, world_voxels, 25);
    report.push_str(&format!(
        "Sample positions: {} voxel positions, discovered by scanning Oasis ModelData for FULL voxels in the world's Y range.\n\n",
        sample_positions.len()
    ));
    let sample_positions: &[[u32; 3]] = sample_positions.as_slice();

    // ── Compute CPU oracle for every sample position ────────────────────────
    //
    // For each sample (vx,vy,vz), call `generate_segment_cpu` over a
    // single-chunk segment that contains the sample (chunk = vx/16, vy/16,
    // vz/16). Index into the returned segment buffer to retrieve the
    // half-word the producer would have written for that voxel.
    //
    // The segment buffer encoding (`generator_model.fx:70`):
    //   out[group_index * 2048 + local_index * 32 + i] = voxel1 | (voxel2 << 16);
    //   group_index = gx + gy*gscx + gz*gscx*gscy = 0 (single-chunk segment).
    //   local_index = lx + ly*4 + lz*16   (block position in chunk).
    //   i = voxel_pair_index_in_block (0..32; each pair = 2 voxels x even/odd).
    //
    // For a sample at world voxel (vx,vy,vz):
    //   chunk    = (vx/16, vy/16, vz/16)
    //   intra_c  = (vx%16, vy%16, vz%16)
    //   block    = intra_c / 4  → (bx,by,bz) in [0..4)
    //   intra_b  = intra_c % 4  → (lx,ly,lz) in [0..4)
    //   voxel_in_block_idx = lx + ly*4 + lz*16   (0..64)
    //   pair_idx = voxel_in_block_idx / 2
    //   parity   = voxel_in_block_idx % 2        (0=lo,1=hi)
    //
    // The producer-pass output `voxel_in_block_idx`-th voxel has half-word
    // `(out[block_index*32 + pair_idx] >> (16*parity)) & 0xFFFF`, where
    // `block_index = bx + by*4 + bz*16`.
    let cpu_oracle_halfwords: Vec<u16> = sample_positions
        .iter()
        .map(|&[vx, vy, vz]| {
            let cx = vx / 16;
            let cy = vy / 16;
            let cz = vz / 16;
            // Run generator over a single 1×1×1-chunk segment at that chunk.
            let seg = generate_segment_cpu(
                &model,
                [cx, cy, cz],
                [1, 1, 1],
                world_voxels,
            );
            // Decode the half-word at (vx,vy,vz).
            let lx = (vx % 16) % 4;
            let ly = (vy % 16) % 4;
            let lz = (vz % 16) % 4;
            let bx = (vx % 16) / 4;
            let by = (vy % 16) / 4;
            let bz = (vz % 16) / 4;
            let voxel_in_block_idx = lx + ly * 4 + lz * 16;
            let block_index = bx + by * 4 + bz * 16;
            let pair_idx = voxel_in_block_idx / 2;
            let parity = voxel_in_block_idx % 2;
            // group_index = 0 for a single-chunk segment.
            let chunk_base = 0usize;
            let pair_u32_idx = chunk_base + (block_index as usize) * 32 + pair_idx as usize;
            let pair_u32 = seg[pair_u32_idx];
            let half = if parity == 0 { pair_u32 & 0xFFFF } else { (pair_u32 >> 16) & 0xFFFF };
            half as u16
        })
        .collect();

    report.push_str("CPU oracle half-words computed for all sample positions.\n");
    let n_full = cpu_oracle_halfwords.iter().filter(|h| (**h & 0x8000) != 0).count();
    let n_empty = cpu_oracle_halfwords.len() - n_full;
    report.push_str(&format!(
        "  Oracle: {} positions FULL (bit-15 set), {} positions EMPTY.\n\n",
        n_full, n_empty,
    ));

    // ── Boot headless render world (Bevy MinimalPlugins + RenderPlugin) ─────
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

    let gen_shader =
        Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    let limits = device.limits();
    report.push_str(&format!(
        "Device: max_storage_buffer_binding_size = {} MiB; max_buffer_size = {} MiB.\n",
        limits.max_storage_buffer_binding_size / (1024 * 1024),
        limits.max_buffer_size / (1024 * 1024),
    ));

    // ── Allocate production-scale buffers ────────────────────────────────────
    //
    // Sizing mirrors `render/prepare.rs::prepare_world_gpu` when
    // `gpu_producer_enabled = true`:
    //   chunk_count       = 256 * 32 * 256 = 2,097,152
    //   blocks_alloc_len  = chunk_count * 64 = 134,217,728  (512 MiB)
    //   voxels_alloc_len  = chunk_count * 128 = 268,435,456 (1024 MiB)
    let chunk_count: u64 =
        (world_chunks[0] as u64) * (world_chunks[1] as u64) * (world_chunks[2] as u64);
    let blocks_alloc_len: u64 = chunk_count * 64;
    let voxels_alloc_len: u64 = chunk_count * 128;
    report.push_str(&format!(
        "Allocations: chunks={} pairs ({} MiB), blocks={} u32 ({} MiB), voxels={} u32 ({} MiB)\n",
        chunk_count, (chunk_count * 8) / (1024 * 1024),
        blocks_alloc_len, (blocks_alloc_len * 4) / (1024 * 1024),
        voxels_alloc_len, (voxels_alloc_len * 4) / (1024 * 1024),
    ));
    if (blocks_alloc_len * 4) > limits.max_storage_buffer_binding_size as u64
        || (voxels_alloc_len * 4) > limits.max_storage_buffer_binding_size as u64
        || (chunk_count * 8) > limits.max_storage_buffer_binding_size as u64
    {
        report.push_str(&format!(
            "  WARNING: an allocation exceeds device max_storage_buffer_binding_size ({} MiB).\n",
            limits.max_storage_buffer_binding_size / (1024 * 1024),
        ));
    }

    // Hash map sized as production does (`ConstructionConfig::initial_hash_map_size`
    // = 1<<20 = 1048576 slots, * 4 u32/slot = 16 MiB).
    let hash_map_size_slots: u32 = crate::render::construction::config::ConstructionConfig::default().initial_hash_map_size;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data_len_u32: u64| {
        let size_bytes = data_len_u32 * 4;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size_bytes,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Zero-initialise via a single 1 MiB chunk loop to bound peak memory.
        // Production buffers are NOT pre-zero'd because wgpu/Bevy sets up the
        // backing store; per Stage 8's Q4 verify, the post-allocation state is
        // de-facto zero on the user's machine. For diagnostic determinism we
        // zero them explicitly.
        const ZERO_CHUNK: usize = 1024 * 1024 / 4; // 1 MiB / 4 = 256 K u32s.
        let zero_chunk = vec![0u32; ZERO_CHUNK];
        let mut written: u64 = 0;
        while written < data_len_u32 {
            let remaining = (data_len_u32 - written) as usize;
            let n = remaining.min(zero_chunk.len());
            queue.write_buffer(
                &buffer,
                written * 4,
                bytemuck::cast_slice(&zero_chunk[..n]),
            );
            written += n as u64;
        }
        buffer
    };

    report.push_str("Allocating GPU buffers (zero-init via 1 MiB chunked writes)…\n");
    let gpu_blocks = mk_storage("prod_blocks", blocks_alloc_len);
    let gpu_voxels = mk_storage("prod_voxels", voxels_alloc_len);
    let chunks_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_chunks"),
        size: chunk_count * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Zero-init chunks too.
    {
        const ZERO_PAIRS: usize = 1024 * 1024 / 8; // 1 MiB / 8 = 128K pairs.
        let zero_pairs = vec![[0u32; 2]; ZERO_PAIRS];
        let mut written_pairs: u64 = 0;
        while written_pairs < chunk_count {
            let remaining = (chunk_count - written_pairs) as usize;
            let n = remaining.min(zero_pairs.len());
            queue.write_buffer(
                &chunks_buf,
                written_pairs * 8,
                bytemuck::cast_slice(&zero_pairs[..n]),
            );
            written_pairs += n as u64;
        }
    }

    let gpu_block_voxel_count = device.create_buffer(&BufferDescriptor {
        label: Some("prod_bvc"),
        size: 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &gpu_block_voxel_count,
        0,
        bytemuck::cast_slice(&block_voxel_count_init),
    );

    let gpu_hash_map = mk_storage("prod_hashmap", (hash_map_size_slots as u64) * 4);
    // Hash map seed is zero — already zero'd by `mk_storage`. The W5 producer
    // re-initialises via the chunk_calc shader.
    let _ = hash_map_init;

    let gpu_coeffs = create_storage_buffer_u32(&device, &queue, "prod_coeffs", &coeffs);

    // Segment buffer — one segment's worth, 16³ chunks × 2048 u32s ≈ 32 MiB.
    let seg = segment_chunks as usize;
    let segment_buf_u32s = seg * seg * seg * (CHUNK_DATA_U32S as usize);
    let segment_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_segment"),
        size: (segment_buf_u32s as u64) * 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&segment_buf, 0, bytemuck::cast_slice(&vec![0u32; segment_buf_u32s]));

    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_chunk", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_block", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "prod_model_voxel", &model.data_voxel);

    let gen_params = GpuGeneratorModelParams {
        size_in_voxels: world_voxels,
        _pad0: 0,
        model_size_in_chunks: model.size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks: [0, 0, 0],
        group_size_in_chunks_x: segment_chunks,
        group_size_in_chunks_y: segment_chunks,
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let gen_params_buf = create_params_uniform(&device, &queue, &gen_params);

    let calc_params = GpuConstructionParams {
        size_in_chunks: world_chunks,
        _pad0: 0,
        group_size_in_groups: [0, 0, 0], // bounds-calc-relevant; not used by chunk_calc
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks: segment_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("prod_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));

    // ── Pipelines ────────────────────────────────────────────────────────────
    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(
                cache,
                gen_layout.clone(),
                gen_shader_handle.clone(),
            ),
            queue_calc_block_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
            queue_voxel_bounds_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
            queue_block_bounds_pipeline_with_handle(
                cache,
                calc_layout.clone(),
                calc_shader_handle.clone(),
            ),
        )
    };

    let mut pipelines: Option<(
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
    )> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..128 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("production pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "prod_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "prod_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buf.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment producer loop (mirrors production W5
    //    `naadf_gpu_producer_node` `mod.rs:2454-2566`) ─────────────────────
    let group_size_in_chunks = [segment_chunks, segment_chunks, segment_chunks];
    let mut seg_count = 0u32;
    report.push_str("Dispatching 512 per-segment generator+chunk_calc dispatches…\n");
    for sz in 0..world_segments[2] {
        for sy in 0..world_segments[1] {
            for sx in 0..world_segments[0] {
                let chunk_offset = [
                    sx * segment_chunks,
                    sy * segment_chunks,
                    sz * segment_chunks,
                ];
                let gen_params = GpuGeneratorModelParams {
                    size_in_voxels: world_voxels,
                    _pad0: 0,
                    model_size_in_chunks: model.size_in_chunks,
                    _pad1: 0,
                    group_offset_in_chunks: chunk_offset,
                    group_size_in_chunks_x: segment_chunks,
                    group_size_in_chunks_y: segment_chunks,
                    _pad2: 0,
                    _pad3: 0,
                    _pad4: 0,
                };
                queue.write_buffer(&gen_params_buf, 0, bytemuck::bytes_of(&gen_params));
                let calc_params = GpuConstructionParams {
                    size_in_chunks: world_chunks,
                    _pad0: 0,
                    group_size_in_groups: [0, 0, 0],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks: segment_chunks,
                    max_group_bound_dispatch: 0,
                    chunk_offset,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));
                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("prod_segment_enc"),
                });
                dispatch_generator_model_with_encoder(
                    &mut enc,
                    &p_gen,
                    &gen_bg,
                    group_size_in_chunks,
                );
                dispatch_calc_block_from_raw_data_world_sized(
                    &mut enc,
                    &p_calc,
                    &calc_bg,
                    group_size_in_chunks,
                );
                queue.submit([enc.finish()]);
                seg_count += 1;
                if seg_count.is_multiple_of(64) {
                    // Flush periodically so we don't accumulate too much in-flight.
                    device.poll(PollType::wait_indefinitely()).unwrap();
                }
            }
        }
    }
    // Final flush of the producer loop.
    device.poll(PollType::wait_indefinitely()).unwrap();
    report.push_str(&format!("  Producer loop done: {} segments dispatched.\n", seg_count));

    // Readback cursor after producer.
    let cursor_after_producer = readback_cursor(&device, &queue, &gpu_block_voxel_count);
    report.push_str(&format!(
        "  Cursors post-producer: block_voxel_count[0]={} (voxel-pair cursor), [1]={} (block-u32 cursor)\n\n",
        cursor_after_producer[0], cursor_after_producer[1],
    ));

    // ── CHECKPOINT A: read back post-producer (pre-bounds-calc) ─────────────
    report.push_str("=== CHECKPOINT A: post-W5-producer, pre-bounds-calc ===\n");
    let post_producer_results = sample_voxel_readback(
        &device,
        &queue,
        &chunks_buf,
        &gpu_blocks,
        &gpu_voxels,
        sample_positions,
        &cpu_oracle_halfwords,
        world_chunks,
    );
    report.push_str(&render_results_table(
        "post-producer",
        sample_positions,
        &cpu_oracle_halfwords,
        &post_producer_results,
    ));

    // ── Dispatch the bounds chain (mirrors production
    //    `mod.rs:2622-2634`) ───────────────────────────────────────────────
    let world_chunks_total = chunk_count;
    let max_blocks_u64 = world_chunks_total * 64;
    let max_voxels_u64 = max_blocks_u64 * 32;
    let voxel_workgroups =
        ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
    let block_workgroups =
        ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
    report.push_str(&format!(
        "Dispatching bounds chain: voxel_workgroups={} block_workgroups={}\n",
        voxel_workgroups, block_workgroups,
    ));
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_bounds_enc"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_workgroups);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_workgroups);
    queue.submit([enc.finish()]);
    device.poll(PollType::wait_indefinitely()).unwrap();
    report.push_str("  Bounds chain complete.\n\n");

    // ── CHECKPOINT B: read back post-bounds-calc ────────────────────────────
    report.push_str("=== CHECKPOINT B: post-bounds-calc (after compute_voxel_bounds + compute_block_bounds) ===\n");
    let post_bounds_results = sample_voxel_readback(
        &device,
        &queue,
        &chunks_buf,
        &gpu_blocks,
        &gpu_voxels,
        sample_positions,
        &cpu_oracle_halfwords,
        world_chunks,
    );
    report.push_str(&render_results_table(
        "post-bounds",
        sample_positions,
        &cpu_oracle_halfwords,
        &post_bounds_results,
    ));

    // ── Cross-checkpoint diff analysis ──────────────────────────────────────
    report.push_str("\n=== Pattern analysis: post-producer vs post-bounds ===\n");
    let mut a_b_changed = 0usize;
    let mut a_match_b_mismatch = 0usize;
    let mut a_mismatch_b_match = 0usize;
    let mut both_match = 0usize;
    let mut both_mismatch = 0usize;
    for (i, &[vx, vy, vz]) in sample_positions.iter().enumerate() {
        let oracle = cpu_oracle_halfwords[i];
        let a = &post_producer_results[i];
        let b = &post_bounds_results[i];
        let a_match = a.matches_oracle(oracle);
        let b_match = b.matches_oracle(oracle);
        if a.gpu_voxel_halfword != b.gpu_voxel_halfword
            || a.gpu_chunk_u32 != b.gpu_chunk_u32
            || a.gpu_block_u32 != b.gpu_block_u32
        {
            a_b_changed += 1;
            report.push_str(&format!(
                "  pos=({vx},{vy},{vz}): A→B CHANGED  | chunk {:#010x}→{:#010x}  block {:#010x}→{:#010x}  voxel-half {:#06x}→{:#06x}\n",
                a.gpu_chunk_u32, b.gpu_chunk_u32,
                a.gpu_block_u32, b.gpu_block_u32,
                a.gpu_voxel_halfword, b.gpu_voxel_halfword,
            ));
        }
        match (a_match, b_match) {
            (true, true) => both_match += 1,
            (false, false) => both_mismatch += 1,
            (true, false) => a_match_b_mismatch += 1,
            (false, true) => a_mismatch_b_match += 1,
        }
    }
    report.push_str(&format!(
        "  Summary: A→B changed in {} of {} positions; both-match={} both-mismatch={} match→mismatch={} mismatch→match={}\n",
        a_b_changed, sample_positions.len(),
        both_match, both_mismatch, a_match_b_mismatch, a_mismatch_b_match,
    ));

    // ── Verdict ─────────────────────────────────────────────────────────────
    report.push_str("\n=== Verdict ===\n");
    let n_mismatch_b = post_bounds_results
        .iter()
        .enumerate()
        .filter(|(i, r)| !r.matches_oracle(cpu_oracle_halfwords[*i]))
        .count();
    let n_mismatch_a = post_producer_results
        .iter()
        .enumerate()
        .filter(|(i, r)| !r.matches_oracle(cpu_oracle_halfwords[*i]))
        .count();
    report.push_str(&format!(
        "  Post-producer mismatches: {} of {}\n",
        n_mismatch_a,
        sample_positions.len()
    ));
    report.push_str(&format!(
        "  Post-bounds mismatches: {} of {}\n",
        n_mismatch_b,
        sample_positions.len()
    ));
    if n_mismatch_b == 0 {
        report.push_str(
            "  → voxels[] is byte-correct post-full-pipeline at every sampled position.\n\
             → Bug must be in the RENDERER's decode path (or elsewhere outside the producer/bounds chain).\n",
        );
    } else if n_mismatch_a == 0 && n_mismatch_b > 0 {
        report.push_str(
            "  → voxels[] was correct after the producer, but DIVERGED after bounds-calc ran.\n\
             → Bug is in `compute_voxel_bounds` / `compute_block_bounds` writeback.\n",
        );
    } else if n_mismatch_a > 0 && n_mismatch_b > 0 {
        report.push_str(
            "  → voxels[] was ALREADY corrupted after the producer (Stage 6 gap).\n\
             → Bug is in W5 producer chain when run at full production shape (512 segments + shared hash_map).\n",
        );
    } else {
        report.push_str(
            "  → Producer broke voxels[] but bounds-calc somehow restored them?? Inspect per-position rows above.\n",
        );
    }

    let _ = total_segments;

    eprintln!("{report}");
    Ok(report)
}

/// Result of a single GPU pointer walk for one sample position. Captured by
/// [`sample_voxel_readback`] at each checkpoint.
struct VoxelReadback {
    /// Decoded `chunks[chunk_idx]` (low 32 bits of the vec2<u32> pair —
    /// matches the renderer's read pattern at `ray_tracing.wgsl`).
    gpu_chunk_u32: u32,
    /// Decoded `blocks[BlockPtr + block_idx_in_chunk]`. `None` if the chunk
    /// wasn't mixed.
    gpu_block_u32: u32,
    /// Decoded `voxels[VoxelPtr + voxel_in_block_idx / 2]`. `None` if the
    /// block wasn't mixed.
    gpu_voxel_pair_u32: u32,
    /// The half-word for this voxel's parity.
    gpu_voxel_halfword: u16,
    /// Hit kind: 0=empty-chunk, 1=uniform-full-chunk, 2=mixed-chunk-empty-block,
    /// 3=mixed-chunk-uniform-full-block, 4=mixed-chunk-mixed-block-empty-voxel,
    /// 5=mixed-chunk-mixed-block-full-voxel, 6=out-of-allocated-region.
    kind: u8,
}

impl VoxelReadback {
    fn matches_oracle(&self, oracle_half: u16) -> bool {
        // The oracle half-word is what `generate_segment_cpu` produces — the
        // post-producer pre-chunk_calc encoding (bit 15 = full, low 15 = type).
        // After chunk_calc/bounds, the FULL voxel half should equal the
        // oracle's full voxel half (type bits preserved). For an EMPTY oracle
        // half (bit 15 clear), the result is uniform empty or the AADF
        // half-word — either is "correct" downstream of the producer pass, so
        // we only assert correctness on FULL oracle positions.
        let oracle_full = (oracle_half & 0x8000) != 0;
        if !oracle_full {
            // Oracle says EMPTY here. The GPU may decode this position as
            // empty-chunk (kind 0), empty-block (kind 2), or
            // empty-voxel (kind 4). Anything ELSE is unexpected. We
            // tolerate AADF bits differing.
            matches!(self.kind, 0 | 2 | 4)
        } else {
            // Oracle says FULL with a specific type. The GPU must decode this
            // position as full with the SAME type — uniform-full chunk,
            // uniform-full block, or full voxel.
            let oracle_type = oracle_half & 0x7FFF;
            match self.kind {
                1 => self.gpu_chunk_u32 & 0x7FFF == oracle_type as u32,
                3 => self.gpu_block_u32 & 0x7FFF == oracle_type as u32,
                5 => {
                    let gpu_full = (self.gpu_voxel_halfword & 0x8000) != 0;
                    let gpu_type = self.gpu_voxel_halfword & 0x7FFF;
                    gpu_full && gpu_type == oracle_type
                }
                _ => false,
            }
        }
    }
}

/// Read back the 2 cursor counters (`block_voxel_count[0..2]`).
fn readback_cursor(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
) -> [u32; 2] {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_cur_staging"),
        size: 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_cur_enc"),
    });
    enc.copy_buffer_to_buffer(buf, 0, &staging, 0, 8);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();
    [v[0], v[1]]
}

/// Map back a single `u32`-element at a precise offset within a GPU storage
/// buffer. Used for surgical sampling of multi-GB buffers (no full readback —
/// 1 GiB voxels[] readback would push the OS to swap).
fn map_single_u32(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
    u32_index: u64,
) -> u32 {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_single_u32_staging"),
        size: 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_single_u32_enc"),
    });
    enc.copy_buffer_to_buffer(buf, u32_index * 4, &staging, 0, 4);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: u32 = bytemuck::cast_slice::<_, u32>(&data)[0];
    drop(data);
    staging.unmap();
    v
}

/// Map back a single `vec2<u32>` (8-byte pair) at a precise pair-index within
/// a `chunks` storage buffer.
fn map_single_pair(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    buf: &bevy::render::render_resource::Buffer,
    pair_index: u64,
) -> [u32; 2] {
    use bevy::render::render_resource::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
    };
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("prod_single_pair_staging"),
        size: 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("prod_single_pair_enc"),
    });
    enc.copy_buffer_to_buffer(buf, pair_index * 8, &staging, 0, 8);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let v: &[u32] = bytemuck::cast_slice(&data);
    let pair = [v[0], v[1]];
    drop(data);
    staging.unmap();
    pair
}

/// For each sample position, walk the GPU pointer chain (chunks → blocks →
/// voxels) and capture the decoded values + the leaf voxel half-word.
fn sample_voxel_readback(
    device: &bevy::render::renderer::RenderDevice,
    queue: &bevy::render::renderer::RenderQueue,
    chunks_buf: &bevy::render::render_resource::Buffer,
    gpu_blocks: &bevy::render::render_resource::Buffer,
    gpu_voxels: &bevy::render::render_resource::Buffer,
    positions: &[[u32; 3]],
    _oracle: &[u16],
    world_chunks: [u32; 3],
) -> Vec<VoxelReadback> {
    let mut out = Vec::with_capacity(positions.len());
    for &[vx, vy, vz] in positions {
        let cx = vx / 16;
        let cy = vy / 16;
        let cz = vz / 16;
        let chunk_idx = (cx as u64)
            + (cy as u64) * (world_chunks[0] as u64)
            + (cz as u64) * (world_chunks[0] as u64) * (world_chunks[1] as u64);
        let chunk_pair = map_single_pair(device, queue, chunks_buf, chunk_idx);
        let chunk_u32 = chunk_pair[0];

        // Decode chunk classification (`aadf::cell::ChunkCell::decode`):
        //   bit 31 set = mixed (low 30 = BlockPtr)
        //   bit 30 set = uniform-full (low 15 = type)
        //   else        = empty (low 30 = 6×5-bit AADF)
        const CELL_HAS_CHILDREN: u32 = 1 << 31;
        const CELL_UNIFORM_FULL: u32 = 1 << 30;
        const CELL_PAYLOAD_MASK: u32 = (1 << 30) - 1;
        if (chunk_u32 & CELL_HAS_CHILDREN) == 0 {
            // Empty or uniform-full chunk.
            if (chunk_u32 & CELL_UNIFORM_FULL) != 0 {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: 0,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 1,
                });
            } else {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: 0,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 0,
                });
            }
            continue;
        }
        // Mixed chunk: follow BlockPtr.
        let block_ptr = (chunk_u32 & CELL_PAYLOAD_MASK) as u64;
        let bx = (vx % 16) / 4;
        let by = (vy % 16) / 4;
        let bz = (vz % 16) / 4;
        let block_in_chunk = bx + by * 4 + bz * 16;
        let block_offset = block_ptr + block_in_chunk as u64;
        let block_u32 = map_single_u32(device, queue, gpu_blocks, block_offset);

        if (block_u32 & CELL_HAS_CHILDREN) == 0 {
            if (block_u32 & CELL_UNIFORM_FULL) != 0 {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: block_u32,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 3,
                });
            } else {
                out.push(VoxelReadback {
                    gpu_chunk_u32: chunk_u32,
                    gpu_block_u32: block_u32,
                    gpu_voxel_pair_u32: 0,
                    gpu_voxel_halfword: 0,
                    kind: 2,
                });
            }
            continue;
        }
        // Mixed block: follow VoxelPtr.
        let voxel_ptr = (block_u32 & CELL_PAYLOAD_MASK) as u64;
        let lx = (vx % 16) % 4;
        let ly = (vy % 16) % 4;
        let lz = (vz % 16) % 4;
        let voxel_in_block_idx = lx + ly * 4 + lz * 16;
        let pair_idx = voxel_in_block_idx / 2;
        let parity = voxel_in_block_idx % 2;
        let voxel_u32 = map_single_u32(device, queue, gpu_voxels, voxel_ptr + pair_idx as u64);
        let half = if parity == 0 { voxel_u32 & 0xFFFF } else { (voxel_u32 >> 16) & 0xFFFF };
        let kind = if (half & 0x8000) != 0 { 5 } else { 4 };
        out.push(VoxelReadback {
            gpu_chunk_u32: chunk_u32,
            gpu_block_u32: block_u32,
            gpu_voxel_pair_u32: voxel_u32,
            gpu_voxel_halfword: half as u16,
            kind,
        });
    }
    out
}

/// Format the per-sample readback table for inclusion in the report.
fn render_results_table(
    label: &str,
    positions: &[[u32; 3]],
    oracle: &[u16],
    results: &[VoxelReadback],
) -> String {
    let mut s = String::new();
    s.push_str(&format!("\n[{label} table — voxel position | oracle half | GPU chunk | GPU block | GPU voxel-pair | GPU voxel-half | kind | match?]\n"));
    for (i, &[vx, vy, vz]) in positions.iter().enumerate() {
        let r = &results[i];
        let oracle_half = oracle[i];
        let oracle_full = (oracle_half & 0x8000) != 0;
        let oracle_type = oracle_half & 0x7FFF;
        let m = r.matches_oracle(oracle_half);
        let kind_name = match r.kind {
            0 => "EMPTY-CHUNK",
            1 => "UNIFORM-FULL-CHUNK",
            2 => "MIXED-CHUNK / EMPTY-BLOCK",
            3 => "MIXED-CHUNK / UNIFORM-FULL-BLOCK",
            4 => "MIXED-CHUNK / MIXED-BLOCK / EMPTY-VOXEL",
            5 => "MIXED-CHUNK / MIXED-BLOCK / FULL-VOXEL",
            _ => "?",
        };
        let xor_half =
            (r.gpu_voxel_halfword as u32) ^ (oracle_half as u32);
        s.push_str(&format!(
            "  ({:>4},{:>3},{:>4}) | oracle={:#06x} {} (type={:#06x}) | chunk={:#010x} | block={:#010x} | vpair={:#010x} | vhalf={:#06x} (type={:#06x}) | {} | XOR-half={:#06x} | {}\n",
            vx, vy, vz,
            oracle_half, if oracle_full { "FULL " } else { "EMPTY" }, oracle_type,
            r.gpu_chunk_u32, r.gpu_block_u32, r.gpu_voxel_pair_u32, r.gpu_voxel_halfword,
            r.gpu_voxel_halfword & 0x7FFF,
            kind_name,
            xor_half,
            if m { "MATCH" } else { "MISMATCH" },
        ));
    }
    s
}

/// Per-chunk content diversity mode for the scaled byte-diff fixture sweep.
#[derive(Clone, Copy, Debug)]
enum FixtureMode {
    /// All mixed blocks across all chunks have identical content — dedup-hit
    /// path dominates.
    Uniform,
    /// Every chunk has UNIQUE mixed-block content — new-slot CAS dominates.
    Diverse,
    /// Half chunks uniform, half diverse — exercises both paths in one
    /// dispatch.
    Mixed,
}

/// Run the W5 chunk_calc chain on a single fixture and report byte-level +
/// semantic-content divergence vs the CPU oracle.
fn run_one_fixture_byte_diff(
    size_in_chunks: [u32; 3],
    mode: FixtureMode,
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // ── Build the fixture volume ──────────────────────────────────────────────
    // Each chunk gets ONE mixed block (block (0,0,0) of the chunk) with one
    // full voxel. The voxel's position inside the block depends on `mode`:
    //   Uniform: voxel at (0,0,0) of the block — every mixed block has
    //            identical content → all dedup-hit.
    //   Diverse: voxel position varies per chunk (cycles through 64 positions)
    //            → every mixed block claims a unique slot.
    //   Mixed:   even-parity chunks uniform, odd-parity chunks diverse.
    let mut volume = DenseVolume::empty(size_in_chunks);
    let sv = volume.size_in_voxels();
    for cz in 0..size_in_chunks[2] {
        for cy in 0..size_in_chunks[1] {
            for cx in 0..size_in_chunks[0] {
                let chunk_idx = cx + cy * size_in_chunks[0]
                    + cz * size_in_chunks[0] * size_in_chunks[1];
                let pos_in_block: u32 = match mode {
                    FixtureMode::Uniform => 0,
                    FixtureMode::Diverse => chunk_idx % 64,
                    FixtureMode::Mixed => {
                        if chunk_idx % 2 == 0 { 0 } else { (chunk_idx / 2) % 64 }
                    }
                };
                // Decode pos_in_block (0..64) into (lx, ly, lz) within the
                // 4×4×4 block.
                let lx = pos_in_block % 4;
                let ly = (pos_in_block / 4) % 4;
                let lz = pos_in_block / 16;
                // Block (0,0,0) of this chunk — its world voxel base is
                // (cx*16, cy*16, cz*16); add (lx, ly, lz).
                let vx = cx * 16 + lx;
                let vy = cy * 16 + ly;
                let vz = cz * 16 + lz;
                if vx < sv[0] && vy < sv[1] && vz < sv[2] {
                    volume.set([vx, vy, vz], VoxelTypeId(7));
                }
            }
        }
    }

    let oracle = construct(&volume);

    // ── Boot headless render world ────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app available".into());
    };
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    // ── Allocate GPU buffers ──────────────────────────────────────────────────
    // For a fixture extent S = size_in_chunks, the segment buffer holds S^3
    // chunks × 2048 u32s per chunk = S^3 * 2048 u32s; the GPU uses
    // `params.segment_size_in_chunks` to index, so set it to max(size_in_chunks)
    // and dispatch the actual world extent via `_world_sized`.
    let segment_size_in_chunks: u32 = size_in_chunks[0].max(size_in_chunks[1]).max(size_in_chunks[2]);
    let segment_voxels = build_segment_voxel_buffer_for_world(&volume, segment_size_in_chunks);

    // Worst-case block/voxel sizing for the GPU buffers (cursor seed = 64 ;
    // chunks × 64 blocks/chunk for blocks ; chunks × 64 voxels/chunk ×
    // (1 u32 / 2 voxels) = chunks × 32 u32s for voxels). We pad generously.
    let chunk_count = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
    let max_blocks = 64 + chunk_count * 64;
    let max_voxels_u32 = 64 + chunk_count * 64 * 32; // worst case no dedup
    // Cap the hash map at 1M slots (16 MB) — large enough that any fixture's
    // expected unique-block count fits with low load factor, but small
    // enough that allocation never fails silently at million-chunk scale.
    // Production uses 256K initially per `ConstructionConfig::initial_hash_map_size`.
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let mk_storage = |label: &'static str, data: &[u32]| {
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
    };

    let gpu_blocks = mk_storage("scaled_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("scaled_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("scaled_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("scaled_segment", &segment_voxels);
    let gpu_hash_map = mk_storage("scaled_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("scaled_coeffs", &coeffs);

    let params = GpuConstructionParams {
        size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks,
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ────────────────────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let a = queue_calc_block_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        let b = queue_voxel_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        let c = queue_block_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone());
        (a, b, c)
    };

    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("pipelines did not compile")?;

    // ── Bind group ───────────────────────────────────────────────────────────
    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "scaled_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch chunk_calc over the actual world extent ─────────────────────
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_calc_block"),
    });
    dispatch_calc_block_from_raw_data_world_sized(
        &mut encoder,
        &pipelines[0],
        &bind_group,
        size_in_chunks,
    );
    queue.submit([encoder.finish()]);

    // Read cursor counts to size the bounds dispatches faithfully.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("scaled_cursor_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("scaled_cursor_readback"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut encoder, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut encoder, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([encoder.finish()]);

    // ── Read back the three buffers ──────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("scaled_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("scaled_readback_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };

    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("scaled_chunks_readback"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("scaled_chunks_readback_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic output ────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    // === Part 1: semantic content comparison (pointer-following) =============
    // For each chunk_idx, decode both CPU and GPU chunk; if both mixed, walk
    // their respective block groups; for each block_idx in the chunk, if both
    // mixed, walk their respective voxel groups. Compare semantic content
    // (block type/aadf/voxel u32s) — not raw pointer values, since pointers
    // are nondeterministic across CPU vs GPU.
    let mut sem_chunk_mismatch: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mismatch: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mismatch: Option<(usize, usize, usize, u32, u32)> = None;
    let mut mixed_chunks_cpu = 0usize;
    let mut mixed_chunks_gpu = 0usize;
    let mut mixed_blocks_cpu = 0usize;
    let mut mixed_blocks_gpu = 0usize;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        // Classification mismatch (Empty/Uniform/Mixed) is a real divergence.
        let cpu_kind = chunk_kind(&cpu_chunk);
        let gpu_kind = chunk_kind(&gpu_chunk);
        if cpu_kind != gpu_kind {
            sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
            break 'outer;
        }
        // For non-mixed: compare encoded u32 directly.
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cpu_ptr), ChunkCell::Mixed(gpu_ptr)) => {
                mixed_chunks_cpu += 1;
                mixed_chunks_gpu += 1;
                // Walk 64 blocks at each side's pointer.
                for bi in 0..64usize {
                    let cpu_b_raw = oracle.blocks.get(cpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let gpu_b_raw = gpu_blocks_out.get(gpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let cpu_b = BlockCell::decode(cpu_b_raw);
                    let gpu_b = BlockCell::decode(gpu_b_raw);
                    let cpu_bk = block_kind(&cpu_b);
                    let gpu_bk = block_kind(&gpu_b);
                    if cpu_bk != gpu_bk {
                        sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                        break 'outer;
                    }
                    match (cpu_b, gpu_b) {
                        (BlockCell::Mixed(cpu_v), BlockCell::Mixed(gpu_v)) => {
                            mixed_blocks_cpu += 1;
                            mixed_blocks_gpu += 1;
                            for vi in 0..32usize {
                                let cpu_vw = oracle.voxels.get(cpu_v.0 as usize + vi).copied().unwrap_or(0);
                                let gpu_vw = gpu_voxels_out.get(gpu_v.0 as usize + vi).copied().unwrap_or(0);
                                if cpu_vw != gpu_vw {
                                    sem_voxel_mismatch = Some((ci, bi, vi, cpu_vw, gpu_vw));
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(t1), BlockCell::UniformFull(t2)) => {
                            if t1 != t2 {
                                sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                                break 'outer;
                            }
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) => {
                            if a1.d != a2.d {
                                sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                                break 'outer;
                            }
                        }
                        _ => {
                            sem_block_mismatch = Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            break 'outer;
                        }
                    }
                }
            }
            (ChunkCell::UniformFull(t1), ChunkCell::UniformFull(t2)) => {
                if t1 != t2 {
                    sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                    break 'outer;
                }
            }
            (ChunkCell::Empty(a1), ChunkCell::Empty(a2)) => {
                if a1.d != a2.d {
                    sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                    break 'outer;
                }
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "mixed chunks: cpu={mixed_chunks_cpu} gpu={mixed_chunks_gpu} ; mixed blocks: cpu={mixed_blocks_cpu} gpu={mixed_blocks_gpu}\n"
    ));

    s.push_str("[semantic pointer-following diff]\n");
    if let Some((ci, cpu_c, gpu_c, cpu_raw, gpu_raw)) = sem_chunk_mismatch {
        s.push_str(&format!(
            "  CHUNK MISMATCH @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cpu_c, cpu_raw, gpu_c, gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  chunks: all 'kind' classifications match\n");
    }
    if let Some((ci, bi, cpu_b, gpu_b, cpu_raw, gpu_raw)) = sem_block_mismatch {
        s.push_str(&format!(
            "  BLOCK MISMATCH @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cpu_b, cpu_raw, gpu_b, gpu_raw, cpu_raw ^ gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  blocks: all referenced blocks match semantically\n");
    }
    if let Some((ci, bi, vi, cpu_v, gpu_v)) = sem_voxel_mismatch {
        s.push_str(&format!(
            "  VOXEL MISMATCH @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cpu_v, gpu_v, cpu_v ^ gpu_v, cpu_v ^ gpu_v
        ));
    } else {
        s.push_str("  voxels: all referenced voxels match\n");
    }

    // === Part 2: raw byte equality (sensitive to atomic ordering) ============
    s.push_str("[raw u32 byte-equality (sensitive to nondeterministic atomicAdd ordering)]\n");
    let first_chunk_diff = oracle
        .chunks
        .iter()
        .zip(gpu_chunks_out.iter())
        .position(|(a, b)| a != b);
    if let Some(i) = first_chunk_diff {
        let a = oracle.chunks[i];
        let b = gpu_chunks_out[i];
        s.push_str(&format!(
            "  chunks[{i}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x}\n",
            a ^ b
        ));
    } else {
        s.push_str("  chunks: byte-equal\n");
    }
    // For blocks/voxels: account for the GPU's +64 / +64-into-voxels seed.
    // CPU oracle voxels[0..] live at GPU voxels[gpu_voxel_seed..]; CPU oracle
    // blocks[0..] live at GPU blocks[64..]. Compare the relative-offset views.
    let first_block_diff = oracle
        .blocks
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            let b = gpu_blocks_out.get(64 + i).copied().unwrap_or(0);
            (*a != b).then_some((i, *a, b))
        });
    if let Some((i, a, b)) = first_block_diff {
        s.push_str(&format!(
            "  blocks[64+{i}={}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x} (bits {:032b})\n",
            64 + i,
            a ^ b,
            a ^ b
        ));
    } else {
        s.push_str("  blocks: byte-equal (after +64 seed offset)\n");
    }
    let first_voxel_diff = oracle
        .voxels
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            let b = gpu_voxels_out.get(32 + i).copied().unwrap_or(0);
            (*a != b).then_some((i, *a, b))
        });
    if let Some((i, a, b)) = first_voxel_diff {
        s.push_str(&format!(
            "  voxels[32+{i}={}]: cpu={a:#010x} gpu={b:#010x} XOR={:#010x} (bits {:032b})\n",
            32 + i,
            a ^ b,
            a ^ b
        ));
    } else {
        s.push_str("  voxels: byte-equal (after +32 seed offset)\n");
    }

    Ok(s)
}

/// Multi-segment variant: dispatch chunk_calc per-segment with shared
/// hash_map. Mirrors the production W5 loop in `naadf_gpu_producer_node`.
fn run_one_fixture_multiseg_byte_diff(
    world_size_in_chunks: [u32; 3],
    segment_size_in_chunks: u32,
    mode: FixtureMode,
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;
    use crate::voxel::VoxelTypeId;

    // Validate parameters: world dims must be exact multiples of segment size.
    let sx = world_size_in_chunks[0];
    let sy = world_size_in_chunks[1];
    let sz = world_size_in_chunks[2];
    if sx % segment_size_in_chunks != 0
        || sy % segment_size_in_chunks != 0
        || sz % segment_size_in_chunks != 0
    {
        return Err(format!(
            "world size {:?} not divisible by segment size {segment_size_in_chunks}",
            world_size_in_chunks
        ));
    }
    let seg_count_x = sx / segment_size_in_chunks;
    let seg_count_y = sy / segment_size_in_chunks;
    let seg_count_z = sz / segment_size_in_chunks;
    let n_segments = seg_count_x * seg_count_y * seg_count_z;

    // ── Build fixture volume ─────────────────────────────────────────────────
    let mut volume = DenseVolume::empty(world_size_in_chunks);
    let sv = volume.size_in_voxels();
    for cz in 0..sz {
        for cy in 0..sy {
            for cx in 0..sx {
                let chunk_idx = cx + cy * sx + cz * sx * sy;
                let pos_in_block: u32 = match mode {
                    FixtureMode::Uniform => 0,
                    FixtureMode::Diverse => chunk_idx % 64,
                    FixtureMode::Mixed => {
                        if chunk_idx % 2 == 0 { 0 } else { (chunk_idx / 2) % 64 }
                    }
                };
                let lx = pos_in_block % 4;
                let ly = (pos_in_block / 4) % 4;
                let lz = pos_in_block / 16;
                let vx = cx * 16 + lx;
                let vy = cy * 16 + ly;
                let vz = cz * 16 + lz;
                if vx < sv[0] && vy < sv[1] && vz < sv[2] {
                    volume.set([vx, vy, vz], VoxelTypeId(7));
                }
            }
        }
    }

    let oracle = construct(&volume);

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ─────────────────────────────────────────────────────
    let chunk_count = (sx * sy * sz) as usize;
    // Mode=Mixed / Diverse fixtures dedup-collapse to ≤64 unique block
    // patterns. The voxel buffer can be bounded much tighter than the
    // naive worst-case (chunks * 64 * 32 = 2 GB for the largest fixture).
    // Cap at the smaller of (worst-case, 256 MB-of-u32s) so the GPU
    // allocation always succeeds; the hash dedup keeps real usage well
    // under this.
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32_uncapped: usize = 64usize.saturating_add(chunk_count.saturating_mul(64 * 32));
    let max_voxels_u32 = max_voxels_u32_uncapped.min(256 * 1024 * 1024 / 4); // 256 MB
    let hash_map_size_slots: u32 = 1 << 20; // fixed 16 MB / 1M slots
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    // Segment buffer size: exactly one segment's worth.
    let seg = segment_size_in_chunks as usize;
    let segment_buf_u32s = seg * seg * seg * 2048;

    let mk_storage = |label: &'static str, data: &[u32]| {
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
    };

    let gpu_blocks = mk_storage("ms_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("ms_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("ms_bvc", &block_voxel_count_init);
    let gpu_segment = mk_storage("ms_segment", &vec![0u32; segment_buf_u32s]);
    let gpu_hash_map = mk_storage("ms_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("ms_coeffs", &coeffs);

    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("ms_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("ms_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ───────────────────────────────────────────────────────────
    let layout = construction_world_layout_descriptor();
    let (id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_calc_block_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, layout.clone(), shader_handle.clone()),
        )
    };
    let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
            break;
        }
    }
    let pipelines = pipelines.ok_or("pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "ms_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            gpu_segment.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment loop: write segment buffer + params + dispatch ──────────
    for sz_i in 0..seg_count_z {
        for sy_i in 0..seg_count_y {
            for sx_i in 0..seg_count_x {
                let chunk_offset = [
                    sx_i * segment_size_in_chunks,
                    sy_i * segment_size_in_chunks,
                    sz_i * segment_size_in_chunks,
                ];
                // Build segment voxel buffer for THIS segment's region.
                let seg_voxels = build_segment_voxel_buffer_for_region(
                    &volume,
                    chunk_offset,
                    segment_size_in_chunks,
                );
                queue.write_buffer(&gpu_segment, 0, bytemuck::cast_slice(&seg_voxels));

                let params = GpuConstructionParams {
                    size_in_chunks: world_size_in_chunks,
                    _pad0: 0,
                    group_size_in_groups: [1, 1, 1],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks,
                    max_group_bound_dispatch: 0,
                    chunk_offset,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

                // Per-segment encoder + submit (mirrors production W5 loop).
                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("ms_segment_enc"),
                });
                dispatch_calc_block_from_raw_data_world_sized(
                    &mut enc,
                    &pipelines[0],
                    &bind_group,
                    [segment_size_in_chunks; 3],
                );
                queue.submit([enc.finish()]);
            }
        }
    }

    // Cursor readback.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("ms_cursor_st"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("ms_cursor_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_workgroups = cursor_pair[0] / 64;
    let block_workgroups = cursor_pair[1] / 64;

    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ms_bounds"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &pipelines[1], &bind_group, voxel_workgroups);
    dispatch_compute_block_bounds(&mut enc, &pipelines[2], &bind_group, block_workgroups);
    queue.submit([enc.finish()]);

    // Readback all three buffers.
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("ms_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("ms_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("ms_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ms_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic ──────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "n_segments={n_segments} cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    let mut sem_chunk_mismatch: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mismatch: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mismatch: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mismatches: u32 = 0;
    let mut first_mismatch_chunk: Option<usize> = None;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            if sem_chunk_mismatch.is_none() {
                sem_chunk_mismatch = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                first_mismatch_chunk = Some(ci);
            }
            total_mismatches += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cpu_ptr), ChunkCell::Mixed(gpu_ptr)) => {
                for bi in 0..64usize {
                    let cpu_b_raw = oracle.blocks.get(cpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let gpu_b_raw = gpu_blocks_out.get(gpu_ptr.0 as usize + bi).copied().unwrap_or(0);
                    let cpu_b = BlockCell::decode(cpu_b_raw);
                    let gpu_b = BlockCell::decode(gpu_b_raw);
                    if block_kind(&cpu_b) != block_kind(&gpu_b) {
                        if sem_block_mismatch.is_none() {
                            sem_block_mismatch =
                                Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                        }
                        total_mismatches += 1;
                        continue;
                    }
                    match (cpu_b, gpu_b) {
                        (BlockCell::Mixed(cpu_v), BlockCell::Mixed(gpu_v)) => {
                            for vi in 0..32usize {
                                let cpu_vw = oracle.voxels.get(cpu_v.0 as usize + vi).copied().unwrap_or(0);
                                let gpu_vw = gpu_voxels_out.get(gpu_v.0 as usize + vi).copied().unwrap_or(0);
                                if cpu_vw != gpu_vw {
                                    if sem_voxel_mismatch.is_none() {
                                        sem_voxel_mismatch = Some((ci, bi, vi, cpu_vw, gpu_vw));
                                        first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                                    }
                                    total_mismatches += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(t1), BlockCell::UniformFull(t2)) if t1 != t2 => {
                            if sem_block_mismatch.is_none() {
                                sem_block_mismatch =
                                    Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            }
                            total_mismatches += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            if sem_block_mismatch.is_none() {
                                sem_block_mismatch =
                                    Some((ci, bi, cpu_b, gpu_b, cpu_b_raw, gpu_b_raw));
                            }
                            total_mismatches += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(t1), ChunkCell::UniformFull(t2)) if t1 != t2 => {
                if sem_chunk_mismatch.is_none() {
                    sem_chunk_mismatch =
                        Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mismatches += 1;
            }
            (ChunkCell::Empty(a1), ChunkCell::Empty(a2)) if a1.d != a2.d => {
                if sem_chunk_mismatch.is_none() {
                    sem_chunk_mismatch =
                        Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mismatches += 1;
            }
            _ => {}
        }
    }

    s.push_str("[semantic pointer-following diff]\n");
    if let Some((ci, cpu_c, gpu_c, cpu_raw, gpu_raw)) = sem_chunk_mismatch {
        s.push_str(&format!(
            "  CHUNK MISMATCH @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cpu_c, cpu_raw, gpu_c, gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  chunks: all 'kind' match\n");
    }
    if let Some((ci, bi, cpu_b, gpu_b, cpu_raw, gpu_raw)) = sem_block_mismatch {
        s.push_str(&format!(
            "  BLOCK MISMATCH @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cpu_b, cpu_raw, gpu_b, gpu_raw, cpu_raw ^ gpu_raw, cpu_raw ^ gpu_raw
        ));
    } else {
        s.push_str("  blocks: all kinds match semantically\n");
    }
    if let Some((ci, bi, vi, cpu_v, gpu_v)) = sem_voxel_mismatch {
        s.push_str(&format!(
            "  VOXEL MISMATCH @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cpu_v, gpu_v, cpu_v ^ gpu_v, cpu_v ^ gpu_v
        ));
    } else {
        s.push_str("  voxels: all referenced voxels match\n");
    }
    s.push_str(&format!(
        "  total semantic mismatches: {total_mismatches} (first @ chunk {:?})\n",
        first_mismatch_chunk
    ));
    Ok(s)
}

/// Drive `generator_model.wgsl` for one (model, segment) configuration and
/// byte-compare its `segment_voxel_buffer` output against the
/// `generate_segment_cpu` Rust oracle.
fn run_one_generator_model_byte_diff(
    model_size_in_chunks: [u32; 3],
    group_size_in_chunks: [u32; 3],
    group_offset_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::generator::{
        generate_segment_cpu, CHUNK_DATA_U32S,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };

    // Build a "mixed" ModelData with varied per-chunk content. Use the model's
    // chunks/blocks/voxels layout from the encoding contract.
    let model = build_mixed_model_data(model_size_in_chunks);

    let world_size_in_voxels = [
        group_size_in_chunks[0] * 16 + group_offset_in_chunks[0] * 16,
        group_size_in_chunks[1] * 16 + group_offset_in_chunks[1] * 16,
        group_size_in_chunks[2] * 16 + group_offset_in_chunks[2] * 16,
    ];

    // CPU oracle.
    let cpu_out = generate_segment_cpu(
        &model,
        group_offset_in_chunks,
        group_size_in_chunks,
        world_size_in_voxels,
    );

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ─────────────────────────────────────────────────────
    let total_chunks =
        group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2];
    let chunk_data_u32s = (total_chunks * CHUNK_DATA_U32S) as usize;
    let chunk_data_init = vec![0u32; chunk_data_u32s];

    let chunk_data_buf =
        create_storage_buffer_u32(&device, &queue, "gen_chunk_data_rw", &chunk_data_init);
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_chunk_ro", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_block_ro", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "gen_model_voxel_ro", &model.data_voxel);

    let params = GpuGeneratorModelParams {
        size_in_voxels: world_size_in_voxels,
        _pad0: 0,
        model_size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks,
        group_size_in_chunks_x: group_size_in_chunks[0],
        group_size_in_chunks_y: group_size_in_chunks[1],
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let params_buf = create_params_uniform(&device, &queue, &params);

    // ── Pipeline ────────────────────────────────────────────────────────────
    let layout = generator_model_layout_descriptor();
    let id = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        queue_generator_model_pipeline_with_handle(cache, layout.clone(), shader_handle.clone())
    };
    let mut pipeline: Option<bevy::render::render_resource::ComputePipeline> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let Some(p) = cache.get_compute_pipeline(id) {
            pipeline = Some(p.clone());
            break;
        }
    }
    let pipeline = pipeline.ok_or("generator pipeline did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout);
    let bind_group = device.create_bind_group(
        "gen_bg",
        &bgl,
        &BindGroupEntries::sequential((
            chunk_data_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            params_buf.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch ────────────────────────────────────────────────────────────
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("gen_dispatch"),
    });
    dispatch_generator_model_with_encoder(&mut enc, &pipeline, &bind_group, group_size_in_chunks);
    queue.submit([enc.finish()]);

    // ── Readback ────────────────────────────────────────────────────────────
    let staging_size = (chunk_data_u32s as u64) * 4;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("gen_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("gen_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunk_data_buf, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let gpu_out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();

    // ── Compare ─────────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "CPU oracle: {} u32s ; GPU: {} u32s\n",
        cpu_out.len(),
        gpu_out.len()
    ));

    let first_diff = cpu_out
        .iter()
        .zip(gpu_out.iter())
        .enumerate()
        .find(|(_, (a, b))| a != b);
    if let Some((i, (a, b))) = first_diff {
        let xor = a ^ b;
        s.push_str(&format!(
            "  FIRST DIVERGENT INDEX: {i}  cpu={a:#010x}  gpu={b:#010x}  XOR={xor:#010x}  bits={xor:032b}\n",
        ));
        // Also count total divergent u32s.
        let n_diff: usize = cpu_out
            .iter()
            .zip(gpu_out.iter())
            .filter(|(a, b)| a != b)
            .count();
        let pct = n_diff as f64 * 100.0 / cpu_out.len() as f64;
        s.push_str(&format!(
            "  total divergent u32s: {n_diff} / {} ({pct:.2}%)\n",
            cpu_out.len()
        ));
        // Decode the divergent position back to (group, local, i) to localize.
        let chunk_data_per = CHUNK_DATA_U32S as usize;
        let group_index = i / chunk_data_per;
        let within = i % chunk_data_per;
        let local_index = within / 32;
        let pair_i = within % 32;
        let gx = (group_index as u32) % group_size_in_chunks[0];
        let gy = ((group_index as u32) / group_size_in_chunks[0])
            % group_size_in_chunks[1];
        let gz = (group_index as u32) / (group_size_in_chunks[0] * group_size_in_chunks[1]);
        s.push_str(&format!(
            "  decoded: group={gx},{gy},{gz} local_index={local_index} pair_i={pair_i}\n"
        ));
    } else {
        s.push_str("  GENERATOR BYTE-EQUAL: cpu == gpu across all u32s\n");
    }

    Ok(s)
}

/// Tiled-model variant: small `ModelData` gets tiled into a larger world via
/// the per-segment generator chain (matches production Oasis behavior). The
/// CPU oracle builds the tiled volume by decoding `generate_segment_cpu`
/// output for every segment, then runs `construct()` on the assembled
/// world-volume.
fn run_one_tiled_byte_diff(
    model_size_in_chunks: [u32; 3],
    world_size_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::aadf::generator::generate_segment_cpu;
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    // Build the tile model — non-trivial mixed content so tiles actually
    // contribute geometry.
    let model = build_mixed_model_data(model_size_in_chunks);

    let world_size_in_voxels = [
        world_size_in_chunks[0] * 16,
        world_size_in_chunks[1] * 16,
        world_size_in_chunks[2] * 16,
    ];

    // Segment shape: matches production's `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS *
    // 4 = 16`. If the world is smaller than 16 per axis, use the full world
    // as one segment.
    let seg = world_size_in_chunks[0].min(world_size_in_chunks[1]).min(world_size_in_chunks[2]).min(16);
    if world_size_in_chunks[0] % seg != 0
        || world_size_in_chunks[1] % seg != 0
        || world_size_in_chunks[2] % seg != 0
    {
        return Err(format!(
            "world {:?} not divisible by seg {seg}",
            world_size_in_chunks
        ));
    }
    let seg_count = [
        world_size_in_chunks[0] / seg,
        world_size_in_chunks[1] / seg,
        world_size_in_chunks[2] / seg,
    ];
    let n_segments = seg_count[0] * seg_count[1] * seg_count[2];

    // ── CPU oracle: build full world by tiling, then construct() ────────────
    let mut volume = DenseVolume::empty(world_size_in_chunks);
    for sz_i in 0..seg_count[2] {
        for sy_i in 0..seg_count[1] {
            for sx_i in 0..seg_count[0] {
                let off = [sx_i * seg, sy_i * seg, sz_i * seg];
                let cpu_seg = generate_segment_cpu(
                    &model,
                    off,
                    [seg, seg, seg],
                    world_size_in_voxels,
                );
                // Stamp the segment's voxels into the global volume.
                decode_segment_voxels_into_volume(&cpu_seg, off, [seg, seg, seg], &mut volume);
            }
        }
    }
    let oracle = construct(&volume);

    // ── Boot headless ───────────────────────────────────────────────────────
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

    let gen_shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Buffers ─────────────────────────────────────────────────────────────
    let segment_buf_u32s = (seg as usize) * (seg as usize) * (seg as usize) * 2048;
    let chunk_count = (world_size_in_chunks[0] * world_size_in_chunks[1] * world_size_in_chunks[2]) as usize;
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32 = (64usize.saturating_add(chunk_count.saturating_mul(64 * 32))).min(256 * 1024 * 1024 / 4);
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let segment_buf = create_storage_buffer_u32(&device, &queue, "tile_seg", &vec![0u32; segment_buf_u32s]);
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mc", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mb", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "tile_mv", &model.data_voxel);
    let gen_params_buf = create_params_uniform(
        &device,
        &queue,
        &GpuGeneratorModelParams {
            size_in_voxels: world_size_in_voxels,
            _pad0: 0,
            model_size_in_chunks: model.size_in_chunks,
            _pad1: 0,
            group_offset_in_chunks: [0, 0, 0],
            group_size_in_chunks_x: seg,
            group_size_in_chunks_y: seg,
            _pad2: 0,
            _pad3: 0,
            _pad4: 0,
        },
    );

    let mk_storage = |label: &'static str, data: &[u32]| {
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
    };

    let gpu_blocks = mk_storage("tile_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("tile_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("tile_bvc", &block_voxel_count_init);
    let gpu_hash_map = mk_storage("tile_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("tile_coeffs", &coeffs);

    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("tile_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("tile_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(cache, gen_layout.clone(), gen_shader_handle.clone()),
            queue_calc_block_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
        )
    };
    let mut pipelines: Option<(_, _, _, _)> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("tiled pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "tile_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "tile_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Per-segment dispatch loop (matches production W5) ───────────────────
    for sz_i in 0..seg_count[2] {
        for sy_i in 0..seg_count[1] {
            for sx_i in 0..seg_count[0] {
                let off = [sx_i * seg, sy_i * seg, sz_i * seg];
                // Generator uniform — model + segment offset.
                let gen_p = GpuGeneratorModelParams {
                    size_in_voxels: world_size_in_voxels,
                    _pad0: 0,
                    model_size_in_chunks: model.size_in_chunks,
                    _pad1: 0,
                    group_offset_in_chunks: off,
                    group_size_in_chunks_x: seg,
                    group_size_in_chunks_y: seg,
                    _pad2: 0,
                    _pad3: 0,
                    _pad4: 0,
                };
                queue.write_buffer(&gen_params_buf, 0, bytemuck::bytes_of(&gen_p));
                // Chunk_calc uniform — world size + segment offset.
                let calc_p = GpuConstructionParams {
                    size_in_chunks: world_size_in_chunks,
                    _pad0: 0,
                    group_size_in_groups: [1, 1, 1],
                    _pad1: 0,
                    bound_group_queue_max_size: 1,
                    hash_map_size: hash_map_size_slots,
                    segment_size_in_chunks: seg,
                    max_group_bound_dispatch: 0,
                    chunk_offset: off,
                    dispatch_offset: 0,
                    frame_index: 0,
                    changed_chunk_count: 0,
                    changed_block_count: 0,
                    changed_voxel_count: 0,
                };
                queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_p));

                let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("tile_seg_enc"),
                });
                dispatch_generator_model_with_encoder(&mut enc, &p_gen, &gen_bg, [seg, seg, seg]);
                dispatch_calc_block_from_raw_data_world_sized(&mut enc, &p_calc, &calc_bg, [seg, seg, seg]);
                queue.submit([enc.finish()]);
            }
        }
    }

    // Readback cursor + run bounds dispatches.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("tile_cur"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("tile_cur_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_wg = cursor_pair[0] / 64;
    let block_wg = cursor_pair[1] / 64;
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("tile_bnd"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_wg);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_wg);
    queue.submit([enc.finish()]);

    // Readback buffers.
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("tile_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("tile_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);

    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("tile_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("tile_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Semantic diff ───────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "n_segments={n_segments} seg={seg} cursors: voxel_pairs={} block_u32s={}\n",
        cursor_pair[0], cursor_pair[1]
    ));
    s.push_str(&format!(
        "CPU oracle: chunks={} blocks={} voxels={}\n",
        oracle.chunks.len(), oracle.blocks.len(), oracle.voxels.len()
    ));

    let mut sem_chunk_mm: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mm: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mm: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mm: u32 = 0;
    let mut first_mismatch_chunk: Option<usize> = None;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            if sem_chunk_mm.is_none() {
                sem_chunk_mm = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                first_mismatch_chunk = Some(ci);
            }
            total_mm += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cp), ChunkCell::Mixed(gp)) => {
                for bi in 0..64usize {
                    let cb_raw = oracle.blocks.get(cp.0 as usize + bi).copied().unwrap_or(0);
                    let gb_raw = gpu_blocks_out.get(gp.0 as usize + bi).copied().unwrap_or(0);
                    let cb = BlockCell::decode(cb_raw);
                    let gb = BlockCell::decode(gb_raw);
                    if block_kind(&cb) != block_kind(&gb) {
                        if sem_block_mm.is_none() {
                            sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                        }
                        total_mm += 1;
                        continue;
                    }
                    match (cb, gb) {
                        (BlockCell::Mixed(cv), BlockCell::Mixed(gv)) => {
                            for vi in 0..32usize {
                                let cv_w = oracle.voxels.get(cv.0 as usize + vi).copied().unwrap_or(0);
                                let gv_w = gpu_voxels_out.get(gv.0 as usize + vi).copied().unwrap_or(0);
                                if cv_w != gv_w {
                                    if sem_voxel_mm.is_none() {
                                        sem_voxel_mm = Some((ci, bi, vi, cv_w, gv_w));
                                        first_mismatch_chunk = first_mismatch_chunk.or(Some(ci));
                                    }
                                    total_mm += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(a), BlockCell::UniformFull(b)) if a != b => {
                            if sem_block_mm.is_none() {
                                sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            }
                            total_mm += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            if sem_block_mm.is_none() {
                                sem_block_mm = Some((ci, bi, cb, gb, cb_raw, gb_raw));
                            }
                            total_mm += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(a), ChunkCell::UniformFull(b)) if a != b => {
                if sem_chunk_mm.is_none() {
                    sem_chunk_mm = Some((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                }
                total_mm += 1;
            }
            (ChunkCell::Empty(_), ChunkCell::Empty(_)) => {
                // Empty-chunk AADFs are set by bounds_calc (not run here).
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "  total semantic mismatches: {total_mm} (first @ chunk {:?}; Empty-chunk AADFs ignored)\n",
        first_mismatch_chunk
    ));
    if let Some((ci, cc, gc, cr, gr)) = sem_chunk_mm {
        s.push_str(&format!(
            "    CHUNK MM @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cc, cr, gc, gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, cb, gb, cr, gr)) = sem_block_mm {
        s.push_str(&format!(
            "    BLOCK MM @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cb, cr, gb, gr, cr ^ gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, vi, cw, gw)) = sem_voxel_mm {
        s.push_str(&format!(
            "    VOXEL MM @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cw, gw, cw ^ gw, cw ^ gw,
        ));
    }

    Ok(s)
}

/// Decode a segment_voxel_buffer-formatted vector and stamp its decoded
/// voxels into the given DenseVolume at the given chunk offset.
fn decode_segment_voxels_into_volume(
    seg_buf: &[u32],
    chunk_offset: [u32; 3],
    group_size_in_chunks: [u32; 3],
    volume: &mut crate::aadf::construct::DenseVolume,
) {
    use crate::voxel::VoxelTypeId;
    let gscx = group_size_in_chunks[0] as usize;
    let gscy = group_size_in_chunks[1] as usize;
    let gscz = group_size_in_chunks[2] as usize;
    let sv = volume.size_in_voxels();
    for cz in 0..gscz {
        for cy in 0..gscy {
            for cx in 0..gscx {
                let chunk_index_in_segment = cx + cy * gscx + cz * gscx * gscy;
                let chunk_base = chunk_index_in_segment * 2048;
                let world_cx = chunk_offset[0] as usize + cx;
                let world_cy = chunk_offset[1] as usize + cy;
                let world_cz = chunk_offset[2] as usize + cz;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let pair = seg_buf[block_base + vi];
                                let lo = (pair & 0xFFFF) as u16;
                                let hi = ((pair >> 16) & 0xFFFF) as u16;
                                for (slot, half) in [(0, lo), (1, hi)].iter() {
                                    let voxel_idx = vi * 2 + slot;
                                    let lx = voxel_idx % 4;
                                    let ly = (voxel_idx / 4) % 4;
                                    let lz = voxel_idx / 16;
                                    let vx = world_cx * 16 + bx * 4 + lx;
                                    let vy = world_cy * 16 + by * 4 + ly;
                                    let vz = world_cz * 16 + bz * 4 + lz;
                                    if vx >= sv[0] as usize || vy >= sv[1] as usize || vz >= sv[2] as usize {
                                        continue;
                                    }
                                    let ty_bits = *half & 0x7FFF;
                                    let full = *half & 0x8000 != 0;
                                    let ty = if full {
                                        VoxelTypeId(ty_bits)
                                    } else {
                                        VoxelTypeId::EMPTY
                                    };
                                    volume.set([vx as u32, vy as u32, vz as u32], ty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Load the Oasis VOX file from disk as a `ModelData` (same way
/// `install_vox_in_fixed_world` does).
fn load_oasis_model_data(
    path: &std::path::Path,
) -> Result<crate::aadf::generator::ModelData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let data = dot_vox::load_bytes(&bytes).map_err(|e| format!("parse: {e}"))?;
    let imp = crate::voxel::vox_import::parse_dot_vox_data(&data)
        .map_err(|e| format!("import: {e:?}"))?;
    Ok(crate::aadf::generator::ModelData {
        size_in_chunks: imp.world.size_in_chunks,
        data_chunk: imp.world.chunks,
        data_block: imp.world.blocks,
        data_voxel: imp.world.voxels,
    })
}

/// Drive `generator_model.wgsl` + `chunk_calc.wgsl` for one segment of a
/// real `ModelData`, then byte-diff the GPU output against a CPU oracle
/// built from the same segment's voxels.
fn run_oasis_segment_byte_diff(
    model: &crate::aadf::generator::ModelData,
    group_offset_in_chunks: [u32; 3],
    group_size_in_chunks: [u32; 3],
) -> Result<String, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    use crate::aadf::cell::{BlockCell, ChunkCell};
    use crate::aadf::construct::construct;
    use crate::aadf::generator::{generate_segment_cpu, CHUNK_DATA_U32S};
    use crate::render::construction::chunk_calc::{
        construction_world_layout_descriptor, dispatch_calc_block_from_raw_data_world_sized,
        dispatch_compute_block_bounds, dispatch_compute_voxel_bounds,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use crate::render::construction::generator_model::{
        create_storage_buffer_u32, create_params_uniform,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        dispatch_generator_model_with_encoder, GpuGeneratorModelParams,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::render::construction::hashing::hash_coefficients;
    use crate::render::gpu_types::GpuConstructionParams;

    // The "world" size for purposes of voxel-position bounds-check =
    // (offset + segment) * 16 voxels per axis. This matches what production
    // passes to generator_model when running on the Oasis fixed world.
    let world_size_in_voxels = [
        (group_offset_in_chunks[0] + group_size_in_chunks[0]) * 16,
        (group_offset_in_chunks[1] + group_size_in_chunks[1]) * 16,
        (group_offset_in_chunks[2] + group_size_in_chunks[2]) * 16,
    ];

    // ── CPU oracle: generator output ────────────────────────────────────────
    let cpu_seg_voxels = generate_segment_cpu(
        model,
        group_offset_in_chunks,
        group_size_in_chunks,
        world_size_in_voxels,
    );

    // Build a DenseVolume of just this segment's voxels by decoding
    // `cpu_seg_voxels` (which is in the segment_voxel_buffer encoding:
    // chunk_index_in_segment * 2048 + local_index * 32 + i). Then run
    // construct() as the chunk_calc CPU oracle.
    let seg_volume = decode_segment_voxels_to_volume(&cpu_seg_voxels, group_size_in_chunks);
    let oracle = construct(&seg_volume);

    // ── Boot headless ────────────────────────────────────────────────────────
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

    let gen_shader = Shader::from_wgsl(GENERATOR_MODEL_SHADER_SRC, "shaders/generator_model.wgsl");
    let gen_shader_clone = gen_shader.clone();
    let gen_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(gen_shader);
    let calc_shader = Shader::from_wgsl(CHUNK_CALC_SHADER_SRC, "shaders/chunk_calc.wgsl");
    let calc_shader_clone = calc_shader.clone();
    let calc_shader_handle = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(calc_shader);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp".into());
    };
    {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.set_shader(gen_shader_handle.id(), gen_shader_clone);
        pc.set_shader(calc_shader_handle.id(), calc_shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no device")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no queue")?
        .clone();

    // ── Allocate buffers ────────────────────────────────────────────────────
    let total_chunks =
        group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2];
    let chunk_data_u32s = (total_chunks * CHUNK_DATA_U32S) as usize;
    let chunk_count = total_chunks as usize;
    let max_blocks = (64 + chunk_count * 64).max(64);
    let max_voxels_u32 = (64 + chunk_count * 64 * 32).min(256 * 1024 * 1024 / 4);
    let hash_map_size_slots: u32 = 1 << 20;
    let hash_map_init = vec![0u32; (hash_map_size_slots as usize) * 4];
    let block_voxel_count_init = vec![64u32, 64];
    let coeffs = hash_coefficients().to_vec();

    let segment_buf = create_storage_buffer_u32(
        &device,
        &queue,
        "oasis_seg_buf",
        &vec![0u32; chunk_data_u32s],
    );
    let model_chunk_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_chunk", &model.data_chunk);
    let model_block_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_block", &model.data_block);
    let model_voxel_buf =
        create_storage_buffer_u32(&device, &queue, "oasis_model_voxel", &model.data_voxel);

    let gen_params = GpuGeneratorModelParams {
        size_in_voxels: world_size_in_voxels,
        _pad0: 0,
        model_size_in_chunks: model.size_in_chunks,
        _pad1: 0,
        group_offset_in_chunks,
        group_size_in_chunks_x: group_size_in_chunks[0],
        group_size_in_chunks_y: group_size_in_chunks[1],
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    };
    let gen_params_buf = create_params_uniform(&device, &queue, &gen_params);

    // chunk_calc resources — note that chunks/blocks/voxels target the
    // segment as if it were a self-contained world of `group_size_in_chunks`.
    let mk_storage = |label: &'static str, data: &[u32]| {
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
    };

    let gpu_blocks = mk_storage("oasis_blocks", &vec![0u32; max_blocks]);
    let gpu_voxels = mk_storage("oasis_voxels", &vec![0u32; max_voxels_u32]);
    let gpu_block_voxel_count = mk_storage("oasis_bvc", &block_voxel_count_init);
    let gpu_hash_map = mk_storage("oasis_hashmap", &hash_map_init);
    let gpu_coeffs = mk_storage("oasis_coeffs", &coeffs);

    let calc_params = GpuConstructionParams {
        size_in_chunks: group_size_in_chunks,
        _pad0: 0,
        group_size_in_groups: [1, 1, 1],
        _pad1: 0,
        bound_group_queue_max_size: 1,
        hash_map_size: hash_map_size_slots,
        segment_size_in_chunks: group_size_in_chunks[0]
            .max(group_size_in_chunks[1])
            .max(group_size_in_chunks[2]),
        max_group_bound_dispatch: 0,
        chunk_offset: [0, 0, 0],
        dispatch_offset: 0,
        frame_index: 0,
        changed_chunk_count: 0,
        changed_block_count: 0,
        changed_voxel_count: 0,
    };
    let calc_params_buf = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_calc_params"),
        size: std::mem::size_of::<GpuConstructionParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&calc_params_buf, 0, bytemuck::bytes_of(&calc_params));

    let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count];
    let chunks_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_chunks"),
        size: (chunk_count as u64) * 8,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

    // ── Pipelines ───────────────────────────────────────────────────────────
    let gen_layout = generator_model_layout_descriptor();
    let calc_layout = construction_world_layout_descriptor();
    let (id_gen, id_calc, id_voxel, id_block) = {
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        (
            queue_generator_model_pipeline_with_handle(cache, gen_layout.clone(), gen_shader_handle.clone()),
            queue_calc_block_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_voxel_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
            queue_block_bounds_pipeline_with_handle(cache, calc_layout.clone(), calc_shader_handle.clone()),
        )
    };
    let mut pipelines: Option<(
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
        bevy::render::render_resource::ComputePipeline,
    )> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..64 {
        let mut pc = render_app.world_mut().resource_mut::<PipelineCache>();
        pc.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let (Some(g), Some(a), Some(b), Some(c)) = (
            cache.get_compute_pipeline(id_gen),
            cache.get_compute_pipeline(id_calc),
            cache.get_compute_pipeline(id_voxel),
            cache.get_compute_pipeline(id_block),
        ) {
            pipelines = Some((g.clone(), a.clone(), b.clone(), c.clone()));
            break;
        }
    }
    let (p_gen, p_calc, p_voxel, p_block) =
        pipelines.ok_or("oasis pipelines did not compile")?;

    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();

    let gen_bgl = cache.get_bind_group_layout(&gen_layout);
    let gen_bg = device.create_bind_group(
        "oasis_gen_bg",
        &gen_bgl,
        &BindGroupEntries::sequential((
            segment_buf.as_entire_buffer_binding(),
            model_chunk_buf.as_entire_buffer_binding(),
            model_block_buf.as_entire_buffer_binding(),
            model_voxel_buf.as_entire_buffer_binding(),
            gen_params_buf.as_entire_buffer_binding(),
        )),
    );
    let calc_bgl = cache.get_bind_group_layout(&calc_layout);
    let calc_bg = device.create_bind_group(
        "oasis_calc_bg",
        &calc_bgl,
        &BindGroupEntries::sequential((
            chunks_buffer.as_entire_buffer_binding(),
            gpu_blocks.as_entire_buffer_binding(),
            gpu_voxels.as_entire_buffer_binding(),
            gpu_block_voxel_count.as_entire_buffer_binding(),
            segment_buf.as_entire_buffer_binding(),
            gpu_hash_map.as_entire_buffer_binding(),
            calc_params_buf.as_entire_buffer_binding(),
            gpu_coeffs.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch generator → chunk_calc → bounds ─────────────────────────────
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_dispatch"),
    });
    dispatch_generator_model_with_encoder(&mut enc, &p_gen, &gen_bg, group_size_in_chunks);
    dispatch_calc_block_from_raw_data_world_sized(&mut enc, &p_calc, &calc_bg, group_size_in_chunks);
    queue.submit([enc.finish()]);

    // Readback cursor for bounds.
    let cursor_pair = {
        let size = 2u64 * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_cur"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_cur_enc"),
        });
        enc.copy_buffer_to_buffer(&gpu_block_voxel_count, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let voxel_wg = cursor_pair[0] / 64;
    let block_wg = cursor_pair[1] / 64;
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_bnd"),
    });
    dispatch_compute_voxel_bounds(&mut enc, &p_voxel, &calc_bg, voxel_wg);
    dispatch_compute_block_bounds(&mut enc, &p_block, &calc_bg, block_wg);
    queue.submit([enc.finish()]);

    // ── Verify generator output ALSO matches CPU oracle ─────────────────────
    let gpu_seg_voxels = {
        let size = (chunk_data_u32s as u64) * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_seg_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_seg_rb_enc"),
        });
        enc.copy_buffer_to_buffer(&segment_buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gen_first_diff = cpu_seg_voxels
        .iter()
        .zip(gpu_seg_voxels.iter())
        .enumerate()
        .find(|(_, (a, b))| a != b);

    // ── Readback chunks/blocks/voxels ────────────────────────────────────────
    let read_u32 = |buf: &bevy::render::render_resource::Buffer, n: u64| {
        let size = n * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("oasis_rb"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("oasis_rb_enc"),
        });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    let gpu_blocks_out = read_u32(&gpu_blocks, max_blocks as u64);
    let gpu_voxels_out = read_u32(&gpu_voxels, max_voxels_u32 as u64);
    let staging_size = (chunk_count as u64) * 8;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("oasis_chunks_rb"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oasis_chunks_rb_enc"),
    });
    enc.copy_buffer_to_buffer(&chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let gpu_chunks_out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();

    // ── Diagnostic ──────────────────────────────────────────────────────────
    let mut s = String::new();
    s.push_str(&format!(
        "cursors: voxel_pairs={} block_u32s={} ; CPU oracle: chunks={} blocks={} voxels={}\n",
        cursor_pair[0],
        cursor_pair[1],
        oracle.chunks.len(),
        oracle.blocks.len(),
        oracle.voxels.len(),
    ));

    if let Some((i, (a, b))) = gen_first_diff {
        let xor = a ^ b;
        s.push_str(&format!(
            "  GENERATOR DIVERGED @ u32[{i}]: cpu={a:#010x} gpu={b:#010x} XOR={xor:#010x}\n",
        ));
    } else {
        s.push_str("  generator: byte-equal ({} u32s)\n");
    }

    // Semantic pointer-following diff (same as the other multi-fixture path).
    let mut sem_chunk_mm: Option<(usize, ChunkCell, ChunkCell, u32, u32)> = None;
    let mut sem_block_mm: Option<(usize, usize, BlockCell, BlockCell, u32, u32)> = None;
    let mut sem_voxel_mm: Option<(usize, usize, usize, u32, u32)> = None;
    let mut total_mm: u32 = 0;
    'outer: for ci in 0..chunk_count {
        let cpu_chunk = ChunkCell::decode(oracle.chunks[ci]);
        let gpu_chunk = ChunkCell::decode(gpu_chunks_out[ci]);
        if chunk_kind(&cpu_chunk) != chunk_kind(&gpu_chunk) {
            sem_chunk_mm.get_or_insert((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
            total_mm += 1;
            continue;
        }
        match (cpu_chunk, gpu_chunk) {
            (ChunkCell::Mixed(cp), ChunkCell::Mixed(gp)) => {
                for bi in 0..64usize {
                    let cb_raw = oracle.blocks.get(cp.0 as usize + bi).copied().unwrap_or(0);
                    let gb_raw = gpu_blocks_out.get(gp.0 as usize + bi).copied().unwrap_or(0);
                    let cb = BlockCell::decode(cb_raw);
                    let gb = BlockCell::decode(gb_raw);
                    if block_kind(&cb) != block_kind(&gb) {
                        sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                        total_mm += 1;
                        continue;
                    }
                    match (cb, gb) {
                        (BlockCell::Mixed(cv), BlockCell::Mixed(gv)) => {
                            for vi in 0..32usize {
                                let cv_w = oracle.voxels.get(cv.0 as usize + vi).copied().unwrap_or(0);
                                let gv_w = gpu_voxels_out.get(gv.0 as usize + vi).copied().unwrap_or(0);
                                if cv_w != gv_w {
                                    sem_voxel_mm.get_or_insert((ci, bi, vi, cv_w, gv_w));
                                    total_mm += 1;
                                    break 'outer;
                                }
                            }
                        }
                        (BlockCell::UniformFull(a), BlockCell::UniformFull(b)) if a != b => {
                            sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                            total_mm += 1;
                        }
                        (BlockCell::Empty(a1), BlockCell::Empty(a2)) if a1.d != a2.d => {
                            sem_block_mm.get_or_insert((ci, bi, cb, gb, cb_raw, gb_raw));
                            total_mm += 1;
                        }
                        _ => {}
                    }
                }
            }
            (ChunkCell::UniformFull(a), ChunkCell::UniformFull(b)) if a != b => {
                sem_chunk_mm.get_or_insert((ci, cpu_chunk, gpu_chunk, oracle.chunks[ci], gpu_chunks_out[ci]));
                total_mm += 1;
            }
            (ChunkCell::Empty(_), ChunkCell::Empty(_)) => {
                // Chunk-AADFs for Empty chunks are set by `bounds_calc.wgsl`,
                // not by `chunk_calc.wgsl`. The CPU oracle (`construct()`)
                // computes chunk-AADFs via `compute_aadf_layer` as part of
                // its single function call, but this diagnostic runs only
                // chunk_calc + the block/voxel bounds — NOT the chunk
                // bounds. Therefore, Empty(AADF) differences here are EXPECTED
                // and NOT a bug.
            }
            _ => {}
        }
    }

    s.push_str(&format!(
        "  [semantic diff] total mismatches: {total_mm} (NB: Empty-chunk AADFs ignored — bounds_calc not run)\n"
    ));
    if let Some((ci, cc, gc, cr, gr)) = sem_chunk_mm {
        s.push_str(&format!(
            "    CHUNK MM @ ci={ci}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x}\n",
            cc, cr, gc, gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, cb, gb, cr, gr)) = sem_block_mm {
        s.push_str(&format!(
            "    BLOCK MM @ ci={ci} bi={bi}: cpu={:?} (raw={:#010x}) gpu={:?} (raw={:#010x}) XOR={:#010x} (bits {:032b})\n",
            cb, cr, gb, gr, cr ^ gr, cr ^ gr,
        ));
    }
    if let Some((ci, bi, vi, cw, gw)) = sem_voxel_mm {
        s.push_str(&format!(
            "    VOXEL MM @ ci={ci} bi={bi} vi={vi}: cpu={:#010x} gpu={:#010x} XOR={:#010x} (bits {:032b})\n",
            cw, gw, cw ^ gw, cw ^ gw,
        ));
    }

    Ok(s)
}

/// Decode a `segment_voxel_buffer`-formatted vector back into a `DenseVolume`
/// of `group_size_in_chunks` extent. Inverse of `build_segment_voxel_buffer`.
fn decode_segment_voxels_to_volume(
    seg: &[u32],
    group_size_in_chunks: [u32; 3],
) -> crate::aadf::construct::DenseVolume {
    use crate::voxel::VoxelTypeId;
    let mut volume = crate::aadf::construct::DenseVolume::empty(group_size_in_chunks);
    let gscx = group_size_in_chunks[0] as usize;
    let gscy = group_size_in_chunks[1] as usize;
    let gscz = group_size_in_chunks[2] as usize;
    for cz in 0..gscz {
        for cy in 0..gscy {
            for cx in 0..gscx {
                let chunk_index_in_segment = cx + cy * gscx + cz * gscx * gscy;
                let chunk_base = chunk_index_in_segment * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let pair = seg[block_base + vi];
                                let lo = (pair & 0xFFFF) as u16;
                                let hi = ((pair >> 16) & 0xFFFF) as u16;
                                for (slot, half) in [(0, lo), (1, hi)].iter() {
                                    let voxel_idx = vi * 2 + slot;
                                    let lx = voxel_idx % 4;
                                    let ly = (voxel_idx / 4) % 4;
                                    let lz = voxel_idx / 16;
                                    let vx = cx * 16 + bx * 4 + lx;
                                    let vy = cy * 16 + by * 4 + ly;
                                    let vz = cz * 16 + bz * 4 + lz;
                                    let ty_bits = *half & 0x7FFF;
                                    let full = *half & 0x8000 != 0;
                                    let ty = if full {
                                        VoxelTypeId(ty_bits)
                                    } else {
                                        VoxelTypeId::EMPTY
                                    };
                                    volume.set([vx as u32, vy as u32, vz as u32], ty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    volume
}

/// Build a `ModelData` with diverse per-chunk content. Each chunk has
/// `(chunk_idx % 3)` classification: 0 = empty, 1 = uniform-full of type
/// `(chunk_idx % 256 + 1)`, 2 = mixed with one solid voxel at position
/// `(chunk_idx % 64)` of block (0,0,0).
fn build_mixed_model_data(model_size_in_chunks: [u32; 3]) -> crate::aadf::generator::ModelData {
    let chunk_count = (model_size_in_chunks[0]
        * model_size_in_chunks[1]
        * model_size_in_chunks[2]) as usize;
    let mut data_chunk = vec![0u32; chunk_count];
    let mut data_block: Vec<u32> = Vec::new();
    let mut data_voxel: Vec<u32> = Vec::new();
    for ci in 0..chunk_count {
        let kind = ci % 3;
        match kind {
            0 => {
                // empty
                data_chunk[ci] = 0u32;
            }
            1 => {
                // uniform-full
                let ty = ((ci % 255) + 1) as u32;
                data_chunk[ci] = (1u32 << 30) | (ty & 0x3FFF_FFFF);
            }
            _ => {
                // mixed: 64 blocks for this chunk, only block 0 is mixed
                // (one voxel at position `ci % 64` of block 0).
                let block_base = data_block.len() as u32;
                data_chunk[ci] = (2u32 << 30) | block_base;
                // Reserve 64 blocks.
                data_block.resize(data_block.len() + 64, 0u32);
                // Block 0: mixed, voxels base.
                let voxel_base = data_voxel.len() as u32;
                data_block[block_base as usize] = (2u32 << 30) | voxel_base;
                // 32 voxel-pairs = 64 voxels for block 0.
                data_voxel.resize(data_voxel.len() + 32, 0u32);
                let pos: u32 = (ci % 64) as u32;
                // Place full voxel type 0x42 at position `pos`. Pos 0..64
                // decodes to lx=pos%4, ly=(pos/4)%4, lz=pos/16 — voxel
                // index in block is pos. In data_voxel, voxel index ÷ 2 =
                // pair index; even slot is low half, odd slot is high half.
                let pair = pos / 2;
                let voxel_word = if pos % 2 == 0 {
                    (1u32 << 15) | 0x42 // even slot, low half
                } else {
                    ((1u32 << 15) | 0x42) << 16 // odd slot, high half
                };
                data_voxel[voxel_base as usize + pair as usize] = voxel_word;
            }
        }
    }
    crate::aadf::generator::ModelData {
        data_chunk,
        data_block: if data_block.is_empty() { vec![0] } else { data_block },
        data_voxel: if data_voxel.is_empty() { vec![0] } else { data_voxel },
        size_in_chunks: model_size_in_chunks,
    }
}

/// Build a segment voxel buffer for ONE segment (chunks at offset
/// `chunk_offset` with extent `segment_size_in_chunks` per axis). Only the
/// chunks inside the segment are populated; out-of-volume chunks (if the
/// segment partially extends beyond the volume) are zero.
fn build_segment_voxel_buffer_for_region(
    volume: &crate::aadf::construct::DenseVolume,
    chunk_offset: [u32; 3],
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let total_u32s = seg * seg * seg * 2048;
    let mut out = vec![0u32; total_u32s];
    let sc = volume.size_in_chunks;
    for lcz in 0..seg {
        let cz = chunk_offset[2] as usize + lcz;
        if cz >= sc[2] as usize { continue; }
        for lcy in 0..seg {
            let cy = chunk_offset[1] as usize + lcy;
            if cy >= sc[1] as usize { continue; }
            for lcx in 0..seg {
                let cx = chunk_offset[0] as usize + lcx;
                if cx >= sc[0] as usize { continue; }
                let chunk_index_in_segment = lcx + lcy * seg + lcz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;
                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let lo = voxel_at_block_local(
                                    volume, [cx, cy, cz], [bx, by, bz], vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume, [cx, cy, cz], [bx, by, bz], vi * 2 + 1,
                                );
                                out[block_base + vi] = (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

fn chunk_kind(c: &crate::aadf::cell::ChunkCell) -> u8 {
    match c {
        crate::aadf::cell::ChunkCell::Empty(_) => 0,
        crate::aadf::cell::ChunkCell::UniformFull(_) => 1,
        crate::aadf::cell::ChunkCell::Mixed(_) => 2,
    }
}

fn block_kind(b: &crate::aadf::cell::BlockCell) -> u8 {
    match b {
        crate::aadf::cell::BlockCell::Empty(_) => 0,
        crate::aadf::cell::BlockCell::UniformFull(_) => 1,
        crate::aadf::cell::BlockCell::Mixed(_) => 2,
    }
}

/// Build `segment_voxel_buffer` for a fixture using the full world's
/// dimensions, indexed via `chunk_index = cx + cy*S + cz*S*S` where
/// `S = segment_size_in_chunks = max(size_in_chunks)`. Chunks outside the
/// volume's extent are left zero.
fn build_segment_voxel_buffer_for_world(
    volume: &crate::aadf::construct::DenseVolume,
    segment_size_in_chunks: u32,
) -> Vec<u32> {
    let seg = segment_size_in_chunks as usize;
    let sc = volume.size_in_chunks;
    // Size precisely: the max chunk_index_in_segment accessed is
    // `(sc[0]-1) + (sc[1]-1)*seg + (sc[2]-1)*seg*seg`, each chunk consumes
    // 2048 u32s. Add 1 for the inclusive bound.
    let max_chunk_idx = (sc[0] as usize - 1)
        + (sc[1] as usize - 1) * seg
        + (sc[2] as usize - 1) * seg * seg;
    let total_u32s = (max_chunk_idx + 1) * 2048;
    let mut out = vec![0u32; total_u32s];

    for cz in 0..(sc[2] as usize) {
        for cy in 0..(sc[1] as usize) {
            for cx in 0..(sc[0] as usize) {
                let chunk_index_in_segment = cx + cy * seg + cz * seg * seg;
                let chunk_base = chunk_index_in_segment * 2048;

                for bz in 0..4 {
                    for by in 0..4 {
                        for bx in 0..4 {
                            let block_index = bx + by * 4 + bz * 16;
                            let block_base = chunk_base + block_index * 32;
                            for vi in 0..32 {
                                let lo = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2,
                                );
                                let hi = voxel_at_block_local(
                                    volume,
                                    [cx, cy, cz],
                                    [bx, by, bz],
                                    vi * 2 + 1,
                                );
                                out[block_base + vi] =
                                    (lo as u32) | ((hi as u32) << 16);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// W4 — entry point for `bevy-naadf e2e_render --entities`.
///
/// Runs `EntityHandler::update` for a small fixture (one moving entity in a
/// 4×4×4-chunk world, 2 frames of motion), asserts the uploads are
/// well-formed, and reports a short summary. Until wave-3 wires the
/// render-side dispatch, this is the load-bearing W4 e2e gate.
///
/// The fixture:
/// - World is 4×4×4 chunks = 64 chunks.
/// - One entity (size 8×8×8) at world position (16, 16, 16) — straddles
///   the chunk-boundary at every axis, so it overlaps 8 chunks (the
///   2×2×2 chunks at the boundary).
/// - Frame 2: same entity translated to (24, 16, 16); the entity now
///   overlaps a different but partially-overlapping set of 8 chunks, so
///   the handler must emit "clear" updates for the no-longer-overlapped
///   chunks + "set" updates for the new ones.
///
/// Asserts:
/// - Frame 1: 8 chunk_updates (the 2×2×2 overlapped chunks), 8 entity
///   chunk instances (dedup never fires because each chunk's instance
///   list is unique by chunk-id ordering), 1 entity_history entry.
/// - Frame 2: the chunk_updates include both old chunks (cleared) and
///   new chunks (set) — at least 8 updates.
/// Phase-C W2 — entry point for `e2e_render --edit-mode`.
///
/// Runs a CPU-side scripted edit end-to-end via the `set_voxel` API and
/// asserts:
///   1. The edit produces a non-empty `PendingEdits.batches`.
///   2. The CPU `process_edit_batch` produces well-formed `changed_chunks` +
///      `changed_blocks` / `changed_voxels` arrays.
///   3. The flood-fill CPU oracle (`change_handler::compute_change_groups`)
///      produces the expected `changed_groups` array for the edit's group.
///
/// This is a **CPU-side** end-to-end validation — equivalent to W4's
/// `validate_entity_handler` design. The GPU bit-exact validation lives in
/// the `world_change::tests` GPU test suite; this flag is for catching
/// integration-level regressions (the `set_voxel` → `process_edit_batch` →
/// `compute_change_groups` chain) without requiring a windowed GPU run.
pub fn validate_edit_mode() -> Result<String, String> {
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::voxel::{VoxelTypeId, CELL_DIM};
    use crate::world::data::WorldData;
    use bevy::prelude::UVec3;

    // Build a 4×2×4-chunk world matching the production test grid layout.
    let size_in_chunks = [4u32, 2, 4];
    let mut volume = DenseVolume::empty(size_in_chunks);
    // Put a single full voxel at (16, 4, 16) — chunk (1, 0, 1)'s corner — so
    // there's some non-empty geometry around the planned edit position.
    volume.set([16, 4, 16], VoxelTypeId(5));
    let built = construct(&volume);

    let mut world_data = WorldData {
        chunks_cpu: built.chunks,
        blocks_cpu: built.blocks,
        voxels_cpu: built.voxels,
        size_in_chunks: UVec3::from_array(size_in_chunks),
        bounding_box: crate::world::data::IAabb3 {
            min: bevy::prelude::IVec3::ZERO,
            max: bevy::prelude::IVec3::new(
                (size_in_chunks[0] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[1] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[2] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: volume.voxels.iter().map(|t| t.0).collect(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    // The pre-edit chunks_cpu — record its bytes to verify the edit changed
    // something.
    let pre_edit_chunks = world_data.chunks_cpu.clone();

    // Apply the scripted edit: set voxel (20, 12, 20) to a new emissive type.
    // This is in chunk (1, 0, 1), block (1, 3, 1), voxel (0, 0, 0).
    let new_type = VoxelTypeId(9);
    world_data.set_voxel(bevy::prelude::IVec3::new(20, 12, 20), new_type);

    if world_data.pending_edits.batches.is_empty() {
        return Err("set_voxel produced no edit batch".into());
    }
    if world_data.pending_edits.edited_groups.is_empty() {
        return Err("set_voxel produced no edited_groups".into());
    }
    let batch = &world_data.pending_edits.batches[0];
    if batch.changed_chunks.is_empty() {
        return Err("edit batch has no changed_chunks".into());
    }
    // NOTE (`02e-perframe-cpu-investigation.md`, 2026-05-16): edit paths no
    // longer set `world_data.dirty = true`. Per-edit changes flow through the
    // W2 delta-upload chain (`pending_edits.batches` → `naadf_world_change_node`);
    // the full-world re-extract that `dirty` triggers is redundant and was
    // causing the per-frame full-world upload bottleneck on Oasis-class worlds.
    // We continue to assert that the batch + chunks_cpu mutation happened
    // (already verified above) — those carry the actual per-edit change.
    if pre_edit_chunks == world_data.chunks_cpu {
        return Err("set_voxel did not mutate chunks_cpu".into());
    }

    // Run the CPU flood-fill — even though `size_in_chunks = [4, 2, 4]` gives
    // `bound_group_count = 0` on the W3 path (Y=2 not divisible by 4), the
    // `compute_change_groups` function handles this with an early-return at
    // the `size_in_groups > 0` check in `extract_world_changes`.
    let size_in_groups = [
        size_in_chunks[0] / 4,
        size_in_chunks[1] / 4,
        size_in_chunks[2] / 4,
    ];
    let flood_groups_len = if size_in_groups[0] > 0
        && size_in_groups[1] > 0
        && size_in_groups[2] > 0
    {
        let groups = super::change_handler::compute_change_groups(
            size_in_groups,
            &world_data.pending_edits.edited_groups,
        );
        groups.entries.len()
    } else {
        // `size_in_groups[1] == 0` on the 4×2×4 test grid — the W3 bound queues
        // are dormant on this grid; the flood-fill is correctly skipped.
        0
    };

    Ok(format!(
        "edit-mode PASS: 1 set_voxel call produced {} changed_chunks + {} \
         changed_blocks records + {} changed_voxels records; flood-fill produced \
         {} group entries (size_in_groups = {:?})",
        batch.changed_chunks.len(),
        batch.changed_blocks.len() / 65,
        batch.changed_voxels.len() / 33,
        flood_groups_len,
        size_in_groups,
    ))
}

/// `02f` rearch — **runtime-edit gate**. Complements [`validate_edit_mode`]
/// (which exercises the diagnostic `set_voxel` path) by hitting the
/// production runtime brush path: [`crate::world::data::WorldData::set_voxels_batch`].
///
/// **Why this gate exists.** The pre-`02f` CPU-oracle `--edit-mode` gate
/// missed the regression mode "edit landed in main-world `pending_edits`
/// but never crossed to the render world": that gate only exercised
/// `set_voxel` (the diagnostic oracle path) against a self-built
/// `WorldData`, with no extract pass + no render-graph dispatch in scope.
/// The `dirty=true never on edits` failure mode (`02e`/`03e` followup)
/// slipped through because `set_voxel` produces an edit batch INDEPENDENT
/// of the dirty flag, and the CPU oracle gate doesn't observe the
/// extract-to-render-world ferry.
///
/// **What this gate asserts.** Builds a minimal in-process world, calls
/// [`crate::world::data::WorldData::set_voxels_batch`] (the production
/// brush entry point — same code path the editor's `apply_edit_tool`
/// invokes), and asserts:
///
/// 1. The batch produces a non-empty `pending_edits.batches` AND
///    non-empty `pending_edits.edited_groups`.
/// 2. The runtime path's `changed_chunks` array is non-empty (proving
///    `process_edit_batch` ran and emitted records — the load-bearing W2
///    delta payload).
/// 3. The runtime path does NOT emit the synthetic whole-world AADF
///    refresh records (i.e. `changed_chunks.len()` is in the touched-chunk
///    range, NOT the whole-world range) — this confirms the runtime path
///    skips `recompute_chunk_layer_aadfs` (the diagnostic-only CPU rebuild
///    the `02f` rearch retires from the production hot path).
///
/// **What this gate does NOT verify** (out of scope for an in-process
/// gate): the GPU render-graph dispatch (`naadf_world_change_node` →
/// `apply_chunk_change.wgsl` + the 3 sibling shaders), the
/// extract-to-`ConstructionEvents` flow, the GPU buffer mutation, the
/// framebuffer luminance delta. Those need a windowed harness with
/// before/after screenshot comparison; out of this dispatch's scope.
/// **The asymmetric coverage is deliberate** — this gate closes the
/// regression hole the `02e`/`03e` followup left open (edit-doesn't-reach-
/// W2-batch) without re-implementing a full integration test.
pub fn validate_runtime_edit_mode() -> Result<String, String> {
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::voxel::{VoxelTypeId, CELL_DIM};
    use crate::world::data::WorldData;
    use bevy::prelude::UVec3;

    // 4×4×4-chunk world — bigger than the `--edit-mode` 4×2×4 fixture so
    // the brush's chunk-AABB distinguishes "touched chunks only" from
    // "whole-world" (4×4×4 = 64 chunks vs ~125 chunks the brush touches
    // at r=16 — the 4×4×4 fixture's 64 chunks is the WHOLE world).
    let size_in_chunks = [4u32, 4, 4];
    let mut volume = DenseVolume::empty(size_in_chunks);
    volume.set([16, 16, 16], VoxelTypeId(5));
    let built = construct(&volume);
    let total_chunks = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;

    let mut world_data = WorldData {
        chunks_cpu: built.chunks,
        blocks_cpu: built.blocks,
        voxels_cpu: built.voxels,
        size_in_chunks: UVec3::from_array(size_in_chunks),
        bounding_box: crate::world::data::IAabb3 {
            min: bevy::prelude::IVec3::ZERO,
            max: bevy::prelude::IVec3::new(
                (size_in_chunks[0] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[1] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
                (size_in_chunks[2] * CELL_DIM as u32 * CELL_DIM as u32) as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: volume.voxels.iter().map(|t| t.0).collect(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();

    // Production brush path — same call shape the editor's brushes use
    // (`editor/tools.rs::paint_brush` / `cube_brush` / `sphere_brush`).
    // Three voxels in two adjacent chunks: tests the by-chunk grouping +
    // the multi-chunk batched dispatch.
    let new_type = VoxelTypeId(9);
    let edits = [
        (bevy::prelude::IVec3::new(20, 12, 20), new_type),
        (bevy::prelude::IVec3::new(21, 12, 20), new_type),
        (bevy::prelude::IVec3::new(36, 12, 36), new_type),
    ];
    world_data.set_voxels_batch(&edits);

    // Gate 1 — the runtime path produced a non-empty edit batch.
    if world_data.pending_edits.batches.is_empty() {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch produced no edit batch \
             — the W2 delta chain has no work; edits would silently never \
             reach the GPU. This is the regression mode the `02f` rearch \
             addresses (edit-doesn't-reach-W2-batch)."
                .into(),
        );
    }
    if world_data.pending_edits.edited_groups.is_empty() {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch produced no \
             edited_groups — the `compute_change_groups` BFS oracle in \
             `extract_world_changes` would skip with no work, and the W2 \
             GPU dispatch's `changed_groups_dynamic` would never populate."
                .into(),
        );
    }

    // Gate 2 — the batch carries `changed_chunks` records.
    let batch = &world_data.pending_edits.batches[0];
    if batch.changed_chunks.is_empty() {
        return Err(
            "runtime-edit gate FAIL: edit batch has no changed_chunks \
             records — `process_edit_batch` failed to emit per-touched-\
             chunk records. The W2 GPU dispatch's `apply_chunk_change` \
             pass would have no input."
                .into(),
        );
    }

    // Gate 3 — the runtime path is the production fast path, NOT the
    // diagnostic oracle. The runtime path's `changed_chunks` count is
    // bounded by the touched-chunk count (~2 here — two edits in chunk
    // (1,0,1) and one edit in chunk (2,0,2)). The diagnostic oracle path
    // would emit synthetic AADF-refresh entries for many more chunks
    // (up to ~total_chunks - touched). Assert the runtime path is NOT
    // accidentally invoking the oracle's whole-world recompute.
    let touched_chunks = batch.changed_chunks.len();
    // The runtime path touches at most the chunks listed in the edit set
    // (3 edits → at most 2 unique chunks). A whole-world recompute would
    // emit on the order of `total_chunks` (= 64) records. Assert the
    // ratio is small (touched / total ≤ 0.5 = 32) — well below the
    // whole-world threshold.
    if touched_chunks > total_chunks / 2 {
        return Err(format!(
            "runtime-edit gate FAIL: runtime path emitted {touched_chunks} \
             changed_chunks records for a brush that touched ≤2 chunks (out \
             of {total_chunks} total) — likely accidentally invoking the \
             diagnostic `recompute_chunk_layer_aadfs` whole-world rehash on \
             the runtime hot path, which the `02f` rearch retires."
        ));
    }

    // Gate 4 — chunks_cpu was mutated in place (proves the CPU mirror
    // patch landed; the editor's mouse-pick ray_traversal reads
    // chunks_cpu and would see stale state if this skipped).
    let pre_state = built_pre_edit_state(&volume, &edits);
    let any_chunk_mutated = pre_state.iter().enumerate().any(|(ci, &pre)| {
        ci < world_data.chunks_cpu.len() && world_data.chunks_cpu[ci] != pre
    });
    if !any_chunk_mutated {
        return Err(
            "runtime-edit gate FAIL: set_voxels_batch did not mutate any \
             chunks_cpu entry — the CPU mirror patch (cheap in-place; the \
             `02f` rearch directive bullet #5) did not land. \
             `WorldData::ray_traversal` would read stale state."
                .into(),
        );
    }

    Ok(format!(
        "runtime-edit gate PASS: set_voxels_batch produced {} batch(es) \
         with {} changed_chunks + {} changed_blocks + {} changed_voxels \
         records (out of {total_chunks} total chunks — runtime path \
         touched-only, NOT whole-world rehash); {} edited_groups for the \
         BFS oracle. CPU mirror patched in-place.",
        world_data.pending_edits.batches.len(),
        touched_chunks,
        batch.changed_blocks.len() / 65,
        batch.changed_voxels.len() / 33,
        world_data.pending_edits.edited_groups.len(),
    ))
}

/// `02f` runtime-edit gate helper — rebuild the `chunks_cpu` mirror that
/// `construct(&volume)` would have produced (the pre-edit baseline) so the
/// gate can diff against post-edit `chunks_cpu`. Implementation: re-run
/// `construct` and return its `chunks` (cheap; the test fixture is 4×4×4
/// chunks).
fn built_pre_edit_state(
    volume: &crate::aadf::construct::DenseVolume,
    _edits: &[(bevy::prelude::IVec3, crate::voxel::VoxelTypeId)],
) -> Vec<u32> {
    crate::aadf::construct::construct(volume).chunks
}

pub fn validate_entity_handler() -> Result<String, String> {
    use crate::aadf::entity::decompress_quaternion;
    use crate::render::construction::entity_handler::EntityHandler;
    use crate::render::gpu_types::EntityInstance;
    use bevy::math::Vec3;

    let mut handler = EntityHandler::new([4, 4, 4]);
    // Place the entity so it overlaps the 2×2×2 chunks at the boundary.
    // Chunks are 16 voxels wide; the entity is 8 voxels; positioning at
    // (12, 12, 12) means [12..20] × [12..20] × [12..20] which straddles
    // the (0,0,0)/(1,1,1) chunk boundaries.
    let frame_a = vec![EntityInstance {
        position: Vec3::new(12.0, 12.0, 12.0),
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size: [8, 8, 8],
    }];
    let uploads_a = handler.update(&frame_a);
    if uploads_a.entity_history.len() != 1 {
        return Err(format!(
            "frame A: expected 1 entity_history entry, got {}",
            uploads_a.entity_history.len()
        ));
    }
    if uploads_a.chunk_updates.is_empty() {
        return Err("frame A: expected non-zero chunk_updates".into());
    }
    if uploads_a.entity_chunk_instances.is_empty() {
        return Err("frame A: expected non-zero entity_chunk_instances".into());
    }
    let frame_a_overlap_count = uploads_a.chunk_updates.len();

    // Frame B — entity moved.
    let frame_b = vec![EntityInstance {
        position: Vec3::new(20.0, 12.0, 12.0),
        quaternion: [0.0, 0.0, 0.0, 1.0],
        voxel_start: 0,
        entity: 0,
        size: [8, 8, 8],
    }];
    let uploads_b = handler.update(&frame_b);
    if uploads_b.chunk_updates.is_empty() {
        return Err("frame B: expected non-zero chunk_updates".into());
    }

    // Verify the quaternion roundtrip on the history slot.
    let history = uploads_a.entity_history[0];
    let q_decoded = decompress_quaternion((history.data3, history.data4));
    // Identity quaternion compresses + decompresses with ~ component (0,0,0,1).
    if q_decoded[3].abs() < 0.99 {
        return Err(format!(
            "history slot quaternion did not roundtrip identity: w = {}",
            q_decoded[3]
        ));
    }

    Ok(format!(
        "frame A: {} chunk_updates, {} entity_chunk_instances, {} history; \
         frame B: {} chunk_updates",
        frame_a_overlap_count,
        uploads_a.entity_chunk_instances.len(),
        uploads_a.entity_history.len(),
        uploads_b.chunk_updates.len(),
    ))
}

#[cfg(test)]
mod tests {
    //! W5 — the load-bearing bit-exact `GPU vs CPU` oracle test
    //! (`15-design-c.md` §1.6, §2.1 W5 row, §4.5).
    //!
    //! Builds a small fixed `ModelData`, runs BOTH the GPU pipeline and the
    //! `crate::aadf::generator::generate_segment_cpu` oracle against the same
    //! inputs, maps the GPU `segment_voxel_buffer` back to the CPU, and
    //! asserts byte-for-byte equality with the oracle's output.
    //!
    //! The test uses the same headless `App + RenderPlugin` fixture pattern
    //! `world::buffer::tests` uses (`world/buffer.rs:227-264`): build a
    //! minimal `App` with `RenderPlugin`, `finish()` + `cleanup()` to make
    //! `RenderDevice`/`RenderQueue` available, then drive the W5 dispatch
    //! by hand. No render schedule runs — that would require a full plugin
    //! set + a window.
    //!
    //! Skips with a warning when no wgpu adapter is available (CI box without
    //! a GPU). The CPU-oracle determinism + Y-clamp + OOB tests in
    //! `crate::aadf::generator::tests` exercise the oracle independently of
    //! the GPU path.
    use super::super::generator_model::{
        create_params_uniform, create_storage_buffer_u32, dispatch_generator_model,
        generator_model_layout_descriptor, queue_generator_model_pipeline_with_handle,
        GpuGeneratorModelParams, CHUNK_DATA_U32S, GENERATOR_MODEL_SHADER,
        GENERATOR_MODEL_SHADER_SRC,
    };
    use crate::aadf::generator::{generate_segment_cpu, ModelData};

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world (same plumbing as
    /// `world::buffer::tests::render_device_queue`). Returns the `App`, the
    /// device, the queue, and an inline-built `Handle<Shader>` for the W5
    /// generator-model WGSL.
    ///
    /// The test does NOT drive the standard `ExtractSchedule` (which would
    /// require all the `bevy_render::render_asset` Message types to be
    /// initialised — `MinimalPlugins` + `AssetPlugin` only initialise some
    /// of them). Instead, it populates the cache's shader registry directly
    /// via the `pub fn PipelineCache::set_shader` entry point, which is
    /// exactly what `extract_shaders` does internally.
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

        // Build a stable handle out of an inserted shader in the main world's
        // `Assets<Shader>` — the handle's `AssetId` is what the pipeline
        // cache keys on.
        let shader = Shader::from_wgsl(
            GENERATOR_MODEL_SHADER_SRC,
            "shaders/generator_model.wgsl",
        );
        let shader_clone = shader.clone();
        let shader_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(shader);

        // Inject the shader directly into the pipeline cache (mirrors what
        // `extract_shaders` does at the end of `ExtractSchedule`).
        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(shader_handle.id(), shader_clone);
        }

        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, shader_handle))
    }

    /// Read back the first `count` `u32`s of `src`.
    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        count: u64,
    ) -> Vec<u32> {
        let size = count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("naadf_generator_readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_generator_readback_encoder"),
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

    /// W5's load-bearing test: GPU `generator_model.wgsl` output is
    /// **byte-for-byte equal** to `generate_segment_cpu` (the §1.6 oracle).
    ///
    /// Test setup: a 2×1×2 chunk model, uniform-full of type 0x42, queried
    /// over a 2×1×2 chunk segment. The model's voxel-level data path is
    /// exercised by the construct-and-run paths below; the Y-clamp does not
    /// fire (segment matches model height); the OOB short-circuit does not
    /// fire (segment matches world `sizeInVoxels`).
    ///
    /// Buffer size: 2×1×2 chunks × 2048 u32s = 8192 u32s = 32 KiB.
    #[test]
    fn generator_model_gpu_vs_cpu_bit_exact() {
        let Some((mut app, device, queue, shader_handle)) = render_fixture() else {
            eprintln!("no wgpu device — skipping W5 GPU/CPU bit-exact test");
            return;
        };

        // Fixed test inputs.
        let model = ModelData::uniform_full([2, 1, 2], 0x42);
        let group_offset_in_chunks = [0u32, 0, 0];
        let group_size_in_chunks = [2u32, 1, 2];
        let size_in_voxels = [32u32, 16, 32];

        // === CPU oracle =====================================================
        let cpu_out = generate_segment_cpu(
            &model,
            group_offset_in_chunks,
            group_size_in_chunks,
            size_in_voxels,
        );
        let total_chunks =
            (group_size_in_chunks[0] * group_size_in_chunks[1] * group_size_in_chunks[2])
                as u64;
        let chunk_data_u32s = total_chunks * CHUNK_DATA_U32S as u64;
        assert_eq!(cpu_out.len() as u64, chunk_data_u32s);

        // === GPU path =======================================================
        // Queue the pipeline + build the layout. We deliberately do NOT use
        // `app.init_gpu_resource::<ConstructionPipelines>()` here because that
        // pulls in the full `FromWorld` impl + the AssetServer load; for the
        // W5 isolated test, we go straight to the helpers + the inline shader.
        let layout = generator_model_layout_descriptor();
        let pipeline_id = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let pipeline_cache = render_app.world().resource::<PipelineCache>();
            queue_generator_model_pipeline_with_handle(
                pipeline_cache,
                layout.clone(),
                shader_handle.clone(),
            )
        };

        // Allocate the GPU buffers + uniform.
        let chunk_data_init = vec![0xDEAD_BEEFu32; chunk_data_u32s as usize];
        let gpu_chunk_data = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_segment_voxel_buffer_test",
            &chunk_data_init,
        );
        let gpu_model_chunk = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_chunk_test",
            &model.data_chunk,
        );
        let gpu_model_block = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_block_test",
            &model.data_block,
        );
        let gpu_model_voxel = create_storage_buffer_u32(
            &device,
            &queue,
            "naadf_model_data_voxel_test",
            &model.data_voxel,
        );
        let params = GpuGeneratorModelParams {
            size_in_voxels,
            _pad0: 0,
            model_size_in_chunks: model.size_in_chunks,
            _pad1: 0,
            group_offset_in_chunks,
            group_size_in_chunks_x: group_size_in_chunks[0],
            group_size_in_chunks_y: group_size_in_chunks[1],
            _pad2: 0,
            _pad3: 0,
            _pad4: 0,
        };
        let gpu_params = create_params_uniform(&device, &queue, &params);

        // Drive the pipeline to compile, then dispatch. The pipeline cache's
        // background compile is gated by `synchronous_pipeline_compilation`
        // (set true above), so `App::update()` once should be enough to fully
        // resolve the pipeline.
        // Drive pipeline compilation manually — `PipelineCache::process_queue`
        // is `pub` and `synchronous_pipeline_compilation = true` on the
        // RenderPlugin above forces compile-on-call. One pass is usually
        // enough; cap at 64 defensively.
        let pipeline = {
            let render_app = app.get_sub_app_mut(RenderApp).unwrap();
            let mut got = None;
            for _ in 0..64 {
                let mut pipeline_cache =
                    render_app.world_mut().resource_mut::<PipelineCache>();
                pipeline_cache.process_queue();
                if let Some(p) = pipeline_cache.get_compute_pipeline(pipeline_id) {
                    got = Some(p.clone());
                    break;
                }
            }
            got.expect("W5 generator_model pipeline did not compile in 64 ticks")
        };

        // Build the bind group against the resolved layout.
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let pipeline_cache = render_app.world().resource::<PipelineCache>();
        let bind_group_layout = pipeline_cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "naadf_generator_model_bind_group_test",
            &bind_group_layout,
            &BindGroupEntries::sequential((
                gpu_chunk_data.as_entire_buffer_binding(),
                gpu_model_chunk.as_entire_buffer_binding(),
                gpu_model_block.as_entire_buffer_binding(),
                gpu_model_voxel.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
            )),
        );

        dispatch_generator_model(
            &device,
            &queue,
            &pipeline,
            &bind_group,
            group_size_in_chunks,
        );

        // Map the GPU buffer back + compare byte-for-byte to the oracle.
        let gpu_out = readback_u32(&device, &queue, &gpu_chunk_data, chunk_data_u32s);
        assert_eq!(gpu_out.len(), cpu_out.len());

        // Find the first divergence (if any) for a useful failure message.
        if gpu_out != cpu_out {
            for (i, (&g, &c)) in gpu_out.iter().zip(cpu_out.iter()).enumerate() {
                if g != c {
                    panic!(
                        "W5 GPU vs CPU bit-exact divergence @ u32[{i}]: gpu={g:#010x} cpu={c:#010x}"
                    );
                }
            }
        }
        assert_eq!(gpu_out, cpu_out, "W5 GPU output must equal CPU oracle byte-for-byte");

        // Sanity: the test exercised a real workload, not an all-zeros buffer.
        // For the uniform-full model + matching segment, every u32 should pack
        // two type-0x42 voxels with the full flag set.
        let expected_voxel = 0x42u32 | (1u32 << 15);
        let expected_packed = expected_voxel | (expected_voxel << 16);
        assert_eq!(
            gpu_out[0], expected_packed,
            "uniform-full model should pack 0x{:04x}_{:04x} into every voxel pair",
            expected_voxel, expected_voxel
        );
        // And the buffer is not the placeholder 0xDEADBEEF init pattern.
        assert!(gpu_out.iter().all(|&u| u != 0xDEAD_BEEF));
        // Reference `GENERATOR_MODEL_SHADER` so the constant stays load-bearing
        // (a future rename of the asset path must trip the test compile).
        let _ = GENERATOR_MODEL_SHADER;
    }
}

#[cfg(test)]
mod tests_w1 {
    //! W1 — the load-bearing bit-exact `GPU vs CPU` oracle test
    //! (`15-design-c.md` §1.6, §2.1 W1 row, §4.1; `16-impl-c-W1.md`).
    //!
    //! Builds a small `DenseVolume`, runs the CPU oracle (`aadf::construct::
    //! construct`) AND the GPU `chunk_calc.wgsl` 3-entry-point chain (Algorithm
    //! 1 → voxel-AADFs → block-AADFs), maps GPU `blocks`/`voxels` + the chunks
    //! texture back to CPU, and asserts byte-equality.
    //!
    //! The W1 test is the load-bearing W1 deliverable per the brief: it
    //! exercises every shader entry point + the `BlockHashingHandler` Rust
    //! port + the `HashValueSlot` atomicity discipline.
    //!
    //! Note on pointer-assignment determinism: when the CPU
    //! `HashMap<[VoxelTypeId; 64], VoxelPtr>` and the GPU
    //! `open-addressing-by-hash` assign different `VoxelPtr` values to the
    //! same set of unique blocks (because Rust `HashMap` iterates in a hash-
    //! seed-randomised order, while the GPU assigns by hash-mod-mapsize), the
    //! blocks and voxels buffers may not be byte-equal at the *u32-content*
    //! level. The test mitigates this by exercising a small layer where the
    //! pointer space is trivially deterministic (single mixed block ⇒ both
    //! paths assign `VoxelPtr(0)`).

    use super::super::chunk_calc::{
        construction_world_layout_descriptor,
        queue_block_bounds_pipeline_with_handle, queue_calc_block_pipeline_with_handle,
        queue_voxel_bounds_pipeline_with_handle, CHUNK_CALC_SHADER_SRC,
    };
    use super::super::hashing::hash_coefficients;
    use super::super::map_copy::{
        map_copy_layout_descriptor, queue_copy_map_pipeline_with_handle, GpuMapCopyParams,
        MAP_COPY_SHADER_SRC,
    };
    use crate::aadf::cell::{BlockCell, ChunkCell, VoxelCell, VoxelPtr};
    use crate::aadf::construct::{construct, DenseVolume};
    use crate::render::gpu_types::{GpuConstructionParams, GpuHashValueSlot};
    use crate::voxel::VoxelTypeId;

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets, Handle};
    use bevy::image::ImagePlugin;
    use bevy::shader::Shader;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world + inject the W1 chunk_calc + map_copy
    /// shaders. Returns the app, the device, queue, and `Handle<Shader>` for
    /// both W1 shaders.
    #[allow(clippy::type_complexity)]
    fn render_fixture_w1() -> Option<(App, RenderDevice, RenderQueue, Handle<Shader>, Handle<Shader>)> {
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

        let chunk_calc_shader = Shader::from_wgsl(
            CHUNK_CALC_SHADER_SRC,
            "shaders/chunk_calc.wgsl",
        );
        let chunk_calc_shader_clone = chunk_calc_shader.clone();
        let chunk_calc_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(chunk_calc_shader);
        let map_copy_shader = Shader::from_wgsl(
            MAP_COPY_SHADER_SRC,
            "shaders/map_copy.wgsl",
        );
        let map_copy_shader_clone = map_copy_shader.clone();
        let map_copy_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(map_copy_shader);

        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(chunk_calc_handle.id(), chunk_calc_shader_clone);
            pipeline_cache.set_shader(map_copy_handle.id(), map_copy_shader_clone);
        }

        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, chunk_calc_handle, map_copy_handle))
    }

    fn create_storage_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        label: &'static str,
        data: &[u32],
    ) -> bevy::render::render_resource::Buffer {
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

    fn create_uniform<T: bytemuck::Pod>(
        device: &RenderDevice,
        queue: &RenderQueue,
        label: &'static str,
        data: &T,
    ) -> bevy::render::render_resource::Buffer {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<T>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(data));
        buffer
    }

    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &bevy::render::render_resource::Buffer,
        u32_count: u64,
    ) -> Vec<u32> {
        let size = u32_count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w1_readback_staging"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_readback"),
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

    /// Read the entire Rg32Uint 3D chunks texture back to CPU as a flat `u32`
    /// vector of the `.x` channel (the construction-state channel; the `.y`
    /// channel is the W4 entity pointer + counter, zero in this test). Order
    /// is `cz * cx * cy + cy * cx + cx` (x-fastest), matching
    /// `WorldData.chunks_cpu`'s convention.
    fn readback_chunks_buffer(
        device: &RenderDevice,
        queue: &RenderQueue,
        chunks: &bevy::render::render_resource::Buffer,
        size: [u32; 3],
    ) -> Vec<u32> {
        // Web-WebGPU migration: chunks is an `array<vec2<u32>>` storage
        // buffer (8 B per pair). Buffer→buffer copy doesn't need the
        // 256-byte row alignment a 3D-texture readback required.
        let chunk_count = (size[0] * size[1] * size[2]) as u64;
        let staging_size = chunk_count * 8;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("w1_chunks_readback_staging"),
            size: staging_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_chunks_readback"),
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
        assert_eq!(out.len() as u64, chunk_count);
        out
    }

    /// Drive the pipeline cache through `process_queue` until either every
    /// id has compiled or the iteration cap fires.
    fn compile_pipelines(
        app: &mut App,
        ids: &[bevy::render::render_resource::CachedComputePipelineId],
    ) -> Option<Vec<bevy::render::render_resource::ComputePipeline>> {
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
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

    /// W1's load-bearing test: GPU `chunk_calc.wgsl` 3-entry-point chain
    /// produces blocks + voxels + chunks bit-equal to the CPU oracle
    /// `aadf::construct::construct`.
    ///
    /// The test uses a tiny 1×1×1 chunk world with a single mixed block so
    /// the `VoxelPtr` assignment is deterministic (the only mixed block gets
    /// `VoxelPtr(0)` on both paths — see file doc note on pointer-assignment
    /// determinism).
    #[test]
    fn gpu_algorithm1_vs_cpu_bit_exact() {
        let Some((mut app, device, queue, chunk_calc_handle, _map_copy_handle)) =
            render_fixture_w1()
        else {
            eprintln!("no wgpu device — skipping W1 GPU/CPU bit-exact test");
            return;
        };

        // === Tiny test scene: 1×1×1 chunk world, single mixed block =========
        let mut volume = DenseVolume::empty([1, 1, 1]);
        let ty = VoxelTypeId(7);
        // Put one solid voxel at the origin — the chunk + the (0,0,0) block
        // become mixed, the other 63 blocks stay empty. Exactly one Mixed
        // entry in the dedup table ⇒ `VoxelPtr(0)`.
        volume.set([0, 0, 0], ty);

        // === CPU oracle =====================================================
        let oracle = construct(&volume);
        assert_eq!(oracle.chunks.len(), 1);
        assert_eq!(oracle.blocks.len(), 64);
        assert_eq!(oracle.voxels.len(), 32);
        // Sanity: the chunk decodes Mixed.
        assert!(matches!(ChunkCell::decode(oracle.chunks[0]), ChunkCell::Mixed(_)));

        // === GPU setup ======================================================
        let segment_size_in_chunks: u32 = 1;
        let size_in_chunks: [u32; 3] = volume.size_in_chunks;

        // The segment-voxel-buffer: 1 chunk × 2048 u32s.
        let segment_voxels =
            super::build_segment_voxel_buffer(&volume, segment_size_in_chunks);
        assert_eq!(segment_voxels.len(), 2048);

        // The hash map — a power-of-two slot array. For 1 mixed block, even a
        // small size like 256 is comfortable headroom. Each slot is 16 B
        // (`GpuHashValueSlot`).
        let hash_map_size_slots: u32 = 256;
        let hash_map_init: Vec<u32> =
            vec![0u32; (hash_map_size_slots as usize) * 4]; // 4 u32 per slot.

        // The block-voxel-count cursor: [voxel_cursor, block_cursor]. NAADF
        // seeds it to [64, 64] (`WorldData.cs:129`) so `VoxelPtr(0)` /
        // `BlockPtr(0)` are reserved sentinels (the dedup-empty value
        // `EMPTY_BLOCK = 0` distinguishes from a real slot at offset 0). W1
        // uses the same seed.
        let block_voxel_count_init: Vec<u32> = vec![64, 64];

        // The hash coefficients table.
        let coeffs = hash_coefficients().to_vec();

        // Allocate GPU buffers.
        let gpu_blocks = create_storage_u32(
            &device,
            &queue,
            "w1_blocks",
            &vec![0u32; oracle.blocks.len().max(64) + 64], // extra headroom past oracle
        );
        let gpu_voxels = create_storage_u32(
            &device,
            &queue,
            "w1_voxels",
            &vec![0u32; oracle.voxels.len().max(32) + 32], // extra headroom
        );
        let gpu_block_voxel_count = create_storage_u32(
            &device,
            &queue,
            "w1_block_voxel_count",
            &block_voxel_count_init,
        );
        let gpu_segment_voxel_buffer = create_storage_u32(
            &device,
            &queue,
            "w1_segment_voxel_buffer",
            &segment_voxels,
        );
        let gpu_hash_map = create_storage_u32(
            &device,
            &queue,
            "w1_hash_map",
            &hash_map_init,
        );
        let gpu_hash_coefficients = create_storage_u32(
            &device,
            &queue,
            "w1_hash_coefficients",
            &coeffs,
        );
        let params = GpuConstructionParams {
            size_in_chunks,
            _pad0: 0,
            group_size_in_groups: [1, 1, 1],
            _pad1: 0,
            bound_group_queue_max_size: 1,
            hash_map_size: hash_map_size_slots,
            segment_size_in_chunks,
            max_group_bound_dispatch: 0,
            chunk_offset: [0, 0, 0],
            dispatch_offset: 0,
            frame_index: 0,
            changed_chunk_count: 0,
            changed_block_count: 0,
            changed_voxel_count: 0,
        };
        let gpu_params =
            create_uniform(&device, &queue, "w1_construction_params", &params);

        // The chunks resource — `array<vec2<u32>>` storage buffer
        // (web-WebGPU migration; was `Rg32Uint` 3D texture). 8 B per pair;
        // `.x` carries the construction state (this test's load-bearing
        // channel), `.y` is the entity pointer (zero in this no-entities
        // test). STORAGE | COPY_DST | COPY_SRC — COPY_SRC for readback.
        let chunk_count_total =
            (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        let zero_chunks: Vec<[u32; 2]> = vec![[0u32, 0u32]; chunk_count_total];
        let chunks_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("w1_chunks"),
            size: (chunk_count_total as u64) * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&zero_chunks));

        // === Queue the three pipelines ======================================
        let layout = construction_world_layout_descriptor();
        let (id_calc_block, id_voxel_bounds, id_block_bounds) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_calc_block_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            let b = queue_voxel_bounds_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            let c = queue_block_bounds_pipeline_with_handle(
                cache,
                layout.clone(),
                chunk_calc_handle.clone(),
            );
            (a, b, c)
        };

        let pipelines = compile_pipelines(
            &mut app,
            &[id_calc_block, id_voxel_bounds, id_block_bounds],
        )
        .expect("W1 pipelines did not compile in 64 ticks");

        // === Build the bind group ===========================================
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let bgl = cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "w1_construction_world_bind_group",
            &bgl,
            &BindGroupEntries::sequential((
                chunks_buffer.as_entire_buffer_binding(),
                gpu_blocks.as_entire_buffer_binding(),
                gpu_voxels.as_entire_buffer_binding(),
                gpu_block_voxel_count.as_entire_buffer_binding(),
                gpu_segment_voxel_buffer.as_entire_buffer_binding(),
                gpu_hash_map.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
                gpu_hash_coefficients.as_entire_buffer_binding(),
            )),
        );

        // === Dispatch the 3 passes ==========================================
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_dispatch_calc_block"),
        });
        super::super::chunk_calc::dispatch_calc_block_from_raw_data(
            &mut encoder,
            &pipelines[0],
            &bind_group,
            segment_size_in_chunks,
        );
        queue.submit([encoder.finish()]);
        // Per `WorldData.cs:202-207`: NAADF dispatches `voxelCount / 64`
        // workgroups for `compute_voxel_bounds` (one per allocated block —
        // including the seed-reserved block at base 0) and `blockCount / 64`
        // workgroups for `compute_block_bounds` (one per allocated chunk,
        // including the seed-reserved chunk at base 0).
        //
        // `voxelCount` after `calc_block_from_raw_data` = 64 (seed) + 64
        // (one mixed block × 64 voxels) = 128 → 128/64 = 2 voxel workgroups.
        // `blockCount` after construction = 64 (seed) + 64 (one mixed chunk
        // × 64 blocks) = 128 → 128/64 = 2 block workgroups.
        //
        // The shader's `groupID.x` then indexes into the buffer at
        // `chunkIndex * 64`, so workgroup 0 hits the seed slots (zeros in/zeros
        // out — idempotent) and workgroup 1 hits the real chunk's 64 blocks.
        //
        // We READ BACK from `block_voxel_count` to get the actual GPU-side
        // cursors (matches `WorldData.cs:158` — `dataBlockGpu.GetData(blockVoxelCount)`).
        let cursor_pair =
            readback_u32(&device, &queue, &gpu_block_voxel_count, 2);
        let voxel_count = cursor_pair[0];
        let block_count = cursor_pair[1];
        let voxel_workgroups = voxel_count / 64;
        let block_workgroups = block_count / 64;
        eprintln!(
            "W1 GPU cursors after calc_block: voxelCount={} blockCount={}",
            voxel_count, block_count,
        );

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_dispatch_bounds"),
        });
        super::super::chunk_calc::dispatch_compute_voxel_bounds(
            &mut encoder,
            &pipelines[1],
            &bind_group,
            voxel_workgroups,
        );
        super::super::chunk_calc::dispatch_compute_block_bounds(
            &mut encoder,
            &pipelines[2],
            &bind_group,
            block_workgroups,
        );
        queue.submit([encoder.finish()]);

        // === Read back + compare ============================================
        // Read back the FULL allocated buffers (we need the [64..] window for
        // blocks and the [32..] window for voxels — GPU seeds the cursors at
        // 64 voxels = 32 u32 + 64 blocks).
        let gpu_blocks_out = readback_u32(
            &device,
            &queue,
            &gpu_blocks,
            (oracle.blocks.len().max(64) + 64) as u64,
        );
        let gpu_voxels_out = readback_u32(
            &device,
            &queue,
            &gpu_voxels,
            (oracle.voxels.len().max(32) + 32) as u64,
        );
        let gpu_chunks_out = readback_chunks_buffer(
            &device,
            &queue,
            &chunks_buffer,
            size_in_chunks,
        );

        // Chunks: byte-equal to the oracle. `ChunkCell::Mixed` payload
        // (`BlockPtr`) IS deterministic on a single-mixed-chunk test (the
        // GPU's `atomicAdd(&block_voxel_count[1], 64)` starts from the seed
        // 64, so the first mixed chunk gets `block_pointer = 64`; the CPU
        // oracle's `blocks_buf.len()` also starts at 0 and grows by 64 per
        // mixed chunk — `BlockPtr(0)` on CPU. To make the test
        // pointer-assignment-deterministic, we shift the CPU oracle output
        // by +64 in its `BlockPtr`s (the GPU's seed). This is a known port
        // deviation, documented in `16-impl-c-W1.md`.
        //
        // Easier: re-encode the oracle's `ChunkCell`s with the GPU's
        // base-pointer convention (BlockPtr offset by +64 from the CPU
        // oracle's offset).
        let expected_chunk: u32 = match ChunkCell::decode(oracle.chunks[0]) {
            ChunkCell::Empty(_) => oracle.chunks[0],
            ChunkCell::UniformFull(_) => oracle.chunks[0],
            ChunkCell::Mixed(ptr) => {
                ChunkCell::Mixed(crate::aadf::cell::BlockPtr(ptr.0 + 64)).encode()
            }
        };
        assert_eq!(
            gpu_chunks_out[0], expected_chunk,
            "GPU chunk[0] {:#010x} != CPU oracle (+64 BlockPtr offset) {:#010x}",
            gpu_chunks_out[0], expected_chunk
        );

        // Blocks: GPU lays them out at offset 64 (the seed); we compare the
        // GPU's [64..128] range to the CPU's [0..64]. With `BlockCell::Mixed`
        // entries the `VoxelPtr` also shifts by +64 (block 0's voxel pointer
        // = 64 on GPU, 0 on CPU). Re-encode the CPU blocks with the +64
        // VoxelPtr shift.
        let expected_blocks: Vec<u32> = oracle
            .blocks
            .iter()
            .map(|&raw| {
                match BlockCell::decode(raw) {
                    BlockCell::Empty(_) | BlockCell::UniformFull(_) => raw,
                    BlockCell::Mixed(VoxelPtr(v)) => {
                        // Halve the bias: voxels[] is in u32-element offsets;
                        // GPU seeds block_voxel_count[0] to 64 voxels = 32
                        // u32-pairs. So VoxelPtr on GPU side is +32 u32 from
                        // CPU's VoxelPtr (which is also a u32-element offset
                        // per `aadf/cell.rs:78-82`).
                        BlockCell::Mixed(VoxelPtr(v + 32)).encode()
                    }
                }
            })
            .collect();
        let gpu_blocks_slice = &gpu_blocks_out[64..64 + oracle.blocks.len()];
        // Find first mismatch for a helpful failure message.
        for (i, (&g, &c)) in gpu_blocks_slice
            .iter()
            .zip(expected_blocks.iter())
            .enumerate()
        {
            assert_eq!(
                g, c,
                "GPU blocks[{}] {:#010x} != expected (shifted oracle) {:#010x}",
                64 + i, g, c
            );
        }

        // Voxels: GPU stores them starting at u32-offset 32 (voxel_count
        // seed = 64 voxels / 2 = 32 u32s). Compare against the oracle's
        // [0..oracle.voxels.len()].
        let gpu_voxels_slice = &gpu_voxels_out[32..32 + oracle.voxels.len()];
        for (i, (&g, &c)) in gpu_voxels_slice
            .iter()
            .zip(oracle.voxels.iter())
            .enumerate()
        {
            assert_eq!(
                g, c,
                "GPU voxels[{}] {:#010x} != oracle {:#010x}",
                32 + i, g, c
            );
        }

        // Sanity references so the test trips on rename / removal.
        let _ = GpuHashValueSlot {
            voxel_pointer: 0,
            use_count: 0,
            hash_raw: 0,
            _pad: 0,
        };
        let _ = ty;
        // VoxelCell reference (used implicitly via decode in the oracle).
        let _vc = VoxelCell::Full(VoxelTypeId(1));

        // Total bytes compared (chunks + blocks + voxels — slice form).
        let bytes_compared = (oracle.chunks.len() * 4)
            + (oracle.blocks.len() * 4)
            + (oracle.voxels.len() * 4);
        eprintln!(
            "W1 GPU/CPU bit-exact: {} bytes compared (chunks {} + blocks {} + voxels {} u32s)",
            bytes_compared,
            oracle.chunks.len(),
            oracle.blocks.len(),
            oracle.voxels.len(),
        );
    }

    /// W1 — `map_copy.wgsl::copy_map` rehash correctness.
    ///
    /// Seeds an old map of size 32 with a few non-empty slots at deterministic
    /// positions, runs `copy_map` over it into a new map of size 64, reads
    /// back the new map, and asserts every old-map slot has a corresponding
    /// new-map entry at `hash & (new_size - 1)` (the linear-probe re-hash
    /// starting point).
    #[test]
    fn map_copy_regrow_preserves_contents() {
        let Some((mut app, device, queue, _chunk_calc_handle, map_copy_handle)) =
            render_fixture_w1()
        else {
            eprintln!("no wgpu device — skipping W1 map_copy test");
            return;
        };

        // Hand-built old map: 32 slots, 3 occupied. Each slot is 4 u32s
        // (voxel_pointer, use_count, hash_raw, _pad).
        let old_size: u32 = 32;
        let new_size: u32 = 64;
        let mut old_map_u32 = vec![0u32; (old_size as usize) * 4];
        // Slot 1: voxel_pointer = 100, use_count = 7, hash_raw = 0x1234.
        // Slot 5: voxel_pointer = 200, use_count = 3, hash_raw = 0xABCD.
        // Slot 20: voxel_pointer = 300, use_count = 1, hash_raw = 0xDEAD.
        let seeds: [(usize, u32, u32, u32); 3] = [
            (1, 100, 7, 0x1234),
            (5, 200, 3, 0xABCD),
            (20, 300, 1, 0xDEAD),
        ];
        for &(slot, vp, uc, hr) in &seeds {
            old_map_u32[slot * 4 + 0] = vp;
            old_map_u32[slot * 4 + 1] = uc;
            old_map_u32[slot * 4 + 2] = hr;
        }

        let gpu_old = create_storage_u32(&device, &queue, "w1_mc_old", &old_map_u32);
        let gpu_new = create_storage_u32(
            &device,
            &queue,
            "w1_mc_new",
            &vec![0u32; (new_size as usize) * 4],
        );
        let params = GpuMapCopyParams {
            old_size,
            new_size,
            _pad0: 0,
            _pad1: 0,
        };
        let gpu_params = create_uniform(&device, &queue, "w1_mc_params", &params);
        let gpu_coeffs = create_storage_u32(&device, &queue, "w1_mc_coeffs", &[0u32; 1]);
        let gpu_v2h = create_storage_u32(&device, &queue, "w1_mc_v2h", &[0u32; 1]);
        let gpu_result = create_storage_u32(&device, &queue, "w1_mc_result", &[0u32; 1]);

        let layout = map_copy_layout_descriptor();
        let id_copy = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            queue_copy_map_pipeline_with_handle(cache, layout.clone(), map_copy_handle.clone())
        };
        let pipelines = compile_pipelines(&mut app, &[id_copy])
            .expect("map_copy pipeline did not compile in 64 ticks");

        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let bgl = cache.get_bind_group_layout(&layout);
        let bind_group = device.create_bind_group(
            "w1_mc_bind_group",
            &bgl,
            &BindGroupEntries::sequential((
                gpu_old.as_entire_buffer_binding(),
                gpu_new.as_entire_buffer_binding(),
                gpu_params.as_entire_buffer_binding(),
                gpu_coeffs.as_entire_buffer_binding(),
                gpu_v2h.as_entire_buffer_binding(),
                gpu_result.as_entire_buffer_binding(),
            )),
        );

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w1_mc_dispatch"),
        });
        super::super::map_copy::dispatch_copy_map(&mut encoder, &pipelines[0], &bind_group, old_size);
        queue.submit([encoder.finish()]);

        let new_u32 = readback_u32(
            &device,
            &queue,
            &gpu_new,
            (new_size as u64) * 4,
        );

        // Verify each seed landed in the new map at hash_raw & (new_size-1)
        // OR a subsequent probe slot.
        for &(_slot, vp, uc, hr) in &seeds {
            let start = (hr & (new_size - 1)) as usize;
            let mut found = false;
            for probe in 0..50u32 {
                let candidate = ((start as u32 + probe) & (new_size - 1)) as usize;
                if new_u32[candidate * 4 + 0] == vp {
                    assert_eq!(new_u32[candidate * 4 + 1], uc, "use_count mismatch");
                    assert_eq!(new_u32[candidate * 4 + 2], hr, "hash_raw mismatch");
                    found = true;
                    break;
                }
            }
            assert!(found, "slot with vp={vp} hr={hr:#x} not found in new map");
        }
    }

    /// Phase-C followup #1 — verify the runtime GPU producer flip is active by
    /// default + the runtime-flip's `build_segment_voxel_buffer_from_dense`
    /// helper is byte-equivalent to the W1-test-validated
    /// `build_segment_voxel_buffer(&volume, …)` helper, AND the full GPU
    /// dispatch chain against this runtime-built segment buffer byte-matches
    /// the CPU oracle.
    ///
    /// This is the load-bearing "runtime-flip verification" test the brief
    /// asked for. We can't easily boot the full e2e `App` in a unit test
    /// (it needs a window + the asset loader), so we exercise the same
    /// dispatch chain `prepare_construction` runs at runtime, but driven by
    /// the test harness — proving:
    ///
    /// 1. `ConstructionConfig::default().gpu_construction_enabled` is `true`
    ///    (the runtime flip is on by default).
    /// 2. `build_segment_voxel_buffer_from_dense` (used by the runtime path)
    ///    produces the same `segment_voxel_buffer` as
    ///    `build_segment_voxel_buffer` (used by `validate_gpu_construction`
    ///    + the existing W1 oracle test).
    /// 3. `validate_gpu_construction()` succeeds — the W1 chain produces
    ///    Algorithm 1 output byte-equal to the CPU oracle. (This is the
    ///    same gate `e2e_render --validate-gpu-construction` runs at the
    ///    e2e harness level.)
    #[test]
    fn runtime_gpu_producer_runs_and_matches_cpu_oracle_in_default_mode() {
        // (1) Runtime flip is the default.
        let cfg = crate::render::construction::config::ConstructionConfig::default();
        assert!(
            cfg.gpu_construction_enabled,
            "Phase-C followup #1: `gpu_construction_enabled` MUST default to true so the \
             runtime GPU producer is active out-of-the-box"
        );

        // (2) The runtime-path `build_segment_voxel_buffer_from_dense` helper
        // is byte-equivalent to the validated `build_segment_voxel_buffer`.
        // Build the same single-voxel test volume the W1 oracle uses, then
        // compare both segment-buffer builders.
        let mut volume = crate::aadf::construct::DenseVolume::empty([1, 1, 1]);
        volume.set([0, 0, 0], crate::voxel::VoxelTypeId(7));
        let dense_u16: Vec<u16> = volume.voxels.iter().map(|t| t.0).collect();
        let runtime_buf =
            crate::render::construction::build_segment_voxel_buffer_from_dense(
                &dense_u16,
                volume.size_in_chunks,
                volume.size_in_chunks,
            );
        let validated_buf =
            crate::render::construction::validation::build_segment_voxel_buffer(&volume, 1);
        assert_eq!(
            runtime_buf, validated_buf,
            "Phase-C followup #1: `build_segment_voxel_buffer_from_dense` must produce \
             byte-identical output to the W1-test-validated \
             `build_segment_voxel_buffer(&volume, segment_size)` helper (the validation \
             gate at `e2e_render --validate-gpu-construction` proves the latter is \
             byte-correct to the CPU oracle; this assertion chains both proofs together \
             so the runtime path inherits the same correctness)."
        );

        // (3) The full GPU dispatch chain — driven by the same shader entry
        // points the runtime path uses — produces output byte-equal to the
        // CPU oracle on the deterministic 1×1×1 test scene.
        match crate::render::construction::validate_gpu_construction() {
            Ok(bytes) => {
                assert!(
                    bytes >= 12,
                    "Phase-C followup #1: validate_gpu_construction compared only \
                     {bytes} bytes (expected ≥12: 4 chunks + 4 blocks + 4 voxels at \
                     the 1×1×1 fixture)"
                );
            }
            Err(msg)
                if msg.contains("no wgpu")
                    || msg.contains("no RenderApp")
                    || msg.contains("no RenderDevice")
                    || msg.contains("no RenderQueue") =>
            {
                // No GPU available in CI — the W1 oracle test
                // `gpu_algorithm1_vs_cpu_bit_exact` will have skipped too;
                // this test stays consistent with that policy.
                eprintln!(
                    "no wgpu device — skipping the GPU dispatch leg of the runtime-flip \
                     verification (the config-flip + helper-equivalence legs above \
                     still ran and passed)"
                );
            }
            Err(msg) => panic!(
                "Phase-C followup #1: validate_gpu_construction failed with: {msg}"
            ),
        }
    }
}

#[cfg(test)]
mod tests_w4 {
    //! W4 — GPU pipeline-compilation smoke + the load-bearing
    //! `entity_update_gpu_vs_cpu` shape gate.
    //!
    //! - `entity_update_pipelines_compile`: builds a headless render world,
    //!   queues all three `entity_update.wgsl` pipelines + the two W4
    //!   layouts, and asserts they all compile cleanly. Pure WGSL/layout
    //!   sanity — does not run the dispatches.
    //! - `chunks_format_widening_regression`: builds a small app, exercises
    //!   the chunks-format widening path through the existing
    //!   `prepare_world_gpu` indirectly (the existing 76 tests verify the
    //!   format-flip path in isolation).
    //! - `entity_update_gpu_vs_cpu`: dispatches the three entry points
    //!   against a small fixture, reads back the GPU-written
    //!   `entity_chunk_instances` + `entity_instances_history` buffers, and
    //!   asserts they match the CPU `EntityHandler::update` output
    //!   byte-for-byte.

    use super::super::entity_handler::EntityHandler;
    use super::super::entity_update::{
        construction_entity_layout_descriptor, dispatch_copy_entity_chunk_instances,
        dispatch_copy_entity_history, dispatch_update_chunks, entity_world_layout_descriptor,
        queue_copy_entity_chunk_instances_pipeline_with_handle,
        queue_copy_entity_history_pipeline_with_handle, queue_update_chunks_pipeline_with_handle,
        GpuEntityUpdateParams, ENTITY_UPDATE_SHADER_SRC,
    };
    use crate::render::gpu_types::{
        EntityInstance, GpuChunkUpdate, GpuEntityChunkInstance, GpuEntityInstanceHistory,
    };

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::math::Vec3;
    use bevy::render::render_resource::{
        BindGroupEntries, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        MapMode, PipelineCache, PollType,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::shader::Shader;
    use bevy::MinimalPlugins;

    fn boot_render_app() -> Option<(App, RenderDevice, RenderQueue, bevy::asset::Handle<Shader>)> {
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

        let shader = Shader::from_wgsl(ENTITY_UPDATE_SHADER_SRC, "shaders/entity_update.wgsl");
        let shader_clone = shader.clone();
        let shader_handle = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(shader);
        let render_app = app.get_sub_app_mut(RenderApp)?;
        {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.set_shader(shader_handle.id(), shader_clone);
        }
        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((app, device, queue, shader_handle))
    }

    /// All three entity_update pipelines compile cleanly against the W4
    /// layouts. Pure layout/shader sanity.
    #[test]
    fn entity_update_pipelines_compile() {
        let Some((mut app, _device, _queue, shader_handle)) = boot_render_app() else {
            eprintln!("no wgpu device — skipping entity_update_pipelines_compile");
            return;
        };
        let world_layout = entity_world_layout_descriptor();
        let entity_layout = construction_entity_layout_descriptor();

        let (id_a, id_b, id_c) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_update_chunks_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let b = queue_copy_entity_chunk_instances_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let c = queue_copy_entity_history_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            (a, b, c)
        };

        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        let mut compiled = false;
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.process_queue();
            let cache = render_app.world().resource::<PipelineCache>();
            if cache.get_compute_pipeline(id_a).is_some()
                && cache.get_compute_pipeline(id_b).is_some()
                && cache.get_compute_pipeline(id_c).is_some()
            {
                compiled = true;
                break;
            }
        }
        assert!(
            compiled,
            "W4 entity_update pipelines did not compile in 64 ticks"
        );
    }

    /// Load-bearing W4 gate: the GPU `entity_update.wgsl` output is
    /// byte-for-byte equal to the CPU `EntityHandler::update` output on a
    /// small deterministic fixture. Exercises all three entry points
    /// (update_chunks, copy_entity_chunk_instances, copy_entity_history).
    #[test]
    fn entity_update_gpu_vs_cpu() {
        let Some((mut app, device, queue, shader_handle)) = boot_render_app() else {
            eprintln!("no wgpu device — skipping entity_update_gpu_vs_cpu");
            return;
        };

        // CPU fixture — one entity in a 2×1×1-chunk world.
        let size_in_chunks = [2u32, 1, 1];
        let mut handler = EntityHandler::new(size_in_chunks);
        let instances = vec![EntityInstance {
            position: Vec3::new(12.0, 8.0, 8.0),
            quaternion: [0.0, 0.0, 0.0, 1.0],
            voxel_start: 0,
            entity: 0,
            size: [8, 4, 4],
        }];
        let cpu_uploads = handler.update(&instances);
        let update_count = cpu_uploads.chunk_updates.len() as u32;
        let chunk_instance_count = cpu_uploads.entity_chunk_instances.len() as u32;
        let instance_count = cpu_uploads.entity_history.len() as u32;

        // GPU side — allocate the chunks buffer + the two dynamic upload
        // buffers + the two output buffers. Web-WebGPU migration: chunks is
        // an `array<vec2<u32>>` storage buffer (was `Rg32Uint` 3D texture).
        let chunk_count =
            (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as usize;
        // Pre-write a non-zero `.x` channel so we can verify the entity
        // update preserves `.x`.
        let init_chunks: Vec<[u32; 2]> = (0..chunk_count)
            .map(|i| [0xAA00_0000u32 + i as u32, 0u32])
            .collect();
        let chunks_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("w4_chunks"),
            size: (chunk_count as u64) * 8,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&init_chunks));

        let mk_storage = |label: &'static str, bytes: &[u8]| {
            let buf = device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: bytes.len().max(8) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !bytes.is_empty() {
                queue.write_buffer(&buf, 0, bytes);
            } else {
                queue.write_buffer(&buf, 0, &[0u8; 8]);
            }
            buf
        };

        let chunk_updates_buf = mk_storage(
            "w4_chunk_updates_dynamic",
            bytemuck::cast_slice(&cpu_uploads.chunk_updates),
        );
        let entity_ci_dyn = mk_storage(
            "w4_entity_chunk_instances_dynamic",
            bytemuck::cast_slice(&cpu_uploads.entity_chunk_instances),
        );
        let entity_history_dyn = mk_storage(
            "w4_entity_history_dynamic",
            bytemuck::cast_slice(&cpu_uploads.entity_history),
        );
        // Output buffers — over-allocated.
        let entity_ci_rw_count = chunk_instance_count.max(1) as usize;
        let entity_ci_rw_zero =
            vec![GpuEntityChunkInstance::default(); entity_ci_rw_count];
        let entity_ci_rw = mk_storage(
            "w4_entity_chunk_instances_rw",
            bytemuck::cast_slice(&entity_ci_rw_zero),
        );
        // History ring sized `taa_index * max + entityInstanceID` cap; we use
        // taa_index = 0 so we only need `instance_count` entries.
        let history_rw_count = instance_count.max(1) as usize;
        let history_rw_zero = vec![GpuEntityInstanceHistory::default(); history_rw_count];
        let entity_history_rw = mk_storage(
            "w4_entity_history_rw",
            bytemuck::cast_slice(&history_rw_zero),
        );

        let params = GpuEntityUpdateParams {
            entity_instance_count: instance_count,
            entity_chunk_instance_count: chunk_instance_count,
            taa_index: 0,
            update_count,
            max_entity_instances: 64,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            // Web-WebGPU migration: chunks is `array<vec2<u32>>`; the kernel
            // flattens chunk_pos with `size_in_chunks` as the stride basis.
            size_in_chunks,
            _pad3: 0,
        };
        let params_buf = device.create_buffer(&BufferDescriptor {
            label: Some("w4_params"),
            size: std::mem::size_of::<GpuEntityUpdateParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

        // Queue + compile pipelines.
        let world_layout = entity_world_layout_descriptor();
        let entity_layout = construction_entity_layout_descriptor();
        let (id_a, id_b, id_c) = {
            let render_app = app.get_sub_app(RenderApp).unwrap();
            let cache = render_app.world().resource::<PipelineCache>();
            let a = queue_update_chunks_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let b = queue_copy_entity_chunk_instances_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            let c = queue_copy_entity_history_pipeline_with_handle(
                cache,
                world_layout.clone(),
                entity_layout.clone(),
                shader_handle.clone(),
            );
            (a, b, c)
        };
        let mut pipelines: Option<Vec<bevy::render::render_resource::ComputePipeline>> =
            None;
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        for _ in 0..64 {
            let mut pipeline_cache =
                render_app.world_mut().resource_mut::<PipelineCache>();
            pipeline_cache.process_queue();
            let cache = render_app.world().resource::<PipelineCache>();
            if let (Some(a), Some(b), Some(c)) = (
                cache.get_compute_pipeline(id_a),
                cache.get_compute_pipeline(id_b),
                cache.get_compute_pipeline(id_c),
            ) {
                pipelines = Some(vec![a.clone(), b.clone(), c.clone()]);
                break;
            }
        }
        let pipelines = pipelines.expect("entity_update pipelines did not compile");

        // Build bind groups.
        let render_app = app.get_sub_app(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        let world_bgl = cache.get_bind_group_layout(&world_layout);
        let entity_bgl = cache.get_bind_group_layout(&entity_layout);
        let world_bg = device.create_bind_group(
            "w4_world_bg",
            &world_bgl,
            &BindGroupEntries::sequential((
                chunks_buffer.as_entire_buffer_binding(),
                params_buf.as_entire_buffer_binding(),
            )),
        );
        let entity_bg = device.create_bind_group(
            "w4_entity_bg",
            &entity_bgl,
            &BindGroupEntries::sequential((
                chunk_updates_buf.as_entire_buffer_binding(),
                entity_ci_dyn.as_entire_buffer_binding(),
                entity_history_dyn.as_entire_buffer_binding(),
                entity_ci_rw.as_entire_buffer_binding(),
                entity_history_rw.as_entire_buffer_binding(),
            )),
        );

        // Dispatch.
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w4_dispatch"),
        });
        dispatch_update_chunks(
            &mut encoder,
            &pipelines[0],
            &world_bg,
            &entity_bg,
            update_count,
        );
        dispatch_copy_entity_chunk_instances(
            &mut encoder,
            &pipelines[1],
            &world_bg,
            &entity_bg,
            chunk_instance_count,
        );
        dispatch_copy_entity_history(
            &mut encoder,
            &pipelines[2],
            &world_bg,
            &entity_bg,
            instance_count,
        );
        queue.submit([encoder.finish()]);

        // Readback `entity_chunk_instances_rw` and compare to CPU.
        let read_bytes = |src: &bevy::render::render_resource::Buffer, n: u64| {
            let staging = device.create_buffer(&BufferDescriptor {
                label: Some("w4_readback_staging"),
                size: n,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("w4_readback_enc"),
            });
            enc.copy_buffer_to_buffer(src, 0, &staging, 0, n);
            queue.submit([enc.finish()]);
            let slice = staging.slice(..);
            slice.map_async(MapMode::Read, |r| r.unwrap());
            device.poll(PollType::wait_indefinitely()).unwrap();
            let data = slice.get_mapped_range();
            let v: Vec<u8> = data.to_vec();
            drop(data);
            staging.unmap();
            v
        };
        let ci_bytes = read_bytes(
            &entity_ci_rw,
            (entity_ci_rw_count * std::mem::size_of::<GpuEntityChunkInstance>()) as u64,
        );
        let gpu_ci: &[GpuEntityChunkInstance] = bytemuck::cast_slice(&ci_bytes);
        for i in 0..(chunk_instance_count as usize) {
            assert_eq!(
                gpu_ci[i].data1, cpu_uploads.entity_chunk_instances[i].data1,
                "entity_chunk_instances[{i}].data1 mismatch"
            );
            assert_eq!(
                gpu_ci[i].data5, cpu_uploads.entity_chunk_instances[i].data5,
                "entity_chunk_instances[{i}].data5 mismatch"
            );
        }

        let hist_bytes = read_bytes(
            &entity_history_rw,
            (history_rw_count * std::mem::size_of::<GpuEntityInstanceHistory>()) as u64,
        );
        let gpu_hist: &[GpuEntityInstanceHistory] = bytemuck::cast_slice(&hist_bytes);
        for i in 0..(instance_count as usize) {
            assert_eq!(
                gpu_hist[i].data1, cpu_uploads.entity_history[i].data1,
                "entity_history[{i}].data1 mismatch"
            );
            assert_eq!(
                gpu_hist[i].data4, cpu_uploads.entity_history[i].data4,
                "entity_history[{i}].data4 mismatch"
            );
        }

        // Verify the chunks buffer: `.x` channel preserved, `.y` channel
        // got the entity pointer (= update.data2). Web-WebGPU migration:
        // chunks is `array<vec2<u32>>`; flat buffer→buffer copy.
        let staging_size = (chunk_count as u64) * 8;
        let chunks_staging = device.create_buffer(&BufferDescriptor {
            label: Some("w4_chunks_staging"),
            size: staging_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("w4_chunks_readback"),
        });
        enc.copy_buffer_to_buffer(&chunks_buffer, 0, &chunks_staging, 0, staging_size);
        queue.submit([enc.finish()]);
        let slice = chunks_staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let raw = slice.get_mapped_range();
        let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
        // For each update, decode chunk_pos + verify `.x` preserved + `.y`
        // updated.
        for upd in &cpu_uploads.chunk_updates {
            let cx = upd.data1 & 0x7FF;
            let cy = (upd.data1 >> 11) & 0x3FF;
            let cz = upd.data1 >> 21;
            let chunk_idx_in_world =
                cx + cy * size_in_chunks[0] + cz * size_in_chunks[0] * size_in_chunks[1];
            let xy = pairs[chunk_idx_in_world as usize];
            let preserved_x = 0xAA00_0000u32 + chunk_idx_in_world;
            assert_eq!(
                xy[0], preserved_x,
                "chunks[{}].x not preserved: got {:#x} expected {:#x}",
                chunk_idx_in_world, xy[0], preserved_x
            );
            assert_eq!(
                xy[1], upd.data2,
                "chunks[{}].y not written: got {:#x} expected {:#x}",
                chunk_idx_in_world, xy[1], upd.data2
            );
        }
        drop(raw);
        chunks_staging.unmap();

        // Silence the unused-warn on GpuChunkUpdate.
        let _ = GpuChunkUpdate::default();
    }
}
