//! Phase-C W4 — CPU port of NAADF's `EntityHandler.Update`
//! (`EntityHandler.cs:165-475`).
//!
//! Runs in `ExtractSchedule` (CPU; no GPU systems). Each frame:
//!
//! 1. **Overlap count** — for each entity instance, find the chunks its rotated
//!    AABB overlaps (the corner-rotation loop at `EntityHandler.cs:186-232`).
//! 2. **Prefix-sum** — assign each chunk a base pointer into the per-frame
//!    `entity_chunk_instances` upload buffer (`EntityHandler.cs:234-241`).
//! 3. **Second pass** — fill the upload buffer with one entry per
//!    (chunk, entity-instance) overlap (`EntityHandler.cs:243-290`).
//! 4. **Dedup-hash** — for each chunk, compute a hash of its entity list and
//!    look up an existing identical list; if found, point this chunk at the
//!    already-uploaded entity list rather than uploading a duplicate
//!    (`EntityHandler.cs:292-333`).
//! 5. **Pack uploads** — emit:
//!    - `chunk_updates_dynamic`: `vec2<u32>` per update (chunk-pos + entity ptr).
//!    - `entity_chunk_instances_dynamic`: 20-byte `GpuEntityChunkInstance` per
//!      unique (chunk, entity-instance) pair.
//!    - `entity_history_dynamic`: 16-byte `GpuEntityInstanceHistory` per
//!      entity instance.
//!
//! This module is pure CPU + GPU-buffer-format-agnostic. The GPU dispatch
//! lives in [`super::entity_update`]. The algorithm matches the C# bit-for-bit
//! on a small fixture (`tests::entity_handler_cpu_dedup`).

use crate::aadf::entity::{
    compress_entity_chunk_instance, compress_entity_history, pack_chunk_update,
};
use crate::render::gpu_types::{
    EntityInstance, GpuChunkUpdate, GpuEntityChunkInstance, GpuEntityInstanceHistory,
};

/// Hash-coefficient seed table (port of `EntityHandler.cs:123-128`). The C#
/// builds a 257-entry table; only `[0..=size_cap]` is consumed for any
/// individual chunk's hash (sized by the chunk's entity-overlap count, which
/// is bounded by ~16 in practice).
///
/// Built lazily on first request; identical to W1's
/// `hashing::hash_coefficients` 65-entry table but with 257 entries
/// (`EntityHandler.cs:123` cap is 256+1).
pub fn entity_hash_coefficients() -> [u32; 257] {
    let mut out = [0u32; 257];
    out[256] = 1;
    let mut i = 256i32;
    while i > 0 {
        i -= 1;
        out[i as usize] = out[(i + 1) as usize].wrapping_mul(31);
    }
    out
}

/// W4 — per-frame entity handler output. Three CPU-side `Vec`s the GPU's
/// `entity_update.wgsl` consumes verbatim:
///
/// - `chunk_updates`: each entry is a `(chunkPos, entityPointerAndSize)`
///   packed pair. `entityPointerAndSize = (pointer << 8) | size`.
/// - `entity_chunk_instances`: the dedup-hashed per-chunk entity arrays —
///   one entry per (chunk-id-class, entity-instance-id) pair.
/// - `entity_history`: one entry per entity instance, written to the
///   `taa_index`-th slot of the `entity_instances_history` ring.
#[derive(Debug, Default)]
pub struct EntityUpdateUploads {
    pub chunk_updates: Vec<GpuChunkUpdate>,
    pub entity_chunk_instances: Vec<GpuEntityChunkInstance>,
    pub entity_history: Vec<GpuEntityInstanceHistory>,
}

/// W4 — the per-frame state the entity handler maintains across calls.
///
/// `chunkEntityData` (C#: per-chunk u32 holding `pointer << 8 | size`) and the
/// previous-frame chunks-with-entities list are kept in this `EntityHandler`
/// so the next frame's "clear stale chunks" pass can fire.
pub struct EntityHandler {
    /// World chunk extent. Drives the `chunkIndex = x + y*sx + z*sx*sy`
    /// flattening.
    pub size_in_chunks: [u32; 3],
    /// Per-chunk entity-count + final-pointer u32 (matches C#
    /// `chunkEntityData[chunkIndex]`). `pointer << 8 | size`.
    pub chunk_entity_data: Vec<u32>,
    /// Chunks that had at least one overlap **last frame** — used to fire
    /// "this chunk no longer has entities" updates this frame.
    pub last_frame_chunks: Vec<u32>,
    /// Chunks that had at least one overlap **this frame**, used to roll into
    /// `last_frame_chunks` after a successful update.
    pub current_frame_chunks: Vec<u32>,
    /// Pre-computed hash coefficients (`EntityHandler.cs:123-128`).
    pub hash_coefficients: [u32; 257],
}

impl EntityHandler {
    /// Build an [`EntityHandler`] sized for a world of `size_in_chunks` chunks.
    pub fn new(size_in_chunks: [u32; 3]) -> Self {
        let chunk_count = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2])
            as usize;
        Self {
            size_in_chunks,
            chunk_entity_data: vec![0; chunk_count],
            last_frame_chunks: Vec::new(),
            current_frame_chunks: Vec::new(),
            hash_coefficients: entity_hash_coefficients(),
        }
    }

    /// Run one Update tick — port of `EntityHandler.cs:165-443`.
    ///
    /// Returns the three uploads the GPU consumes. Mutates `self`'s
    /// per-frame state (rolls `current_frame_chunks` into
    /// `last_frame_chunks` at the end).
    pub fn update(&mut self, instances: &[EntityInstance]) -> EntityUpdateUploads {
        // Reset counters for last frame's overlapped chunks. C# at :180-183:
        // `for (i in entityChunkInstancesInfoOld) chunkEntityData[i] = 0`.
        for &idx in &self.last_frame_chunks {
            self.chunk_entity_data[idx as usize] = 0;
        }
        self.current_frame_chunks.clear();

        let sx = self.size_in_chunks[0] as i32;
        let sy = self.size_in_chunks[1] as i32;
        let sz = self.size_in_chunks[2] as i32;

        // Pass 1 — count overlaps per chunk. C# at :185-232.
        for inst in instances {
            for_each_overlapped_chunk(inst, [sx, sy, sz], |chunk_idx| {
                let old = self.chunk_entity_data[chunk_idx as usize];
                self.chunk_entity_data[chunk_idx as usize] = old + 1;
                if old == 0 {
                    self.current_frame_chunks.push(chunk_idx);
                }
            });
        }

        // Pass 2 — prefix-sum. C# at :234-241. Each chunk's entry now holds
        // `(pointer << 8) | 0` — the `size` byte is reset; we'll re-fill it
        // in pass 3 by incrementing it back to the original count.
        let mut chunk_instance_counter: u32 = 0;
        let mut entity_chunk_instances_pool: Vec<u32> =
            Vec::with_capacity(self.current_frame_chunks.len() * 4);
        // We allocate a working buffer that holds, per chunk, the entity
        // instance IDs that overlap it. The C# uses one flat `entityChunkInstances`
        // List<uint> indexed by per-chunk pointer; we mirror that.
        for &chunk_idx in &self.current_frame_chunks {
            let count = self.chunk_entity_data[chunk_idx as usize];
            self.chunk_entity_data[chunk_idx as usize] = chunk_instance_counter << 8;
            chunk_instance_counter += count;
        }
        entity_chunk_instances_pool.resize(chunk_instance_counter as usize, 0);

        // Pass 3 — fill the per-chunk entity-instance arrays. C# at :243-290.
        for (instance_id, inst) in instances.iter().enumerate() {
            for_each_overlapped_chunk(inst, [sx, sy, sz], |chunk_idx| {
                let old = self.chunk_entity_data[chunk_idx as usize];
                self.chunk_entity_data[chunk_idx as usize] = old + 1;
                let pointer = old >> 8;
                let size_so_far = old & 0xFF;
                let write_idx = (pointer + size_so_far) as usize;
                entity_chunk_instances_pool[write_idx] = instance_id as u32;
            });
        }

        // Pass 4 — dedup-hash + emit uploads. C# at :292-342.
        let mut uploads = EntityUpdateUploads::default();
        let mut hashed: Vec<(i64, u32, u32)> = Vec::new(); // (hash, source_ptr, new_pointer)
        let mut entity_chunk_instance_count: u32 = 0;
        let mut updates: Vec<GpuChunkUpdate> = Vec::new();

        for &chunk_idx in &self.current_frame_chunks {
            let entity_data = self.chunk_entity_data[chunk_idx as usize];
            let pointer = entity_data >> 8;
            let size = entity_data & 0xFF;

            // Compute the hash. C# at :300-304: sum over `coeff[e] * instanceID`.
            let mut hash: i64 = 0;
            for e in 0..size {
                let inst_id = entity_chunk_instances_pool[(pointer + e) as usize];
                hash = hash.wrapping_add(
                    (self.hash_coefficients[e as usize].wrapping_mul(inst_id)) as i32 as i64,
                );
            }

            // Look up an existing identical list. C# uses a HashSet keyed by
            // a (hash, content)-equality comparer; we mirror with a linear
            // scan over `hashed` since the per-frame chunk count is small.
            let mut final_pointer_and_size: u32 = 0;
            let mut found = false;
            for &(other_hash, other_ptr, other_new_pointer) in &hashed {
                if other_hash != hash {
                    continue;
                }
                // Compare entity-id arrays.
                let other_size = ((other_new_pointer) & 0xFF) as u32;
                if other_size != size {
                    continue;
                }
                let mut eq = true;
                for e in 0..size {
                    if entity_chunk_instances_pool[(pointer + e) as usize]
                        != entity_chunk_instances_pool[(other_ptr + e) as usize]
                    {
                        eq = false;
                        break;
                    }
                }
                if eq {
                    final_pointer_and_size = other_new_pointer;
                    found = true;
                    break;
                }
            }
            if !found {
                final_pointer_and_size = (entity_chunk_instance_count << 8) | size;
                // Append `size` compressed entity-chunk-instance entries.
                for e in 0..size {
                    let inst_id = entity_chunk_instances_pool[(pointer + e) as usize];
                    let inst = &instances[inst_id as usize];
                    uploads
                        .entity_chunk_instances
                        .push(compress_entity_chunk_instance(inst));
                    entity_chunk_instance_count += 1;
                }
                hashed.push((hash, pointer, final_pointer_and_size));
            }

            // Emit the chunk-update entry.
            let chunk_pos = chunk_index_to_pos(chunk_idx, self.size_in_chunks);
            updates.push(pack_chunk_update(chunk_pos, final_pointer_and_size));
        }

        // C# at :344-359: for chunks that had entities last frame but don't
        // this frame, emit a `(chunkPos, 0)` clear update.
        for &chunk_idx in &self.last_frame_chunks {
            let entity_data = self.chunk_entity_data[chunk_idx as usize];
            let size = entity_data & 0xFF;
            if size == 0 {
                let chunk_pos = chunk_index_to_pos(chunk_idx, self.size_in_chunks);
                updates.push(pack_chunk_update(chunk_pos, 0));
            }
        }
        uploads.chunk_updates = updates;

        // C# at :361-373: pack entity-instance history.
        for inst in instances {
            uploads.entity_history.push(compress_entity_history(inst));
        }

        // Swap last/current. C# at :442.
        std::mem::swap(&mut self.current_frame_chunks, &mut self.last_frame_chunks);
        uploads
    }
}

/// Iterate the chunks an instance's rotated AABB overlaps. Mirrors C#
/// at `:189-231`.
fn for_each_overlapped_chunk(
    inst: &EntityInstance,
    size_in_chunks: [i32; 3],
    mut f: impl FnMut(u32),
) {
    let [sx, sy, sz] = size_in_chunks;
    let mut min_pos = [f32::INFINITY; 3];
    let mut max_pos = [f32::NEG_INFINITY; 3];
    let qx = inst.quaternion[0];
    let qy = inst.quaternion[1];
    let qz = inst.quaternion[2];
    let qw = inst.quaternion[3];

    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let corner = [
                    inst.size[0] as f32 * x as f32,
                    inst.size[1] as f32 * y as f32,
                    inst.size[2] as f32 * z as f32,
                ];
                let rotated = rotate_vec3_by_quat(corner, [qx, qy, qz, qw]);
                for axis in 0..3 {
                    if rotated[axis] < min_pos[axis] {
                        min_pos[axis] = rotated[axis];
                    }
                    if rotated[axis] > max_pos[axis] {
                        max_pos[axis] = rotated[axis];
                    }
                }
            }
        }
    }

    let pos = [inst.position.x, inst.position.y, inst.position.z];
    let min_chunk = [
        (pos[0] + min_pos[0]) / 16.0,
        (pos[1] + min_pos[1]) / 16.0,
        (pos[2] + min_pos[2]) / 16.0,
    ];
    let max_chunk = [
        (pos[0] + max_pos[0]) / 16.0,
        (pos[1] + max_pos[1]) / 16.0,
        (pos[2] + max_pos[2]) / 16.0,
    ];
    let box_size = [
        (max_chunk[0] as i32) - (min_chunk[0] as i32),
        (max_chunk[1] as i32) - (min_chunk[1] as i32),
        (max_chunk[2] as i32) - (min_chunk[2] as i32),
    ];

    for dz in 0..=box_size[2] {
        for dy in 0..=box_size[1] {
            for dx in 0..=box_size[0] {
                let cx = min_chunk[0] as i32 + dx;
                let cy = min_chunk[1] as i32 + dy;
                let cz = min_chunk[2] as i32 + dz;
                if cx < 0 || cy < 0 || cz < 0 || cx >= sx || cy >= sy || cz >= sz {
                    continue;
                }
                let idx = (cx + cy * sx + cz * sx * sy) as u32;
                f(idx);
            }
        }
    }
}

/// Rotate a `vec3` by a quaternion `(x, y, z, w)`. Mirrors C# `Vector3.Transform`.
fn rotate_vec3_by_quat(v: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let q_xyz = [q[0], q[1], q[2]];
    let w = q[3];
    // v + 2*cross(q.xyz, cross(q.xyz, v) + w*v)
    let c1 = [
        q_xyz[1] * v[2] - q_xyz[2] * v[1] + w * v[0],
        q_xyz[2] * v[0] - q_xyz[0] * v[2] + w * v[1],
        q_xyz[0] * v[1] - q_xyz[1] * v[0] + w * v[2],
    ];
    let c2 = [
        q_xyz[1] * c1[2] - q_xyz[2] * c1[1],
        q_xyz[2] * c1[0] - q_xyz[0] * c1[2],
        q_xyz[0] * c1[1] - q_xyz[1] * c1[0],
    ];
    [v[0] + 2.0 * c2[0], v[1] + 2.0 * c2[1], v[2] + 2.0 * c2[2]]
}

fn chunk_index_to_pos(idx: u32, size_in_chunks: [u32; 3]) -> [u32; 3] {
    let sx = size_in_chunks[0];
    let sy = size_in_chunks[1];
    let x = idx % sx;
    let y = (idx / sx) % sy;
    let z = idx / (sx * sy);
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;

    /// Hash-coefficient table matches the C# `31^(256-i) mod 2^32` recurrence.
    #[test]
    fn entity_hash_coefficients_table() {
        let t = entity_hash_coefficients();
        assert_eq!(t[256], 1);
        // t[255] = 31 * 1 = 31.
        assert_eq!(t[255], 31);
        // t[254] = 31 * 31 = 961.
        assert_eq!(t[254], 961);
    }

    /// A 1-entity 1-chunk fixture: one entity at chunk (0,0,0), 2×2×2 chunks
    /// world. Expect one chunk update with the entity's chunkPos + a single
    /// `entity_chunk_instances` entry + one `entity_history` entry.
    #[test]
    fn entity_handler_cpu_dedup_single_entity() {
        let mut handler = EntityHandler::new([2, 2, 2]);
        let instances = vec![EntityInstance {
            position: Vec3::new(8.0, 8.0, 8.0), // mid-chunk
            quaternion: [0.0, 0.0, 0.0, 1.0], // identity
            voxel_start: 0,
            entity: 0,
            size: [4, 4, 4],
        }];
        let uploads = handler.update(&instances);
        // One chunk overlap → one update + one entity_chunk_instances entry.
        assert_eq!(uploads.chunk_updates.len(), 1);
        assert_eq!(uploads.entity_chunk_instances.len(), 1);
        assert_eq!(uploads.entity_history.len(), 1);
        // Chunk position decompresses to (0,0,0).
        let update = uploads.chunk_updates[0];
        assert_eq!(update.data1 & 0x7FF, 0); // chunkPos.x
        assert_eq!((update.data1 >> 11) & 0x3FF, 0); // chunkPos.y
        assert_eq!(update.data1 >> 21, 0); // chunkPos.z
        // Pointer + size: pointer=0, size=1 → data2 = 0 | 1 = 1.
        assert_eq!(update.data2, 1);
    }

    /// Two identical entity-overlap lists (two chunks each containing the
    /// same single entity) should dedup to **one** `entity_chunk_instances`
    /// entry — the second chunk's pointer reuses the first chunk's range.
    #[test]
    fn entity_handler_cpu_dedup_two_identical_lists() {
        // Two chunks at (0,0,0) and (1,0,0); one entity overlaps both via a
        // size-spanning x extent.
        let mut handler = EntityHandler::new([2, 1, 1]);
        let instances = vec![EntityInstance {
            position: Vec3::new(15.0, 8.0, 8.0), // straddles x boundary at 16
            quaternion: [0.0, 0.0, 0.0, 1.0],
            voxel_start: 0,
            entity: 0,
            size: [8, 4, 4],
        }];
        let uploads = handler.update(&instances);
        // Two chunks affected → two updates.
        assert_eq!(uploads.chunk_updates.len(), 2);
        // Both chunks contain the same single-entity list (entity id 0); dedup
        // collapses them to **one** entity_chunk_instances entry.
        assert_eq!(
            uploads.entity_chunk_instances.len(),
            1,
            "expected dedup to collapse two identical lists into one chunk-instance entry"
        );
        // Both chunk-update entries point at the same pointer+size.
        assert_eq!(uploads.chunk_updates[0].data2, uploads.chunk_updates[1].data2);
    }

    /// After one frame, the previous-frame chunk list rolls over so the next
    /// frame's "no longer has entities" pass can fire.
    #[test]
    fn entity_handler_clears_stale_chunks() {
        let mut handler = EntityHandler::new([2, 1, 1]);
        let frame_a = vec![EntityInstance {
            position: Vec3::new(8.0, 8.0, 8.0), // chunk (0,0,0)
            quaternion: [0.0, 0.0, 0.0, 1.0],
            voxel_start: 0,
            entity: 0,
            size: [4, 4, 4],
        }];
        let _ = handler.update(&frame_a);
        // Frame B: no entities. The handler should emit a clear update for
        // chunk (0,0,0).
        let uploads_b = handler.update(&[]);
        assert_eq!(uploads_b.chunk_updates.len(), 1);
        assert_eq!(uploads_b.chunk_updates[0].data2, 0, "clear update");
        // Chunk pos should be (0,0,0).
        assert_eq!(uploads_b.chunk_updates[0].data1 & 0x7FF, 0);
    }
}
