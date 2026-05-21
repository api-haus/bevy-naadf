# mobile-budget — design

## delegate-architect findings (2026-05-21)

## Design

### 1. Probe-app pattern

The probe app is a throwaway `App` spun up **before** `build_app_with_args`, run just far enough that `RenderPlugin`'s async device creation resolves, then dropped. The exact plugin set is the smallest one that produces a usable `RenderDevice`: the precedent set by `world/buffer.rs:246-264` (test helper) and `validate_gpu_construction_production_scale` at `crates/bevy_naadf/src/render/construction/validation.rs:1002-1046`.

#### Probe-app `App` setup (mirrors `world/buffer.rs:246-264`)

```rust
fn probe_render_device() -> Option<RenderDevice> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(ImagePlugin::default())
        .add_plugins(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::default()),
            synchronous_pipeline_compilation: true,
            debug_flags: Default::default(),
        });
    // `Plugin::ready` blocks `app.finish()` until the wgpu device future
    // resolves; `Plugin::finish` then unpacks `RenderDevice`/`RenderQueue`
    // into the render sub-app. We never run a render schedule — the
    // canonical pattern at `world/buffer.rs:258-259` proves this is the
    // minimum-viable boot to surface limits without dragging in `WinitPlugin`
    // (which would require a real surface and fight Android's GameActivity
    // boot order).
    app.finish();
    app.cleanup();
    let render_app = app.get_sub_app(RenderApp)?;
    let device = render_app.world().get_resource::<RenderDevice>()?.clone();
    Some(device)
    // `app` drops at scope-exit — render sub-app + adapter + device-instance go
    // with it. The cloned `RenderDevice` (an `Arc<wgpu::Device>` under the hood)
    // keeps the wgpu device alive only as long as we hold it. Once
    // `select_budget(...)` extracts the `Limits` snapshot, the cloned
    // `RenderDevice` is dropped too, releasing the probe's wgpu device.
}
```

**Disposal model.** `App` drop releases all main-world + render-world resources. The `RenderDevice` we clone is held only for the duration of `probe_limits` (one `device.limits()` call), then dropped. By the time `build_app_with_args` runs, the probe's wgpu device, adapter, and instance are all gone — the real `App` re-creates them via its own `RenderPlugin`. Verified analogue: `world/buffer.rs::tests` runs this exact pattern, then in the same test body opens a *new* `App` for the real workload.

**Why this drop is safe.** wgpu allows multiple `Instance` / `Adapter` / `Device` triples on the same physical device; the OS-level driver re-acquisition is free on Mali / iOS Metal. The cost is one extra ~150 ms cold-boot on the probe (DefaultPlugins-only — `crates/bevy_naadf/src/android_main.rs` already pays this and survives on the Tab A8 per the device facts in `docs/todo/android-build.md:28`).

**Location.** New module: `crates/bevy_naadf/src/render/budget.rs`. The probe function signature:

```rust
/// Spin up a throwaway render app, read `RenderDevice::limits()`, drop it.
/// Returns `None` if the render sub-app failed to surface a `RenderDevice`
/// (e.g. headless CI with no adapter) — caller falls back to default budget.
pub fn probe_limits() -> Option<wgpu::Limits>;

/// Select a budget that fits inside `limits` with `MOBILE_HEADROOM_FACTOR`
/// headroom. Pure function — no I/O, no Bevy types beyond `UVec3`. The
/// selected `taa_ring_depth` and `world_size_in_segments` are returned in a
/// `BudgetCaps` struct that the caller writes into `AppArgs` + the
/// `EffectiveWorldSize` resource before `build_app_with_args` runs.
pub fn select_budget(limits: &wgpu::Limits) -> BudgetCaps;

#[derive(Clone, Copy, Debug)]
pub struct BudgetCaps {
    pub taa_ring_depth: u32,
    pub world_size_in_segments: UVec3,
    /// The cap that drove the decision — for the startup log line.
    pub max_storage_buffer_binding_size: u64,
    /// The headroom factor applied — for the startup log line.
    pub headroom_factor: f32,
    /// Per-binding bytes for the chosen pair (post-selection sanity log).
    pub voxels_bytes: u64,
    pub blocks_bytes: u64,
    pub taa_samples_bytes_per_megapixel: u64,
}
```

**Return type — both?** The probe returns `Option<wgpu::Limits>` (raw); `select_budget` consumes a `&Limits` and returns `BudgetCaps` (derived). Splitting the two keeps the selection logic unit-testable without booting an `App` (the unit tests feed in synthetic `Limits` and assert against `BudgetCaps`).

### 2. Budget selection algorithm

#### Constants

```rust
// crates/bevy_naadf/src/render/budget.rs
pub const MIN_STORAGE_BINDING_CAP_BYTES: u64 = 256 * 1024 * 1024;
pub const MOBILE_HEADROOM_FACTOR: f32 = 0.75;

/// TAA ring-depth ladder, descending. The budget routine picks the deepest
/// rung whose `taa_samples` binding fits the headroom.
pub const TAA_RING_DEPTH_LADDER: &[u32] = &[32, 24, 16, 8, 4, 0];

/// World-size-in-segments ladder, descending. `(16, 2, 16)` is the C#
/// canonical value (matches `crate::WORLD_SIZE_IN_SEGMENTS`). Y stays at 2
/// across the whole ladder — the cap is per binding-stage and the bottleneck
/// is `voxels = chunk_count * 128 * 4 B`; scaling XZ alone is the cheapest
/// shrink. Each rung is divisible by 1 in segments (no alignment constraint
/// — chunks-per-segment is the fixed `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4 =
/// 16` from `crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`).
pub const WORLD_SIZE_LADDER: &[UVec3] = &[
    UVec3::new(16, 2, 16), // canonical, 1024 MiB voxels  (desktop)
    UVec3::new(12, 2, 12), //            576 MiB voxels
    UVec3::new(8,  2, 8),  //            256 MiB voxels  (fails 75% headroom)
    UVec3::new(6,  2, 6),  //            144 MiB voxels  (fits headroom)
    UVec3::new(4,  2, 4),  //             64 MiB voxels  (fits with margin)
];

/// Reference pixel count for the TAA sizing check during selection. iPhone-
/// native 3.0 MP (`docs/orchestrate/mobile-budget/01-context.md` §"Lever #1").
/// Galaxy Tab A8 at 1920×1200 ≈ 2.3 MP, so the iPhone number is the worst
/// case across the two locked mobile targets. The selection algorithm uses
/// the larger number so the chosen depth is safe on both devices regardless
/// of which one boots first.
pub const SELECTION_PIXEL_COUNT_REFERENCE: u64 = 3_000_000;
```

#### Per-binding sizing formulas (verified against `prepare/world.rs:320-346`, `taa.rs:476-505`)

For a `(taa_depth, world_size_in_segments)` candidate:

```rust
// crates/bevy_naadf/src/world_size.rs derivation (const-fn, mirrored at runtime):
//   chunks = segments * WORLD_GEN_SEGMENT_SIZE_IN_GROUPS(=4) * 4 = segments * 16
let chunks = world_size_in_segments * 16;
let chunk_count: u64 = (chunks.x as u64) * (chunks.y as u64) * (chunks.z as u64);

// Mirrors render/prepare/world.rs:337-346 (the gpu_producer_enabled=true branch):
let voxels_bytes: u64 = chunk_count * 128 * 4;
let blocks_bytes: u64 = chunk_count *  64 * 4;
let chunks_bytes: u64 = chunk_count *   8;       // (x: ChunkCell, y: entity ptr)

// Mirrors render/taa.rs:483-488:
let taa_samples_bytes: u64 = SELECTION_PIXEL_COUNT_REFERENCE * (taa_depth as u64) * 8;

// taa_sample_accum is NOT depth-scaled — render/taa.rs:489-495 sizes it
// `pixel_count * 8`, i.e. ~24 MiB at 3 MP. It is NOT one of the four "big"
// bindings. See Side note #1.
```

#### Selection logic

```rust
pub fn select_budget(limits: &wgpu::Limits) -> BudgetCaps {
    let cap = limits.max_storage_buffer_binding_size as u64;
    let headroom = (cap as f64 * MOBILE_HEADROOM_FACTOR as f64) as u64;

    // Iterate world-size descending (outer loop = "prefer bigger world"),
    // then taa-depth descending (inner = "prefer deeper TAA at chosen size").
    // The first (world, depth) pair where all four bindings fit headroom wins.
    for &world in WORLD_SIZE_LADDER {
        let chunks = world * 16; // segments → chunks
        let chunk_count = (chunks.x as u64) * (chunks.y as u64) * (chunks.z as u64);
        let voxels_bytes = chunk_count * 128 * 4;
        let blocks_bytes = chunk_count *  64 * 4;
        let chunks_bytes = chunk_count *   8;

        // Voxels is the bottleneck — if it doesn't fit headroom, no taa depth
        // can rescue this world.
        if voxels_bytes > headroom || blocks_bytes > headroom
                                   || chunks_bytes > headroom {
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
                    taa_samples_bytes_per_megapixel:
                        (taa as u64) * 8 * 1_000_000,
                };
            }
        }
        // Voxels fit but no TAA depth fits — fall through to next world rung.
        // The ladder includes taa=0 (TAA disabled), so this can only happen
        // if even taa=0 fails — taa=0 byte-cost is `0`, so it always fits.
        // Unreachable in practice; kept for ladder-extension safety.
    }

    // Pathological: the smallest world failed headroom. This means the device
    // reports a cap so tight not even (4,2,4) + taa=0 fits. Return the
    // smallest-possible budget and let the downstream limits check at
    // `prepare/world.rs:390-426` fire its `error!` log so the user knows.
    let world = *WORLD_SIZE_LADDER.last().unwrap();
    let chunks = world * 16;
    let chunk_count = (chunks.x as u64) * (chunks.y as u64) * (chunks.z as u64);
    BudgetCaps {
        taa_ring_depth: 0,
        world_size_in_segments: world,
        max_storage_buffer_binding_size: cap,
        headroom_factor: MOBILE_HEADROOM_FACTOR,
        voxels_bytes: chunk_count * 128 * 4,
        blocks_bytes: chunk_count *  64 * 4,
        taa_samples_bytes_per_megapixel: 0,
    }
}
```

#### Headroom factor — 75%, kept

Audit's recommendation (75% × 256 MiB = 192 MiB) holds. Justification:

- WebGPU spec mandates `max_storage_buffer_binding_size ≥ 256 MiB` as the floor; mobile drivers report **exactly** 256 MiB (Mali-G52 confirmed, iOS Safari spec floor).
- The four bindings are sized at app startup and never grow (`prepare/world.rs:43-66` is a build-once system). No resize-driven growth that needs runtime slack.
- The headroom protects against: (a) future WGSL additions to `GpuConstructionParams` widening derived sizes, (b) wgpu/driver-internal binding-table padding (some drivers add 4 KiB), (c) NDC pad-stage rounding when ground-truth limits report e.g. 268,435,000 instead of exactly `268,435,456`.
- Going tighter (50%) would force `(4,2,4)` segments even when `(6,2,6)` fits, sacrificing fly-around volume for no functional gain. Going looser (90%) eats the safety margin.

#### Tiebreaker order — prefer bigger world over deeper TAA

The loop nesting above implements: **outer iterates worlds (descending)**, **inner iterates TAA depths (descending)**, returning the first fit. This means: a (12, 2, 12) world with TAA depth 8 wins over a (4, 2, 4) world with TAA depth 32.

Justification: the world-size lever caps the fly-around volume (the user has to fit in their decided spatial extent); the TAA-depth lever trades temporal denoise for memory but the renderer still draws something. Bigger world + shallower TAA degrades **noise** (recoverable by sitting still); smaller world + deeper TAA degrades **explorable space** (not recoverable in-app). The user explicitly listed both levers as in scope but the tiebreaker has to pick one — pick the lever whose degradation the user can mitigate at runtime.

#### Desktop pass-through

On desktop, `limits.max_storage_buffer_binding_size` reports 1-4 GiB depending on backend:
- DX12: ~2-4 GiB
- Metal: ~4 GiB
- Vulkan (NVIDIA / AMD / Intel): typically 2 GiB

At 1 GiB cap: `voxels` (1024 MiB) at canonical `(16, 2, 16)` is **exactly** the cap — fails the 75% headroom check. Concerning.

But we measured: `crates/bevy_naadf/src/render/prepare/world.rs:390-426` Q4 diagnostic has not fired since landing (no `vox-gpu-rewrite Q4 CONFIRMED` reports in commit history). So real desktops report ≥ 1.35 GiB (75% headroom of 1.35 GiB = 1.01 GiB > 1024 MiB voxels). Vulkan on real desktop GPUs reports `max_storage_buffer_binding_size = u32::MAX` (4 GiB).

**Desktop selection arithmetic.** Vulkan/NVIDIA desktop reports cap ≈ 4 GiB; headroom = 3 GiB. Voxels at (16,2,16) = 1 GiB < 3 GiB ✓. taa_samples at depth 32 + 3 MP = 720 MiB < 3 GiB ✓. → Returns `(taa=32, world=(16,2,16))` — pass-through identical to today's `AppArgs::default()`.

**Mali-G52 selection arithmetic.** cap = 256 MiB; headroom = 192 MiB. Iterate worlds:
- (16,2,16): voxels = 1024 MiB > 192 ✗
- (12,2,12): voxels = 576 MiB > 192 ✗
- (8, 2, 8): voxels = 256 MiB > 192 ✗
- (6, 2, 6): voxels = 144 MiB ≤ 192 ✓, blocks = 72 MiB ✓, chunks = 2.25 MiB ✓ → iterate taa: 32→768 MiB ✗, 24→576 MiB ✗, 16→384 MiB ✗, 8→192 MiB ≤ 192 ✓ (exactly at headroom — fine, the check is `<=`). **Returns (taa=8, world=(6, 2, 6))**.

If the user wants more headroom on TAA (e.g. for resolution scaling later), drop to `(4, 2, 4)` + `taa=16`:
- (4, 2, 4): voxels = 64 MiB ✓, blocks = 32 MiB ✓, chunks = 1 MiB ✓ → iterate taa: 32→768 ✗, 24→576 ✗, 16→384 ✗, 8→192 ✓ → also returns taa=8.

Even at (4,2,4) the TAA bottleneck holds the ladder at 8. The TAA selection is dominated by `taa_samples_bytes = pixels × depth × 8` independent of world. **At 3 MP reference, taa=8 is the highest depth that fits 192 MiB headroom.** Lower pixel counts allow higher depth; we use the conservative iPhone-native 3 MP as the selection reference.

### 3. `EffectiveWorldSize` resource shape + migration

#### Resource

```rust
// crates/bevy_naadf/src/render/budget.rs  (or a sibling world_size module —
// see Decision #5)

/// Runtime override of the C# canonical [`crate::WORLD_SIZE_IN_SEGMENTS`] /
/// [`crate::WORLD_SIZE_IN_CHUNKS`] / [`crate::WORLD_SIZE_IN_VOXELS`]
/// compile-time constants. Inserted by the budget routine BEFORE
/// [`build_app_with_args`] runs; desktop value = the canonical const, mobile
/// value = the scaled-down ladder rung the budget routine picked.
///
/// The compile-time `pub const`s + the C#-canonical pin test at
/// `crates/bevy_naadf/src/world_size.rs:46-54` remain intact. The mobile
/// divergence is **expressed entirely through this resource** — diagnostic
/// and faithful-port test code paths that need the C# numbers continue to
/// read the const.
#[derive(Resource, Clone, Copy, Debug)]
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

    /// Construct from a chosen segments rung. Derivation mirrors
    /// [`crate::world_size::mul_uvec3`] but at runtime.
    pub fn from_segments(in_segments: UVec3) -> Self {
        let in_chunks = UVec3::new(
            in_segments.x * crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4,
            in_segments.y * crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4,
            in_segments.z * crate::WORLD_GEN_SEGMENT_SIZE_IN_GROUPS * 4,
        );
        let in_voxels = in_chunks * 16;
        Self { in_segments, in_chunks, in_voxels }
    }
}

impl Default for EffectiveWorldSize {
    fn default() -> Self { Self::canonical() }
}
```

#### Insertion point

`EffectiveWorldSize` is inserted **before** `add_plugins(DefaultPlugins)` in `build_app_with_args` — same insertion shape as the existing `AppArgs` insert at `crates/bevy_naadf/src/lib.rs:185-186`. The resource is in the main world; the render sub-app reads it via `extract` into a parallel `ExtractedEffectiveWorldSize` (only if any render-world consumer needs it — see migration list below; none currently does, since `WorldData.size_in_chunks` flows down naturally and the producer's segment loop is main-world-free in the sense that it does not currently read the resource — see exception below).

To keep `build_app_with_args` budget-agnostic, the caller (production binary `main.rs` + `android_main.rs`) is responsible for inserting `EffectiveWorldSize` **into the `AppArgs` flow** — concretely, the budget routine returns `BudgetCaps`; the caller:
1. Mutates `args.taa_ring_depth = caps.taa_ring_depth`.
2. Inserts `EffectiveWorldSize::from_segments(caps.world_size_in_segments)` into the app world.

The cleanest seam: add an optional argument to `build_app_with_args` OR insert the resource by-value as a sibling at the same insertion point. Picked: **the caller inserts the resource directly into `app` BEFORE calling `build_app_with_args`**. This requires `build_app_with_args` to NOT seed a default — but we want desktop's existing `build_app` callers (every e2e gate, every test) to see `EffectiveWorldSize::canonical()` if nobody inserted one. Resolution: `build_app_with_args` inserts the resource defensively only if not already present:

```rust
// In build_app_with_args (lib.rs:142), right after `app.insert_resource(cfg)`:
if !app.world().contains_resource::<crate::render::budget::EffectiveWorldSize>() {
    app.insert_resource(crate::render::budget::EffectiveWorldSize::canonical());
}
```

This makes the default behaviour (every existing caller) byte-identical to today.

#### Migration list — exhaustive

Consumer sites of `WORLD_SIZE_IN_*` (62 grep-hit lines, deduplicated to load-bearing read sites):

| File | Lines | Const used | Action | Rationale |
|---|---|---|---|---|
| `crates/bevy_naadf/src/world_size.rs:16, :36, :40` | 3 | self-definitions | **stay on const** | The const is THE source of truth for desktop / C# canonical pin. Untouched. |
| `crates/bevy_naadf/src/world_size.rs:46-54` (test) | 1 | both | **stay on const** | The C#-faithful-port pin test. Asserts the canonical 256×32×256 / 4096×512×4096; deliberately compile-time. |
| `crates/bevy_naadf/src/lib.rs:38-39` | 1 (pub use) | re-export | **stay** | Public re-export of the const. Library consumers (e2e binaries, validation diagnostics) read the canonical value. |
| `crates/bevy_naadf/src/voxel/grid.rs:34` | 1 (import) | both | **migrate** | The install path needs the runtime value to size `WorldData.size_in_chunks` correctly when mobile reduces the world. |
| `crates/bevy_naadf/src/voxel/grid.rs:67, :84-87` (`DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS`, `demo_origin_v`) | 4 | `WORLD_SIZE_IN_CHUNKS` (for centring) | **migrate** | The demo-centring math (`(WORLD_SIZE_IN_CHUNKS.x - small.x) / 2`) needs the effective size to keep the small demo at the centre of the actual installed world. `demo_origin_v` becomes a function that takes `&EffectiveWorldSize` (or reads it from a `Res`). Five call sites of `demo_origin_v` exist; they each migrate. |
| `crates/bevy_naadf/src/voxel/grid.rs:187, :191-193` (`install_world_at_fixed_size`) | 4 | `WORLD_SIZE_IN_CHUNKS`, `WORLD_SIZE_IN_VOXELS` | **migrate** | This is the build-once seed: `WorldData.size_in_chunks` and `WorldData.bounding_box`. Take `&EffectiveWorldSize` as a parameter (the helper is called from each `install_*_in_fixed_world`). |
| `crates/bevy_naadf/src/voxel/grid.rs:245-250` (`install_empty_world` log) | 6 | both | **migrate** | Diagnostic log — read from resource for consistency. |
| `crates/bevy_naadf/src/voxel/grid.rs:299, :301, :307` (`install_default_embedded_in_fixed_world`) | 3 | `WORLD_SIZE_IN_CHUNKS` | **migrate** | Demo-centring offset + `compose_default_scene_into_fixed_world` target. Pass the runtime value. |
| `crates/bevy_naadf/src/voxel/grid.rs:310` (`size_v`) | 1 | `WORLD_SIZE_IN_VOXELS` | **migrate** | Log line — read from resource. |
| `crates/bevy_naadf/src/voxel/grid.rs:323-325` (log) | 3 | `WORLD_SIZE_IN_CHUNKS` | **migrate** | Diagnostic log. |
| `crates/bevy_naadf/src/voxel/grid.rs:518, :527-532` (`install_vox_in_fixed_world` log) | 7 | both | **migrate** | Diagnostic log. |
| `crates/bevy_naadf/src/voxel/grid.rs:1184, :1266` (tests) | 2 | `WORLD_SIZE_IN_CHUNKS` | **stay on const** | Unit tests asserting `compose_default_scene_into_fixed_world` correctness at the C# canonical world size. The composition logic is shape-agnostic; testing at the canonical shape is sufficient + faster than parameterising. |
| `crates/bevy_naadf/src/render/construction/producer.rs:138-140` (`world_size_in_voxels` GPU uniform) | 3 | `WORLD_SIZE_IN_VOXELS` | **migrate** | This feeds the per-segment `generator_model.fx` uniform. MUST be the runtime value or the GPU producer overwrites buffers it should leave alone. Read from `Res<EffectiveWorldSize>` (added as a system param in `run_gpu_producer`). |
| `crates/bevy_naadf/src/render/construction/producer.rs:179-181` (segment dispatch loop bounds) | 3 | `WORLD_SIZE_IN_SEGMENTS` | **migrate** | The triple-nested loop must dispatch only `effective_segments.x * y * z` segments, not 512. Same `Res<EffectiveWorldSize>`. |
| `crates/bevy_naadf/src/render/construction/producer.rs:230-247` (construction params) | 13 | `WORLD_SIZE_IN_CHUNKS` | **migrate** | Per-segment `GpuConstructionParams.size_in_chunks` field — must match the runtime chunk count or downstream chunk indexing is wrong. |
| `crates/bevy_naadf/src/render/construction/producer.rs:334-336` (`world_chunks` bounds upper bound) | 3 | `WORLD_SIZE_IN_CHUNKS` | **migrate** | Worst-case workgroup count for the post-loop bounds chain. Read from resource. |
| `crates/bevy_naadf/src/render/construction/mod.rs:1003` (`SEGMENT_CHUNKS`) | 1 | `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS` | **stay on const** | This is `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`, not `WORLD_SIZE_IN_*`. Invariant — segments are always 16 chunks per axis. Not part of the migration. |
| `crates/bevy_naadf/src/render/construction/validation.rs:887-900` (production-scale diagnostic) | 9 | all three | **stay on const** | Diagnostic-only function exposed via `--validate-gpu-construction-production` short-circuit at `e2e_render.rs:181`. Runs without the budget routine — should test the C# canonical shape. |
| `crates/bevy_naadf/src/e2e/small_edit_visual.rs:245, :247, :293, :297-299` | 5 | `DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS` | **stay** | These reference the *small* demo footprint, not the fixed-world container. The small-world dimensions are 4×2×4 chunks and unrelated to mobile budget. Untouched. |
| `crates/bevy_naadf/src/e2e/gates.rs:26` (doc comment) | 1 | (comment) | **stay** | Doc-comment text. |
| `crates/bevy_naadf/src/render/construction/test_fixture.rs:19` (doc comment) | 1 | (comment) | **stay** | Doc-comment text. |

**Plumbing detail for the producer.** `producer.rs`'s `run` body is inside an `impl Node`'s `run`. The trivial way to plumb the resource: add a `Res<EffectiveWorldSize>` extraction inside the render world. Two options:

1. **Mirror the `TaaRingConfig` pattern** (`render/mod.rs:105-118`): read `EffectiveWorldSize` from main-world `AppArgs`-adjacent context at `NaadfRenderPlugin::build`, insert it as a render-sub-app `Resource`. The node reads `world.get_resource::<EffectiveWorldSize>()` (mirrors `TaaRingConfig`'s consumer at `pipelines.rs:363`).
2. **Plumb through `ConstructionConfig`**: add `effective_world_size: UVec3` to `ConstructionConfig`, populated by `ConstructionConfig::from(&AppArgs)`. The W2 audit at `render/construction/config.rs:252-288` already does platform-conditional clamps here; this is the right precedent.

**Picked: Option 1** (TaaRingConfig mirror). Reasons:
- The `TaaRingConfig` pattern is the explicit template the brief identifies (audit Candidate #1, audit "Top reuse recommendation"). Two parallel mirror pairs (`TaaRingConfig` + `EffectiveWorldSize`) are easier to grok than one mirror + one piggybacked config field.
- `ConstructionConfig` already mixes too many concerns (Q4 hash-map sizes, GPU producer toggles, max group-bound dispatch). Adding world-size piggybacks orthogonal information.
- Option 2 still works as a fallback if Option 1's mirror creates ordering issues (it shouldn't — `NaadfRenderPlugin::build` already reads `AppArgs` for `taa_ring_depth`, so reading the resource the caller inserted next to `AppArgs` is the same idiom).

#### Render-sub-app mirror

```rust
// crates/bevy_naadf/src/render/budget.rs

/// Render-sub-app mirror of [`EffectiveWorldSize`]. Inserted by
/// [`NaadfRenderPlugin::build`] from the main-world resource so the
/// `producer.rs` Node can `world.get_resource()` without crossing the
/// sub-app boundary mid-frame.
#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderEffectiveWorldSize(pub EffectiveWorldSize);
```

Insertion point: `crates/bevy_naadf/src/render/mod.rs:105-118` (same `app.world().get_resource::<...>()` pattern that reads `AppArgs.taa_ring_depth`). Adds 4 lines.

### 4. New module — `crates/bevy_naadf/src/render/budget.rs`

```rust
//! GPU budget preselection — startup-time selection of `taa_ring_depth` and
//! `EffectiveWorldSize` against `RenderDevice::limits()`, so the four
//! oversized storage-buffer bindings (`voxels`, `blocks`, `taa_samples`,
//! `taa_sample_accum`) fit inside the mobile WebGPU 256 MiB
//! `max_storage_buffer_binding_size` ceiling.
//!
//! The module pairs with `crates/bevy_naadf/src/world_size.rs`:
//! - `world_size.rs` carries the **C# canonical compile-time constants**
//!   (`WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` and its derivations) and the
//!   pin test that guards faithful-port invariants.
//! - `budget.rs` carries the **runtime mobile override** (`EffectiveWorldSize`
//!   resource) + the probe-then-select routine that picks values when the
//!   device cap is below the desktop default.
//!
//! See `docs/orchestrate/mobile-budget/02-design.md` for the design narrative.

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
/// mobile WebGPU implementation (Mali, Adreno, iOS Safari) reports exactly
/// this number; desktop reports ≥ 1 GiB.
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

/// World-size-in-segments ladder, descending. The first rung must equal
/// [`crate::WORLD_SIZE_IN_SEGMENTS`] (the C# canonical value) so desktop
/// stays byte-identical to pre-budget behaviour.
pub const WORLD_SIZE_LADDER: &[UVec3] = &[
    UVec3::new(16, 2, 16),
    UVec3::new(12, 2, 12),
    UVec3::new(8,  2, 8),
    UVec3::new(6,  2, 6),
    UVec3::new(4,  2, 4),
];

/// Reference pixel count for the TAA sizing check during selection. iPhone-
/// native 3.0 MP — the worst case across the two locked mobile targets
/// (Android Mali-G52 tablets at 1920×1200 ≈ 2.3 MP; iPhone Safari ≈ 3.0 MP).
pub const SELECTION_PIXEL_COUNT_REFERENCE: u64 = 3_000_000;

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[derive(Resource, Clone, Copy, Debug)]
pub struct EffectiveWorldSize {
    pub in_segments: UVec3,
    pub in_chunks: UVec3,
    pub in_voxels: UVec3,
}
impl EffectiveWorldSize {
    pub const fn canonical() -> Self { /* … as above … */ }
    pub fn from_segments(in_segments: UVec3) -> Self { /* … as above … */ }
}
impl Default for EffectiveWorldSize {
    fn default() -> Self { Self::canonical() }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderEffectiveWorldSize(pub EffectiveWorldSize);

// ---------------------------------------------------------------------------
// Probe + select
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct BudgetCaps {
    pub taa_ring_depth: u32,
    pub world_size_in_segments: UVec3,
    pub max_storage_buffer_binding_size: u64,
    pub headroom_factor: f32,
    pub voxels_bytes: u64,
    pub blocks_bytes: u64,
    pub taa_samples_bytes_per_megapixel: u64,
}

pub fn probe_limits() -> Option<wgpu::Limits> { /* … as above … */ }

pub fn select_budget(limits: &wgpu::Limits) -> BudgetCaps { /* … as above … */ }

/// Convenience: probe + select + log + return `BudgetCaps`. Callers that
/// don't want to thread the probe themselves use this.
pub fn probe_and_select() -> BudgetCaps {
    match probe_limits() {
        Some(l) => {
            let caps = select_budget(&l);
            log_budget_decision(&caps, &l);
            caps
        }
        None => {
            bevy::log::warn!(
                "[budget] probe_limits returned None — falling back to canonical \
                 (taa_ring_depth=32, world=(16,2,16)). Mobile may OOM."
            );
            BudgetCaps {
                taa_ring_depth: crate::DEFAULT_TAA_RING_DEPTH,
                world_size_in_segments: crate::WORLD_SIZE_IN_SEGMENTS,
                max_storage_buffer_binding_size: 0,
                headroom_factor: MOBILE_HEADROOM_FACTOR,
                voxels_bytes: 0, blocks_bytes: 0,
                taa_samples_bytes_per_megapixel: 0,
            }
        }
    }
}

fn log_budget_decision(caps: &BudgetCaps, limits: &wgpu::Limits) {
    bevy::log::info!(
        "[budget] device cap max_storage_buffer_binding_size = {} MiB; \
         headroom_factor = {:.2} → ceiling {} MiB. Selected: \
         taa_ring_depth = {}, world_size_in_segments = ({}, {}, {}). \
         Estimated binding sizes: voxels = {} MiB, blocks = {} MiB, \
         taa_samples = ~{} MiB / Mpix.",
        limits.max_storage_buffer_binding_size / (1024 * 1024),
        caps.headroom_factor,
        (caps.max_storage_buffer_binding_size as f64
            * caps.headroom_factor as f64) as u64 / (1024 * 1024),
        caps.taa_ring_depth,
        caps.world_size_in_segments.x,
        caps.world_size_in_segments.y,
        caps.world_size_in_segments.z,
        caps.voxels_bytes / (1024 * 1024),
        caps.blocks_bytes / (1024 * 1024),
        caps.taa_samples_bytes_per_megapixel / (1024 * 1024),
    );
}
```

### 5. `android_main.rs` changes

Current state (`crates/bevy_naadf/src/android_main.rs:35-49`) is a minimal-probe `App` that only loads `DefaultPlugins`. Replace with a budget-aware production entry:

```rust
#[bevy_main]
fn main() {
    use bevy_naadf::{build_app_with_args, AppArgs, AppConfig};
    use bevy_naadf::render::budget::{probe_and_select, EffectiveWorldSize};

    // 1. Probe-app (throwaway): boots `MinimalPlugins + Asset + Image +
    //    RenderPlugin`, reads `max_storage_buffer_binding_size`, drops.
    //    On Galaxy Tab A8 / Mali-G52: ~150 ms cold-boot, ~250 MiB PSS
    //    peak (matches the empty-probe baseline at `docs/todo/android-build.md:28`).
    let caps = probe_and_select();

    // 2. Apply the budget to AppArgs + insert the EffectiveWorldSize before
    //    `build_app_with_args` runs.
    let mut args = AppArgs::default();
    args.taa_ring_depth = caps.taa_ring_depth;

    // 3. Build the real App. `build_app_with_args` reads `AppArgs.taa_ring_depth`
    //    at `lib.rs:185` (existing path); for `EffectiveWorldSize` we insert
    //    the resource here so the defensive `contains_resource` check inside
    //    `build_app_with_args` sees our value.
    let cfg = AppConfig::windowed();

    // Build + run. We can't use `build_app_with_args` directly because the
    // `EffectiveWorldSize` insertion needs to happen on the same `App`, and
    // the helper builds it internally. Two options:
    //
    //   (a) Add an `EffectiveWorldSize` insertion to `build_app_with_args`
    //       gated on caller-provided `Option<EffectiveWorldSize>` — pollutes
    //       the function signature.
    //   (b) Build the App in `build_app_with_args`, then insert the resource
    //       AFTER it returns but BEFORE `.run()` — but `setup_test_grid` is
    //       a `Startup` system that fires inside `.run()`; resources inserted
    //       before `.run()` are visible at `Startup` time. So this works.
    //
    // Picked (b): build the App via the helper, override the resource
    // post-build, then run.
    let mut app = build_app_with_args(cfg, args);
    app.insert_resource(EffectiveWorldSize::from_segments(
        caps.world_size_in_segments,
    ));
    // Mobile-specific: full-screen borderless + WinitSettings::mobile from the
    // original android_main, preserved.
    {
        let mut window_q = app
            .world_mut()
            .query::<&mut Window>();
        if let Ok(mut window) = window_q.single_mut(app.world_mut()) {
            window.resizable = false;
            window.mode = bevy::window::WindowMode::BorderlessFullscreen(
                MonitorSelection::Primary,
            );
        }
    }
    app.insert_resource(bevy::winit::WinitSettings::mobile());
    app.run();
}
```

**Note on `build_app_with_args` defensive insertion** (Decision #3 in the next section). The defensive `contains_resource::<EffectiveWorldSize>()` check inside `build_app_with_args` runs ONLY if no caller inserted the resource. Android pre-inserts via `app.insert_resource(EffectiveWorldSize::from_segments(...))`. Desktop (`main.rs`) and every e2e gate skips insertion — the defensive seed fires and they get `EffectiveWorldSize::canonical()`. **However** — the cleanest seam is to insert the resource into the `App` returned by `build_app_with_args` AFTER the call, not before. The defensive seed fires first, then the Android caller overrides it. Resource override is a single `app.insert_resource(...)` call (Bevy's resource storage replaces on second insertion). This shape works and is exactly what the snippet above does.

### 6. Desktop pass-through behavior

Concrete evidence desktop is unchanged:

1. **Production binary (`main.rs`).** Does NOT call the budget routine. `build_app_with_args` runs its defensive `contains_resource::<EffectiveWorldSize>()` check, finds none, inserts `EffectiveWorldSize::canonical()`. Every migrated consumer site reads canonical values; output is byte-identical to today.
2. **e2e binary (`e2e_render`).** Same as production — no budget call. The post-boot `prepare/world.rs:390-426` diagnostic still fires; on a healthy desktop GPU (≥ 1.35 GiB cap), no overrun is logged.
3. **`cargo test --workspace --lib`.** The `world_size.rs::tests::world_size_matches_csharp` test (`world_size.rs:46-54`) continues to assert against the **const**, not the resource. Untouched. Result: green.
4. **New unit tests** (added in `budget.rs`):
   - `select_budget_desktop_returns_canonical` — given `limits.max_storage_buffer_binding_size = 4 * 1024 * 1024 * 1024` (Vulkan typical), returns `taa_ring_depth = 32, world_size_in_segments = (16, 2, 16)`.
   - `select_budget_mobile_returns_scaled` — given `limits.max_storage_buffer_binding_size = 256 * 1024 * 1024`, returns `taa_ring_depth = 8, world_size_in_segments = (6, 2, 6)`.
   - `select_budget_pathological_falls_back_to_smallest` — given `limits.max_storage_buffer_binding_size = 16 * 1024 * 1024` (impossibly small), returns smallest-ladder + `taa=0`.
5. **`EffectiveWorldSize::default() == EffectiveWorldSize::canonical()`** unit test pins the desktop semantics.

### 7. CLI `--probe` flag — in scope or deferred?

**Deferred.** Reasoning:

- Android already has a no-op probe entry — `android_main.rs:36-49` is exactly the surface a `--probe` flag would yield. Once the budget routine lands, the same JNI entry point gains the budget log automatically via `log_budget_decision` (called from `probe_and_select`); the empty-app log line we're keeping for backwards-compat (`[naadf-probe] wgpu device limits = ...`) at `android_main.rs:72` can be replaced by the budget log line.
- The audit's side-note #3 lays out the 5-line CLI extension to `main.rs`. **Not part of the budget routine's correctness;** adding it is a parallel CI/diag affordance that we can land in a follow-up dispatch if the user wants to re-verify a new device cheaply.
- `e2e_render` has the no-Bevy-boot short-circuit pattern (`bin/e2e_render.rs:138-156`) where a `--probe` would fit naturally. **Out of scope for this dispatch — the user explicitly excluded "iOS build path, touch input, release-mode optimization" and probe-mode is in the same diag-affordance bucket.**
- Recommendation: if the implementer wants probe-mode for their own debugging, they can run `cargo run --bin e2e_render -- --validate-gpu-construction-production` which already reads + logs `device.limits()` (`validation.rs:1048-1053`) without spinning up the full Naadf world.

### 8. Verification plan for the impl phase

#### Unit tests (land alongside the impl, in `crates/bevy_naadf/src/render/budget.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::Limits;

    fn limits_with_cap(cap_bytes: u64) -> Limits {
        Limits {
            max_storage_buffer_binding_size: cap_bytes as u32,
            ..Limits::default()
        }
    }

    #[test]
    fn select_budget_desktop_returns_canonical() {
        let l = limits_with_cap(4 * 1024 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.taa_ring_depth, 32);
        assert_eq!(caps.world_size_in_segments, UVec3::new(16, 2, 16));
    }

    #[test]
    fn select_budget_mobile_256mib_picks_safe_combination() {
        let l = limits_with_cap(MIN_STORAGE_BINDING_CAP_BYTES);
        let caps = select_budget(&l);
        // Per the design's selection arithmetic.
        assert_eq!(caps.world_size_in_segments, UVec3::new(6, 2, 6));
        assert_eq!(caps.taa_ring_depth, 8);
        // All four binding sizes must fit headroom.
        let headroom = (MIN_STORAGE_BINDING_CAP_BYTES as f64
                     * MOBILE_HEADROOM_FACTOR as f64) as u64;
        assert!(caps.voxels_bytes <= headroom);
        assert!(caps.blocks_bytes <= headroom);
    }

    #[test]
    fn select_budget_intermediate_caps_pick_intermediate_world() {
        // A device that reports 512 MiB (rare but spec-legal). Should pick
        // (8, 2, 8) → voxels = 256 MiB ≤ 384 MiB headroom.
        let l = limits_with_cap(512 * 1024 * 1024);
        let caps = select_budget(&l);
        assert_eq!(caps.world_size_in_segments, UVec3::new(8, 2, 8));
    }

    #[test]
    fn effective_world_size_default_is_canonical() {
        assert_eq!(EffectiveWorldSize::default().in_segments,
                   crate::WORLD_SIZE_IN_SEGMENTS);
        assert_eq!(EffectiveWorldSize::default().in_chunks,
                   crate::WORLD_SIZE_IN_CHUNKS);
        assert_eq!(EffectiveWorldSize::default().in_voxels,
                   crate::WORLD_SIZE_IN_VOXELS);
    }

    #[test]
    fn effective_world_size_from_segments_matches_const_derivation() {
        let runtime = EffectiveWorldSize::from_segments(
            crate::WORLD_SIZE_IN_SEGMENTS);
        let canonical = EffectiveWorldSize::canonical();
        assert_eq!(runtime.in_segments, canonical.in_segments);
        assert_eq!(runtime.in_chunks, canonical.in_chunks);
        assert_eq!(runtime.in_voxels, canonical.in_voxels);
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
```

#### `cargo test --workspace --lib`

Existing tests that MUST stay green:

- `crates/bevy_naadf/src/world_size.rs::tests::world_size_matches_csharp` — the C#-canonical pin, untouched.
- `crates/bevy_naadf/src/app_args.rs::tests::default_taa_ring_depth_is_32` — `AppArgs::default()` still seeds 32; budget routine only mutates on mobile.
- `crates/bevy_naadf/src/app_args.rs::tests::default_taa_ring_depth_is_a_supported_lever_value` — still asserts the `default` is in `{16, 24, 32}`. The budget routine writes runtime values from the new ladder (`{32, 24, 16, 8, 4, 0}`), but **that mutation happens AFTER `AppArgs::default()` returns**, so the test's invariant holds. See audit side-note #6 for the trap; we are explicitly in case (b) — keep the default at 32, only the runtime value gets overridden.
- `crates/bevy_naadf/src/voxel/grid.rs::tests::composed_default_is_centered_with_full_area_ground` (`grid.rs:1180-1258`) and `composed_default_decodes_cleanly` (`grid.rs:1262-...`) — these read `WORLD_SIZE_IN_CHUNKS` directly to compose at canonical size. **Kept on const** per the migration table; tests stay green.

#### `cargo build --workspace`

Compiles. The migration touches ~30 call sites; each is a mechanical rewrite (`crate::WORLD_SIZE_IN_*` → `Res<EffectiveWorldSize>` field access). `voxel/grid.rs`'s `install_*` functions gain an `&EffectiveWorldSize` parameter (or `Res<EffectiveWorldSize>` if they become systems — they're called from `setup_test_grid` which is already a system).

#### e2e_render gates

No new mode needed. The existing `cargo run --bin e2e_render -- baseline` / `--vox-e2e` / `--oasis-edit-visual` / `--runtime-edit-mode` paths all boot through `build_app_with_args` → `EffectiveWorldSize::canonical()` (defensive seed). Desktop output is byte-identical to today.

#### Android APK rebuild + install + logcat watch

Implementer runs (per `docs/todo/android-build.md:86-107`):

```bash
export ANDROID_NDK_HOME=/home/midori/Android/Sdk/ndk/28.2.13676358
export ANDROID_SDK_ROOT=/home/midori/Android/Sdk
cargo ndk -t arm64-v8a --platform 31 -o android/app/src/main/jniLibs build -p bevy-naadf --lib

"$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip \
  --strip-debug android/app/src/main/jniLibs/arm64-v8a/libbevy_naadf.so

export JAVA_HOME=/usr/lib/jvm/java-21-openjdk
export PATH="$JAVA_HOME/bin:$PATH"
export ANDROID_HOME=/home/midori/Android/Sdk
android/gradlew -p android assembleDebug

adb install -r -t android/app/build/outputs/apk/debug/app-debug.apk
adb logcat -c
adb shell am start -n io.naadf.bevy/.MainActivity
adb logcat | grep -E 'naadf-probe|\[budget\]|RustStdoutStderr|FATAL|signal'
```

**Implementer verification gate.** A successful budget routine emits `[budget] device cap max_storage_buffer_binding_size = 256 MiB; headroom_factor = 0.75 → ceiling 192 MiB. Selected: taa_ring_depth = 8, world_size_in_segments = (6, 2, 6). …` to logcat within the first ~5 seconds of app launch. **Implementer's success criterion: the line appears AND the device does not reboot.** No framebuffer assert (the user does the visual check per `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`).

If the line appears but the device reboots, the implementer adds the chunked init-buffer zero-fill (the validation.rs:1097 hint about "Zero-initialise via a single 1 MiB chunk loop to bound peak memory" — wgpu's default buffer-creation path may allocate the full binding-size + backing on Mali; the chunked path is the workaround). That's a follow-up if it happens; the budget routine itself is correct independent of zero-fill mechanics.

### 9. Step-by-step implementation plan

**Phase A — `budget.rs` skeleton (no migrations).** Compile-green checkpoint after each step.

1. Create `crates/bevy_naadf/src/render/budget.rs` with constants (`MIN_STORAGE_BINDING_CAP_BYTES`, `MOBILE_HEADROOM_FACTOR`, `TAA_RING_DEPTH_LADDER`, `WORLD_SIZE_LADDER`, `SELECTION_PIXEL_COUNT_REFERENCE`), `BudgetCaps`, `EffectiveWorldSize`, `RenderEffectiveWorldSize`.
2. Add `pub mod budget;` to `crates/bevy_naadf/src/render/mod.rs` (above `pub mod construction;`).
3. Add unit tests (the `mod tests` block above). `cargo test --workspace --lib budget::tests` — green.
4. Add `probe_limits()`, `select_budget()`, `probe_and_select()`, `log_budget_decision()`.

**Phase B — defensive insertion in `build_app_with_args`.** Single-file change.

5. In `crates/bevy_naadf/src/lib.rs:185` (after `app.insert_resource(cfg)`), add the defensive insertion:
   ```rust
   if !app.world().contains_resource::<crate::render::budget::EffectiveWorldSize>() {
       app.insert_resource(crate::render::budget::EffectiveWorldSize::canonical());
   }
   ```
   `cargo build --workspace` — green. `cargo test --workspace --lib` — green (every existing test still sees canonical world).

**Phase C — migrate `voxel/grid.rs` install path.** Touches the 23 lines listed in the migration table for `voxel/grid.rs`.

6. Change `setup_test_grid` (`voxel/grid.rs:114`) to take `Res<EffectiveWorldSize>` in addition to `Res<AppArgs>`.
7. Thread `&EffectiveWorldSize` through `install_default_embedded_in_fixed_world`, `install_vox_in_fixed_world`, `install_empty_world`, `install_world_at_fixed_size`. Each function gains an `effective_world: &EffectiveWorldSize` parameter; every `WORLD_SIZE_IN_CHUNKS` / `WORLD_SIZE_IN_VOXELS` read inside these functions becomes `effective_world.in_chunks` / `effective_world.in_voxels`.
8. `demo_origin_v` becomes `demo_origin_v(effective_world: &EffectiveWorldSize) -> Vec3` — all five callers thread the resource.
9. `cargo test --workspace --lib` — green. Tests at `voxel/grid.rs:1180, :1262` still pass because they directly compose at canonical size with `WORLD_SIZE_IN_CHUNKS`, no resource needed.

**Phase D — migrate `render/construction/producer.rs`.** Touches the 22 lines listed in the migration table.

10. In `crates/bevy_naadf/src/render/mod.rs:115-118`, after `insert_resource(TaaRingConfig { ... })`, insert the render-sub-app mirror:
    ```rust
    let effective_world = app.world()
        .get_resource::<crate::render::budget::EffectiveWorldSize>()
        .copied()
        .unwrap_or_else(crate::render::budget::EffectiveWorldSize::canonical);
    // (and inside the `render_app.insert_resource(...)` chain:)
    .insert_resource(crate::render::budget::RenderEffectiveWorldSize(effective_world))
    ```
11. In `producer.rs:138-336`, change every `crate::WORLD_SIZE_IN_*` read to `effective.0.in_*` where `effective: &RenderEffectiveWorldSize` is fetched from the render world (the Node's `run` body can `world.resource::<RenderEffectiveWorldSize>()`).
12. `cargo build --workspace` — green. `cargo test --workspace --lib` — green (no producer test relies on world-size values; the production-scale diagnostic is `validation.rs` and stays on const).

**Phase E — `android_main.rs` flip-back.**

13. Rewrite `crates/bevy_naadf/src/android_main.rs` per the snippet in §5 above. Compile via `cargo ndk -t arm64-v8a --platform 31 build -p bevy-naadf --lib` to confirm. (Implementer does NOT run `cargo run --bin bevy-naadf` per project rule.)

**Phase F — verify on-device.**

14. Run the APK rebuild + install + logcat sequence in §8. Confirm the budget log line appears and the device does not reboot. User does the visual check.

**Phase G (optional, deferred).** `--probe` CLI flag on production binary or `e2e_render`. NOT part of this implementation; flagged in side notes.

## Decisions & rejected alternatives

1. **Probe-app teardown via `App` drop**, NOT explicit `wgpu::Device::destroy`.
   - **Chosen:** rely on `App` going out of scope at the end of `probe_limits()`. The cloned `RenderDevice` (`Arc<wgpu::Device>`) drops with it.
   - **Rejected:** call `wgpu::Device::destroy()` explicitly. Bevy's `RenderDevice` wraps the wgpu device behind several layers; reaching the raw `wgpu::Device` is annoying. The drop path is correct.
   - **Flip condition:** if Mali turns out to refuse a second device creation post-probe (driver bug). Mitigation: re-probe in the same App by keeping the probe-app alive and inserting `RenderEffectiveWorldSize` directly into the probe app's render sub-app before running, then never tearing down. Adds complexity; not chosen up front.

2. **Resource shape: `EffectiveWorldSize { in_segments, in_chunks, in_voxels }`** as a 3-field struct, NOT a single `UVec3`.
   - **Chosen:** the struct mirrors the three derived values `world_size.rs` exposes. Consumers can read the derivation they need without re-deriving at every call site.
   - **Rejected:** store just `in_segments: UVec3` and derive at point of use. Adds repeated `*16` / `*256` math at every consumer; 5+ call sites in `voxel/grid.rs` alone read `*_in_voxels`. The struct is cheap (12 bytes × 3 = 36 bytes; `Copy`).
   - **Flip condition:** if any consumer mutates the resource mid-frame (no such consumer is in scope; the budget value is locked at app startup).

3. **`build_app_with_args` defensively seeds `EffectiveWorldSize::canonical()`**, callers override afterward.
   - **Chosen:** keep `build_app_with_args` signature unchanged; mobile entry inserts `EffectiveWorldSize::from_segments(...)` either before or after the helper call (Bevy resource `insert_resource` overwrites on second call).
   - **Rejected:** add an `effective_world: EffectiveWorldSize` parameter to `build_app_with_args`. Pollutes the signature; every existing caller (production + 17 e2e gates) needs to pass `EffectiveWorldSize::canonical()`. Breaks the "minimal API change" hygiene.
   - **Flip condition:** if Bevy's resource-storage semantics change such that `insert_resource` is no longer an overwrite (it is in 0.19.2; unlikely to change).

4. **Tiebreaker: prefer bigger world over deeper TAA.**
   - **Chosen:** outer iterates world (descending), inner iterates TAA (descending).
   - **Rejected:** outer iterates TAA, inner iterates world. Would yield e.g. `(taa=32, world=(4,2,4))` instead of `(taa=8, world=(6,2,6))`. The user explicitly cited "fly-around volume" concerns in `01-context.md` §"Lever #2" and noted "the shader uses `voxelPos % modelSize` tiling so cutting XZ doesn't break content rendering; it caps where you can fly to". TAA depth degradation is recoverable by sitting still; world-size cap is not.
   - **Flip condition:** if the user post-impl decides noise-floor is worse than mobility loss, swap the loop order — single-edit change in `budget.rs:select_budget`.

5. **`budget.rs` module location: `crates/bevy_naadf/src/render/budget.rs`.**
   - **Chosen:** the `render/` subtree owns everything that consumes `RenderDevice`. The probe-app pattern reuses `world/buffer.rs:246-264` but `EffectiveWorldSize` is conceptually a render-pipeline parameter.
   - **Rejected:** `crates/bevy_naadf/src/mobile_budget.rs` (top-level). Too vague; the routine is GPU-budget-specific. Also rejected: `crates/bevy_naadf/src/world_size.rs` extension. The file is a tight C#-canonical-pin home; adding runtime state pollutes the SSoT story for the const.
   - **Flip condition:** if a future iOS-build dispatch needs to read budget data from a non-render path (e.g. asset-loading throttle), the location may need to move up to `crates/bevy_naadf/src/budget/` (folder). Single-file rename / re-export.

6. **TAA-supported-values test (`app_args.rs:228-235`) stays as-is — pinning the default to `{16, 24, 32}` only.**
   - **Chosen:** the test's text explicitly asserts the **default** is in `{16, 24, 32}`. Audit side-note #6 case (b). The runtime value the budget routine writes (8, 4, or 0) is OUTSIDE the supported-default set, but the test pins the `Default::default()` value, not the runtime value. No test change needed.
   - **Rejected:** widen the test to `{32, 24, 16, 8, 4, 0}` (audit's case (a)). Loses the canonical-default guard that catches future "someone changed `DEFAULT_TAA_RING_DEPTH` to 4" regressions.
   - **Flip condition:** if a future test wants to pin both the default AND the runtime-budget-output, add a new test rather than widening this one.

7. **`--probe` CLI flag: DEFERRED.**
   - **Chosen:** out of scope. The user's brief explicitly enumerated TWO big levers (TAA depth + world size); `--probe` is a diagnostic affordance, not a budget lever.
   - **Rejected:** add `--probe` to `main.rs` or `e2e_render`. The existing `cargo run --bin e2e_render -- --validate-gpu-construction-production` already reads + logs `device.limits()` for hardware-validation purposes (audit side-note #3); the post-budget `android_main` will log the budget decision unconditionally. Adding another flag is gilding.
   - **Flip condition:** user asks for a probe-only desktop binary that doesn't load the full Naadf world. 5-line addition; cheap follow-up dispatch.

8. **Validation diagnostic at `validation.rs:887-900` STAYS on `WORLD_SIZE_IN_*` const.**
   - **Chosen:** this is a `--validate-gpu-construction-production-scale` diagnostic that runs short-circuited (no Bevy app boot via `build_app_with_args`); it intentionally tests the C# canonical 256×32×256 dispatch shape.
   - **Rejected:** migrate to runtime resource. The function has no access to `EffectiveWorldSize` (no `App` is built); plumbing it would require either a CLI flag (over-engineered) or hard-coding to the resource default (= the canonical const, no behaviour change).

9. **Headroom factor: 75%** (= 192 MiB on 256 MiB cap).
   - **Chosen:** audit's recommendation; tight enough to allow `(6,2,6)` segments (=144 MiB voxels), loose enough to absorb driver-internal padding.
   - **Rejected:** 50% (would force `(4,2,4)` — 64 MiB voxels — without functional gain), 90% (eats safety margin at exactly 256 MiB cap; one shader struct widening can OOB).
   - **Flip condition:** if Mali OOMs at 144 MiB voxels post-impl, drop to 60% → 154 MiB ceiling → still allows `(6,2,6)` only marginally (144 ≤ 154). If 60% fails, drop to 50% → 128 MiB ceiling → forces `(4,2,4)` (64 MiB voxels).

10. **Selection reference pixel count: 3 MP** (iPhone-native), not Tab A8's 2.3 MP.
    - **Chosen:** worst-case across the two locked mobile targets. Future-proof against iOS dispatch landing without re-tuning.
    - **Rejected:** read the actual physical viewport size from the probe app. The probe app has no window, so `physical_viewport_size()` is `None`. Possible to query the primary display's pixel dimensions from winit, but that's an extra plugin in the probe (`WinitPlugin` would need to be added), inflating cold-boot time + Android-bring-up complexity. 3 MP is conservative.

## Assumptions made

1. **`RenderPlugin` finishes synchronously after `app.finish() + app.cleanup()`.** Verified against `crates/bevy_naadf/src/world/buffer.rs:246-264` (in-tree precedent — that test fn extracts `RenderDevice` immediately after `app.finish(); app.cleanup();`). If Bevy 0.19's `RenderPlugin::ready` returns `false` once and never `true` on Android cold-boot, `app.finish()` blocks indefinitely; this would need a timeout or a `tokio::time::timeout`-style guard. Believed safe; flag if probe hangs >5 s on first boot.
2. **`wgpu::Limits` is `Default`-constructible in tests.** Verified by `wgpu` crate docs (`Limits::default()` exists). Used in the unit tests above.
3. **Bevy's `Resource::insert_resource` second-call semantics: overwrite.** Verified against `bevy_ecs::world::World::insert_resource` (0.19.2): second insertion replaces. The defensive seed + Android override pattern relies on this.
4. **Cloning `RenderDevice` (Arc<wgpu::Device>) is cheap and safe.** Standard wgpu pattern; the cloned device shares the same underlying GPU adapter and queue.
5. **The 4th oversized binding (`taa_sample_accum`) per the brief is NOT actually depth-scaled.** Per `render/taa.rs:489-495`, `taa_sample_accum = pixel_count × 8 B` — at 3 MP that's 24 MiB, well under any mobile cap. The brief's claim "`taa_sample_accum` @ iPhone-like res ~720 MiB" is **inaccurate**; only `taa_samples` is depth-scaled. Design treats `taa_samples` as the third Big-Binding alongside `voxels` + `blocks`. (Side note #1 below.)
6. **The 15 consumer sites in `01-context.md` were ~15; actual count was 23 read-sites across 4 files after dedup.** I enumerated each one in the migration table. The "test sites at `voxel/grid.rs:1184, :1266`" in `01-context.md` stay on const per the locked Q2 ("the const + its compile-time pin stay intact"); the test asserts the canonical shape, which is exactly the resource's `Default`.
7. **The audit's `:3090` reference in `01-context.md` line 74 is wrong** (no `WORLD_SIZE_IN_*` use at `validation.rs:3090` — there's a `world_size_in_chunks` local parameter that's confusingly named). I exhaustively grep-verified the 9 actual use-sites at 887-900 instead. (Side note #2.)
8. **The Android entry's full-screen + `WinitSettings::mobile` config from the current `android_main.rs:38-46` is mobile-correct.** Preserved in the new entry verbatim. If the implementer finds that `BorderlessFullscreen` is incompatible with insets-aware UI on Android 12 (Tab A8), it's a follow-up.
9. **`cargo ndk` rebuild after `android_main.rs` rewrite produces a working `libbevy_naadf.so`** in ~5 min cold / ~30 s incremental (per `docs/todo/android-build.md:87`). No new C deps; no `build.rs` regeneration.
10. **`ConstructionConfig`'s `cfg(target_os = "android")` clamp at `render/construction/config.rs:265-287`** does not conflict with the new `EffectiveWorldSize` resource. The clamp adjusts `max_group_bound_dispatch` + `n_bounds_rounds`, which are dispatch-loop knobs orthogonal to world size. Verified by reading the audit's "Top reuse recommendation" prose.

## Side notes / observations / complaints

1. **The brief's "four bindings" framing is misleading.** Per `crates/bevy_naadf/src/render/taa.rs:489-495`, `taa_sample_accum` is sized `pixel_count × 8 B` — at iPhone-native 3 MP that's **24 MiB**, not "~720 MiB". The brief and the audit both copy a sizing claim that doesn't match the code: only **`taa_samples` is depth-scaled** (`pixel_count × depth × 8`). At default depth=32 and 3 MP, `taa_samples` is ~720 MiB. The fourth binding (`taa_sample_accum`) fits easily under any mobile cap. So the budget routine only needs to gate THREE big bindings (`voxels`, `blocks`, `taa_samples`), and the algorithm I designed reflects that. The doc copy in `01-context.md` line 41 says `taa_sample_accum @ iPhone-like res ~720 MiB` — this is wrong, and someone should correct the context doc when the impl lands.

2. **Audit `01-context.md:74` cites `validation.rs:3090` as a `WORLD_SIZE_IN_*` use — false.** Verified with grep: `validation.rs:3090` is inside a function that takes `world_size_in_chunks` as a parameter (similarly named local). No `WORLD_SIZE_IN_*` constant is referenced at or near that line. The audit chain seems to have crossed wires on parameter name vs const reference. My migration list relies on grep verification, not the audit's enumeration.

3. **The Q4 limits-check diagnostic at `render/prepare/world.rs:390-426` SHOULD STAY** even after the upstream budget routine lands. Defence-in-depth: if a future config option (e.g. `--vox-gpu-oracle-cpu-phase`) bypasses the budget routine or someone misconfigures `AppArgs.taa_ring_depth` to a value the budget didn't pick, the post-allocation diagnostic still catches it. The `01-context.md:104-105` audit said "stay (defense-in-depth) or delete it — architect's call". My call: KEEP IT. The cost is one `device.limits()` call + log at app startup; the value is "wrong" budgets get loud-reported even after the upstream routine.

4. **`build_app_with_args`'s current shape doesn't cleanly express "callers should be able to inject pre-build state".** The Android caller needs to insert `EffectiveWorldSize` AFTER `build_app_with_args` returns (so the helper's defensive seed runs first, then the caller overrides). This works but feels backwards. A cleaner refactor would be to split `build_app_with_args` into `App::new_with_naadf_plugins(cfg) → App` and let the caller insert resources before adding the Naadf plugins. **Out of scope for this dispatch** — but worth flagging that the helper's API is a small impediment to clean composition. Future refactor candidate.

5. **`AppArgs` is becoming a god-bag of mode booleans.** 14 of the 17 fields are e2e-mode flags. The TAA-budget and world-size-budget knobs are conceptually separate from "which e2e gate to run". A future refactor that splits `AppArgs` into `AppArgs { runtime_knobs, e2e_mode }` would let the budget routine touch only the `runtime_knobs` half. Not addressed here.

6. **`producer.rs:138-336`'s use of `crate::WORLD_SIZE_IN_*` from inside an `impl Node`'s `run` body is awkward.** The Node currently reads constants directly; migrating means it reads a `RenderEffectiveWorldSize` resource via `world.get_resource::<>()`. The Node already does this for other resources (line ~120: `world.get_resource::<ConstructionPipelines>()` etc.); the mechanical change is consistent with existing patterns. But the deeper issue: this Node's `run` body is hundreds of lines long and reads ~10 resources to dispatch the segment loop. Could be cleaner as a system with `Res<>`-typed params. Not addressing here.

7. **The "256 MiB universal mobile ceiling" framing in the brief glosses over real-world variation.** Mali-G52 reports exactly 256 MiB; **iOS Safari WebGPU spec floor IS 256 MiB but Apple's actual implementation reports 384 MiB on M-series Macs** (a different code path from iOS Safari, but adjacent). Adreno is reported as 256-512 MiB depending on driver vintage. The budget routine handles this correctly — `select_budget` works for any cap, not just 256 MiB — but the brief's "treat 256 MiB as universal" risks under-utilising devices that report more. The intermediate `(8, 2, 8)` and `(12, 2, 12)` rungs in the ladder cover this: a 512 MiB device picks `(8, 2, 8)`, a 1 GiB device picks `(12, 2, 12)`, etc.

8. **The user mentioned in `01-context.md:13` that lever #3 (internal-resolution scale) is deferred but** the `taa_samples` sizing math at SELECTION_PIXEL_COUNT_REFERENCE = 3 MP suggests lever #3 would be the single biggest lever for TAA depth. At 1.5 MP (0.5× internal scale), depth 16 fits in 192 MiB. At 0.75 MP (0.5× × 0.5× combined), depth 32 fits with margin. **If the FPS gauge says we still need more headroom post-budget, lever #3 is the obvious next dispatch.** Flagging because the brief's "deferred" decision is sound but it should be explicit follow-up scope.

9. **The current architecture is fine for this task** — the foundation is not rotten. The probe-app pattern, the `TaaRingConfig` mirror template, and the `WorldData.size_in_chunks` flow are all clean precedents to copy. The migration list looks long (23 sites) but is mechanical. No smell-driven escape needed; the design slots into existing patterns.

10. **Subjective: I'm cautiously confident on the `(6, 2, 6)` ladder rung choice for Mali.** `(8, 2, 8)` would be `2× the size` of `(4, 2, 4)` and at `voxels = 256 MiB` it's exactly the cap — fails the 75% headroom check by design. `(6, 2, 6)` gives voxels = 144 MiB which is 56% of the cap — comfortably inside headroom. But: voxel-count scales as `XZ²`, so the choice of intermediate ladder rungs (6 vs 7 vs 8) has a quadratic effect. If `(6, 2, 6)` turns out to be inadequate for the user's spatial-exploration intent ("oasis" is a wide scene), the implementer should feel free to add a `(10, 2, 10)` rung (400 MiB voxels — fails at 256 MiB cap but fits 512 MiB caps for newer mobile chipsets). Cheap to extend the ladder.
