//! W1 — CPU-side hash-table bookkeeping for `chunk_calc.wgsl` /
//! `map_copy.wgsl` (`15-design-c.md` §2.1 W1, §1.4, §4.4).
//!
//! Ports the host-side half of NAADF's `BlockHashingHandler` (C#
//! `World/Data/BlockHashingHandler.cs`):
//!
//! - The 65-entry hash-coefficient table (`31^(64-i) mod 2^32`)
//!   — `BlockHashingHandler.cs:50-55`. Single immutable table, uploaded once
//!   to GPU at startup. Byte-for-byte identical to what NAADF computes; the
//!   tests below pin every value.
//! - The hash-map occupancy tracker — `BlockHashingHandler.cs:78-83` /
//!   `:177-201`. Tracks how many slots are claimed; when occupancy crosses
//!   `wanted_empty_ratio * map_size`, the CPU triggers a `map_copy.wgsl`
//!   dispatch that doubles the map and re-hashes. The size growth formula
//!   matches the C# verbatim:
//!
//!     ```text
//!     newSize = mapSize * 2;
//!     while newSize * wanted_empty_ratio < count + minReservedCount {
//!         newSize *= 2;
//!     }
//!     ```
//!
//! W1 ships the table + the tracker; the actual `map_copy` dispatch lives in
//! `render::construction::map_copy::dispatch_map_copy`. The tracker is consumed
//! by W1's `run_gpu_construction_startup` (decides whether to grow before
//! starting Algorithm 1) and by W2's editing path (decides whether to grow
//! between batches of edits).

/// Compute the 65-entry hash-coefficient table used by `chunkCalc.fx`'s 64-
/// voxel block hash (`BlockHashingHandler.cs:50-55`).
///
/// `c[64] = 1`; `c[i] = (c[i+1] * 31) mod 2^32` for `i = 63..0`. The hash of a
/// 64-voxel block is then:
///
/// ```text
/// H = c[0] + Σᵢ c[i*2+1] * (v[i] & 0x7FFF)
///          + c[i*2+2] * ((v[i] >> 16) & 0x7FFF)
/// ```
///
/// where `v[i]` is the i-th `u32` of the 32-element packed-voxel block
/// (`chunkCalc.fx:126-136`).
pub fn hash_coefficients() -> [u32; 65] {
    let mut c = [0u32; 65];
    c[64] = 1;
    for i in (0..64).rev() {
        c[i] = c[i + 1].wrapping_mul(31);
    }
    c
}

/// Compute the initial hash-map size for a given segment size, mirroring the
/// C# `BlockHashingHandler` constructor's doubling loop
/// (`BlockHashingHandler.cs:36-46`).
///
/// Starts at `max(1, start_size)`; doubles until
/// `size * wanted_empty_ratio >= min_reserved_count`. The result is always a
/// power of two (provided `start_size` is or `start_size <= 1`).
///
/// The C# uses `minReservedCount = (worldGenSegmentSizeInVoxels^3) / 32` for
/// the GPU construction path (`WorldData.cs:132`) — for the default
/// `worldGenSegmentSizeInVoxels = 64`, that is `64^3 / 32 = 8192`. The
/// `wantedEmptyRatio = 0.5` constant pushes the initial size to `16384` for
/// the default segment.
pub fn initial_map_size(start_size: u32, wanted_empty_ratio: f32, min_reserved_count: u32) -> u32 {
    let mut size = start_size.max(1);
    while (size as f32) * wanted_empty_ratio < min_reserved_count as f32 {
        size = size.saturating_mul(2);
    }
    size
}

/// CPU-side occupancy tracker for the GPU hash map (`BlockHashingHandler.cs:78-83`).
///
/// The C# wraps the same logic in `SetNewUsedCount(count)`: every time
/// `worldData.voxelCount` (the global voxel cursor) advances, the tracker
/// recomputes occupancy and fires an `IncreaseSizeToNewCount` if the
/// threshold is crossed. W1 ports this CPU side; the GPU dispatch path
/// (`render::construction::map_copy::dispatch_map_copy`) handles the actual
/// re-hash.
#[derive(Clone, Copy, Debug)]
pub struct HashMapOccupancyTracker {
    /// Current power-of-two map size (slot count).
    pub map_size: u32,
    /// `wantedEmptyRatio` — threshold above which the map regrows (NAADF
    /// default 0.5).
    pub wanted_empty_ratio: f32,
    /// `minReservedCount` — the minimum number of free slots to maintain
    /// (NAADF default `maxNewVoxelsPerGenSegment / 32`).
    pub min_reserved_count: u32,
}

impl HashMapOccupancyTracker {
    /// New tracker — runs the same doubling-loop constructor as
    /// `BlockHashingHandler` to seed `map_size`.
    pub fn new(start_size: u32, wanted_empty_ratio: f32, min_reserved_count: u32) -> Self {
        Self {
            map_size: initial_map_size(start_size, wanted_empty_ratio, min_reserved_count),
            wanted_empty_ratio,
            min_reserved_count,
        }
    }

    /// Decide whether to grow the map given a new used-slot count. Mirrors
    /// `SetNewUsedCount(count)` from `BlockHashingHandler.cs:78-83`:
    ///
    /// ```cs
    /// if (count + minReservedCount > wantedEmptyRatio * mapSize) {
    ///     IncreaseSizeToNewCount(count);
    /// }
    /// ```
    ///
    /// Returns `Some(new_size)` when growth is needed (the caller dispatches
    /// `map_copy.wgsl` against the new buffer); returns `None` when the
    /// current size still has headroom. On growth, updates `self.map_size`.
    pub fn check_and_grow(&mut self, used_count: u32) -> Option<u32> {
        let threshold = self.wanted_empty_ratio * (self.map_size as f32);
        if (used_count + self.min_reserved_count) as f32 <= threshold {
            return None;
        }
        // `BlockHashingHandler.cs:177-184` — double + keep doubling while the
        // threshold is still below `count + minReservedCount`.
        let mut new_size = self.map_size.saturating_mul(2);
        while (new_size as f32) * self.wanted_empty_ratio
            < (used_count + self.min_reserved_count) as f32
        {
            new_size = new_size.saturating_mul(2);
        }
        self.map_size = new_size;
        Some(new_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first few values of the hash-coefficient table — pinned against
    /// what `BlockHashingHandler.cs:50-55`'s loop produces under u32-wrapping
    /// multiplication by 31.
    #[test]
    fn hash_coefficients_first_few_values() {
        let c = hash_coefficients();
        // c[64] = 1 by definition.
        assert_eq!(c[64], 1);
        // c[63] = 31 * c[64] = 31.
        assert_eq!(c[63], 31);
        // c[62] = 31 * c[63] = 961.
        assert_eq!(c[62], 961);
        // c[61] = 31 * c[62] = 29791.
        assert_eq!(c[61], 29791);
        // c[0] = 31^64 mod 2^32 — a specific (but huge) wrapping value.
        // Computed independently against u32 wrapping multiplication.
        let mut expected: u32 = 1;
        for _ in 0..64 {
            expected = expected.wrapping_mul(31);
        }
        assert_eq!(c[0], expected);
    }

    /// Every `c[i]` should equal `31^(64 - i) mod 2^32` — the canonical formula
    /// from `BlockHashingHandler.cs:50-55`. This is the load-bearing invariant
    /// that the GPU hash function expects.
    #[test]
    fn hash_coefficients_match_31_pow_64_minus_i() {
        let c = hash_coefficients();
        for i in 0..=64usize {
            let exp = 64 - i;
            let mut pow: u32 = 1;
            for _ in 0..exp {
                pow = pow.wrapping_mul(31);
            }
            assert_eq!(c[i], pow, "c[{i}] != 31^{exp} mod 2^32");
        }
    }

    /// Reference hash test against a hand-crafted input: a 32-u32 array of
    /// zero voxels hashes to `c[0]` (all the per-voxel contributions are
    /// zero). The C# `chunkCalc.fx:126-136` hash function:
    ///
    /// ```hlsl
    /// hash = hashCoefficients[0];
    /// for i in 0..32:
    ///     hash += hashCoefficients[i*2+1] * (voxel & 0x7FFF);
    ///     hash += hashCoefficients[i*2+2] * ((voxel >> 16) & 0x7FFF);
    /// ```
    #[test]
    fn hash_of_zero_block_equals_c0() {
        let c = hash_coefficients();
        let voxels = [0u32; 32];
        let mut hash = c[0];
        for (i, &v) in voxels.iter().enumerate() {
            hash = hash.wrapping_add(c[i * 2 + 1].wrapping_mul(v & 0x7FFF));
            hash = hash.wrapping_add(c[i * 2 + 2].wrapping_mul((v >> 16) & 0x7FFF));
        }
        assert_eq!(hash, c[0]);
    }

    /// `initial_map_size` matches the C# doubling-loop. For the default test
    /// segment (`worldGenSegmentSizeInVoxels = 64` → `minReservedCount = 64^3 /
    /// 32 = 8192`, `wantedEmptyRatio = 0.5`), the loop starts at `start_size
    /// = 1` and doubles until `size * 0.5 >= 8192` — i.e. `size >= 16384`.
    #[test]
    fn initial_map_size_default_segment() {
        let size = initial_map_size(0, 0.5, 8192);
        assert_eq!(size, 16384);
        // Sanity: also starts at 1 when start_size is 1.
        let size = initial_map_size(1, 0.5, 8192);
        assert_eq!(size, 16384);
    }

    /// `HashMapOccupancyTracker::check_and_grow` — fires on threshold crossing.
    #[test]
    fn occupancy_tracker_fires_at_threshold() {
        // Map size 16384 + 0.5 ratio → threshold = 8192 free slots.
        let mut tracker = HashMapOccupancyTracker::new(0, 0.5, 8192);
        assert_eq!(tracker.map_size, 16384);
        // Used count 0 + minReserved 8192 = 8192 ≤ 8192 (the threshold) — no
        // growth on the boundary case. (The C# uses `>`, not `>=`.)
        assert!(tracker.check_and_grow(0).is_none());
        // Used count 1 + 8192 = 8193 > 8192 — grow.
        let new = tracker.check_and_grow(1).expect("should grow");
        assert_eq!(new, 32768);
        assert_eq!(tracker.map_size, 32768);
        // After the grow, the new threshold is 16384; 1 + 8192 = 8193 ≤ 16384
        // — no further growth.
        assert!(tracker.check_and_grow(1).is_none());
    }

    /// `check_and_grow` re-doubles past one growth step if necessary
    /// (`BlockHashingHandler.cs:180-184`).
    #[test]
    fn occupancy_tracker_doubles_past_one_step() {
        let mut tracker = HashMapOccupancyTracker::new(0, 0.5, 8192);
        // Used count 32768 + 8192 = 40960. Threshold at size 16384 is 8192;
        // at 32768 → 16384; at 65536 → 32768; at 131072 → 65536 ≥ 40960.
        let new = tracker.check_and_grow(32768).expect("should grow");
        assert_eq!(new, 131072);
        assert_eq!(tracker.map_size, 131072);
    }
}
