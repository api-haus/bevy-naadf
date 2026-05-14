//! `WorldPlugin` — wires the voxel-world resources, the GPU extract step, and
//! the render nodes (`03-design.md` §4).
//!
//! - [`data`] — the `WorldData` resource (the three CPU mirrors + sizes).
//! - [`buffer`] — the `GrowableBuffer<T>` abstraction.
//!
//! Filled by design §8 steps 7–8 (Batch 2).

pub mod buffer;
pub mod data;
