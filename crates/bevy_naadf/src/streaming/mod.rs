//! `streaming` — procedural-noise generation + sliding-window residency layer
//! for the streaming-world feature (`docs/orchestrate/streaming-world`).
//!
//! ## Phase 1 (this revision)
//!
//! - [`noise_fastnoiselite`] — WGSL FastNoiseLite port + GPU oracle runner.
//! - [`noise_fastnoiselite_cpu_oracle`] — Rust port of the same GLSL functions,
//!   used as the CPU reference for the `--wgsl-noise-oracle` e2e gate.
//!
//! Phase 1 is **purely compute-only and self-contained.** It does not touch the
//! renderer (`render::construction`), does not introduce a new `GridPreset`
//! variant, and does not depend on the `voxel_noise` crate at runtime. The only
//! consumers are unit tests + the new `--wgsl-noise-oracle` e2e gate
//! ([`crate::e2e::wgsl_noise_oracle`]).
//!
//! ## Phase 2 (future revision — not yet implemented)
//!
//! Adds the residency manager, the per-frame W5 gate inversion, the
//! `noise_terrain.wgsl` consumer of [`noise_fastnoiselite`], and the
//! `--streaming-window` e2e gate. See `02b-design-plan-b.md` §§ D-K.

pub mod noise_fastnoiselite;
pub mod noise_fastnoiselite_cpu_oracle;
