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

// ---------------------------------------------------------------------------
// Probe + select
// ---------------------------------------------------------------------------

/// The output of [`select_budget`] — the values the caller writes into
/// [`crate::AppArgs`] + the [`EffectiveWorldSize`] resource before
/// [`crate::build_app_with_args`] runs.
#[derive(Clone, Copy, Debug)]
pub struct BudgetCaps {
    pub taa_ring_depth: u32,
    pub world_size_in_segments: UVec3,
    /// The cap that drove the decision — for the startup log line.
    pub max_storage_buffer_binding_size: u64,
    /// The headroom factor applied — for the startup log line.
    pub headroom_factor: f32,
    /// Estimated per-binding bytes for the chosen pair (post-selection sanity log).
    pub voxels_bytes: u64,
    pub blocks_bytes: u64,
    pub taa_samples_bytes: u64,
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
pub fn select_budget(limits: &wgpu::Limits) -> BudgetCaps {
    let cap: u64 = limits.max_storage_buffer_binding_size;
    // f64 to avoid f32 precision loss on multi-GiB caps.
    let headroom = (cap as f64 * MOBILE_HEADROOM_FACTOR as f64) as u64;

    // Outer iterates world (descending) — prefer bigger fly-around volume.
    // Inner iterates TAA depth (descending) — prefer deeper temporal denoise.
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

        // Voxels is the bottleneck — if it doesn't fit headroom, no TAA depth
        // can rescue this world.
        if voxels_bytes > headroom || blocks_bytes > headroom || chunks_bytes > headroom {
            continue;
        }

        for &taa in TAA_RING_DEPTH_LADDER {
            let taa_bytes = SELECTION_PIXEL_COUNT_REFERENCE * (taa as u64) * 8;
            if taa_bytes <= headroom {
                return BudgetCaps {
                    taa_ring_depth: taa,
                    world_size_in_segments: world,
                    max_storage_buffer_binding_size: cap,
                    headroom_factor: MOBILE_HEADROOM_FACTOR,
                    voxels_bytes,
                    blocks_bytes,
                    taa_samples_bytes: taa_bytes,
                };
            }
        }
        // taa=0 always fits (byte cost is 0); the inner loop never falls off
        // this path in practice. Kept for ladder-extension safety.
    }

    // Pathological fallback: even the smallest world failed headroom. Return
    // smallest + taa=0 so the downstream limits check at
    // `prepare/world.rs:390-426` fires its existing error log; caller can
    // observe the failure in logcat.
    let world = *WORLD_SIZE_LADDER.last().expect("WORLD_SIZE_LADDER non-empty");
    let chunks_per_segment = crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4;
    let chunk_count = (world.x * chunks_per_segment) as u64
        * (world.y * chunks_per_segment) as u64
        * (world.z * chunks_per_segment) as u64;
    BudgetCaps {
        taa_ring_depth: 0,
        world_size_in_segments: world,
        max_storage_buffer_binding_size: cap,
        headroom_factor: MOBILE_HEADROOM_FACTOR,
        voxels_bytes: chunk_count * 128 * 4,
        blocks_bytes: chunk_count * 64 * 4,
        taa_samples_bytes: 0,
    }
}

/// Convenience: probe + select + log + return [`BudgetCaps`]. Callers that
/// don't want to thread the probe themselves call this. On probe failure
/// (no adapter — e.g. headless CI), returns a canonical-shaped budget and
/// warns; mobile will then OOM at the actual allocation, but the failure
/// surface is visible in the log.
pub fn probe_and_select() -> BudgetCaps {
    match probe_limits() {
        Some(limits) => {
            let caps = select_budget(&limits);
            log_budget_decision(&caps, &limits);
            caps
        }
        None => {
            bevy::log::warn!(
                "[budget] probe_limits returned None (no RenderDevice). \
                 Falling back to canonical desktop budget — mobile may OOM."
            );
            BudgetCaps {
                taa_ring_depth: crate::DEFAULT_TAA_RING_DEPTH,
                world_size_in_segments: crate::WORLD_SIZE_IN_SEGMENTS,
                max_storage_buffer_binding_size: 0,
                headroom_factor: MOBILE_HEADROOM_FACTOR,
                voxels_bytes: 0,
                blocks_bytes: 0,
                taa_samples_bytes: 0,
            }
        }
    }
}

fn log_budget_decision(caps: &BudgetCaps, limits: &wgpu::Limits) {
    let ceiling_mib = (caps.max_storage_buffer_binding_size as f64
        * caps.headroom_factor as f64) as u64
        / (1024 * 1024);
    bevy::log::info!(
        "[budget] device cap max_storage_buffer_binding_size = {} MiB; \
         headroom_factor = {:.2} → ceiling {} MiB. Selected: \
         taa_ring_depth = {}, world_size_in_segments = ({}, {}, {}). \
         Estimated binding sizes: voxels = {} MiB, blocks = {} MiB, \
         taa_samples (@ {} MP reference) = {} MiB.",
        limits.max_storage_buffer_binding_size / (1024 * 1024),
        caps.headroom_factor,
        ceiling_mib,
        caps.taa_ring_depth,
        caps.world_size_in_segments.x,
        caps.world_size_in_segments.y,
        caps.world_size_in_segments.z,
        caps.voxels_bytes / (1024 * 1024),
        caps.blocks_bytes / (1024 * 1024),
        SELECTION_PIXEL_COUNT_REFERENCE / 1_000_000,
        caps.taa_samples_bytes / (1024 * 1024),
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
    }

    #[test]
    fn select_budget_mobile_256mib_picks_safe_combination() {
        let l = limits_with_cap(MIN_STORAGE_BINDING_CAP_BYTES);
        let caps = select_budget(&l);
        // Mali-G52 selection arithmetic, per design §2:
        // cap=256 MiB → headroom=192 MiB.
        // (16,2,16) voxels=1024 MiB ✗; (12,2,12)=576 ✗; (8,2,8)=256 ✗;
        // (6,2,6)=144 ✓; inner: 32→768 ✗, 24→576 ✗, 16→384 ✗, 8→192 ✓ (==)
        assert_eq!(caps.world_size_in_segments, UVec3::new(6, 2, 6));
        assert_eq!(caps.taa_ring_depth, 8);

        let headroom = (MIN_STORAGE_BINDING_CAP_BYTES as f64
            * MOBILE_HEADROOM_FACTOR as f64) as u64;
        assert!(caps.voxels_bytes <= headroom);
        assert!(caps.blocks_bytes <= headroom);
        assert!(caps.taa_samples_bytes <= headroom);
    }

    #[test]
    fn select_budget_intermediate_caps_pick_intermediate_world() {
        // A device that reports 512 MiB (spec-legal, some Adreno reports this).
        // Headroom = 384 MiB. (16,2,16)=1024 ✗; (12,2,12)=576 ✗;
        // (8,2,8) voxels=256 ✓, blocks=128 ✓, chunks=2.25 MiB ✓.
        let l = limits_with_cap(512 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.world_size_in_segments, UVec3::new(8, 2, 8));
    }

    #[test]
    fn select_budget_pathological_falls_back_to_smallest() {
        // 16 MiB cap — even (4,2,4) voxels=64 MiB doesn't fit any headroom.
        let l = limits_with_cap(16 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.world_size_in_segments, *WORLD_SIZE_LADDER.last().unwrap());
        assert_eq!(caps.taa_ring_depth, 0);
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
