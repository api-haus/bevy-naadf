//! AADF — the Nested Axis-Aligned Distance Fields three-layer cell hierarchy.
//!
//! Re-derived from the paper (Q3), cross-checked against the C# bit layouts
//! verified in `02-research.md` §1.1.2. See `03-design.md` §2.
//!
//! - [`block_hash`] — content-addressable storage for mixed-block voxel
//!   payloads (port of C# `BlockHashingHandler`). Houses the SSoT-6
//!   [`block_hash::hash_coefficients`] table consumed by both the CPU
//!   block-dedup path and D5's GPU hash upload.
//! - [`bounds`] — CPU-side AADF cuboid expansion (paper §3.3).
//! - [`cell`] — `Chunk`/`Block`/`Voxel` cell encode/decode (bit layouts).
//! - [`construct`] — CPU-side dense-voxel → three-layer buffers + hash
//!   dedup. Produces the build-once world state.
//! - [`edit`] — Phase-C W2 CPU oracles for `world_change.wgsl` + the
//!   `process_edit_batch` editing-handler port.
//! - [`entity`] — Phase-C W4 CPU oracle for `entityHandler.fx` — packed-u32
//!   entity-voxel AADF kernel (faithful inline port of C# `EntityData`).
//! - [`generator`] — Phase-C W5 CPU oracle for `generatorModel.fx` (the GPU
//!   world generator that produces the input to Algorithm 1).

pub mod block_hash;
pub mod bounds;
pub mod cell;
pub mod construct;
pub mod edit;
pub mod entity;
pub mod generator;
