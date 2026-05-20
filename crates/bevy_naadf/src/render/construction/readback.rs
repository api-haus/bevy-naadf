//! Phase-C — cross-frame GPU→CPU readback state machine for the
//! `WorldData::{chunks,blocks,voxels}_cpu` mirror.
//!
//! Owns:
//!   - [`ReadbackStage`] — the 4-stage cursor / full-set / done state enum
//!     (`vox-gpu-rewrite W5.3-fix Stage 5`, `web-vox-async-loading Q3`).
//!   - [`CpuMirrorReadback`] — the state aggregate stored as a field of
//!     [`super::ConstructionGpu`].
//!   - [`READBACK_STALL_BUDGET_FRAMES`] — the 600-frame budget per
//!     `feedback-e2e-gates-must-fail-fast`.
//!   - [`populate_cpu_mirror_from_gpu_producer`] — the `ExtractSchedule`
//!     system that ticks the state machine.
//!
//! Target-agnostic — works identically on native (where `Device::poll` is a
//! real non-blocking poll) and WebGPU (where `poll` is a no-op but the JS
//! `mapAsync` promise resolves on subsequent event-loop ticks).

use bevy::prelude::*;
use bevy::render::render_resource::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use super::ConstructionGpu;

/// Frame budget for the cross-frame CPU mirror readback state machine. If
/// the state machine does not progress past its current stage within this
/// many `populate_cpu_mirror_from_gpu_producer` ticks the system emits a
/// diagnostic and force-advances to `Done`. 600 frames ≈ 10s @ 60fps —
/// per `feedback-e2e-gates-must-fail-fast.md`.
pub const READBACK_STALL_BUDGET_FRAMES: u32 = 600;

/// State machine stage for the cross-frame CPU mirror readback (Q3).
///
/// The state machine runs once per `.vox` install (gated on
/// `gpu_producer_has_run && model_data.is_some() && !cpu_mirror_populated`).
/// Once it reaches `Done` it stays there for the lifetime of the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ReadbackStage {
    /// Initial state — gate not yet satisfied (or just satisfied this frame
    /// and the cursor copy hasn't been issued yet).
    #[default]
    NotStarted,
    /// Cursor copy + `map_async` issued; waiting for the callback's
    /// `AtomicBool` to flip.
    CursorPending,
    /// Full-set (chunks + blocks + voxels) copies + `map_async`s issued;
    /// waiting for all three callbacks' atomics to flip.
    FullSetPending,
    /// Readback complete. `cpu_mirror_populated` is true; the state machine
    /// stays here for the rest of the run.
    Done,
}

/// Cross-frame readback state — owned by `ConstructionGpu`. Aggregates the
/// stage, staging buffers, completion atomics, sizes, and stall counter.
///
/// **Target-agnostic** — works identically on native (where `Device::poll`
/// is a real non-blocking poll) and WebGPU (where `poll` is a no-op but the
/// JS `mapAsync` promise resolves on subsequent event-loop ticks). See
/// `03-architecture.md` § Q3 for the design rationale.
#[derive(Default)]
pub struct CpuMirrorReadback {
    /// Current state.
    pub stage: ReadbackStage,
    /// Cursor staging buffer (2 u32s) — populated in `NotStarted → CursorPending`.
    pub cursor_staging: Option<Buffer>,
    /// `AtomicBool` set by the `map_async` callback on the cursor buffer.
    pub cursor_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Chunks staging buffer — sized once the cursor is read.
    pub chunks_staging: Option<Buffer>,
    /// `AtomicBool` for the chunks `map_async` callback.
    pub chunks_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Blocks staging buffer.
    pub blocks_staging: Option<Buffer>,
    /// `AtomicBool` for the blocks `map_async` callback.
    pub blocks_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Voxels staging buffer.
    pub voxels_staging: Option<Buffer>,
    /// `AtomicBool` for the voxels `map_async` callback.
    pub voxels_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Cursor[0] = voxels-buffer fill in u32-pairs (×2 to get u32 count).
    pub voxels_u32_count: u64,
    /// Cursor[1] = blocks-buffer fill in u32s.
    pub blocks_u32_count: u64,
    /// Chunks staging size, computed from `world_gpu.chunks_size_in_chunks`.
    pub chunks_pair_count_u32: u64,
    /// Frames spent in the current non-terminal stage. Reset when the stage
    /// advances. If it exceeds `READBACK_STALL_BUDGET_FRAMES` the state
    /// machine bails with a diagnostic.
    pub stall_frames: u32,
}

/// vox-gpu-rewrite W5.3-fix Stage 5 (D1 fix) — GPU→CPU readback that
/// populates the main-world `WorldData::{chunks_cpu, blocks_cpu, voxels_cpu}`
/// from the W5 GPU producer's output (`WorldGpu::chunks_buffer`,
/// `WorldGpu::blocks`, `WorldGpu::voxels`) the first frame after
/// `gpu_producer_has_run` flips true.
///
/// **Why.** `install_vox_in_fixed_world` (`voxel/grid.rs:317-429`)
/// constructs a `WorldData` with empty CPU mirror buffers — the W5 GPU
/// producer chain populates the GPU buffers, but the CPU mirror stayed
/// empty. The CPU-side `WorldData::ray_traversal` (used by the editor's
/// mouse-pick) immediately returns `None` when `chunk_idx >=
/// self.chunks_cpu.len()` (i.e., always, since `len() == 0`), so every
/// edit-mode raycast misses. This system mirrors C# `WorldData.cs:158-198`
/// (`dataChunkGpu.GetData(dataChunk)` +
/// `CopyFromStructuredBufferLarge(dataBlockGpu/dataVoxelGpu)` after the
/// segment loop) — without it the editor brush has no CPU mirror to
/// raycast against.
///
/// **Shape B** per `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md:387-413`:
/// after the GPU readback, the system also calls
/// `WorldData::seed_block_hashing()` so the CPU-side edit-time hash table
/// is in sync with the just-readback voxel buffer (matches C#'s
/// post-`GetData()` editor state).
///
/// **One-shot.** Gated on `gpu_producer_has_run = true` AND
/// `cpu_mirror_populated = false`. The readback uses `device.poll()` to
/// drive the staging-buffer map, which is synchronous and stalls the
/// extract-schedule thread for the duration of the readback. For Oasis
/// at the 256×32×256-chunk fixed-world size this is ~16 MiB chunks (×2
/// for the pair-channel) + N MiB blocks + M MiB voxels — N+M+~32 MiB
/// total — ~10-20 ms one-shot at startup. Per-frame cost after that:
/// one boolean check.
///
/// **Read sizing.** Chunks are sized to the full fixed-world extent
/// (every chunk is read, including empty ones — the renderer reads the
/// full extent too). Blocks/voxels are sized from the
/// `block_voxel_count` cursor pair (mirrors C# where
/// `dataBlock.Length` / `dataVoxel.Length` track the GPU producer's
/// cursor). The cursors include the initial-prefix bump (cursor[0]=64,
/// cursor[1]=64 at producer entry), so the readback sizes are
/// `voxels_cpu.len() = block_voxel_count[0] / 2` and
/// `blocks_cpu.len() = block_voxel_count[1]` directly.
pub fn populate_cpu_mirror_from_gpu_producer(
    main_world: ResMut<bevy::render::MainWorld>,
    mut gpu: Option<ResMut<ConstructionGpu>>,
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    // Only run on the W5 install path — the path where the CPU mirror was
    // installed EMPTY in `install_vox_in_fixed_world`. For the legacy default
    // / sized-to-model paths the CPU mirror is built from CPU `construct()`
    // and overwriting it with the GPU output would defeat the legacy paths'
    // bit-exact CPU oracle (and would propagate any GPU producer bug into
    // the CPU mirror, breaking the editor where it currently works).
    model_data: Option<Res<crate::render::extract::ModelDataRender>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    use bevy::render::render_resource::{MapMode, PollType};
    use std::sync::atomic::Ordering;

    let Some(gpu) = gpu.as_mut() else { return; };
    if !gpu.gpu_producer_has_run || gpu.cpu_mirror_populated {
        return;
    }
    if model_data.is_none() {
        // Legacy paths: CPU mirror is already populated by CPU `construct()`
        // (see `install_default_small_world` / `install_default_embedded_in_fixed_world`
        // / `install_vox_sized_to_model`); the readback is a no-op +
        // unnecessary risk. Mark populated so we don't keep checking.
        gpu.cpu_mirror_populated = true;
        gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
        return;
    }

    let Some(world_gpu) = world_gpu else { return; };

    // web-vox-async-loading Q3 (follow-up dispatch 2026-05-18) — cross-frame
    // CPU-mirror readback state machine. Replaces the sync
    // `Device::poll(wait_indefinitely)` + `get_mapped_range` panic site at
    // `mod.rs:944-957` (interim wasm32 escape hatch deleted per Q7).
    //
    // Each frame in `ExtractSchedule`, tick the state machine ONCE.
    // Target-agnostic — no `#[cfg(target_arch = "wasm32")]` branch on this
    // path (Decision 2).
    let device = render_device.as_ref();
    let queue = render_queue.as_ref();

    // Helper — issue copy_buffer_to_buffer + map_async with a callback that
    // sets `done` on completion. The staging buffer is returned to the caller
    // (it stays alive on `ConstructionGpu` until we read it in a later frame).
    fn issue_copy_and_map(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &Buffer,
        u32_count: u64,
        label: &'static str,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Buffer {
        let size = u32_count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("vox_gpu_rewrite_cpu_mirror_readback_enc"),
        });
        enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        let done_for_cb = done.clone();
        slice.map_async(MapMode::Read, move |r| {
            // Set the flag regardless of map success — the consumer
            // checks the flag, then attempts `get_mapped_range`. A failed
            // map will panic at `get_mapped_range`; we log instead.
            if r.is_err() {
                bevy::log::error!(
                    "vox-gpu-rewrite Q3 readback: map_async callback received \
                     Err — staging buffer map failed"
                );
            }
            done_for_cb.store(true, std::sync::atomic::Ordering::Release);
        });
        staging
    }

    // Drain the device's callback queue without blocking — drives `mapAsync`
    // resolutions on native, no-op on WebGPU (the JS event loop drives that
    // backend's callbacks). Called once per stage tick.
    let poll_result = device.poll(PollType::Poll);
    if poll_result.is_err() {
        bevy::log::error!(
            "vox-gpu-rewrite Q3 readback: device.poll(Poll) returned Err — \
             {:?}",
            poll_result.err()
        );
    }

    match gpu.cpu_mirror_readback.stage {
        ReadbackStage::Done => {
            // Reached terminal state from a previous frame (e.g. legacy path
            // short-circuit). Nothing to do.
        }
        ReadbackStage::NotStarted => {
            let Some(block_voxel_count_buf) = gpu.block_voxel_count.as_ref() else {
                return;
            };
            // Reset atomic, issue cursor copy + map_async.
            gpu.cpu_mirror_readback
                .cursor_done
                .store(false, Ordering::Relaxed);
            let staging = issue_copy_and_map(
                device,
                queue,
                block_voxel_count_buf,
                2,
                "vox_gpu_rewrite_cpu_mirror_readback_cursor",
                gpu.cpu_mirror_readback.cursor_done.clone(),
            );
            gpu.cpu_mirror_readback.cursor_staging = Some(staging);
            gpu.cpu_mirror_readback.stage = ReadbackStage::CursorPending;
            gpu.cpu_mirror_readback.stall_frames = 0;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage NotStarted → CursorPending \
                 (cursor copy issued + map_async dispatched)"
            );
        }
        ReadbackStage::CursorPending => {
            if !gpu.cpu_mirror_readback.cursor_done.load(Ordering::Acquire) {
                // Still waiting for the cursor map_async callback.
                gpu.cpu_mirror_readback.stall_frames += 1;
                if gpu.cpu_mirror_readback.stall_frames >= READBACK_STALL_BUDGET_FRAMES {
                    bevy::log::error!(
                        "vox-gpu-rewrite Q3 readback: STALLED at stage CursorPending \
                         after {} frames (~10s @ 60fps) — `mapAsync` callback for the \
                         cursor staging buffer never fired. Possible causes: device \
                         lost, render-graph submission stuck, wgpu callback queue \
                         starved. Forcing advance to Done to unblock subsequent \
                         frames (CPU mirror stays empty; editor pick-ray will return \
                         None for every position until a subsequent .vox install \
                         re-triggers the producer chain).",
                        READBACK_STALL_BUDGET_FRAMES
                    );
                    gpu.cpu_mirror_populated = true;
                    gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                    gpu.cpu_mirror_readback.cursor_staging = None;
                }
                return;
            }
            // Cursor mapped — read it, size the full set, issue copies.
            let cursor_staging = gpu
                .cpu_mirror_readback
                .cursor_staging
                .as_ref()
                .expect("cursor_staging missing in CursorPending stage")
                .clone();
            let cursor: Vec<u32> = {
                let slice = cursor_staging.slice(..);
                let data = slice.get_mapped_range();
                let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                drop(data);
                cursor_staging.unmap();
                out
            };
            if cursor.len() < 2 {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: block_voxel_count read returned \
                     {} u32s; cannot determine GPU-buffer fill levels — aborting \
                     CPU mirror population (marking populated to avoid retry)",
                    cursor.len(),
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.cursor_staging = None;
                return;
            }
            let voxels_u32_count = (cursor[0] / 2) as u64;
            let blocks_u32_count = cursor[1] as u64;

            let chunks_extent = world_gpu.chunks_size_in_chunks;
            let chunk_count =
                (chunks_extent.x * chunks_extent.y * chunks_extent.z) as u64;
            let chunks_pair_count_u32 = chunk_count * 2;

            gpu.cpu_mirror_readback.voxels_u32_count = voxels_u32_count;
            gpu.cpu_mirror_readback.blocks_u32_count = blocks_u32_count;
            gpu.cpu_mirror_readback.chunks_pair_count_u32 = chunks_pair_count_u32;

            // Reset all three completion atomics.
            gpu.cpu_mirror_readback
                .chunks_done
                .store(false, Ordering::Relaxed);
            gpu.cpu_mirror_readback
                .blocks_done
                .store(false, Ordering::Relaxed);
            gpu.cpu_mirror_readback
                .voxels_done
                .store(false, Ordering::Relaxed);

            // Issue chunks copy + map_async. Always non-zero (the world has
            // at least one chunk).
            let chunks_staging = issue_copy_and_map(
                device,
                queue,
                &world_gpu.chunks_buffer,
                chunks_pair_count_u32,
                "vox_gpu_rewrite_cpu_mirror_readback_chunks",
                gpu.cpu_mirror_readback.chunks_done.clone(),
            );
            gpu.cpu_mirror_readback.chunks_staging = Some(chunks_staging);

            // Blocks + voxels copies — skip if u32_count == 0 (an empty world
            // with no allocated blocks/voxels — the cursor would be at the
            // initial-prefix bump of 64 minimum, so this is mostly defensive).
            if blocks_u32_count > 0 {
                let blocks_staging = issue_copy_and_map(
                    device,
                    queue,
                    world_gpu.blocks.buffer(),
                    blocks_u32_count,
                    "vox_gpu_rewrite_cpu_mirror_readback_blocks",
                    gpu.cpu_mirror_readback.blocks_done.clone(),
                );
                gpu.cpu_mirror_readback.blocks_staging = Some(blocks_staging);
            } else {
                gpu.cpu_mirror_readback
                    .blocks_done
                    .store(true, Ordering::Release);
            }
            if voxels_u32_count > 0 {
                let voxels_staging = issue_copy_and_map(
                    device,
                    queue,
                    world_gpu.voxels.buffer(),
                    voxels_u32_count,
                    "vox_gpu_rewrite_cpu_mirror_readback_voxels",
                    gpu.cpu_mirror_readback.voxels_done.clone(),
                );
                gpu.cpu_mirror_readback.voxels_staging = Some(voxels_staging);
            } else {
                gpu.cpu_mirror_readback
                    .voxels_done
                    .store(true, Ordering::Release);
            }

            gpu.cpu_mirror_readback.cursor_staging = None;
            gpu.cpu_mirror_readback.stage = ReadbackStage::FullSetPending;
            gpu.cpu_mirror_readback.stall_frames = 0;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage CursorPending → FullSetPending \
                 (cursor read: {} voxels-u32s, {} blocks-u32s, {} chunks-pairs-u32s; \
                 chunks_extent={}×{}×{})",
                voxels_u32_count,
                blocks_u32_count,
                chunks_pair_count_u32,
                chunks_extent.x,
                chunks_extent.y,
                chunks_extent.z,
            );
        }
        ReadbackStage::FullSetPending => {
            let chunks_ready =
                gpu.cpu_mirror_readback.chunks_done.load(Ordering::Acquire);
            let blocks_ready =
                gpu.cpu_mirror_readback.blocks_done.load(Ordering::Acquire);
            let voxels_ready =
                gpu.cpu_mirror_readback.voxels_done.load(Ordering::Acquire);
            if !(chunks_ready && blocks_ready && voxels_ready) {
                gpu.cpu_mirror_readback.stall_frames += 1;
                if gpu.cpu_mirror_readback.stall_frames >= READBACK_STALL_BUDGET_FRAMES {
                    bevy::log::error!(
                        "vox-gpu-rewrite Q3 readback: STALLED at stage FullSetPending \
                         after {} frames (~10s @ 60fps). Pending: chunks={}, blocks={}, \
                         voxels={}. Possible causes: device lost, render-graph \
                         submission stuck. Forcing advance to Done (CPU mirror stays \
                         empty).",
                        READBACK_STALL_BUDGET_FRAMES,
                        !chunks_ready,
                        !blocks_ready,
                        !voxels_ready,
                    );
                    gpu.cpu_mirror_populated = true;
                    gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                    gpu.cpu_mirror_readback.chunks_staging = None;
                    gpu.cpu_mirror_readback.blocks_staging = None;
                    gpu.cpu_mirror_readback.voxels_staging = None;
                }
                return;
            }

            // All three mapped — read the contents.
            let chunks_pairs: Vec<u32> = {
                let staging = gpu
                    .cpu_mirror_readback
                    .chunks_staging
                    .as_ref()
                    .expect("chunks_staging missing in FullSetPending stage")
                    .clone();
                let slice = staging.slice(..);
                let data = slice.get_mapped_range();
                let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                drop(data);
                staging.unmap();
                out
            };
            let blocks_cpu: Vec<u32> = match gpu.cpu_mirror_readback.blocks_staging.as_ref() {
                Some(staging) => {
                    let staging = staging.clone();
                    let slice = staging.slice(..);
                    let data = slice.get_mapped_range();
                    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                    drop(data);
                    staging.unmap();
                    out
                }
                None => Vec::new(),
            };
            let voxels_cpu: Vec<u32> = match gpu.cpu_mirror_readback.voxels_staging.as_ref() {
                Some(staging) => {
                    let staging = staging.clone();
                    let slice = staging.slice(..);
                    let data = slice.get_mapped_range();
                    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
                    drop(data);
                    staging.unmap();
                    out
                }
                None => Vec::new(),
            };

            let chunks_pair_count_u32 = gpu.cpu_mirror_readback.chunks_pair_count_u32;
            if chunks_pairs.len() as u64 != chunks_pair_count_u32 {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: chunks_buffer read size mismatch \
                     (got {} u32s, expected {})",
                    chunks_pairs.len(),
                    chunks_pair_count_u32,
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.chunks_staging = None;
                gpu.cpu_mirror_readback.blocks_staging = None;
                gpu.cpu_mirror_readback.voxels_staging = None;
                return;
            }
            let chunk_count = (chunks_pair_count_u32 / 2) as usize;
            let mut chunks_cpu: Vec<u32> = Vec::with_capacity(chunk_count);
            for i in 0..chunk_count {
                chunks_cpu.push(chunks_pairs[i * 2]);
            }

            let chunks_len = chunks_cpu.len();
            let blocks_len = blocks_cpu.len();
            let voxels_len = voxels_cpu.len();

            // Mutate the main-world `WorldData`.
            let main_world: &mut bevy::ecs::world::World =
                &mut **main_world.into_inner();
            let Some(mut world_data) =
                main_world.get_resource_mut::<crate::world::data::WorldData>()
            else {
                bevy::log::warn!(
                    "vox-gpu-rewrite Q3 readback: main-world WorldData not present; \
                     dropping captured CPU mirror data this frame"
                );
                gpu.cpu_mirror_populated = true;
                gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
                gpu.cpu_mirror_readback.chunks_staging = None;
                gpu.cpu_mirror_readback.blocks_staging = None;
                gpu.cpu_mirror_readback.voxels_staging = None;
                return;
            };
            world_data.chunks_cpu = chunks_cpu;
            world_data.blocks_cpu = blocks_cpu;
            world_data.voxels_cpu = voxels_cpu;
            world_data.block_hashing = crate::aadf::block_hash::BlockHashingHandler::new();
            world_data.seed_block_hashing();

            // 2026-05-19 horizon-parity AADF diagnostic — sample chunks +
            // blocks at distances along the cross-target SSIM gate's
            // camera view-ray and log AADF skip-bit decode for each.
            // Native + WASM both pass through this code (same readback);
            // the Playwright spec filters `[aadf-probe]` lines from
            // console output + native stdout, persists them to disk so
            // the orchestrator can diff native vs WASM without
            // copy-pasting log tails.
            //
            // Camera (cross-target gate pose):
            //   pos     = (3880, 497, 3514) voxels
            //   forward = (-0.924, -0.241, -0.297)
            //
            // Chunk word encoding (bits 30-31 = state; bits 0-29 = AADF
            // skip-distances for empty chunks, encoded as 6 × 5-bit
            // fields = (mx, px, my, py, mz, pz)).
            {
                let chunks_cpu = &world_data.chunks_cpu;
                let blocks_cpu = &world_data.blocks_cpu;
                let voxels_cpu = &world_data.voxels_cpu;
                let scx = world_data.size_in_chunks.x as usize;
                let scy = world_data.size_in_chunks.y as usize;
                let scz = world_data.size_in_chunks.z as usize;
                bevy::log::info!(
                    "[aadf-probe] world chunks {}×{}×{} \
                     chunks_cpu.len()={} blocks_cpu.len()={} voxels_cpu.len()={}",
                    scx, scy, scz,
                    chunks_cpu.len(),
                    blocks_cpu.len(),
                    voxels_cpu.len(),
                );
                let cam = [3880.0_f32, 497.0_f32, 3514.0_f32];
                let fwd = [-0.924_f32, -0.241_f32, -0.297_f32];
                for &dist in &[0.0_f32, 500.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0] {
                    let pxw = cam[0] + fwd[0] * dist;
                    let pyw = cam[1] + fwd[1] * dist;
                    let pzw = cam[2] + fwd[2] * dist;
                    if pxw < 0.0 || pyw < 0.0 || pzw < 0.0 {
                        bevy::log::info!(
                            "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) OUT_OF_WORLD_NEGATIVE",
                            dist as u32, pxw, pyw, pzw,
                        );
                        continue;
                    }
                    let cx = (pxw as u32) / 16;
                    let cy = (pyw as u32) / 16;
                    let cz = (pzw as u32) / 16;
                    if cx >= scx as u32 || cy >= scy as u32 || cz >= scz as u32 {
                        bevy::log::info!(
                            "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) chunk=({},{},{}) OUT_OF_WORLD",
                            dist as u32, pxw, pyw, pzw, cx, cy, cz,
                        );
                        continue;
                    }
                    let chunk_idx =
                        cx as usize + cy as usize * scx + cz as usize * scx * scy;
                    let chunk_word = chunks_cpu[chunk_idx];
                    let state = (chunk_word >> 30) & 0x3;
                    let mxd = chunk_word & 0x1F;
                    let pxd = (chunk_word >> 5) & 0x1F;
                    let myd = (chunk_word >> 10) & 0x1F;
                    let pyd = (chunk_word >> 15) & 0x1F;
                    let mzd = (chunk_word >> 20) & 0x1F;
                    let pzd = (chunk_word >> 25) & 0x1F;
                    let state_name = match state {
                        0 => "EMPTY",
                        1 => "FULL",
                        _ => "MIXED",
                    };
                    bevy::log::info!(
                        "[aadf-probe] dist={} pos=({:.0},{:.0},{:.0}) chunk=({},{},{}) \
                         word=0x{:08x} state={} chunk_aadf=[mx={} px={} my={} py={} mz={} pz={}]",
                        dist as u32, pxw, pyw, pzw, cx, cy, cz,
                        chunk_word, state_name, mxd, pxd, myd, pyd, mzd, pzd,
                    );
                    // For mixed chunks, peek at the first block's 2-bit
                    // AADF skip-distances + state. block_base is the
                    // 30-bit pointer in bits 0-29 of chunk_word.
                    if state >= 2 {
                        let block_base = chunk_word & 0x3FFFFFFF;
                        if (block_base as usize) < blocks_cpu.len() {
                            let block_word = blocks_cpu[block_base as usize];
                            let bstate = (block_word >> 30) & 0x3;
                            let bmx = block_word & 0x3;
                            let bpx = (block_word >> 2) & 0x3;
                            let bmy = (block_word >> 4) & 0x3;
                            let bpy = (block_word >> 6) & 0x3;
                            let bmz = (block_word >> 8) & 0x3;
                            let bpz = (block_word >> 10) & 0x3;
                            bevy::log::info!(
                                "[aadf-probe]   block[{}] word=0x{:08x} state={} \
                                 block_aadf=[mx={} px={} my={} py={} mz={} pz={}]",
                                block_base, block_word, bstate,
                                bmx, bpx, bmy, bpy, bmz, bpz,
                            );
                        }
                    }
                }
                let _ = voxels_cpu;
            }
            drop(world_data);

            gpu.cpu_mirror_populated = true;
            gpu.cpu_mirror_readback.stage = ReadbackStage::Done;
            gpu.cpu_mirror_readback.chunks_staging = None;
            gpu.cpu_mirror_readback.blocks_staging = None;
            gpu.cpu_mirror_readback.voxels_staging = None;
            bevy::log::info!(
                "vox-gpu-rewrite Q3 readback: stage FullSetPending → Done — CPU \
                 mirror populated from GPU producer output: chunks_cpu.len() = {}, \
                 blocks_cpu.len() = {}, voxels_cpu.len() = {}",
                chunks_len,
                blocks_len,
                voxels_len,
            );
        }
    }
}
