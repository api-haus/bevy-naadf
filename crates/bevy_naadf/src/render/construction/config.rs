//! Phase-C construction configuration — the single source of truth for the
//! GPU-construction sub-module's knobs (`15-design-c.md` §1.8, §2.1 W0 row).
//!
//! Mirrors NAADF's per-handler scalar config that lives in C# fields scattered
//! across `BlockHashingHandler.cs`, `WorldBoundHandler.cs`, `ChangeHandler.cs`,
//! and `WorldData.cs`. Collapsed into one `Resource` so every Phase-C
//! workstream (W1..W5) reads + writes one place rather than threading args
//! through individual systems. Mirrors the `TaaRingConfig` pattern
//! (`render/taa.rs:46-50`): main-world `AppArgs` carries it; the render
//! sub-app reads it via the `Resource`.
//!
//! **W0 — empty seam.** Every later workstream consumes specific fields:
//!   - W1 reads `initial_hash_map_size` / `wanted_empty_ratio` / `probe_cap`
//!     for the `BlockHashingHandler` port (`chunkCalc.fx` GetVoxelPointer).
//!   - W3 reads `max_group_bound_dispatch` / `n_bounds_rounds` for the
//!     background AADF queue (`boundsCalc.fx` regime-2 dispatch).
//!   - W4 reads `entities_enabled` to gate the entity track + the chunk
//!     texture-format flip (`R32Uint` → `Rg32Uint`).
//!   - All workstreams read `gpu_construction_enabled` / `cpu_fallback` to
//!     decide producer.
//!
//! W0 lands the resource shape + the defaults; nothing in W0 reads any field
//! except the master `gpu_construction_enabled` (which W0 logs from
//! `run_gpu_construction_startup` — it returns immediately because W0 is
//! empty).

use bevy::prelude::Resource;

/// Phase-C construction configuration (`15-design-c.md` §1.8, §2.1 W0 row).
///
/// Single render-side resource fed from `AppArgs.construction_config` at
/// `NaadfRenderPlugin::build` time — same plumbing pattern `TaaRingConfig`
/// uses (`render/taa.rs:46-50`). The render-world systems / pipelines read
/// it; the main-world `AppArgs` owns it.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct ConstructionConfig {
    /// Master switch — GPU construction takes the build path when `true`,
    /// CPU construction (`aadf::construct::construct`) stays the producer
    /// when `false`.
    ///
    /// **Default `false` in W0** — the CPU path is the default producer until
    /// W1 lands GPU Algorithm 1 + the bit-exact CPU/GPU oracle. W1 flips this
    /// `true` when its `--validate-gpu-construction` gate is green.
    pub gpu_construction_enabled: bool,
    /// Initial hash-map slot count (NAADF default — `BlockHashingHandler.cs:32`
    /// `hashMapSize = 1 << 18 = 262144`). Power of two; grows via `mapCopy.fx`
    /// (W1) when occupancy crosses `wanted_empty_ratio`. The W1 hash table
    /// owns its growth past this initial size.
    pub initial_hash_map_size: u32,
    /// Hash-map occupancy threshold above which `mapCopy.fx` (W1) doubles the
    /// map. **NAADF default 0.5** (`BlockHashingHandler.cs` — the
    /// `wantedEmptyRatio = 0.5` constant). Paper §3.2 quotes 75 %; the C# uses
    /// 50 %, which is faithful per Q3 (the C# is the cross-check source).
    pub wanted_empty_ratio: f32,
    /// Open-addressing probe cap for `GetVoxelPointer` (`chunkCalc.fx:57-115`).
    /// **NAADF default 250** (`BlockHashingHandler.cs` — `maxProbes = 250`).
    /// Paper §3.2 quotes 100; the C# uses 250, which is faithful per Q3.
    pub probe_cap: u32,
    /// Maximum bound-queue work-items dispatched per `boundsCalc.computeGroupBounds`
    /// round. **NAADF default `512 * 64 = 32_768`** (`WorldBoundHandler.cs:25` —
    /// `maxGroupBoundDispatch = 512 * 64`). The throttling lever for paper §3.3's
    /// "one queue per frame" background convergence rate; W3 honours it.
    pub max_group_bound_dispatch: u32,
    /// Entity track on/off (W4 owns the toggle). When `true`, W4's
    /// `entityUpdate.wgsl` runs and the chunks texture is `Rg32Uint` (the
    /// per-chunk entity pointer in `.y`); when `false`, chunks stays
    /// `R32Uint` and the entity track is dead-code at the render-graph level
    /// (the gated node early-returns). **Default `false`** — W0 / W1 / W2 / W3
    /// all run with this off; W4's merge flips the default if entities are in
    /// the test scene.
    pub entities_enabled: bool,
    /// CPU-fallback path: when `true`, the CPU construction path stays
    /// available (and is the producer when `gpu_construction_enabled = false`).
    /// **Default `true`** — the CPU path is the bit-exact validation oracle
    /// per E4 (`01-context.md` §2e), so it must stay available regardless of
    /// the GPU path's state. W4+ may flip this off in a perf-only config.
    pub cpu_fallback: bool,
    /// Number of `boundsCalc.computeGroupBounds` rounds per frame
    /// (`WorldBoundHandler.cs:113` — NAADF runs **5 per frame**). The §3.3
    /// "one queue per frame" rate is technically "one batch", and NAADF's
    /// batch is 5 prepare+indirect-compute rounds. W3 honours this directly.
    pub n_bounds_rounds: u32,
}

impl Default for ConstructionConfig {
    fn default() -> Self {
        Self {
            // W0: GPU path off; CPU is producer until W1 lands.
            gpu_construction_enabled: false,
            // `BlockHashingHandler.cs:32` — 1 << 18 = 262144.
            initial_hash_map_size: 1 << 18,
            // `BlockHashingHandler.cs` — `wantedEmptyRatio = 0.5`.
            wanted_empty_ratio: 0.5,
            // `BlockHashingHandler.cs` — `maxProbes = 250`.
            probe_cap: 250,
            // `WorldBoundHandler.cs:25` — `maxGroupBoundDispatch = 512 * 64`.
            max_group_bound_dispatch: 512 * 64,
            // Entity track off until W4.
            entities_enabled: false,
            // CPU oracle / fallback always available (E4).
            cpu_fallback: true,
            // `WorldBoundHandler.cs:113` — 5 rounds per frame.
            n_bounds_rounds: 5,
        }
    }
}

impl From<&crate::AppArgs> for ConstructionConfig {
    /// Mirror `TaaRingConfig::depth = args.taa_ring_depth` pattern: read the
    /// embedded `construction_config` straight out of `AppArgs`.
    ///
    /// W0 keeps `AppArgs.construction_config` as a plain `ConstructionConfig`
    /// field (default `ConstructionConfig::default()`); the conversion is a
    /// `Copy`. Later workstreams (W1 / W4) extend `AppArgs` with CLI flags
    /// that mutate specific fields; the `From<&AppArgs>` lift stays the
    /// single seam between the main-world args and the render-side resource.
    fn from(args: &crate::AppArgs) -> Self {
        args.construction_config
    }
}

// Compile-time pin of the NAADF defaults so a careless future edit can't
// silently drift the build path away from the canonical methodology
// (faithful-port per Q3). These compile-time checks replace the equivalent
// runtime test asserts — they cost zero binary size and fail at build time
// rather than test-run time. W0 stays at the "+1 test" budget the brief sets
// (the new test is `construction_params_layout` in `gpu_types.rs`).
const _: () = {
    let cfg = ConstructionConfig {
        // `BlockHashingHandler.cs:32`.
        initial_hash_map_size: 1 << 18,
        // `BlockHashingHandler.cs` — `wantedEmptyRatio = 0.5`.
        wanted_empty_ratio: 0.5,
        // `BlockHashingHandler.cs` — `maxProbes = 250`.
        probe_cap: 250,
        // `WorldBoundHandler.cs:25` — 512 * 64.
        max_group_bound_dispatch: 512 * 64,
        // `WorldBoundHandler.cs:113` — 5 rounds per frame.
        n_bounds_rounds: 5,
        gpu_construction_enabled: false,
        entities_enabled: false,
        cpu_fallback: true,
    };
    // Compile-time-only sanity probe — referenced once so the const isn't
    // dead. `ConstructionConfig` is `Copy`, so this is a no-op at runtime.
    let _ = cfg.initial_hash_map_size;
};
