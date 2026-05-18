//! Port of C# `BlockHashingHandler` (`World/Data/BlockHashingHandler.cs`).
//!
//! Open-addressed hashmap of `(voxel_data_hash → voxel_slot_pointer,
//! use_count)`. Content-addressable storage for mixed blocks' 32-u32 voxel
//! payloads so identical block content shares one slot in `voxels_cpu`.
//!
//! ## Why this exists in the port
//!
//! Before the port had this, every mixed block of an edited chunk was
//! re-uploaded on every edit (`02c` Divergence #4 — "simplified port appends
//! fresh voxel slots"). For a chunk with 26 mixed blocks, a single-voxel
//! edit pushed 26 × 33 u32s to the GPU `apply_voxel_change` dispatch. That
//! shader is **additive** on bits 0-11 (the AADF fields) and grows them by
//! `+1` per axis per iteration — it relies on input AADFs being 0 to
//! produce correct output. Repeated re-uploads of already-augmented data
//! over-grow the AADFs, overflowing the 2-bit fields and corrupting the
//! empty-voxel state. Visible as inside-out / partially-opaque shapes
//! (`--small-edit-repro` regression captured 2026-05-17).
//!
//! With this handler:
//!   - Unchanged blocks reuse the existing slot (no upload, no AADF growth).
//!   - Identical content across chunks shares one slot (memory savings).
//!   - Refcount drops trigger slot freeing — no per-edit leak.
//!
//! ## Differences from C#
//!
//! - **CPU-only**: the C# port has a GPU `StructuredBuffer<BlockValue>` and
//!   a `mapCopy` shader to rehash on resize because the renderer-side
//!   `worldChange.fx` reads the map atomically. Bevy's edit pipeline runs
//!   entirely on CPU and the GPU only sees the resolved voxel pointers, so
//!   the handler stays CPU-side. (`mapCopy.wgsl` exists in the port but
//!   serves the W1 GPU producer's hash-dedup path — separate from this
//!   editing handler.)
//! - **No `_resizeLock`**: single-threaded calls (the per-chunk parallel
//!   path in `set_voxels_batch` falls back to sequential when the
//!   `ComputeTaskPool` isn't available; a future parallel run wraps the
//!   handler in a `Mutex`).
//! - **`voxels_pointer = 0` as empty-slot sentinel**: matches C#. The first
//!   real voxel slot must therefore be at offset > 0 — `WorldData::construct`
//!   already seeds `voxels_cpu` with the "all-empty" sentinel at offset 0
//!   so this is naturally true; if a future change starts a chunk with a
//!   mixed block at voxel offset 0 the seed would clobber the sentinel.

use std::collections::VecDeque;

/// 32 packed-voxel-pair u32s per block (64 voxels). Matches C#'s 32 in
/// `Array.Copy(..., 32)`.
pub const BLOCK_VOXEL_PAIRS: usize = 32;

/// One slot in the open-addressed hashmap.
///
/// `voxels_pointer == 0` is the "empty slot" sentinel — matches C# struct
/// default (`voxelsPointer = 0x0`).
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockHashEntry {
    /// Offset into `voxels_cpu` where this block's 32 u32s live. 0 = unused.
    pub voxels_pointer: u32,
    /// Reference count — how many block positions across the world point at
    /// this slot. Reaches 0 → slot freed.
    pub use_count: u32,
    /// Cached hash for hashmap probing — avoids re-hashing on resize.
    pub hash: u32,
}

/// Open-addressed hashmap from voxel-block hash → voxel slot pointer.
#[derive(Debug)]
pub struct BlockHashingHandler {
    /// Hash table — size always a power of two so `hash & (size - 1)`
    /// indexes uniformly.
    map: Vec<BlockHashEntry>,
    /// Power-of-two table size (kept in sync with `map.len()`).
    map_size: usize,
    /// Polynomial hash coefficients — `coefficients[i] = 31^(64-i)`.
    /// 65 entries: 64 voxel half-words + 1 leading constant.
    coefficients: [u32; 65],
    /// Slots whose `use_count` has dropped to 0 — reused before extending
    /// `voxels_cpu`.
    pub free_voxel_slots: VecDeque<u32>,
    /// Number of slots currently in use — for the resize trigger.
    used_count: u32,
    /// Minimum free slot reserve before resize triggers.
    min_reserved: u32,
    /// Load-factor target — when used_count + min_reserved exceeds this
    /// fraction of map_size, the map doubles. Matches C# default 0.5.
    wanted_empty_ratio: f32,
    /// Linear-probe cap — matches C# `count < 250`. Beyond this the map
    /// is presumed full / pathologically clustered; panics on overflow.
    probe_cap: u32,
}

impl BlockHashingHandler {
    /// Create a fresh handler with the default starting size (256 slots,
    /// matches C# `startSizeMap` after the `mapSize * wantedEmptyRatio <
    /// minReservedCount` doubling loop with `startSizeMap = 0`).
    pub fn new() -> Self {
        Self::with_size(256)
    }

    /// Create a handler with a specific starting table size. The size is
    /// rounded up to a power of two and to at least `2 * minReserved /
    /// wantedEmptyRatio` (so the initial doubling-on-insert is rare).
    pub fn with_size(start_size: usize) -> Self {
        let mut map_size = start_size.max(1).next_power_of_two();
        let min_reserved: u32 = 64;
        let wanted_empty_ratio: f32 = 0.5;
        while (map_size as f32) * wanted_empty_ratio < min_reserved as f32 {
            map_size *= 2;
        }
        let coefficients = build_polynomial_coefficients();
        Self {
            map: vec![BlockHashEntry::default(); map_size],
            map_size,
            coefficients,
            free_voxel_slots: VecDeque::new(),
            used_count: 0,
            min_reserved,
            wanted_empty_ratio,
            probe_cap: 250,
        }
    }

    /// Compute the polynomial hash of a 32-u32 voxel block.
    ///
    /// Mirrors C# `getHashOfBlock` exactly:
    /// `hash = coef[0] + Σ coef[v*2+1] * (lo & 0x7FFF) + coef[v*2+2] *
    /// (hi & 0x7FFF)` over v = 0..32. The `& 0x7FFF` masks out the full-flag
    /// bit so semantically-identical blocks with different full-flag bits
    /// would hash the same — but the full-flag bit is always set
    /// consistently per voxel-type so this isn't a real collision source.
    pub fn compute_hash(&self, voxel_pairs: &[u32]) -> u32 {
        debug_assert_eq!(voxel_pairs.len(), BLOCK_VOXEL_PAIRS);
        let mut hash = self.coefficients[0];
        for v in 0..BLOCK_VOXEL_PAIRS {
            let pair = voxel_pairs[v];
            let lo = pair & 0x7FFF;
            let hi = (pair >> 16) & 0x7FFF;
            hash = hash.wrapping_add(self.coefficients[v * 2 + 1].wrapping_mul(lo));
            hash = hash.wrapping_add(self.coefficients[v * 2 + 2].wrapping_mul(hi));
        }
        hash
    }

    /// Look up or insert a block's voxel slot. Mirrors C# `AddBlock`.
    ///
    /// Returns `(voxel_pointer, is_new)`:
    ///   - `is_new == false`: the block already existed in the map — its
    ///     `use_count` was incremented, no new slot allocated.
    ///   - `is_new == true`: a fresh slot was allocated (from the free list
    ///     or by extending `voxels_cpu`) and `voxel_pairs` was copied into
    ///     it. The caller must push the slot's data to the GPU upload queue.
    ///
    /// The match check is **byte-equal** on the 32 u32s, not just hash
    /// equality — hash collisions are handled via linear probing.
    pub fn add_block(
        &mut self,
        hash: u32,
        voxel_pairs: &[u32],
        voxels_cpu: &mut Vec<u32>,
    ) -> (u32, bool) {
        debug_assert_eq!(voxel_pairs.len(), BLOCK_VOXEL_PAIRS);
        let mask = (self.map_size as u32).wrapping_sub(1);
        let mut idx = (hash & mask) as usize;
        for _ in 0..self.probe_cap {
            let entry = self.map[idx];
            if entry.voxels_pointer == 0 {
                // Empty slot → allocate a fresh voxel slot, register.
                let ptr = self.alloc_voxel_slot(voxel_pairs, voxels_cpu);
                self.map[idx] = BlockHashEntry {
                    voxels_pointer: ptr,
                    use_count: 1,
                    hash,
                };
                self.used_count += 1;
                self.resize_if_needed();
                return (ptr, true);
            }
            if entry.hash == hash {
                let base = entry.voxels_pointer as usize;
                let existing = &voxels_cpu[base..base + BLOCK_VOXEL_PAIRS];
                if existing == voxel_pairs {
                    self.map[idx].use_count += 1;
                    return (entry.voxels_pointer, false);
                }
                // Hash collision — keep probing.
            }
            idx = ((idx as u32 + 1) & mask) as usize;
        }
        panic!(
            "BlockHashingHandler::add_block: linear probe exceeded {} slots — \
             hashmap is either full or pathologically clustered (used={}, size={})",
            self.probe_cap, self.used_count, self.map_size
        );
    }

    /// Register an EXISTING (already-in-`voxels_cpu`) slot in the hash table
    /// without allocating new storage. Used by [`crate::world::data::WorldData::
    /// seed_block_hashing`] after a fresh world load (or post-GPU-readback) to
    /// teach the handler about pre-existing block slots so subsequent
    /// edit-time `add_block` / `delete_block` calls see correct refcounts AND
    /// dedup against the **original** pointers the renderer / GPU `blocks[]`
    /// already references.
    ///
    /// Behaviour difference from [`Self::add_block`]:
    ///
    /// - On first-occurrence (`is_new = true`): registers `existing_ptr`
    ///   directly. No allocation, no extension of `voxels_cpu`, no copy of
    ///   `voxel_pairs` into the buffer. The hash entry's `voxels_pointer`
    ///   equals `existing_ptr` — matching the pointer already stored in
    ///   `blocks_cpu[block_idx]` (and on GPU's `blocks[]`).
    /// - On dedup hit (`is_new = false`): returns the **earlier** seeded
    ///   pointer (which is also somewhere in the 0-N range that GPU
    ///   `voxels[]` was populated with at construction). Caller patches
    ///   `blocks_cpu[block_idx]` to that pointer if it differs from
    ///   `existing_ptr` — both pointers reference identical voxel content,
    ///   GPU's `voxels[]` has correct data at both addresses, so the patch
    ///   is purely a CPU-side dedup that converges the CPU mirror onto the
    ///   handler's choice of canonical slot.
    ///
    /// Hash-collision tie-breaking uses byte-equality against the
    /// already-registered entry's content (read from `voxels_cpu` —
    /// **immutable borrow only**). Same probe semantics as `add_block`.
    ///
    /// Returns `(registered_ptr, is_new)`:
    ///   - `registered_ptr` — the pointer registered in the hash entry
    ///     (either `existing_ptr` itself, or the earlier seeded canonical
    ///     pointer on dedup).
    ///   - `is_new` — `true` if this is the first time the content has been
    ///     registered, `false` if it dedup-merged with an earlier entry.
    ///
    /// Cross-references:
    ///
    /// - Bug 1 of `docs/orchestrate/vox-gpu-rewrite/17-diagnostic-residual-
    ///   speckle-and-brush-clears.md`. `add_block`'s `alloc_voxel_slot` branch
    ///   appended a duplicate copy of seeded content to the end of
    ///   `voxels_cpu` and stored that END pointer in the hash entry; the
    ///   GPU's `voxels[]` was never written at those END addresses. Edit-time
    ///   `add_block` calls for unchanged blocks then returned the END pointer
    ///   → GPU `apply_block_change` wrote END pointer into `blocks[]` →
    ///   renderer descended into zero data → 16-voxel-wide void around brush.
    /// - C# `WorldData.cs:131-132`: shares a single `BlockHashingHandler`
    ///   instance between the GPU producer and the editor, so there is no
    ///   "seed from GPU readback" step in C# — the same handler reference
    ///   flows through both paths. The Rust port had to add a seed pass
    ///   after the GPU→CPU readback; this method makes that seed pass
    ///   semantically faithful (the handler's pointers match the GPU
    ///   `blocks[]`'s pointers, not duplicated appended copies).
    pub fn seed_block(
        &mut self,
        hash: u32,
        voxel_pairs: &[u32],
        existing_ptr: u32,
        voxels_cpu: &[u32],
    ) -> (u32, bool) {
        debug_assert_eq!(voxel_pairs.len(), BLOCK_VOXEL_PAIRS);
        let mask = (self.map_size as u32).wrapping_sub(1);
        let mut idx = (hash & mask) as usize;
        for _ in 0..self.probe_cap {
            let entry = self.map[idx];
            if entry.voxels_pointer == 0 {
                // Empty slot → register `existing_ptr` directly, no append.
                self.map[idx] = BlockHashEntry {
                    voxels_pointer: existing_ptr,
                    use_count: 1,
                    hash,
                };
                self.used_count += 1;
                self.resize_if_needed();
                return (existing_ptr, true);
            }
            if entry.hash == hash {
                let base = entry.voxels_pointer as usize;
                if base + BLOCK_VOXEL_PAIRS <= voxels_cpu.len() {
                    let existing = &voxels_cpu[base..base + BLOCK_VOXEL_PAIRS];
                    if existing == voxel_pairs {
                        self.map[idx].use_count += 1;
                        return (entry.voxels_pointer, false);
                    }
                }
                // Hash collision (or unreachable existing — keep probing).
            }
            idx = ((idx as u32 + 1) & mask) as usize;
        }
        panic!(
            "BlockHashingHandler::seed_block: linear probe exceeded {} slots — \
             hashmap is either full or pathologically clustered (used={}, size={})",
            self.probe_cap, self.used_count, self.map_size
        );
    }

    /// Decrement a slot's `use_count` by 1. Mirrors C# `DeleteBlock`.
    ///
    /// Returns `true` when the count dropped to 0 — the caller (or this
    /// handler internally) marks the slot as available for reuse via the
    /// `free_voxel_slots` queue. Returns `false` if the slot's count is
    /// still positive OR if the slot wasn't found in the map.
    ///
    /// The slot search probes by `pointer` (not hash) along the chain that
    /// starts at `hash & mask` — same as C#.
    pub fn delete_block(&mut self, hash: u32, pointer: u32) -> bool {
        if pointer == 0 {
            // The sentinel — never registered, nothing to delete.
            return false;
        }
        let mask = (self.map_size as u32).wrapping_sub(1);
        let mut idx = (hash & mask) as usize;
        for _ in 0..self.probe_cap {
            let entry = self.map[idx];
            if entry.voxels_pointer == 0 {
                // Hit an empty slot in the probe chain — pointer isn't here.
                return false;
            }
            if entry.voxels_pointer == pointer {
                let new_count = entry.use_count.saturating_sub(1);
                if new_count == 0 {
                    self.map[idx].voxels_pointer = 0;
                    self.map[idx].use_count = 0;
                    self.map[idx].hash = 0;
                    self.free_voxel_slots.push_back(pointer);
                    self.used_count = self.used_count.saturating_sub(1);
                    return true;
                } else {
                    self.map[idx].use_count = new_count;
                    return false;
                }
            }
            idx = ((idx as u32 + 1) & mask) as usize;
        }
        false
    }

    /// Number of distinct voxel slots currently registered.
    pub fn used_count(&self) -> u32 {
        self.used_count
    }

    /// Current map capacity (power of two).
    pub fn map_size(&self) -> usize {
        self.map_size
    }

    fn alloc_voxel_slot(
        &mut self,
        voxel_pairs: &[u32],
        voxels_cpu: &mut Vec<u32>,
    ) -> u32 {
        if let Some(reuse) = self.free_voxel_slots.pop_front() {
            let base = reuse as usize;
            voxels_cpu[base..base + BLOCK_VOXEL_PAIRS].copy_from_slice(voxel_pairs);
            return reuse;
        }
        let ptr = voxels_cpu.len() as u32;
        voxels_cpu.extend_from_slice(voxel_pairs);
        ptr
    }

    fn resize_if_needed(&mut self) {
        let limit = (self.map_size as f32) * self.wanted_empty_ratio;
        if (self.used_count + self.min_reserved) as f32 <= limit {
            return;
        }
        let mut new_size = self.map_size * 2;
        while (new_size as f32) * self.wanted_empty_ratio
            < (self.used_count + self.min_reserved) as f32
        {
            new_size *= 2;
        }
        let new_mask = (new_size as u32).wrapping_sub(1);
        let mut new_map = vec![BlockHashEntry::default(); new_size];
        for entry in &self.map {
            if entry.voxels_pointer == 0 {
                continue;
            }
            let mut idx = (entry.hash & new_mask) as usize;
            // The new map is at least 2× the old, so the new used_count is
            // ≤ load factor — linear probing terminates without overflow.
            loop {
                if new_map[idx].voxels_pointer == 0 {
                    new_map[idx] = *entry;
                    break;
                }
                idx = ((idx as u32 + 1) & new_mask) as usize;
            }
        }
        self.map = new_map;
        self.map_size = new_size;
    }
}

impl Default for BlockHashingHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn build_polynomial_coefficients() -> [u32; 65] {
    let mut c = [0u32; 65];
    c[64] = 1;
    let mut i = 64;
    while i > 0 {
        i -= 1;
        c[i] = 31u32.wrapping_mul(c[i + 1]);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_block(seed: u8) -> Vec<u32> {
        // 32 u32s with a pattern that varies with `seed`.
        (0..BLOCK_VOXEL_PAIRS as u32)
            .map(|i| (seed as u32).wrapping_mul(0x1234_5678).wrapping_add(i))
            .collect()
    }

    /// New handler is empty + has the C# polynomial coefficients.
    #[test]
    fn coefficients_match_csharp_polynomial() {
        let h = BlockHashingHandler::new();
        // coef[64] = 1, coef[63] = 31, coef[62] = 31² = 961, etc.
        assert_eq!(h.coefficients[64], 1);
        assert_eq!(h.coefficients[63], 31);
        assert_eq!(h.coefficients[62], 31u32.wrapping_mul(31));
        assert_eq!(h.coefficients[61], 31u32.wrapping_mul(31).wrapping_mul(31));
    }

    /// Two equal blocks hash identically.
    #[test]
    fn equal_blocks_hash_equal() {
        let h = BlockHashingHandler::new();
        let a = make_block(7);
        let b = make_block(7);
        assert_eq!(h.compute_hash(&a), h.compute_hash(&b));
    }

    /// First-time add allocates a slot + marks is_new.
    #[test]
    fn add_block_first_time_is_new() {
        let mut h = BlockHashingHandler::new();
        let mut voxels = vec![0u32]; // sentinel slot at 0
        let block = make_block(1);
        let hash = h.compute_hash(&block);
        let (ptr, is_new) = h.add_block(hash, &block, &mut voxels);
        assert!(is_new);
        assert_eq!(ptr, 1, "first slot follows the sentinel at offset 0");
        assert_eq!(voxels.len(), 1 + BLOCK_VOXEL_PAIRS);
        assert_eq!(&voxels[1..], &block[..]);
        assert_eq!(h.used_count(), 1);
    }

    /// Second add of identical content returns the same pointer, no slot.
    #[test]
    fn add_block_dedup_returns_existing() {
        let mut h = BlockHashingHandler::new();
        let mut voxels = vec![0u32];
        let block = make_block(1);
        let hash = h.compute_hash(&block);
        let (ptr1, new1) = h.add_block(hash, &block, &mut voxels);
        let (ptr2, new2) = h.add_block(hash, &block, &mut voxels);
        assert!(new1);
        assert!(!new2);
        assert_eq!(ptr1, ptr2);
        // No extra slots allocated.
        assert_eq!(voxels.len(), 1 + BLOCK_VOXEL_PAIRS);
        assert_eq!(h.used_count(), 1);
    }

    /// Different content with the same hash still falls through to a new slot
    /// (collision handling via linear probing).
    #[test]
    fn add_block_handles_hash_collisions_via_probing() {
        let mut h = BlockHashingHandler::new();
        let mut voxels = vec![0u32];
        let a = make_block(1);
        let b = make_block(2);
        let hash_a = h.compute_hash(&a);
        let hash_b = h.compute_hash(&b);
        let (ptr_a, _) = h.add_block(hash_a, &a, &mut voxels);
        // Force a collision: add `b` at the SAME hash slot — the probe finds
        // the slot occupied with non-matching content and probes next.
        let (ptr_b, is_new_b) = h.add_block(hash_a, &b, &mut voxels);
        assert!(is_new_b);
        assert_ne!(ptr_a, ptr_b);
        // Both `a` and `b` still round-trip cleanly.
        let (ptr_a2, _) = h.add_block(hash_a, &a, &mut voxels);
        let (ptr_b2, _) = h.add_block(hash_a, &b, &mut voxels);
        assert_eq!(ptr_a, ptr_a2);
        assert_eq!(ptr_b, ptr_b2);
        // Hash `b` correctly even though it was added under `hash_a` — the
        // probe along `hash_b` chain finds it (or doesn't, depending on
        // collision pattern; this just checks the API is stable).
        let _ = hash_b;
    }

    /// Delete decrements use_count; only frees when it hits zero.
    #[test]
    fn delete_block_decrements_then_frees() {
        let mut h = BlockHashingHandler::new();
        let mut voxels = vec![0u32];
        let block = make_block(3);
        let hash = h.compute_hash(&block);
        let (ptr, _) = h.add_block(hash, &block, &mut voxels);
        // Add again → use_count = 2.
        let _ = h.add_block(hash, &block, &mut voxels);
        // First delete: count goes 2 → 1, NOT freed.
        let freed_first = h.delete_block(hash, ptr);
        assert!(!freed_first);
        assert!(h.free_voxel_slots.is_empty());
        // Second delete: count goes 1 → 0, freed.
        let freed_second = h.delete_block(hash, ptr);
        assert!(freed_second);
        assert_eq!(h.free_voxel_slots.front().copied(), Some(ptr));
    }

    /// Freed slot is reused on next add — `voxels_cpu` doesn't grow.
    #[test]
    fn freed_slot_reused_on_next_add() {
        let mut h = BlockHashingHandler::new();
        let mut voxels = vec![0u32];
        let a = make_block(4);
        let hash_a = h.compute_hash(&a);
        let (ptr_a, _) = h.add_block(hash_a, &a, &mut voxels);
        // Free A.
        assert!(h.delete_block(hash_a, ptr_a));
        let len_after_free = voxels.len();
        // Add B (different content) — should reuse A's slot.
        let b = make_block(5);
        let hash_b = h.compute_hash(&b);
        let (ptr_b, is_new) = h.add_block(hash_b, &b, &mut voxels);
        assert!(is_new);
        assert_eq!(ptr_b, ptr_a, "B should reuse A's freed slot");
        assert_eq!(voxels.len(), len_after_free, "voxels_cpu must not grow");
        assert_eq!(
            &voxels[ptr_b as usize..ptr_b as usize + BLOCK_VOXEL_PAIRS],
            &b[..]
        );
    }

    /// `seed_block` registers an existing pointer without appending to
    /// `voxels_cpu`, and edit-time `add_block` for the same content returns
    /// THAT pointer (not an appended-end pointer). Regression guard for
    /// Bug 1 of `vox-gpu-rewrite/17-diagnostic-residual-speckle-and-brush-
    /// clears.md`.
    #[test]
    fn seed_block_preserves_existing_pointer_and_dedup_works() {
        let mut h = BlockHashingHandler::new();
        // Simulate post-construction `voxels_cpu` with the sentinel + one
        // mixed block at offset 1 (its "original" pointer the GPU also has).
        let mut voxels = vec![0u32];
        let block = make_block(7);
        voxels.extend_from_slice(&block);
        let existing_ptr = 1u32;
        let hash = h.compute_hash(&block);
        let voxels_len_before = voxels.len();
        // Seed the handler with the existing slot.
        let (registered_ptr, is_new) =
            h.seed_block(hash, &block, existing_ptr, &voxels);
        assert!(is_new, "first seed of a unique content is is_new=true");
        assert_eq!(
            registered_ptr, existing_ptr,
            "seed_block must register the original pointer, not append a duplicate"
        );
        assert_eq!(
            voxels.len(),
            voxels_len_before,
            "seed_block must NOT extend voxels_cpu"
        );
        assert_eq!(h.used_count(), 1);
        // Now simulate an edit-time `add_block` on the unchanged content.
        // It must return the original pointer (not append + return an end
        // pointer). This is the exact bug-1 scenario.
        let (edit_ptr, edit_is_new) =
            h.add_block(hash, &block, &mut voxels);
        assert!(!edit_is_new, "unchanged content must dedup to seeded slot");
        assert_eq!(
            edit_ptr, existing_ptr,
            "edit-time add_block on unchanged content must return the original (seeded) pointer"
        );
        assert_eq!(
            voxels.len(),
            voxels_len_before,
            "edit-time dedup must NOT extend voxels_cpu"
        );
    }

    /// Resize triggers when load factor exceeds the threshold; all prior
    /// entries remain reachable.
    #[test]
    fn resize_preserves_all_entries() {
        // Tiny starting size so the resize triggers quickly.
        let mut h = BlockHashingHandler::with_size(4);
        let initial_size = h.map_size();
        let mut voxels = vec![0u32];
        // Add 200 distinct blocks to force several doublings.
        let mut hashes_and_ptrs: Vec<(u32, u32)> = Vec::new();
        for seed in 1..=200u8 {
            let block = make_block(seed);
            let hash = h.compute_hash(&block);
            let (ptr, _) = h.add_block(hash, &block, &mut voxels);
            hashes_and_ptrs.push((hash, ptr));
        }
        assert!(h.map_size() > initial_size, "map should have grown");
        // Re-lookup every block — should all return the same pointer.
        for (i, (hash, ptr)) in hashes_and_ptrs.iter().enumerate() {
            let block = make_block((i + 1) as u8);
            let (re_ptr, is_new) = h.add_block(*hash, &block, &mut voxels);
            assert!(!is_new, "block #{i} should already exist post-resize");
            assert_eq!(re_ptr, *ptr, "ptr stable across resize for block #{i}");
        }
    }
}
