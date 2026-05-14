//! AADF — the Nested Axis-Aligned Distance Fields three-layer cell hierarchy.
//!
//! Re-derived from the paper (Q3), cross-checked against the C# bit layouts
//! verified in `02-research.md` §1.1.2. See `03-design.md` §2.
//!
//! - [`cell`] — `Chunk`/`Block`/`Voxel` cell encode/decode (bit layouts).
//! - [`construct`] — CPU-side dense-voxel → three-layer buffers + hash dedup.
//! - [`bounds`] — CPU-side AADF cuboid expansion.

pub mod bounds;
pub mod cell;
pub mod construct;
