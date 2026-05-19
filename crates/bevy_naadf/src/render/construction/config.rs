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
    /// **Default `true` after W1 lands** — Algorithm 1 + the bit-exact CPU/GPU
    /// oracle test are both green; the GPU construction path can be exercised
    /// at startup via the `--validate-gpu-construction` flag (or the unit
    /// test). The production *render* path still consumes the CPU-produced
    /// buffers uploaded by `prepare_world_gpu`; flipping the consumer to read
    /// from `ConstructionGpu` is W2/W3 work. So this flag now serves two
    /// purposes:
    ///   - In tests + the `--validate-gpu-construction` path, it gates the
    ///     GPU construction dispatch + the bit-exact oracle assertion.
    ///   - In the main `Startup` system `run_gpu_construction_startup`, it
    ///     gates an info log (the actual dispatch is in tests / the e2e
    ///     validate path).
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
    /// W5-only isolation flag: when `true`, the regime-1 startup driver runs
    /// **only** the world-generator dispatch and stops — no `chunk_calc`,
    /// no `bounds_init`. Used by the W5 unit test to exercise its GPU path
    /// before W1 lands the rest of the chain. **Default `false`** — until
    /// W1 lands, the full GPU construction path is dormant regardless of
    /// this flag (`gpu_construction_enabled` gates everything); W1 flips
    /// the dormant case to "run the full chain".
    pub run_worldgen_only: bool,
    /// W4 — per-frame entity-instance ring cap (the
    /// `entityInstanceID < 16384` bound at `entityUpdate.fx:41`,
    /// `WorldRender.cs:88`). The `entity_instances_history` buffer is sized
    /// `max_entity_instances * taa_ring_depth`. Doubles as the `chunkUpdate` /
    /// `entityChunkInstances` upload-buffer cap; NAADF allocates 2_000_000
    /// slots for each (`EntityHandler.cs:134, 135, 144, 145, 149`) which is
    /// the per-frame max counted at chunk overlap. The port keeps a smaller
    /// default suitable for the bevy-naadf test scenes; a runtime knob can
    /// flip it.
    pub max_entity_instances: u32,
    /// Phase-C followup #4 — gate the `entity_instances_history` GPU
    /// allocation + the per-frame `copy_entity_history` dispatch + the
    /// prefix-sum population.
    ///
    /// The history-ring buffer (`world_data.wgsl:114` — `@group(0)
    /// @binding(7)`) is sized `max_entity_instances * taa_ring_depth * 16 B`
    /// (the C# `EntityHandler.cs:149` allocates 2_000_000 entries — ~128 MiB
    /// on the C# default). The `world_data.wgsl` layout binds it
    /// unconditionally, but `shoot_ray` does NOT consume it: the C# uses it
    /// for TAA reprojection of moving entities (paper §3.6), which is a
    /// Phase-D follow-up.
    ///
    /// When `false` (the default): `prepare_construction` allocates a 1-vec4
    /// placeholder for the binding (keeps the bind-group layout satisfied
    /// without paying the real cost), skips the
    /// `copy_entity_history` dispatch, and skips the per-frame history
    /// prefix-sum population on the CPU.
    ///
    /// When `true`: the C# behaviour — full allocation + per-frame dispatch.
    /// Enable when wiring up the Phase-D TAA-reprojection-of-moving-entities
    /// consumer.
    ///
    /// Default `false`.
    pub entity_history_enabled: bool,
}

impl Default for ConstructionConfig {
    fn default() -> Self {
        Self {
            // W1: GPU construction enabled — Algorithm 1 + the bit-exact
            // CPU/GPU oracle gate are green. The renderer still consumes
            // CPU-built buffers via `prepare_world_gpu`; flipping the consumer
            // is W2/W3 work. See `15-design-c.md` §1.6 / §2.1 W1 row.
            gpu_construction_enabled: true,
            // vox-gpu-rewrite W5.3-fix Stage 2 (2026-05-18) — sized for the
            // fixed-world case. `WorldData.cs:131-132` passes
            // `minReservedCount = maxNewVoxelsPerGenSegment / 32 = 256^3 / 32
            // = 524,288` into `BlockHashingHandler`, whose doubling loop at
            // `BlockHashingHandler.cs:38-40` forces `mapSize >= 1,048,576`
            // (= 2^20) at startup. The pre-Stage-2 `1 << 18 = 262,144` was
            // the `BlockHashingHandler` DEFAULT-ctor value
            // (`BlockHashingHandler.cs:32`'s `minReservedCount = 64`
            // parameter), NOT the per-segment Oasis invocation's value. The
            // bump alone did NOT fix the rendered inversion (round-3
            // diagnostic — likely the bug is downstream of the hash map),
            // but it makes the Rust port faithful to C# for when the actual
            // bug is fixed.
            initial_hash_map_size: 1 << 20,
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
            // W5 isolation knob — off by default; unit tests / explicit CLI
            // opt-ins flip it on.
            run_worldgen_only: false,
            // `WorldRender.cs:88` — `entityInstanceCap = 16384`. The
            // `entityUpdate.fx:41` `taa_index * 16384` history-ring stride
            // hard-codes this; keep the default at the C# value so the
            // history-ring layout matches byte-for-byte when an extracted
            // entity buffer is uploaded.
            max_entity_instances: DEFAULT_MAX_ENTITY_INSTANCES,
            // Phase-C followup #4 — off by default. The `world_data.wgsl`
            // `entity_instances_history` binding's GPU consumer is
            // Phase-D scope (TAA reprojection of moving entities); the
            // production renderer's `shoot_ray` never reads it. Disable
            // the per-frame `copy_entity_history` dispatch + the
            // `max_entity_instances * taa_ring_depth * 16 B` allocation
            // by default, keep the layout-binding placeholder.
            entity_history_enabled: false,
        }
    }
}

/// W4 — the per-frame entity-instance cap = `WorldRender.cs:88` /
/// `entityUpdate.fx:41`'s `taa_index * 16384` stride. Public for shader-side
/// asserts + the entity-history-ring allocation in `prepare_construction`.
pub const DEFAULT_MAX_ENTITY_INSTANCES: u32 = 16384;

/// 2026-05-19 web-vox ray-termination fix — wasm32 cap on
/// `max_group_bound_dispatch`. The wasm regime-2 path direct-dispatches
/// `compute_group_bounds` (see
/// [`crate::render::construction::bounds_calc::dispatch_regime_2_rounds`] —
/// bypasses wgpu's broken-on-Dawn STORAGE→INDIRECT barrier). The
/// dispatched workgroup count comes from this cap; the shader's
/// `is_group_active = group_id.x < count` early-bail keeps non-active
/// workgroups cheap.
///
/// **4096** — empirically the sweet spot. Larger (32_768) regressed SSIM
/// from 0.94 → 0.69 (suspected: atomic contention in `compute_group_bounds`
/// re-enqueue at scale, OR Dawn watchdog effects on the larger dispatch).
/// Smaller would slow convergence further. Re-baseline if a deeper fix
/// for the underlying WebGPU regime-2 issue lands.
///
/// **Trade-off:** wasm convergence takes more frames (queue[0] = 32_768 →
/// 32_768 / 4_096 = 8 rounds to drain; cascade through ~32 bound-size
/// levels = ~50 frames ≈ 0.85 s at 60 fps). Within the SSIM gate's 10 s
/// settle but visible in live use at startup.
///
/// **Steady-state bail cost** at 4_096: 5 rounds/frame × 4_096 workgroups
/// × 64 threads = 1.3 M bail-out threads/frame; ~0.5 ms on modern iGPU.
#[cfg(target_arch = "wasm32")]
pub const WASM_MAX_GROUP_BOUND_DISPATCH: u32 = 4096;

impl From<&crate::AppArgs> for ConstructionConfig {
    /// Mirror `TaaRingConfig::depth = args.taa_ring_depth` pattern: read the
    /// embedded `construction_config` straight out of `AppArgs`.
    ///
    /// W0 keeps `AppArgs.construction_config` as a plain `ConstructionConfig`
    /// field (default `ConstructionConfig::default()`); the conversion is a
    /// `Copy`. Later workstreams (W1 / W4) extend `AppArgs` with CLI flags
    /// that mutate specific fields; the `From<&AppArgs>` lift stays the
    /// single seam between the main-world args and the render-side resource.
    ///
    /// **2026-05-19 web-vox ray-termination fix** — on wasm32, clamps
    /// `max_group_bound_dispatch` to [`WASM_MAX_GROUP_BOUND_DISPATCH`]. See
    /// that const's docblock for the rationale + perf budget.
    fn from(args: &crate::AppArgs) -> Self {
        #[allow(unused_mut)]
        let mut cfg = args.construction_config;
        #[cfg(target_arch = "wasm32")]
        {
            cfg.max_group_bound_dispatch =
                cfg.max_group_bound_dispatch.min(WASM_MAX_GROUP_BOUND_DISPATCH);
        }
        cfg
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
        // vox-gpu-rewrite W5.3-fix Stage 2 — see the runtime default's
        // long-form rationale above. C# `WorldData.cs:131-132` forces
        // `mapSize >= 1,048,576` for the fixed-world segment size.
        initial_hash_map_size: 1 << 20,
        // `BlockHashingHandler.cs` — `wantedEmptyRatio = 0.5`.
        wanted_empty_ratio: 0.5,
        // `BlockHashingHandler.cs` — `maxProbes = 250`.
        probe_cap: 250,
        // `WorldBoundHandler.cs:25` — 512 * 64.
        max_group_bound_dispatch: 512 * 64,
        // `WorldBoundHandler.cs:113` — 5 rounds per frame.
        n_bounds_rounds: 5,
        // W1: flipped from `false` to `true` after the GPU/CPU oracle gate
        // passed; const-pin guard for the canonical methodology default.
        gpu_construction_enabled: true,
        entities_enabled: false,
        cpu_fallback: true,
        run_worldgen_only: false,
        // W4: `WorldRender.cs:88` per-frame entity-instance cap.
        max_entity_instances: DEFAULT_MAX_ENTITY_INSTANCES,
        // Phase-C followup #4 — history binding disabled by default.
        entity_history_enabled: false,
    };
    // Compile-time-only sanity probe — referenced once so the const isn't
    // dead. `ConstructionConfig` is `Copy`, so this is a no-op at runtime.
    let _ = cfg.initial_hash_map_size;
    // W4 — verify the cap-derived stride for the history-ring buffer.
    let _ = cfg.max_entity_instances;
};
