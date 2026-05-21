//! NAADF's fixed startup-world dimensions, expressed in three derived units
//! (segments, chunks, voxels). The values are the C# canonical numbers
//! (`WorldHandler.cs:18-19`) and the relationships are derived `const fn`-style
//! so a single base-constant edit propagates without manual recomputation.

use bevy::math::UVec3;

/// C# `WorldHandler.worldSizeToUseInWorldGenSegments` (`WorldHandler.cs:19`).
///
/// NAADF's fixed startup world size, expressed in **WorldGenSegment** units.
/// One segment is `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 * 16` voxels per axis
/// (4 chunks per group × 16 voxels per chunk = 64 voxels per group × 4 groups
/// per segment = 256 voxels per segment). C# uses `(16, 2, 16)`, which gives
/// the canonical `(4096, 512, 4096)`-voxel world the original engine boots
/// into regardless of whether a model file is present.
pub const WORLD_SIZE_IN_SEGMENTS: UVec3 = UVec3::new(16, 2, 16);

/// C# `WorldHandler.worldGenSegmentSizeInGroups` (`WorldHandler.cs:18`). One
/// group is `4^3` chunks (= `64^3` voxels); this many groups per segment per
/// axis. Combined with [`WORLD_SIZE_IN_SEGMENTS`] this pins the fixed world
/// dimensions.
pub const WORLD_GEN_SEGMENT_SIZE_IN_GROUPS: u32 = 4;

/// `const fn` component-wise scalar multiply. `glam::UVec3 * u32` is not
/// `const`; this helper is.
const fn mul_uvec3(v: UVec3, k: u32) -> UVec3 {
    UVec3::new(v.x * k, v.y * k, v.z * k)
}

/// Derived: world size in chunks (`16 * 4 * 4 = 256`, `2 * 4 * 4 = 32`).
///
/// `WorldData.cs:64-65`: `sizeInVoxels = sizeInWorldGenSegments *
/// worldGenSegmentSizeInVoxels`, then `sizeInChunks = sizeInVoxels / 16`.
/// `WORLD_SIZE_IN_SEGMENTS × WORLD_GEN_SEGMENT_SIZE_IN_GROUPS × 4` (4 chunks
/// per group) collapses that into one expression.
pub const WORLD_SIZE_IN_CHUNKS: UVec3 =
    mul_uvec3(mul_uvec3(WORLD_SIZE_IN_SEGMENTS, WORLD_GEN_SEGMENT_SIZE_IN_GROUPS), 4);

/// Derived: world size in voxels (`256 * 16 = 4096`, `32 * 16 = 512`).
pub const WORLD_SIZE_IN_VOXELS: UVec3 = mul_uvec3(WORLD_SIZE_IN_CHUNKS, 16);

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the derived values against the C# canonical numbers
    /// (`WorldHandler.cs:18-19`). The derivation (segments × groups × 4 for
    /// chunks, × 16 again for voxels) is `const`-checked at compile time,
    /// so this test is solely a faithful-port guard.
    #[test]
    fn world_size_matches_csharp() {
        assert_eq!(WORLD_SIZE_IN_CHUNKS, UVec3::new(256, 32, 256));
        assert_eq!(WORLD_SIZE_IN_VOXELS, UVec3::new(4096, 512, 4096));
    }
}
