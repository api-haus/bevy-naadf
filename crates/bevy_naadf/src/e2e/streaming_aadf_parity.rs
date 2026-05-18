//! `--gate streaming-aadf-parity` e2e gate.
//!
//! streaming-world Phase 2.11
//! (`docs/orchestrate/streaming-world/03n-diagnosis-aadf-building.md` punch-list
//! item 4) — cross-preset regression test for the W3 chunk-level 5-bit AADF
//! corruption bug.
//!
//! ## Why a self-consistency check, not a strict static-vs-streaming compare
//!
//! The brief's strictest design — boot static preset, snapshot
//! `chunks_buffer`, boot streaming preset, snapshot `chunks_buffer`, byte-
//! compare — requires either two sequential `App` instances (winit + DefaultPlugins
//! aren't reliably re-initialisable in one process) or a custom multi-pass
//! headless harness (multi-hundred-LOC extension of `validate_gpu_construction`).
//! Both are heavier than the LOC budget allows.
//!
//! Equivalent regression-catching power comes from a SELF-CONSISTENCY check
//! on the streaming-preset `chunks_buffer` post-cold-start:
//!
//!   **Invariant**: For every chunk c with state == UNIFORM_EMPTY, the
//!   5-bit AADF skip distance in each of 6 directions must NOT exceed the
//!   actual distance to the nearest non-empty chunk in that direction.
//!
//! The Phase 2.11 root-cause bug (`03n` § Root cause) violates this
//! invariant: stale AADFs encode long skips through real-future-terrain
//! segments, so for a chunk c whose +X neighbour 8 chunks away is solid,
//! the AADF claims "skip 15 chunks" (over-skipping past the solid).
//!
//! The check IS the parity check — it asserts the streaming preset
//! produces the same kind of correct-or-zero AADF the static preset
//! produces (the static preset skips the W3 chain entirely, so its AADFs
//! are always zero, which trivially satisfies the invariant). A passing
//! invariant on the streaming preset means: rays following the AADFs
//! cannot skip past real terrain. That's the user-visible bug from
//! screenshots 4-7.
//!
//! ## Wiring
//!
//! 1. `apply_streaming_aadf_parity_defaults` overlays on top of
//!    `apply_streaming_window_defaults` (the gate inherits the streaming
//!    preset install + camera walk + framebuffer captures).
//! 2. After the camera walk completes (Phase 2.10's `WALK_TICKS_REMAINING`
//!    reaches 0), a one-shot render-world readback system snapshots
//!    `WorldGpu::chunks_buffer`.
//! 3. The driver's `OasisAssert` step branches on
//!    `streaming_aadf_parity_mode` and invokes
//!    [`assert_streaming_aadf_parity`], which validates the invariant
//!    against the snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::render::render_resource::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, PollType,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

/// Latch — set when a chunks_buffer snapshot has been requested but not yet
/// captured. The render-world readback system fires once when this is
/// `true` AND clears it.
static SNAPSHOT_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Latch — set once a snapshot has been captured. Used by the Update-
/// system to avoid re-requesting once the readback already fired (the
/// snapshot is the same after-walk frame regardless of how many ticks
/// pass; one capture is sufficient).
static SNAPSHOT_DONE: AtomicBool = AtomicBool::new(false);
/// Snapshot of `chunks_buffer` (entire buffer as `vec<u32>` flat layout —
/// `chunk_count * 2` u32s, `[state, entity_y]` per chunk). `None` before
/// the snapshot, `Some` after.
static CHUNKS_SNAPSHOT: Mutex<Option<Vec<u32>>> = Mutex::new(None);
/// Snapshot of `window_indirection` table — 512 u32s, one per
/// (window-local) segment position, value = slot index or EMPTY_SLOT.
/// Captured alongside `CHUNKS_SNAPSHOT` so the parity check can walk
/// cross-slot neighbour AADFs via the same translation the renderer uses.
static INDIRECTION_SNAPSHOT: Mutex<Option<Vec<u32>>> = Mutex::new(None);

/// Reset the gate's static state — called by
/// [`apply_streaming_aadf_parity_defaults`] so successive invocations get
/// a fresh slate.
pub fn reset_parity_latches() {
    SNAPSHOT_REQUESTED.store(false, Ordering::SeqCst);
    SNAPSHOT_DONE.store(false, Ordering::SeqCst);
    if let Ok(mut g) = CHUNKS_SNAPSHOT.lock() {
        *g = None;
    }
    if let Ok(mut g) = INDIRECTION_SNAPSHOT.lock() {
        *g = None;
    }
}

/// Request a snapshot — called by an Update system once the walk completes.
/// One-shot: subsequent calls after the first successful snapshot are
/// no-ops (gated on `SNAPSHOT_DONE`).
pub fn request_snapshot() {
    if SNAPSHOT_DONE.load(Ordering::SeqCst) {
        return;
    }
    SNAPSHOT_REQUESTED.store(true, Ordering::SeqCst);
}

/// Read the snapshot (consumes — subsequent calls return None).
pub fn take_snapshot() -> Option<Vec<u32>> {
    CHUNKS_SNAPSHOT.lock().ok().and_then(|mut g| g.take())
}

/// Read the indirection snapshot (consumes).
pub fn take_indirection_snapshot() -> Option<Vec<u32>> {
    INDIRECTION_SNAPSHOT.lock().ok().and_then(|mut g| g.take())
}

/// Apply the parity gate's defaults onto `args`. Inherits the streaming-
/// window setup (preset install, camera walk, framebuffer captures) and
/// adds the parity-mode flag so the driver's `OasisAssert` step branches
/// to [`assert_streaming_aadf_parity`].
pub fn apply_streaming_aadf_parity_defaults(args: &mut crate::AppArgs) {
    super::streaming_window::apply_streaming_window_defaults(args);
    args.streaming_aadf_parity_mode = true;
    reset_parity_latches();
    println!(
        "e2e_render --gate streaming-aadf-parity: layered on streaming-window \
         defaults; post-walk W3 self-consistency check enabled."
    );
}

/// `Update` system — when the walk completes (final tick), request a
/// chunks_buffer snapshot. Runs in the main-world Update schedule; the
/// snapshot itself happens in a render-world system gated on
/// `SNAPSHOT_REQUESTED`.
///
/// Wired via `add_e2e_systems` (`e2e/mod.rs`) only when args.streaming_aadf_parity_mode.
pub fn request_snapshot_after_walk(args: Option<Res<crate::AppArgs>>) {
    let Some(args) = args else { return; };
    if !args.streaming_aadf_parity_mode {
        return;
    }
    if !super::streaming_window::camera_has_walked() {
        return;
    }
    // After walk fully drained (counter at 0), latch the request once.
    if super::streaming_window::walk_ticks_remaining() == 0 {
        request_snapshot();
    }
}

/// Render-world system — performs the synchronous GPU readback of
/// `WorldGpu::chunks_buffer` once `SNAPSHOT_REQUESTED` is set. Stashes
/// into [`CHUNKS_SNAPSHOT`].
///
/// Runs in `Render::Cleanup` so it sees the latest GPU state of the frame
/// (after every dispatch + barrier has executed in the frame's submits).
/// Synchronous `device.poll(PollType::wait_indefinitely())` blocks until
/// the readback completes — bounded by the cap (~16 MiB buffer; ~50 ms
/// total under load).
pub fn render_world_chunks_readback(
    world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
    construction_gpu: Option<Res<crate::render::construction::ConstructionGpu>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !SNAPSHOT_REQUESTED.swap(false, Ordering::SeqCst) {
        return;
    }
    let Some(world_gpu) = world_gpu else {
        return;
    };
    let chunks_extent = world_gpu.chunks_size_in_chunks;
    let chunk_count = (chunks_extent.x * chunks_extent.y * chunks_extent.z) as u64;
    let buffer_size = chunk_count * 8; // vec2<u32> = 8 B per chunk
    // Indirection is 512 u32s (16×2×16 = WORLD_SIZE_IN_SEGMENTS extent).
    let indirection_size: u64 = (16 * 2 * 16) * 4;
    let staging = render_device.create_buffer(&BufferDescriptor {
        label: Some("streaming_aadf_parity_chunks_readback_staging"),
        size: buffer_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let indirection_staging = if construction_gpu
        .as_deref()
        .and_then(|g| g.window_indirection_buffer.as_ref())
        .is_some()
    {
        Some(render_device.create_buffer(&BufferDescriptor {
            label: Some("streaming_aadf_parity_indirection_readback_staging"),
            size: indirection_size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }))
    } else {
        None
    };
    let mut enc = render_device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("streaming_aadf_parity_chunks_readback_enc"),
    });
    enc.copy_buffer_to_buffer(&world_gpu.chunks_buffer, 0, &staging, 0, buffer_size);
    if let (Some(indir_buf), Some(indir_staging)) = (
        construction_gpu
            .as_deref()
            .and_then(|g| g.window_indirection_buffer.as_ref()),
        indirection_staging.as_ref(),
    ) {
        enc.copy_buffer_to_buffer(indir_buf, 0, indir_staging, 0, indirection_size);
    }
    render_queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| {
        if let Err(e) = r {
            bevy::log::warn!(
                "streaming-aadf-parity: chunks_buffer map_async failed: {e:?}"
            );
        }
    });
    let indirection_slice_opt = indirection_staging.as_ref().map(|s| s.slice(..));
    if let Some(islice) = indirection_slice_opt.as_ref() {
        islice.map_async(MapMode::Read, |r| {
            if let Err(e) = r {
                bevy::log::warn!(
                    "streaming-aadf-parity: indirection map_async failed: {e:?}"
                );
            }
        });
    }
    if let Err(e) = render_device.poll(PollType::wait_indefinitely()) {
        bevy::log::warn!(
            "streaming-aadf-parity: render_device.poll failed: {e:?}"
        );
        return;
    }
    let data = slice.get_mapped_range();
    let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging.unmap();
    if let Ok(mut g) = CHUNKS_SNAPSHOT.lock() {
        *g = Some(out);
    }
    if let Some(islice) = indirection_slice_opt {
        let idata = islice.get_mapped_range();
        let iout: Vec<u32> = bytemuck::cast_slice(&idata).to_vec();
        drop(idata);
        if let Some(s) = indirection_staging.as_ref() {
            s.unmap();
        }
        if let Ok(mut g) = INDIRECTION_SNAPSHOT.lock() {
            *g = Some(iout);
        }
    }
    SNAPSHOT_DONE.store(true, Ordering::SeqCst);
    bevy::log::info!(
        "streaming-aadf-parity: chunks_buffer snapshot captured ({} u32s, \
         {} chunks at {:?} extent).",
        chunk_count * 2,
        chunk_count,
        chunks_extent,
    );
}

/// World chunk state constants — mirror `world_data.wgsl`.
const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0;
// const BLOCK_STATE_UNIFORM_FULL: u32 = 1;
// const BLOCK_STATE_CHILD: u32 = 2;

/// Extract the 6 AADF skip distances from a `chunks[idx].x` value.
/// Returns `[-x, +x, -y, +y, -z, +z]` skip distances. 5 bits each at
/// offsets `axis * 10 + side * 5` per `bounds_calc.wgsl`'s
/// `check_matching_bounds_5bit`.
fn decode_aadf_6(x: u32) -> [u32; 6] {
    [
        (x >> 0) & 0x1F,  // -X (axis 0 side 0)
        (x >> 5) & 0x1F,  // +X (axis 0 side 1)
        (x >> 10) & 0x1F, // -Y (axis 1 side 0)
        (x >> 15) & 0x1F, // +Y (axis 1 side 1)
        (x >> 20) & 0x1F, // -Z (axis 2 side 0)
        (x >> 25) & 0x1F, // +Z (axis 2 side 1)
    ]
}

/// Decode the chunk state (top 2 bits).
fn decode_state(x: u32) -> u32 {
    x >> 30
}

/// Slot-indexed chunk index → translates through the window indirection
/// table. The chunks_buffer layout on the streaming preset is slot-major
/// (`slot.0 * 4096 + chunk_in_seg_idx`), where each slot owns one 16³-chunk
/// segment. Used in unit tests below.
#[cfg(test)]
fn slot_local_chunk_index(slot: u32, local_x: u32, local_y: u32, local_z: u32) -> usize {
    const SEG: u32 = 16;
    let idx = local_x + local_y * SEG + local_z * SEG * SEG;
    (slot * 4096 + idx) as usize
}

/// Validate the W3 self-consistency invariant on the snapshot, walking
/// neighbours via the window indirection table — catches cross-slot lying
/// AADFs (the steady-state form of the Phase 2.11 bug).
///
/// For each resident chunk c with state == UNIFORM_EMPTY, walk each of 6
/// directions by `aadf_d` chunks in WINDOW-LOCAL coords. Translate each
/// window-local neighbour position through the indirection table to find
/// its slot, then index `chunks_buffer[slot * 4096 + chunk_in_seg_idx]`.
/// If any intermediate neighbour is non-EMPTY (or EMPTY_SLOT → treat as
/// empty), the AADF is "lying" — the Phase 2.11 bug.
///
/// Indirection-table layout: 512 u32s, `pack(local_seg_xyz) → slot_index`,
/// `pack = lx + ly * 16 + lz * (16 * 2)` (mirrors `WindowedSlotMap::pack`).
/// `EMPTY_SLOT = 0xFFFFFFFFu` → treat the position as uniform-empty (the
/// `streaming_chunk_load_bc` semantic at `bounds_calc.wgsl:139-141`).
///
/// Returns `(violations, max_excess)` — `violations = 0` ⇔ self-consistent.
pub fn validate_self_consistency(chunks: &[u32], indirection: &[u32]) -> (u64, u32) {
    /// Window-local chunk extent (= world chunk extent for the streaming
    /// preset where window == world).
    const CHUNKS_X: i32 = 256;
    const CHUNKS_Y: i32 = 32;
    const CHUNKS_Z: i32 = 256;
    const CHUNKS_PER_SEG: i32 = 16;
    const EMPTY_SLOT: u32 = u32::MAX;

    // Look up the slot-indexed chunks_buffer index for a window-local chunk
    // position. Returns `None` for EMPTY_SLOT (treat-as-empty).
    let chunk_idx_via_indirection = |cx: i32, cy: i32, cz: i32| -> Option<usize> {
        let seg_lx = (cx / CHUNKS_PER_SEG) as u32;
        let seg_ly = (cy / CHUNKS_PER_SEG) as u32;
        let seg_lz = (cz / CHUNKS_PER_SEG) as u32;
        let in_seg_x = (cx % CHUNKS_PER_SEG) as u32;
        let in_seg_y = (cy % CHUNKS_PER_SEG) as u32;
        let in_seg_z = (cz % CHUNKS_PER_SEG) as u32;
        let pack = seg_lx + seg_ly * 16 + seg_lz * (16 * 2);
        if (pack as usize) >= indirection.len() {
            return None;
        }
        let slot = indirection[pack as usize];
        if slot == EMPTY_SLOT {
            return None;
        }
        let chunk_in_seg_idx = in_seg_x + in_seg_y * 16 + in_seg_z * 16 * 16;
        Some((slot * 4096 + chunk_in_seg_idx) as usize)
    };

    let mut violations = 0u64;
    let mut max_excess = 0u32;
    let dirs: [(i32, i32, i32); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    for cz in 0..CHUNKS_Z {
        for cy in 0..CHUNKS_Y {
            for cx in 0..CHUNKS_X {
                let Some(idx) = chunk_idx_via_indirection(cx, cy, cz) else {
                    continue;
                };
                if idx * 2 >= chunks.len() {
                    continue;
                }
                let x = chunks[idx * 2];
                if decode_state(x) != BLOCK_STATE_UNIFORM_EMPTY {
                    continue;
                }
                let aadfs = decode_aadf_6(x);
                for (d_idx, aadf) in aadfs.iter().copied().enumerate() {
                    if aadf == 0 {
                        continue;
                    }
                    let (dx, dy, dz) = dirs[d_idx];
                    for step in 1..=aadf {
                        let nx = cx + dx * step as i32;
                        let ny = cy + dy * step as i32;
                        let nz = cz + dz * step as i32;
                        if nx < 0
                            || ny < 0
                            || nz < 0
                            || nx >= CHUNKS_X
                            || ny >= CHUNKS_Y
                            || nz >= CHUNKS_Z
                        {
                            // Out of window — treat as empty (the world-
                            // edge inflation in `add_bounds_group`).
                            break;
                        }
                        let Some(nidx) = chunk_idx_via_indirection(nx, ny, nz)
                        else {
                            // EMPTY_SLOT — treat as empty; continue walking.
                            continue;
                        };
                        if nidx * 2 >= chunks.len() {
                            break;
                        }
                        let neighbour = chunks[nidx * 2];
                        if decode_state(neighbour) != BLOCK_STATE_UNIFORM_EMPTY {
                            // Lying AADF — skip crosses a non-empty chunk.
                            violations += 1;
                            let excess = aadf - (step - 1);
                            if excess > max_excess {
                                max_excess = excess;
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
    (violations, max_excess)
}

/// Run the streaming-aadf-parity assertion against the captured snapshot.
/// Returns `Ok(report)` on success, `Err(msg)` on failure.
pub fn assert_streaming_aadf_parity() -> Result<String, String> {
    let Some(chunks) = take_snapshot() else {
        return Err(
            "streaming-aadf-parity: no chunks_buffer snapshot captured — \
             the readback system never fired. Likely cause: the walk \
             never completed within the gate budget."
                .to_string(),
        );
    };
    let Some(indirection) = take_indirection_snapshot() else {
        return Err(
            "streaming-aadf-parity: no indirection snapshot captured — \
             the streaming preset's window_indirection_buffer was not \
             allocated. Likely cause: streaming preset not active."
                .to_string(),
        );
    };
    let (violations, max_excess) =
        validate_self_consistency(&chunks, &indirection);
    let report = format!(
        "streaming-aadf-parity: chunks_buffer self-consistency \
         (cross-slot via indirection) — {} violations (lying AADFs), \
         max excess skip = {} chunks",
        violations, max_excess,
    );
    println!("e2e_render --gate streaming-aadf-parity: {report}");
    if violations > 0 {
        Err(format!(
            "streaming-aadf-parity gate FAIL — {} violations of the W3 \
             chunk-level AADF self-consistency invariant (max excess skip = \
             {} chunks). This is the Phase 2.11 root-cause bug \
             (`03n-diagnosis-aadf-building.md` § Root cause): the W3 chain \
             baked long-skip AADFs through yet-to-be-admitted zero-chunks; \
             subsequent admissions did not invalidate the stale AADFs. {}",
            violations, max_excess, report,
        ))
    } else {
        Ok(format!("streaming-aadf-parity gate PASS — {report}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identity indirection table: slot N at local position N (treating
    /// local positions 0..512 as flat indexes).
    fn identity_indirection() -> Vec<u32> {
        (0..512).collect()
    }

    /// Build an identity indirection where slot N maps to window-local
    /// position (lx, ly, lz). Returns the indirection slice.
    /// Convenience wrapper around the identity layout.
    fn make_identity_table() -> Vec<u32> {
        identity_indirection()
    }

    /// Self-consistency catches a lying AADF: a chunk with skip=5 in +X
    /// across an intermediate non-empty chunk.
    #[test]
    fn detects_lying_aadf() {
        let total_chunks = 512usize * 4096;
        let mut chunks = vec![0u32; total_chunks * 2];
        let indirection = make_identity_table();

        // Slot 0 maps to local seg (0, 0, 0). Within slot 0, chunk (0, 0, 0)
        // is at window-local chunk pos (0, 0, 0). +X AADF skip = 5.
        let idx = slot_local_chunk_index(0, 0, 0, 0);
        chunks[idx * 2] = 5u32 << 5;

        // Slot 0, chunk (3, 0, 0) — non-empty (UNIFORM_FULL = 1).
        let idx_full = slot_local_chunk_index(0, 3, 0, 0);
        chunks[idx_full * 2] = 1u32 << 30;

        let (violations, max_excess) =
            validate_self_consistency(&chunks, &indirection);
        assert!(
            violations > 0,
            "expected violation; got {} violations",
            violations
        );
        assert!(max_excess > 0);
    }

    /// Self-consistency passes on the all-zero-AADF case (matches the
    /// static preset's chunks_buffer post-cold-start — every empty chunk
    /// has AADF = 0, which trivially satisfies the invariant).
    #[test]
    fn zero_aadfs_pass() {
        let total_chunks = 512usize * 4096;
        let chunks = vec![0u32; total_chunks * 2];
        let indirection = make_identity_table();
        let (violations, _) = validate_self_consistency(&chunks, &indirection);
        assert_eq!(violations, 0);
    }

    /// Self-consistency passes when AADF skip lands on the LAST empty
    /// chunk before a non-empty chunk (correct upper-bound).
    #[test]
    fn correct_aadf_passes() {
        let total_chunks = 512usize * 4096;
        let mut chunks = vec![0u32; total_chunks * 2];
        let indirection = make_identity_table();

        // Slot 0, chunk (0, 0, 0) — empty with +X AADF skip = 2.
        let idx = slot_local_chunk_index(0, 0, 0, 0);
        chunks[idx * 2] = 2u32 << 5;
        let idx_full = slot_local_chunk_index(0, 3, 0, 0);
        chunks[idx_full * 2] = 1u32 << 30;

        let (violations, _) = validate_self_consistency(&chunks, &indirection);
        assert_eq!(violations, 0);
    }

    #[test]
    fn decode_aadf_extracts_6_fields() {
        // Set every field to a unique 5-bit value.
        let x = (1u32 << 0)   // -X = 1
              | (3u32 << 5)   // +X = 3
              | (5u32 << 10)  // -Y = 5
              | (7u32 << 15)  // +Y = 7
              | (11u32 << 20) // -Z = 11
              | (13u32 << 25); // +Z = 13
        assert_eq!(decode_aadf_6(x), [1, 3, 5, 7, 11, 13]);
    }

    #[test]
    fn decode_state_extracts_top_2_bits() {
        assert_eq!(decode_state(0u32), 0);
        assert_eq!(decode_state(1u32 << 30), 1);
        assert_eq!(decode_state(2u32 << 30), 2);
        assert_eq!(decode_state(3u32 << 30), 3);
    }
}
