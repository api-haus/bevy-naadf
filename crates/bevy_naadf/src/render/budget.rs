//! GPU budget preselection — startup-time selection of `taa_ring_depth` and
//! [`EffectiveWorldSize`] against `RenderDevice::limits()`, so the three
//! oversized storage-buffer bindings (`voxels`, `blocks`, `taa_samples`) fit
//! inside the mobile WebGPU 256 MiB `max_storage_buffer_binding_size` ceiling.
//!
//! ## Pairs with `world_size.rs`
//!
//! - `world_size.rs` carries the **C# canonical compile-time constants**
//!   ([`crate::WORLD_SIZE_IN_SEGMENTS`] = `(16, 2, 16)` and its derivations)
//!   plus the pin test that guards faithful-port invariants. **Untouched** by
//!   the budget routine.
//! - This module carries the **runtime mobile override**
//!   ([`EffectiveWorldSize`] resource) plus the probe-then-select routine that
//!   picks values when the device cap is below the desktop default.
//!
//! ## Decision flow
//!
//! 1. Caller (currently only `android_main`) invokes [`probe_and_select`] BEFORE
//!    [`crate::build_app_with_args`]: spins up a throwaway [`bevy::app::App`]
//!    with `MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin` (the
//!    proven pattern from `crates/bevy_naadf/src/world/buffer.rs:246-264`),
//!    extracts `RenderDevice`, reads `device.limits()`, drops the probe app.
//! 2. [`select_budget`] applies [`MOBILE_HEADROOM_FACTOR`] = 0.75 to
//!    `limits.max_storage_buffer_binding_size`, then descends
//!    [`WORLD_SIZE_LADDER`] (outer) and [`TAA_RING_DEPTH_LADDER`] (inner) and
//!    returns the first pair where `voxels`, `blocks`, `chunks` (world buffers)
//!    AND `taa_samples` (per-pixel ring) all fit under the headroom.
//! 3. Caller stuffs `caps.taa_ring_depth` into `AppArgs` and inserts
//!    `EffectiveWorldSize::from_segments(caps.world_size_in_segments)` into
//!    the App. Every desktop call site (production binary, e2e gates) skips
//!    the probe entirely; [`crate::build_app_with_args`] defensively seeds
//!    [`EffectiveWorldSize::canonical`] when the caller did not insert one,
//!    so desktop behaviour is byte-identical to pre-budget code.
//!
//! ## Faithful-port note
//!
//! The world-size mobile divergence is the SOLE user-approved deviation from
//! the C# canonical 256×32×256 chunk layout. The deviation lives entirely in
//! this resource; the compile-time const + its `world_size_matches_csharp`
//! pin test remain untouched. See `docs/orchestrate/mobile-budget/02-design.md`
//! §3 "EffectiveWorldSize resource shape + migration".

use bevy::asset::AssetPlugin;
use bevy::image::ImagePlugin;
use bevy::math::UVec3;
use bevy::prelude::*;
use bevy::render::settings::RenderCreation;
use bevy::render::{RenderApp, RenderPlugin};
use bevy::render::renderer::RenderDevice;
use bevy::MinimalPlugins;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// WebGPU spec minimum `max_storage_buffer_binding_size` — 256 MiB. Every
/// mobile WebGPU implementation (Mali-G52, Adreno, iOS Safari) reports exactly
/// this value; desktop reports ≥ 1 GiB.
pub const MIN_STORAGE_BINDING_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// Fraction of `max_storage_buffer_binding_size` the budget routine treats
/// as the per-binding ceiling. Headroom against driver-internal padding,
/// future struct-size growth, and resize-driven sizing slack. 75 % yields
/// 192 MiB on a 256 MiB cap.
pub const MOBILE_HEADROOM_FACTOR: f32 = 0.75;

/// TAA ring-depth ladder, descending. The budget routine picks the deepest
/// rung whose `taa_samples` binding fits the headroom. 0 = TAA disabled
/// (supported via `AppArgs.taa_ring_depth = 0`).
pub const TAA_RING_DEPTH_LADDER: &[u32] = &[32, 24, 16, 8, 4, 0];

/// World-size-in-segments ladder, descending. The first rung MUST equal
/// [`crate::WORLD_SIZE_IN_SEGMENTS`] (the C# canonical value) so a desktop
/// device whose cap fits the canonical world is byte-identical to today.
/// Y stays at 2 across the ladder — voxel-buffer scales as XZ², so XZ shrink
/// is the cheapest lever; Y stays the canonical 2.
pub const WORLD_SIZE_LADDER: &[UVec3] = &[
    UVec3::new(16, 2, 16), // canonical 1024 MiB voxels (desktop pass-through)
    UVec3::new(12, 2, 12), //            576 MiB voxels
    UVec3::new(8, 2, 8),   //            256 MiB voxels (exactly at cap, fails 75% headroom)
    UVec3::new(6, 2, 6),   //            144 MiB voxels (fits 192 MiB headroom)
    UVec3::new(4, 2, 4),   //             64 MiB voxels (fits with margin)
];

/// Reference pixel count for the TAA sizing check during selection. iPhone-
/// native 3.0 MP — the worst case across the two locked mobile targets
/// (Galaxy Tab A8 ≈ 2.3 MP, iPhone Safari ≈ 3.0 MP). Using the larger number
/// guarantees the chosen depth fits on either device regardless of which boots
/// first.
pub const SELECTION_PIXEL_COUNT_REFERENCE: u64 = 3_000_000;

/// `INVALID_SAMPLE_STORAGE_COUNT` ladder, descending. Picks the deepest unlit-
/// sample ring whose `invalid_samples` binding (sized
/// `pixel_count × storage_count × 16 B`) fits the headroom.
///
/// First rung MUST equal [`crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT`]
/// (the C# canonical value at `WorldRenderBase.cs:161`
/// `globalIllumInvalidSampleStorageCount`) so desktop pass-through is
/// byte-identical to pre-budget behaviour.
///
/// This lever was added 2026-05-21 in the post-deploy consolidated fix
/// (`docs/orchestrate/mobile-budget/05-consolidated-fix.md`) — the original
/// `02-design.md` selection logic missed `invalid_samples` from the
/// per-binding overrun check; on Mali-G52 + 1920×1200, the storage_count=8
/// canonical value produces 281 MiB > 256 MiB cap, triggering bind-group
/// validation errors at `naadf_global_illum_bind_group` binding 4 +
/// `naadf_sample_refine_bind_group` binding 6.
///
/// Mobile selection at 3 MP reference + 192 MiB headroom: 8 → 384 MiB ✗,
/// 4 → 192 MiB ✓ (exact), 2 → 96 MiB ✓. Lands on 4.
pub const INVALID_SAMPLE_STORAGE_COUNT_LADDER: &[u32] = &[8, 4, 2];

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Runtime override of the C# canonical [`crate::WORLD_SIZE_IN_SEGMENTS`] /
/// [`crate::WORLD_SIZE_IN_CHUNKS`] / [`crate::WORLD_SIZE_IN_VOXELS`]
/// compile-time constants. Inserted by the budget routine (mobile path) or
/// defensively seeded to [`Self::canonical`] by [`crate::build_app_with_args`]
/// (desktop / every existing caller).
///
/// The compile-time `pub const`s + the C#-canonical pin test at
/// `crates/bevy_naadf/src/world_size.rs:46-54` remain intact. The mobile
/// divergence is **expressed entirely through this resource** — diagnostic
/// and faithful-port test code paths that need the C# numbers continue to
/// read the const directly.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectiveWorldSize {
    pub in_segments: UVec3,
    pub in_chunks: UVec3,
    pub in_voxels: UVec3,
}

impl EffectiveWorldSize {
    /// The desktop / passthrough value — identical to the C# canonical
    /// compile-time constants. Used as the default if no budget routine ran.
    pub const fn canonical() -> Self {
        Self {
            in_segments: crate::WORLD_SIZE_IN_SEGMENTS,
            in_chunks: crate::WORLD_SIZE_IN_CHUNKS,
            in_voxels: crate::WORLD_SIZE_IN_VOXELS,
        }
    }

    /// Construct from a chosen `(x, y, z)` segments rung. Derivation mirrors
    /// `crate::world_size::mul_uvec3` but at runtime.
    pub fn from_segments(in_segments: UVec3) -> Self {
        let chunks_per_segment = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        let in_chunks = UVec3::new(
            in_segments.x * chunks_per_segment,
            in_segments.y * chunks_per_segment,
            in_segments.z * chunks_per_segment,
        );
        let in_voxels = UVec3::new(
            in_chunks.x * 16,
            in_chunks.y * 16,
            in_chunks.z * 16,
        );
        Self {
            in_segments,
            in_chunks,
            in_voxels,
        }
    }
}

impl Default for EffectiveWorldSize {
    fn default() -> Self {
        Self::canonical()
    }
}

/// Render-sub-app mirror of [`EffectiveWorldSize`]. Inserted by
/// [`crate::render::NaadfRenderPlugin::build`] from the main-world resource
/// so the producer node can `world.get_resource()` without crossing the
/// sub-app boundary mid-frame. Mirrors the [`crate::render::taa::TaaRingConfig`]
/// shape (see `render/mod.rs:105-118`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderEffectiveWorldSize(pub EffectiveWorldSize);

/// Runtime override of [`crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT`]
/// (the C# canonical `globalIllumInvalidSampleStorageCount = 8`).
///
/// Inserted by the budget routine (mobile path: 4 or 2) or defensively
/// seeded to the canonical value 8 by [`crate::build_app_with_args`].
/// `prepare_gi` reads this for the `invalid_samples` buffer size AND for
/// the `GpuGiParams.invalid_sample_storage_count` uniform field; the WGSL
/// shaders already read the value from the uniform
/// (`gi_params.invalid_sample_storage_count` at `naadf_global_illum.wgsl
/// :528`, `sample_refine.wgsl:267,615,665`) — no shader recompile when this
/// value changes.
///
/// The C# const `INVALID_SAMPLE_STORAGE_COUNT = 8` stays intact at
/// `gi.rs:54`; mobile divergence lives entirely in this resource. Same
/// const-vs-resource pattern as [`EffectiveWorldSize`].
///
/// See `docs/orchestrate/mobile-budget/05-consolidated-fix.md` Design §1.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSampleStorageCount(pub u32);

impl InvalidSampleStorageCount {
    pub const fn canonical() -> Self {
        Self(crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT)
    }
}

impl Default for InvalidSampleStorageCount {
    fn default() -> Self {
        Self::canonical()
    }
}

/// Render-sub-app mirror of [`InvalidSampleStorageCount`].
///
/// **Plumbing — extract-driven, NOT plugin-build-snapshot** (2026-05-21
/// post-deploy correction): the world-size + TAA mirrors are snapshotted at
/// [`crate::render::NaadfRenderPlugin::build`] from the main-world resource
/// AT THAT MOMENT. The Android entry's `build_app_with_args` → override-resource
/// sequence happens AFTER plugin-build, so a snapshot-at-build mirror would
/// see the defensive canonical seed (8), not the budget-selected mobile value
/// (typically 4). The world-size mirror works around this because the install
/// path reads the resource at runtime (`setup_test_grid` is a Startup system
/// — runs after the override) and the GPU producer reads through the
/// render-world `RenderEffectiveWorldSize` which IS snapshotted at build but
/// the producer ALSO reads `extracted.size_in_chunks` which flows from the
/// post-override `WorldData`.
///
/// For `invalid_samples`, `prepare_gi` reads the mirror directly — there is
/// no `extract`-driven proxy in between. So this mirror is populated by an
/// `ExtractSchedule` system ([`crate::render::extract::extract_invalid_sample_storage_count`])
/// that copies the main-world value into the render world each frame. The
/// first frame sees the post-override value (= the budget-selected 4 on
/// mobile, the canonical 8 on desktop).
///
/// `Default` = canonical 8 so the resource is always present (used as the
/// `init_resource` seed before the first extract).
#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderInvalidSampleStorageCount(pub u32);

impl Default for RenderInvalidSampleStorageCount {
    fn default() -> Self {
        Self(crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT)
    }
}

// ---------------------------------------------------------------------------
// Probe + select
// ---------------------------------------------------------------------------

/// The output of [`select_budget`] — the values the caller writes into
/// [`crate::AppArgs`] + the [`EffectiveWorldSize`] / [`InvalidSampleStorageCount`]
/// resources before [`crate::build_app_with_args`] runs.
#[derive(Clone, Copy, Debug)]
pub struct BudgetCaps {
    pub taa_ring_depth: u32,
    pub world_size_in_segments: UVec3,
    /// Post-2026-05-21 added lever — the GI unlit-sample ring depth.
    /// The bind groups `naadf_global_illum_bind_group` (binding 4) and
    /// `naadf_sample_refine_bind_group` (binding 6) reference the
    /// `gi_gpu.invalid_samples` buffer sized
    /// `pixel_count × storage_count × 16 B`. Picked from
    /// [`INVALID_SAMPLE_STORAGE_COUNT_LADDER`].
    pub invalid_sample_storage_count: u32,
    /// The cap that drove the decision — for the startup log line.
    pub max_storage_buffer_binding_size: u64,
    /// The headroom factor applied — for the startup log line.
    pub headroom_factor: f32,
    /// Estimated per-binding bytes for the chosen tuple (post-selection sanity log).
    pub voxels_bytes: u64,
    pub blocks_bytes: u64,
    pub taa_samples_bytes: u64,
    pub invalid_samples_bytes: u64,
}

/// Spin up a throwaway render app, read `RenderDevice::limits()`, drop it.
/// Returns `None` if the render sub-app failed to surface a `RenderDevice`
/// (e.g. headless CI with no adapter) — caller falls back to the canonical
/// budget.
///
/// Mirrors the in-tree probe pattern at `crates/bevy_naadf/src/world/buffer.rs:
/// 246-264`: `MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin`,
/// `app.finish() + app.cleanup()`, extract `RenderDevice` from the render
/// sub-app, clone it (`Arc<wgpu::Device>` — cheap), then let the probe app
/// drop at scope-exit. The cloned `RenderDevice` releases when this fn
/// returns its `Limits` snapshot.
pub fn probe_limits() -> Option<wgpu::Limits> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(ImagePlugin::default())
        .add_plugins(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::default()),
            synchronous_pipeline_compilation: true,
            debug_flags: Default::default(),
        });
    app.finish();
    app.cleanup();
    let render_app = app.get_sub_app(RenderApp)?;
    let device = render_app.world().get_resource::<RenderDevice>()?.clone();
    Some(device.limits())
    // `device` + `app` drop at scope-exit, releasing the probe's wgpu device,
    // adapter, and instance. wgpu re-creates them when `build_app_with_args`
    // runs its own `RenderPlugin`.
}

/// Select a budget that fits inside `limits` with [`MOBILE_HEADROOM_FACTOR`]
/// headroom. Pure function — no I/O, no Bevy types beyond `UVec3`. Unit-
/// testable by feeding in synthetic [`wgpu::Limits`].
///
/// Loop nesting: **world (outer, descending) → TAA depth (descending) →
/// invalid-sample-storage-count (descending)**. The first tuple where all
/// per-binding sizes fit `headroom` wins.
///
/// Tiebreaker rationale: prefer bigger world (user can't grow it back), then
/// deeper TAA (recoverable noise), then deeper unlit ring (recoverable noise
/// in reservoir-resampled GI).
pub fn select_budget(limits: &wgpu::Limits) -> BudgetCaps {
    let cap: u64 = limits.max_storage_buffer_binding_size;
    // f64 to avoid f32 precision loss on multi-GiB caps.
    let headroom = (cap as f64 * MOBILE_HEADROOM_FACTOR as f64) as u64;

    for &world in WORLD_SIZE_LADDER {
        let chunks_per_segment = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
        let chunks = UVec3::new(
            world.x * chunks_per_segment,
            world.y * chunks_per_segment,
            world.z * chunks_per_segment,
        );
        let chunk_count = (chunks.x as u64) * (chunks.y as u64) * (chunks.z as u64);
        // Mirrors render/prepare/world.rs:337-346 (gpu_producer_enabled=true).
        let voxels_bytes = chunk_count * 128 * 4;
        let blocks_bytes = chunk_count * 64 * 4;
        let chunks_bytes = chunk_count * 8;

        // Voxels is the bottleneck for the world buffers — if it doesn't fit
        // headroom, no TAA / invalid-sample combo can rescue this world.
        if voxels_bytes > headroom || blocks_bytes > headroom || chunks_bytes > headroom {
            continue;
        }

        for &taa in TAA_RING_DEPTH_LADDER {
            let taa_bytes = SELECTION_PIXEL_COUNT_REFERENCE * (taa as u64) * 8;
            if taa_bytes > headroom {
                continue;
            }
            for &inv in INVALID_SAMPLE_STORAGE_COUNT_LADDER {
                // gi_gpu.invalid_samples sizing: pixel_count * storage_count * 16.
                // (`crates/bevy_naadf/src/render/gi.rs:477-481`)
                let inv_bytes = SELECTION_PIXEL_COUNT_REFERENCE * (inv as u64) * 16;
                if inv_bytes <= headroom {
                    return BudgetCaps {
                        taa_ring_depth: taa,
                        world_size_in_segments: world,
                        invalid_sample_storage_count: inv,
                        max_storage_buffer_binding_size: cap,
                        headroom_factor: MOBILE_HEADROOM_FACTOR,
                        voxels_bytes,
                        blocks_bytes,
                        taa_samples_bytes: taa_bytes,
                        invalid_samples_bytes: inv_bytes,
                    };
                }
            }
        }
    }

    // Pathological fallback: even the smallest world failed headroom. Return
    // smallest + taa=0 + smallest invalid-ring so the downstream limits check
    // at `prepare/world.rs:390-426` fires its existing error log; caller can
    // observe the failure in logcat.
    let world = *WORLD_SIZE_LADDER.last().expect("WORLD_SIZE_LADDER non-empty");
    let chunks_per_segment = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
    let chunk_count = (world.x * chunks_per_segment) as u64
        * (world.y * chunks_per_segment) as u64
        * (world.z * chunks_per_segment) as u64;
    let smallest_inv = *INVALID_SAMPLE_STORAGE_COUNT_LADDER
        .last()
        .expect("INVALID_SAMPLE_STORAGE_COUNT_LADDER non-empty");
    BudgetCaps {
        taa_ring_depth: 0,
        world_size_in_segments: world,
        invalid_sample_storage_count: smallest_inv,
        max_storage_buffer_binding_size: cap,
        headroom_factor: MOBILE_HEADROOM_FACTOR,
        voxels_bytes: chunk_count * 128 * 4,
        blocks_bytes: chunk_count * 64 * 4,
        taa_samples_bytes: 0,
        invalid_samples_bytes: SELECTION_PIXEL_COUNT_REFERENCE * smallest_inv as u64 * 16,
    }
}

/// Convenience: probe + select + log + return [`BudgetCaps`]. Callers that
/// don't want to thread the probe themselves call this. On probe failure
/// (no adapter — e.g. headless CI), returns a canonical-shaped budget and
/// warns; mobile will then OOM at the actual allocation, but the failure
/// surface is visible in the log.
///
/// **Logging mechanism** — `eprintln!` (NOT `bevy::log::info!`). The probe
/// app uses `MinimalPlugins`, which does NOT include `LogPlugin`. No
/// `tracing` subscriber is installed during the probe lifetime, so
/// `bevy::log::info!` events vanish into the no-op default subscriber and
/// never reach logcat. `eprintln!` lands on stderr, which the Android
/// `android-game-activity` harness pipes to logcat under the
/// `RustStdoutStderr` tag (the same tag wgpu uses for its
/// `AdapterInfo` line — see `docs/orchestrate/mobile-budget/05-consolidated-fix.md`
/// Investigation §"Symptom 2" for the root-cause trace).
pub fn probe_and_select() -> BudgetCaps {
    match probe_limits() {
        Some(limits) => {
            let caps = select_budget(&limits);
            log_budget_decision(&caps, &limits);
            caps
        }
        None => {
            eprintln!(
                "[budget] probe_limits returned None (no RenderDevice). \
                 Falling back to canonical desktop budget — mobile may OOM."
            );
            BudgetCaps {
                taa_ring_depth: crate::DEFAULT_TAA_RING_DEPTH,
                world_size_in_segments: crate::WORLD_SIZE_IN_SEGMENTS,
                invalid_sample_storage_count:
                    crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT,
                max_storage_buffer_binding_size: 0,
                headroom_factor: MOBILE_HEADROOM_FACTOR,
                voxels_bytes: 0,
                blocks_bytes: 0,
                taa_samples_bytes: 0,
                invalid_samples_bytes: 0,
            }
        }
    }
}

fn log_budget_decision(caps: &BudgetCaps, limits: &wgpu::Limits) {
    let ceiling_mib = (caps.max_storage_buffer_binding_size as f64
        * caps.headroom_factor as f64) as u64
        / (1024 * 1024);
    eprintln!(
        "[budget] device cap max_storage_buffer_binding_size = {} MiB; \
         headroom_factor = {:.2} -> ceiling {} MiB. Selected: \
         taa_ring_depth = {}, world_size_in_segments = ({}, {}, {}), \
         invalid_sample_storage_count = {}. \
         Estimated binding sizes (@ {} MP reference): \
         voxels = {} MiB, blocks = {} MiB, taa_samples = {} MiB, \
         invalid_samples = {} MiB.",
        limits.max_storage_buffer_binding_size / (1024 * 1024),
        caps.headroom_factor,
        ceiling_mib,
        caps.taa_ring_depth,
        caps.world_size_in_segments.x,
        caps.world_size_in_segments.y,
        caps.world_size_in_segments.z,
        caps.invalid_sample_storage_count,
        SELECTION_PIXEL_COUNT_REFERENCE / 1_000_000,
        caps.voxels_bytes / (1024 * 1024),
        caps.blocks_bytes / (1024 * 1024),
        caps.taa_samples_bytes / (1024 * 1024),
        caps.invalid_samples_bytes / (1024 * 1024),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::Limits;

    fn limits_with_cap(cap_bytes: u64) -> Limits {
        Limits {
            max_storage_buffer_binding_size: cap_bytes,
            ..Limits::default()
        }
    }

    #[test]
    fn select_budget_desktop_returns_canonical() {
        let l = limits_with_cap(4 * 1024 * 1024 * 1024 - 1);
        let caps = select_budget(&l);
        assert_eq!(caps.taa_ring_depth, 32);
        assert_eq!(caps.world_size_in_segments, UVec3::new(16, 2, 16));
        assert_eq!(
            caps.invalid_sample_storage_count,
            crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT
        );
    }

    #[test]
    fn select_budget_mobile_256mib_picks_safe_combination() {
        let l = limits_with_cap(MIN_STORAGE_BINDING_CAP_BYTES);
        let caps = select_budget(&l);
        // Mali-G52 selection arithmetic — post-2026-05-21 (3-lever):
        // cap=256 MiB → headroom=192 MiB.
        // Worlds (16,2,16) (12,2,12) (8,2,8) fail voxels-headroom.
        // (6,2,6): voxels=144 MiB ≤ 192 ✓, blocks=72 MiB ✓, chunks=2.25 MiB ✓.
        //   TAA: 32→768 ✗, 24→576 ✗, 16→384 ✗, 8→192 ✓.
        //     Inv: 8→384 ✗, 4→192 ✓.
        assert_eq!(caps.world_size_in_segments, UVec3::new(6, 2, 6));
        assert_eq!(caps.taa_ring_depth, 8);
        assert_eq!(caps.invalid_sample_storage_count, 4);

        let headroom = (MIN_STORAGE_BINDING_CAP_BYTES as f64
            * MOBILE_HEADROOM_FACTOR as f64) as u64;
        assert!(caps.voxels_bytes <= headroom);
        assert!(caps.blocks_bytes <= headroom);
        assert!(caps.taa_samples_bytes <= headroom);
        assert!(caps.invalid_samples_bytes <= headroom);
    }

    #[test]
    fn select_budget_intermediate_caps_pick_intermediate_world() {
        // A device that reports 512 MiB (spec-legal, some Adreno reports this).
        // Headroom = 384 MiB. (16,2,16)=1024 ✗; (12,2,12)=576 ✗;
        // (8,2,8) voxels=256 ✓, blocks=128 ✓, chunks=2.25 MiB ✓.
        //   TAA: 32→768 ✗, 24→576 ✗, 16→384 ✓ (exact).
        //     Inv: 8→384 ✓ (exact).
        let l = limits_with_cap(512 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.world_size_in_segments, UVec3::new(8, 2, 8));
        assert_eq!(caps.taa_ring_depth, 16);
        assert_eq!(caps.invalid_sample_storage_count, 8);
    }

    #[test]
    fn select_budget_pathological_falls_back_to_smallest() {
        // 16 MiB cap — even (4,2,4) voxels=64 MiB doesn't fit any headroom.
        let l = limits_with_cap(16 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.world_size_in_segments, *WORLD_SIZE_LADDER.last().unwrap());
        assert_eq!(caps.taa_ring_depth, 0);
        assert_eq!(
            caps.invalid_sample_storage_count,
            *INVALID_SAMPLE_STORAGE_COUNT_LADDER.last().unwrap()
        );
    }

    #[test]
    fn invalid_sample_storage_count_ladder_first_rung_matches_canonical() {
        // Guards against future ladder edits silently breaking desktop /
        // C# faithful-port semantics.
        assert_eq!(
            INVALID_SAMPLE_STORAGE_COUNT_LADDER[0],
            crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT
        );
    }

    #[test]
    fn invalid_sample_storage_count_default_is_canonical() {
        assert_eq!(
            InvalidSampleStorageCount::default(),
            InvalidSampleStorageCount(crate::render::gi::INVALID_SAMPLE_STORAGE_COUNT)
        );
    }

    #[test]
    fn effective_world_size_default_is_canonical() {
        let d = EffectiveWorldSize::default();
        assert_eq!(d.in_segments, crate::WORLD_SIZE_IN_SEGMENTS);
        assert_eq!(d.in_chunks, crate::WORLD_SIZE_IN_CHUNKS);
        assert_eq!(d.in_voxels, crate::WORLD_SIZE_IN_VOXELS);
    }

    #[test]
    fn effective_world_size_from_segments_matches_const_derivation() {
        let runtime = EffectiveWorldSize::from_segments(crate::WORLD_SIZE_IN_SEGMENTS);
        let canonical = EffectiveWorldSize::canonical();
        assert_eq!(runtime, canonical);
    }

    #[test]
    fn ladder_first_rung_matches_canonical() {
        // Guards against future ladder edits silently breaking desktop.
        assert_eq!(WORLD_SIZE_LADDER[0], crate::WORLD_SIZE_IN_SEGMENTS);
    }

    #[test]
    fn taa_default_is_in_ladder() {
        assert!(TAA_RING_DEPTH_LADDER.contains(&crate::DEFAULT_TAA_RING_DEPTH));
    }
}
