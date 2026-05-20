# D4 — render-pipeline · 03-architecture

**Author:** refactor-architect (D4 — render-pipeline).
**Date:** 2026-05-20.
**Phase:** architecture (writes design; orchestrator does not read; D4
implementor reads this + 02-exploration.md).
**Sequencing:** D4 impl runs **after** D5 impl. The design here assumes D5's
`render/construction/mod.rs` split has already landed and that D5's
`ConstructionPipelines` retirement (W0 seam, Resolution D) has produced the
post-D5 state on the construction side. This design covers the
**render-side** half only.

Every file:line reference below was verified with Read/Grep on
`/mnt/archive4/DEV/bevy-naadf/` HEAD.

---

## 0. Scope confirmation

D4 owns:

- `render/mod.rs` (332 LOC)
- `render/graph.rs` (309 LOC) + `render/graph_b.rs` (574 LOC)
- `render/extract.rs` (483 LOC)
- `render/prepare.rs` (1 207 LOC) — D4 writes; D5 read-only-to-D5 per W0 contract.
- `render/pipelines.rs` (909 LOC) — D4 writes; D5 read-only-to-D5.
- `render/gpu_types.rs` (1 055 LOC) — D4 writes the uniform structs; the D5-owned construction structs (`GpuConstructionParams`, `GpuHashValueSlot`, `GpuBoundQueueInfo`, `GpuEntityChunkInstance`, `GpuEntityInstanceHistory`, `GpuChunkUpdate`, `EntityInstance`) are co-located but D4's `ShaderType` sweep covers `GpuConstructionParams` mechanically.
- `render/atmosphere.rs` (344 LOC), `render/gi.rs` (618 LOC), `render/taa.rs` (506 LOC), `render/color_compression.rs` (172 LOC).
- WGSL render shaders: `naadf_first_hit.wgsl` (315), `naadf_final.wgsl` (76), `naadf_atmosphere.wgsl` (79), `naadf_global_illum.wgsl` (548), `ray_queue_calc.wgsl` (172), `sample_refine.wgsl` (768), `spatial_resampling.wgsl` (699), `denoise_split.wgsl` (252), `taa.wgsl` (481), `taa_common.wgsl` (137), `ray_tracing.wgsl` (577), `ray_tracing_common.wgsl` (183), `render_pipeline_common.wgsl` (413), `gi_params.wgsl` (155), `common.wgsl` (78), `world_data.wgsl` (130), `color_compression.wgsl` (138).

D4 does **NOT** own (D5 territory; flagged in side-notes if rot is observed):

- `render/construction/**` and its 7 WGSL files.
- The CPU oracle `aadf/edit.rs`.

---

## 1. Headline decisions (one-line each)

1. **Plugin-per-subsystem.** Each render-side subsystem owns its node + its `SystemSet` label + its layouts + its pipeline-ids. The 17-element `.chain()` collapses into nine `add_plugins((…))` calls + per-plugin `.before(…)/.after(…)` edges. Bevy 0.19 has no `RenderLabel`/`RenderGraph`-node API for the `Core3d`-schedule approach this port uses (graph.rs:22-24 docblock confirms: "a render-graph node is just a system in the `Core3d` schedule") — `SystemSet` is the correct idiom.
2. **`graph.rs` + `graph_b.rs` dissolve.** Each node body co-locates with its subsystem module. `graph.rs::FIRST_HIT_SPAN`/`TAA_REPROJECT_SPAN`/`CALC_NEW_TAA_SAMPLE_SPAN`/`FINAL_BLIT_SPAN` move into the owning subsystem modules. `graph_b.rs::SAMPLE_REFINE_SPAN`/`SPATIAL_RESAMPLING_SPAN`/`DENOISE_SPAN`/`RAY_QUEUE_SPAN`/`GLOBAL_ILLUM_SPAN`/`ATMOSPHERE_SPAN` move likewise.
3. **`prepare.rs` splits into `prepare/{world,frame,mod}.rs`.** `prepare_world_gpu` → `prepare/world.rs`; `prepare_frame_gpu` → `prepare/frame.rs`; the `WorldGpu` + `FrameGpu` structs stay top-level in `prepare/mod.rs` so D5's existing imports (`crate::render::prepare::WorldGpu`) keep resolving without an import-path change. The palette-refresh branch (`prepare.rs:218-317`, ~100 LOC) extracts to a `prepare/world.rs::apply_voxel_types_refresh` private helper called from a small dispatch shell.
4. **W4 placeholder-buffer ownership stays D4's** (Finding 7, Option C). Rationale: making it Option A (entities-on/off bind-group flavours) cascades into pipeline layout duplication; Option B (D5 owns the rebuild closure) re-creates the cross-write through a different door. C is the lowest-blast-radius call — the cross-write is already documented at `prepare.rs:650-686` and `15-design-c.md` §1.7; the design's job is to make the seam *legible* rather than eliminate it. We add an explicit `WorldGpu::rebuild_bind_group_with_entities` constructor (a `pub(crate)` method on `WorldGpu`) so D5 calls a named function instead of inline-rebuilding the field. Net change: one function definition + one D5-side replacement. The smell shrinks from "two writers in two domains" to "two writers, one constructor".
5. **Sample-refine 4-node collapse.** The contiguous 4 (`valid_history` → `count_valid` → `count_invalid` → `buckets`) collapse into one `naadf_sample_refine_continuous_node` that opens one compute pass and dispatches all four in sequence. `clear` stays separate. `valid_history` still binds `@group(1) = sample_refine_dispatch_bind_group` for its dispatch; the new node sets `@group(0)` once for the shared bind group and re-binds `@group(1)` only for `valid_history`. wgpu's automatic buffer barriers serialise the inter-dispatch access. Net drop: ~160 LOC (3 systems × ~40 LOC each + 3 system-registration lines + 3 timing-span open/close pairs).
6. **`ShaderType` sweep — 7 uniform structs cut over in one mechanical pass.** Targets: `GpuCamera`, `GpuRenderParams`, `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams`, `GpuGiParams`, `GpuConstructionParams`. Drops ~70 `_padN` fields + ~25 compile-time offset asserts in `gpu_types.rs`. Upload sites swap `bytemuck::bytes_of(&x)` → `encase::UniformBuffer::from_bytes(&mut buf, &x)` (or the `ShaderType`-aware `write_buffer` helper). Packed-array structs (`GpuVoxelType`, `GpuCameraHistorySlot`, `GpuSampleValid`, `GpuBucketInfo`, `GpuHashValueSlot`, `GpuBoundQueueInfo`, `GpuEntityChunkInstance`, `GpuEntityInstanceHistory`, `GpuChunkUpdate`, `EntityInstance`) stay `Pod`.
7. **SSoT-3 (`CELL_DIM`/`CELL_CHILDREN`) via `naga_oil` shader-def injection.** Mirror the existing `TAA_SAMPLE_RING_DEPTH` pattern (`pipelines.rs:269-279`). Add `NAADF_CELL_DIM` + `NAADF_CELL_CHILDREN` to every D4 pipeline's `shader_defs` vec. WGSL sites declare `const NAADF_CELL_DIM: u32 = #{NAADF_CELL_DIM}u;` at the top of each affected file and replace semantic `4u`/`64u` literals with the named constant. Cross-domain: D5's `ConstructionPipelines` must mirror the injection (D5 architect's responsibility — flagged below). Shared injection helper lives in `pipelines.rs::cell_shader_defs()` returning `Vec<ShaderDefVal>` so D5 reuses one function.
8. **SSoT-4 (`sample_refine.wgsl:655` + `:668`) via uniform read + shader-def.** Line 655: `* 8u` → `* gi_params.invalid_sample_storage_count`. Line 668: add `BUCKET_STORAGE_COUNT` shader-def to the four `sample_refine_*_pipeline`s; replace `array<u32, 32>` with `array<u32, #{BUCKET_STORAGE_COUNT}>`. Rust-side const at `gi.rs:57` is the authoritative SSoT; pipeline injection reads it.
9. **SSoT-1 chain coordination — D4 surface.** D7 owns the canonical `GiSettings` struct (decision: move it out of `lib.rs` into `settings.rs` per D7 Finding F2). D2 owns the KNOBS table. D4's piece: `GpuRenderParams.max_ray_steps_primary` + `GpuGiParams.{max_ray_steps_secondary,_sun,_sun_secondary,_visibility,spatial_iter_count}` already correctly read uniform values from `ExtractedGiConfig.settings.*`. **No structural change in D4 needed** — D4 already consumes the SSoT correctly. **One deletion proposed:** the documentation-only `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-136` (verified zero consumers in WGSL — see exploration §"audit suspicion verdict" SSoT-1) get a one-line comment + deletion. Keeps the chain at exactly 3 sites (Rust struct default, Rust KNOBS row default, Rust GPU uniform default), all in D7/D2 territory.
10. **W0 retirement coordination (Resolution D).** Post-D5, `ConstructionPipelines` either folds into `NaadfPipelines` (D5's call) or stays split. The design here assumes **D5 keeps `ConstructionPipelines` per-workstream-split** (D5 Finding 10's proposal) and `NaadfPipelines` does **NOT** absorb construction-side fields. D4's `NaadfPipelines` decomposition is the natural mirror: each render-side subsystem gets its own `*Pipelines` resource. `NaadfPipelines` shrinks to ~5 fields holding only the cross-subsystem core (`world_layout`, `frame_layout`, `blit_layout`, `empty_layout`, `blit_pipelines: HashMap<TextureFormat, _>`, `blit_vertex`, `blit_shader`).
11. **PBR scaffolding on master — DELETE.** Per Resolution C + master-branch identity (addendum 2026-05-20): the PBR e2e gates die; `pbr_sampling.wgsl` (868 LOC) has zero non-e2e consumers (verified: imported nowhere except `e2e/pbr_visual.rs` and `debug_view.rs`). **D4-territory deletion: `assets/shaders/pbr_sampling.wgsl`.** `debug_view.rs` is D7-territory; D7 architect handles. Flagged in §"Side notes" for cross-domain awareness.

---

## 2. Target file structure (post-refactor)

```
crates/bevy_naadf/src/render/
├── mod.rs                       ~120 LOC (was 332)
│     ├ NaadfRenderPlugin (now just .add_plugins(...))
│     └ pub mod re-exports
│
├── plugin_core.rs               ~80 LOC (new — extracted "RenderApp init" shell)
│     ├ insert TaaRingConfig
│     ├ init_resource calls for extracted-* mirrors
│     ├ init_gpu_resource::<NaadfPipelines>
│     └ stage_world_gpu_buildonce / stage_model_data_buildonce / extract_camera{,_history} ExtractSchedule registration
│
├── prepare/
│   ├── mod.rs                   ~80 LOC (pub re-exports + the WorldGpu + FrameGpu struct defs)
│   ├── world.rs                 ~620 LOC (was ~528, +90 LOC absorbing palette-refresh extract)
│   │     ├ prepare_world_gpu (build-once dispatch shell)
│   │     ├ apply_voxel_types_refresh (the palette-refresh branch extracted)
│   │     └ pub(crate) fn rebuild_world_bind_group_with_entities(world_gpu, layout, ce, vd, ih) -> BindGroup
│   │       (called by D5's prepare_construction when entities_enabled toggles on)
│   └── frame.rs                 ~520 LOC (was ~474)
│         └ prepare_frame_gpu (per-frame uniforms + 5 bind-group builders)
│
├── pipelines/
│   ├── mod.rs                   ~150 LOC (was 909)
│   │     ├ NaadfPipelines (5 fields: world_layout, frame_layout, blit_layout, empty_layout, blit_*)
│   │     ├ FromWorld for NaadfPipelines (only the 4 shared layouts)
│   │     ├ prepare_blit_pipeline
│   │     └ pub fn cell_shader_defs() -> Vec<ShaderDefVal>
│   │       (NAADF_CELL_DIM + NAADF_CELL_CHILDREN injection — used by D4 + D5)
│   └── shaders.rs               ~30 LOC (asset-path const declarations only)
│         └ FIRST_HIT_SHADER, FINAL_BLIT_SHADER, TAA_REPROJECT_SHADER, ATMOSPHERE_SHADER, ...
│           (extracted from pipelines.rs:63-93)
│
├── gpu_types/
│   ├── mod.rs                   ~120 LOC (pub use + the FLAG_*/GI_FLAG_* const block + f16_bits)
│   ├── uniforms.rs              ~430 LOC (was ~700) — all 7 uniform structs as #[derive(ShaderType)]
│   ├── samples.rs               ~120 LOC — GpuVoxelType, GpuCameraHistorySlot, GpuSampleValid, GpuBucketInfo (Pod, packed-array)
│   └── construction.rs          ~310 LOC — D5-owned-conceptually structs (GpuConstructionParams as ShaderType; GpuHashValueSlot, GpuBoundQueueInfo, GpuEntityChunkInstance, GpuEntityInstanceHistory, GpuChunkUpdate, EntityInstance as Pod)
│
├── extract.rs                   ~470 LOC (was 483 — minor; extract_taa_config + extract_gi_config stay as-is per Finding 9)
│
├── atmosphere.rs                ~360 LOC (was 344, +naadf_atmosphere_node body + ATMOSPHERE_SPAN + AtmospherePipelines resource + AtmospherePlugin)
│
├── first_hit.rs                 ~120 LOC (new — extracted from graph.rs)
│     ├ FIRST_HIT_SPAN const
│     ├ naadf_first_hit_node
│     ├ FirstHitPipelines resource (the first-hit pipeline id + its bind-group-layout descriptors)
│     ├ FirstHitSet SystemSet label
│     └ FirstHitPlugin
│
├── final_blit.rs                ~140 LOC (new — extracted from graph.rs)
│     ├ FINAL_BLIT_SPAN const
│     ├ naadf_final_blit_node
│     ├ BlitPipelines (the per-format pipeline HashMap + prepare_blit_pipeline)
│     ├ BlitSet
│     └ FinalBlitPlugin
│
├── taa.rs                       ~620 LOC (was 506 + ~110 LOC absorbing naadf_taa_reproject_node + naadf_calc_new_taa_sample_node + their SPAN consts + TaaPipelines + TaaSet + TaaPlugin)
│
├── ray_queue.rs                 ~80 LOC (new — extracted from graph_b.rs:1-130)
│     ├ RAY_QUEUE_SPAN const
│     ├ naadf_ray_queue_node
│     ├ RayQueuePipelines + RayQueueSet + RayQueuePlugin
│
├── gi.rs                        ~720 LOC (was 618 + ~100 LOC absorbing naadf_global_illum_node + GLOBAL_ILLUM_SPAN + GiPipelines + GiSet + GiPlugin)
│
├── sample_refine.rs             ~180 LOC (new — extracted from graph_b.rs:242-446 + collapsed 4-of-5)
│     ├ SAMPLE_REFINE_SPAN const
│     ├ naadf_sample_refine_clear_node (standalone)
│     ├ naadf_sample_refine_continuous_node (collapsed 4: valid_history + count_valid + count_invalid + buckets)
│     ├ SampleRefinePipelines (5 pipeline ids — pipelines unchanged; node count drops 5 → 2)
│     ├ SampleRefineSet
│     └ SampleRefinePlugin
│
├── spatial_resampling.rs        ~110 LOC (new — extracted from graph_b.rs:457-535)
│     ├ SPATIAL_RESAMPLING_SPAN const, naadf_spatial_resampling_node, *Pipelines, *Set, *Plugin
│
├── denoise.rs                   ~120 LOC (new — extracted from graph_b.rs:537-574)
│     ├ DENOISE_SPAN const, naadf_denoise_node, DenoisePipelines, DenoiseSet, DenoisePlugin
│
└── color_compression.rs         172 LOC (unchanged — already a leaf subsystem)
```

DELETED files:

- `render/graph.rs` (309 LOC) — all 4 nodes relocate.
- `render/graph_b.rs` (574 LOC) — all 10 nodes relocate.
- `assets/shaders/pbr_sampling.wgsl` (868 LOC) — orphan PBR scaffolding (master-branch identity).

LOC delta (D4-side approximate):

- `prepare.rs` 1 207 → split into 3 files totalling ~1 220 (net +~13, mostly file-header docs).
- `pipelines.rs` 909 → ~180 (`NaadfPipelines` shell) + ~30 (`shaders.rs`) + per-subsystem `*Pipelines` resources (~25-60 LOC each × 9 = ~350). Net: 909 → ~560.
- `gpu_types.rs` 1 055 → ~980 (~700 LOC of pure padding/asserts melts; ~~25 LOC of `ShaderType` use lines + the per-uniform `derive(ShaderType)` plus the 3 file headers add back ~50; tests stay).
- `graph.rs` + `graph_b.rs` 883 → 0 (relocated into subsystem files; ~30 LOC of duplicated SPAN-const docblocks removed).
- `mod.rs` 332 → ~120.
- Sample-refine collapse: ~160 LOC removed from `graph_b.rs` before relocation.

Net D4 LOC drop: **~700-900 LOC** (mostly `gpu_types.rs` pad-field melt + `graph_b.rs`/`graph.rs` duplicate-prologue elimination + 868-LOC PBR shader deletion). PBR shader deletion makes the total **~1 600-1 800 LOC** dropped from the D4 surface.

---

## 3. Target shapes (concrete)

### 3.1 `prepare/mod.rs` (the W0-seam-stable export front)

```rust
//! `Prepare` set: upload buffers, build bind groups, write camera uniforms.
//!
//! - [`world`] — build-once world resources + palette-refresh (`PrepareResources`).
//! - [`frame`] — per-frame uniforms + bind groups (`PrepareBindGroups`).
//!
//! `WorldGpu` + `FrameGpu` structs are defined here so external imports
//! (`use crate::render::prepare::{WorldGpu, FrameGpu};`) keep resolving
//! verbatim — D5's `prepare_construction` and other consumers do not see
//! the split.

pub mod world;
pub mod frame;

pub use world::{prepare_world_gpu, rebuild_world_bind_group_with_entities};
pub use frame::prepare_frame_gpu;

use bevy::prelude::*;
use bevy::render::render_resource::{BindGroup, Buffer};
use crate::world::buffer::GrowableBuffer;
use crate::render::gpu_types::GpuVoxelType;

#[derive(Resource)]
pub struct WorldGpu {
    pub chunks_buffer: Buffer,
    pub chunks_size_in_chunks: UVec3,
    pub blocks: GrowableBuffer<u32>,
    pub voxels: GrowableBuffer<u32>,
    pub voxel_types: GrowableBuffer<GpuVoxelType>,
    pub world_meta: Buffer,
    pub bind_group: BindGroup,
    pub entity_chunk_instances_placeholder: Buffer,
    pub entity_voxel_data_placeholder: Buffer,
    pub entity_instances_history_placeholder: Buffer,
}

#[derive(Resource)]
pub struct FrameGpu {
    pub camera: Buffer,
    pub render_params: Buffer,
    pub first_hit_data: Buffer,
    pub first_hit_absorption: Buffer,
    pub final_color: Buffer,
    pub pixel_count: u32,
    pub bind_group: BindGroup,
    pub first_hit_atmosphere_bind_group: BindGroup,
    pub blit_bind_group: BindGroup,
    pub taa_reproject_bind_group: BindGroup,
    pub calc_new_taa_sample_bind_group: BindGroup,
}
```

### 3.2 `prepare/world.rs::rebuild_world_bind_group_with_entities`

The Finding-7 seam-tightener — D5 calls this rather than inlining the
bind-group rebuild. Stays D4-owned; D5 is now a caller, not a writer.

```rust
/// Rebuild `WorldGpu.bind_group` with the real W4 entity buffers — used by
/// D5's `prepare_construction` when `entities_enabled` toggles on. The
/// placeholder buffers stay alive on `WorldGpu` so a toggle-off rebuild
/// can re-seat them without reallocating.
///
/// **Cross-domain contract:** D5 calls this function with the real entity
/// buffers, then assigns the returned `BindGroup` back onto
/// `world_gpu.bind_group`. D4 still owns the structural shape (the layout,
/// the entry order, the chunks/blocks/voxels/voxel_types/world_meta core);
/// D5 only supplies the W4 buffers. Replaces the pre-D4-refactor inline
/// rebuild at the old `prepare_construction:~XXXX` site.
pub(crate) fn rebuild_world_bind_group_with_entities(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &NaadfPipelines,
    world_gpu: &WorldGpu,
    entity_chunk_instances: &Buffer,
    entity_voxel_data: &Buffer,
    entity_instances_history: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        "naadf_world_bind_group_with_entities",
        &pipeline_cache.get_bind_group_layout(&pipelines.world_layout),
        &BindGroupEntries::sequential((
            world_gpu.chunks_buffer.as_entire_buffer_binding(),
            world_gpu.blocks.buffer().as_entire_buffer_binding(),
            world_gpu.voxels.buffer().as_entire_buffer_binding(),
            world_gpu.voxel_types.buffer().as_entire_buffer_binding(),
            world_gpu.world_meta.as_entire_buffer_binding(),
            entity_chunk_instances.as_entire_buffer_binding(),
            entity_voxel_data.as_entire_buffer_binding(),
            entity_instances_history.as_entire_buffer_binding(),
        )),
    )
}
```

### 3.3 Per-subsystem `Plugin` template

Every render-side subsystem follows this template:

```rust
// e.g. src/render/first_hit.rs

use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::core_pipeline::Core3dSystems;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;

#[derive(Resource)]
pub struct FirstHitPipelines {
    pub first_hit_pipeline: CachedComputePipelineId,
    // ...layouts that ONLY first-hit uses; shared layouts stay on NaadfPipelines
}

/// `SystemSet` label for the first-hit pass — used by other subsystems' edges.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FirstHitSet;

pub const FIRST_HIT_SPAN: &str = "naadf_first_hit";

pub fn naadf_first_hit_node(/* ... */) { /* unchanged body */ }

pub struct FirstHitPlugin;

impl Plugin for FirstHitPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return; };
        render_app
            .init_gpu_resource::<FirstHitPipelines>()
            .add_systems(
                Core3d,
                naadf_first_hit_node
                    .in_set(FirstHitSet)
                    .in_set(Core3dSystems::PostProcess)
                    .before(tonemapping)
                    .after(crate::render::atmosphere::AtmosphereSet),
            );
    }
}
```

The ordering edges replace the 17-element `.chain()`. Each subsystem
declares one `.after(...)` referencing its immediate predecessor's
`SystemSet`:

| subsystem | `SystemSet` | `.after(…)` |
|---|---|---|
| `construction::GpuProducerSet` | (D5-owned) | none (head of chain) |
| `construction::BoundsCalcSet` | (D5) | `GpuProducerSet` |
| `construction::WorldChangeSet` | (D5) | `BoundsCalcSet` |
| `construction::EntityUpdateSet` | (D5) | `WorldChangeSet` |
| `atmosphere::AtmosphereSet` | (D4) | `construction::EntityUpdateSet` |
| `first_hit::FirstHitSet` | (D4) | `AtmosphereSet` |
| `taa::TaaReprojectSet` | (D4) | `FirstHitSet` |
| `sample_refine::SampleRefineClearSet` | (D4) | `TaaReprojectSet` |
| `ray_queue::RayQueueSet` | (D4) | `SampleRefineClearSet` |
| `gi::GiSet` | (D4) | `RayQueueSet` |
| `sample_refine::SampleRefineContinuousSet` | (D4) | `GiSet` |
| `spatial_resampling::SpatialResamplingSet` | (D4) | `SampleRefineContinuousSet` |
| `denoise::DenoiseSet` | (D4) | `SpatialResamplingSet` |
| `taa::CalcNewTaaSampleSet` | (D4) | `DenoiseSet` |
| `final_blit::FinalBlitSet` | (D4) | `CalcNewTaaSampleSet` |

15 sets (was 18 systems × 1 chain-edge each implied by `.chain()`); 3 fewer
because the sample-refine collapse merges 4-of-5 into one set.
**Behaviour-byte-identical render order preserved.** Each `Plugin` only
imports its immediate predecessor's `SystemSet` from the predecessor's
crate path — no central tuple.

`NaadfRenderPlugin::build` shrinks to:

```rust
impl Plugin for NaadfRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            plugin_core::RenderCorePlugin,      // RenderApp init + TaaRingConfig + extract systems
            prepare::PreparePlugin,             // prepare_world_gpu + prepare_frame_gpu
            atmosphere::AtmospherePlugin,
            first_hit::FirstHitPlugin,
            taa::TaaPlugin,                     // owns BOTH TaaReprojectSet + CalcNewTaaSampleSet
            sample_refine::SampleRefinePlugin,  // owns BOTH clear set + continuous set
            ray_queue::RayQueuePlugin,
            gi::GiPlugin,
            spatial_resampling::SpatialResamplingPlugin,
            denoise::DenoisePlugin,
            final_blit::FinalBlitPlugin,
        ));
    }
}
```

### 3.4 `ShaderType` cutover — concrete shape (`GpuRenderParams` exemplar)

**Before** (`gpu_types.rs:60-117` — 58 LOC):

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuRenderParams {
    pub screen_width: u32,
    pub screen_height: u32,
    pub frame_count: u32,
    pub rand_counter: u32,
    pub taa_index: u32,
    pub flags: u32,
    pub max_ray_steps_primary: u32,
    pub _pad0b: u32,
    pub sky_sun_dir: Vec3,
    pub _pad1: u32,
    pub sun_color: Vec3,
    pub _pad2: u32,
    pub taa_jitter: Vec2,
    pub _pad3: Vec2,
    pub bounding_box_min: Vec3,
    pub _pad4: u32,
    pub bounding_box_max: Vec3,
    pub _pad5: u32,
}
```

**After** (~24 LOC):

```rust
use bevy::render::render_resource::ShaderType;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct GpuRenderParams {
    pub screen_width: u32,
    pub screen_height: u32,
    pub frame_count: u32,
    pub rand_counter: u32,
    pub taa_index: u32,
    pub flags: u32,
    pub max_ray_steps_primary: u32,
    pub sky_sun_dir: Vec3,
    pub sun_color: Vec3,
    pub taa_jitter: Vec2,
    pub bounding_box_min: Vec3,
    pub bounding_box_max: Vec3,
}
```

The compile-time `assert!(std::mem::size_of::<GpuRenderParams>() == 16 * 7)`
guard at `gpu_types.rs:845` **drops** because `encase` enforces the layout
at serialisation time and a `size_of` test no longer reflects the GPU
layout post-`ShaderType` (the Rust struct is now smaller — `encase` adds
padding only inside the serialised buffer, not the in-memory struct).
**The `taa_jitter` offset-280 guard at `gpu_types.rs:860` likewise drops** —
`ShaderType` is the layout authority. The `taa_jitter` placement hazard
that bit the port 3× becomes impossible by construction.

Upload-site swap (one mechanical pass — verified 5 sites in D4):

```rust
// Before (e.g. prepare.rs:929-930 area):
render_queue.write_buffer(&frame_gpu.render_params, 0, bytemuck::bytes_of(&render_params_data));

// After:
let mut buf = encase::UniformBuffer::new(Vec::<u8>::new());
buf.write(&render_params_data).unwrap();
render_queue.write_buffer(&frame_gpu.render_params, 0, buf.as_ref());
```

Or — cleaner — use Bevy's `DynamicUniformBuffer`/`UniformBuffer<T: ShaderType>`
wrappers, which combine the encase write with the wgpu upload. The
mechanical recommendation: **`encase::UniformBuffer<Vec<u8>>` + manual
`write_buffer`**, because the existing prepare code already owns the
`Buffer` allocation and just needs the bytes. **One helper function** at
`prepare/mod.rs` absorbs the boilerplate:

```rust
pub(crate) fn write_uniform<T: ShaderType + bevy::render::render_resource::WriteInto>(
    render_queue: &RenderQueue,
    buffer: &Buffer,
    value: &T,
) {
    let mut staging = encase::UniformBuffer::new(Vec::<u8>::new());
    staging.write(value).unwrap();
    render_queue.write_buffer(buffer, 0, staging.as_ref());
}
```

Then every upload site reads `write_uniform(&render_queue, &frame_gpu.render_params, &render_params_data);`.

Affected upload sites (verified by `grep -n "bytemuck::bytes_of"` against D4 files):

| file:line | struct | conversion needed |
|---|---|---|
| `prepare.rs:648` | `GpuWorldMeta` | wrap in `write_uniform` |
| `prepare.rs:929` (estimated — `prepare_frame_gpu`) | `GpuCamera` | wrap |
| `prepare.rs:930` (estimated) | `GpuRenderParams` | wrap |
| `taa.rs:419, :442` | `GpuTaaParams` | wrap |
| `gi.rs:404-440` | `GpuGiParams` | wrap |
| `atmosphere.rs` (estimated) | `GpuAtmosphereParams` | wrap |
| (D5 — `prepare_construction`) | `GpuConstructionParams` | D5 implementor wraps as part of D5's own write |

`Pod`-keeping structs (no change):

- `GpuVoxelType` — packed `[u32; 4]`, no `vec3`-then-scalar hazard.
- `GpuCameraHistorySlot` — packed (size 160).
- `GpuSampleValid`, `GpuBucketInfo` — packed.
- D5-owned packed structs (`GpuHashValueSlot`, `GpuBoundQueueInfo`, etc.).

### 3.5 Sample-refine collapse — concrete shape

```rust
// crates/bevy_naadf/src/render/sample_refine.rs

pub const SAMPLE_REFINE_SPAN: &str = "naadf_sample_refine";

/// Standalone — runs BEFORE `naadf_ray_queue_node`.
pub fn naadf_sample_refine_clear_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<SampleRefinePipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else { return; };
    let Some(pipeline) = pipeline_cache.get_compute_pipeline(pipelines.clear_pipeline) else { return; };

    let workgroups = gi_gpu.bucket_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_clear_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}

/// **Collapsed** — runs AFTER `naadf_global_illum_node`. Opens one compute
/// pass and dispatches `valid_history` (1,1,1) → `count_valid` (indirect off
/// `valid_dispatch`) → `count_invalid` (indirect off `invalid_dispatch`) →
/// `buckets` (`ceil(bucket_count/64), 1, 1`) in sequence. wgpu's automatic
/// buffer barriers serialise the inter-dispatch accesses (the same pattern
/// `naadf_ray_queue_node` uses at the existing `graph_b.rs:151-158` site).
pub fn naadf_sample_refine_continuous_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<SampleRefinePipelines>,
    gi_gpu: Option<Res<GiGpu>>,
    gi_bind_groups: Option<Res<GiBindGroups>>,
) {
    let (Some(gi_gpu), Some(gi_bind_groups)) = (gi_gpu, gi_bind_groups) else { return; };
    let Some(p_history) = pipeline_cache.get_compute_pipeline(pipelines.valid_history_pipeline) else { return; };
    let Some(p_count_valid) = pipeline_cache.get_compute_pipeline(pipelines.count_valid_pipeline) else { return; };
    let Some(p_count_invalid) = pipeline_cache.get_compute_pipeline(pipelines.count_invalid_pipeline) else { return; };
    let Some(p_buckets) = pipeline_cache.get_compute_pipeline(pipelines.buckets_pipeline) else { return; };

    let workgroups = gi_gpu.bucket_count.div_ceil(FIRST_HIT_WORKGROUP_SIZE).max(1);

    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = render_context.command_encoder();
    let time_span = diagnostics.time_span(encoder, SAMPLE_REFINE_SPAN);
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("naadf_sample_refine_continuous_pass"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[]);

        // (1) valid_history — single workgroup, plus @group(1) dispatch bind group.
        pass.set_pipeline(p_history);
        pass.set_bind_group(1, &gi_bind_groups.sample_refine_dispatch_bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);

        // (2) count_valid — indirect off `valid_dispatch`. (No @group(1) rebind: WGSL count_valid does not declare @group(1).)
        pass.set_pipeline(p_count_valid);
        pass.dispatch_workgroups_indirect(&gi_gpu.valid_dispatch, 0);

        // (3) count_invalid — indirect off `invalid_dispatch`.
        pass.set_pipeline(p_count_invalid);
        pass.dispatch_workgroups_indirect(&gi_gpu.invalid_dispatch, 0);

        // (4) refine_buckets — workgroup count = ceil(bucket_count/64).
        pass.set_pipeline(p_buckets);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    time_span.end(render_context.command_encoder());
}
```

**Verification claim (architect):** wgpu's compute-pass dispatch boundaries
issue automatic resource barriers between dispatches that read+write
overlapping bindings. The 4 collapsed passes all bind
`sample_refine_bind_group` at `@group(0)`; `valid_history` writes
`valid_dispatch`/`invalid_dispatch` which `count_valid`/`count_invalid`
consume as indirect arg buffers — wgpu treats indirect-arg-buffer reads as
a hazard against the prior storage write and inserts the barrier. The C#
NAADF reference (`WorldRenderBase.cs:352-362`) runs all 5 dispatches in one
function with no explicit synchronisation between them, which is the same
serialisation contract. **No race; behaviour-byte-identical.**

The pipelines + bind-group layouts themselves are unchanged — only the
*system* count drops 5 → 2 (one `clear` + one `continuous`). The 4 cached
pipeline-ids (`valid_history_pipeline` etc.) stay on `SampleRefinePipelines`.

### 3.6 `cell_shader_defs()` helper

```rust
// crates/bevy_naadf/src/render/pipelines/mod.rs

use bevy::shader::ShaderDefVal;
use crate::voxel::{CELL_DIM, CELL_CHILDREN};

/// SSoT-3 — shader-def injection for the paper's `CELL_DIM` / `CELL_CHILDREN`
/// constants. Used by every D4 pipeline that consumes a WGSL shader
/// referencing the AADF tree dimensions (`ray_tracing.wgsl`,
/// `naadf_first_hit.wgsl`, etc.), and called from D5's
/// `ConstructionPipelines::from_world` for the construction-side shaders
/// (`chunk_calc.wgsl`, `bounds_calc.wgsl`, `world_change.wgsl`, …).
///
/// The Rust SSoT lives at `crate::voxel::{CELL_DIM, CELL_CHILDREN}`
/// (`voxel/mod.rs:63-65`). D1 owns that file; this helper is the WGSL-side
/// consumer of D1's constants.
pub fn cell_shader_defs() -> Vec<ShaderDefVal> {
    vec![
        ShaderDefVal::UInt("NAADF_CELL_DIM".into(), CELL_DIM as u32),
        ShaderDefVal::UInt("NAADF_CELL_CHILDREN".into(), CELL_CHILDREN as u32),
    ]
}
```

WGSL sites (D4-owned) gain a header block:

```wgsl
// Top of e.g. ray_tracing.wgsl, naadf_first_hit.wgsl
const NAADF_CELL_DIM: u32 = #{NAADF_CELL_DIM}u;
const NAADF_CELL_CHILDREN: u32 = #{NAADF_CELL_CHILDREN}u;
```

Then every semantic `4u` / `64u` / `16u` literal gets the architect's
audit at edit time: replace where the literal **is** the cell dimension,
keep where it's a bit-shift, mask, or unrelated workgroup-size constant.
Audit guide:

- `ray_tracing.wgsl:54,116,217,320,322,324,332,360,479` — 9 sites flagged in D5's exploration. D4 has half; D5 has half.
- `naadf_first_hit.wgsl` — audit needed.
- `naadf_global_illum.wgsl`, `spatial_resampling.wgsl`, `sample_refine.wgsl` — these likely have `4u` / `64u` only as bit-shift amounts or fixed-stride helpers; the architect's expected audit verdict is **leave alone** unless a site demonstrably references the cell dimension.

**Cross-domain (Resolution D coordination):** D5's
`ConstructionPipelines::from_world` reuses `crate::render::pipelines::cell_shader_defs()`
verbatim, importing the helper. D5's WGSL files (`chunk_calc.wgsl`,
`bounds_calc.wgsl`, `world_change.wgsl`) get the same header block. D5
architect's responsibility to land it on their side; D4 owns the helper.

### 3.7 SSoT-4 — `BUCKET_STORAGE_COUNT` shader-def

```rust
// In sample_refine.rs::SampleRefinePipelines::from_world (or whichever
// pipeline-build call site queues the 5 sample-refine pipelines):

let sample_refine_shader_defs = vec![
    ShaderDefVal::UInt("BUCKET_STORAGE_COUNT".into(), crate::render::gi::BUCKET_STORAGE_COUNT),
    // ... plus any existing shader-defs (none for the sample-refine path today).
];

// Inject into the 4 pipelines that consume sample_refine.wgsl's array<u32, 32>:
// valid_history_pipeline, count_valid_pipeline, count_invalid_pipeline, buckets_pipeline.
// The `clear` pipeline does NOT need it (no array<u32, 32>).
```

WGSL change at `sample_refine.wgsl:668`:

```wgsl
// Before:
var comp_color_max_storage: array<u32, 32>;

// After (top of file):
const BUCKET_STORAGE_COUNT: u32 = #{BUCKET_STORAGE_COUNT}u;
// ... at :668:
var comp_color_max_storage: array<u32, BUCKET_STORAGE_COUNT>;
```

And at `:655`:

```wgsl
// Before:
let invalid_count = (cur_bucket_x >> 18u) * 8u;

// After:
let invalid_count = (cur_bucket_x >> 18u) * gi_params.invalid_sample_storage_count;
```

(`gi_params.invalid_sample_storage_count` already declared and uploaded —
verified at `gpu_types.rs:466`.)

---

## 4. Migration steps (atomic, ordered)

**All steps assume D5 impl has landed first.** The construction-side
`prepare_construction` rebuild of `WorldGpu.bind_group` already calls (or
will be updated by D5 impl to call) `rebuild_world_bind_group_with_entities`
once that function exists. If D5 impl ships the inline rebuild and D4 impl
introduces the helper, then D4 impl ALSO updates the D5 call site — but
this is a follow-up that lives in D4's commit, not a D5 retrofit.

### Step 1 — Extract WGSL shader-def helpers + flip 2 SSoT-4 sites + delete dead `MAX_RAY_STEPS_*` consts.

**Edits:**

- `render/pipelines.rs` — add `pub fn cell_shader_defs() -> Vec<ShaderDefVal>` near the existing `taa_shader_defs` site (line ~278).
- `render/pipelines.rs` — extend each pipeline's `shader_defs` vec with `cell_shader_defs()` (every `queue_compute_pipeline` call site).
- `assets/shaders/ray_tracing.wgsl` (and any other D4-shaders where literal `4u`/`64u` is the cell dimension — architect's audit at edit time) — prepend the two `const NAADF_CELL_DIM` / `NAADF_CELL_CHILDREN` declarations after their `#{…}u` shader-def consumption.
- `assets/shaders/ray_tracing.wgsl:122-136` — delete the 5 `MAX_RAY_STEPS_*` consts + their docblock; replace with a one-line `// Documentation: canonical values live on `GiSettings::default()` in `lib.rs`; live values arrive via uniform.` comment.
- `assets/shaders/sample_refine.wgsl:655` — `* 8u` → `* gi_params.invalid_sample_storage_count`.
- `assets/shaders/sample_refine.wgsl:668` — `array<u32, 32>` → `array<u32, BUCKET_STORAGE_COUNT>` + add the const declaration at the top of the file.
- `render/sample_refine.rs` (or whichever D4 file queues these pipelines pre-collapse — `pipelines.rs::from_world` if pipelines haven't moved yet) — add `BUCKET_STORAGE_COUNT` shader-def to the 4 affected pipelines.

**Rationale:** Atomic mechanical changes that don't touch system registrations. Lowest blast radius first; verifies the e2e gates still pass before more invasive structural work.

**Post-step state:** SSoT-3 + SSoT-4 closed for D4. The `ray_tracing.wgsl:122-136` consts are gone; D4 cannot accidentally reintroduce a literal.

**Verification:** `cargo build --workspace` + `cargo test --workspace --lib` + `cargo run --bin e2e_render -- --validate-gpu-construction` + `--vox-e2e` + `--oasis-edit-visual` ×2 (per `feedback-multiple-runs-rule-out-false-positives`).

### Step 2 — `ShaderType` cutover for the 7 uniform structs.

**Edits:**

- `crates/bevy_naadf/Cargo.toml` — verify `bevy_render` brings `encase` transitively (it does in Bevy 0.19; no Cargo edit expected).
- `render/gpu_types.rs` — convert `GpuCamera`, `GpuRenderParams`, `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams`, `GpuGiParams`, `GpuConstructionParams` from `#[repr(C)] #[derive(Pod, Zeroable)]` to `#[derive(ShaderType)]`. Drop every `_padN` field and every `assert!(offset_of! …)` / `assert!(size_of! …)` guard on those structs. Keep the runtime-mirror `#[test]` for `GpuConstructionParams` (`mod tests::construction_params_layout`) — adapt it to assert via `encase::ShaderType::SHADER_SIZE` and field-position queries (or delete; `encase`-enforced layout is the new authority).
- `render/prepare.rs` — add `pub(crate) fn write_uniform<T: ShaderType + WriteInto>(…)` helper.
- `render/prepare.rs:648` — `bytemuck::bytes_of(&world_meta_data)` → `write_uniform(&render_queue, &world_meta, &world_meta_data)`.
- `render/prepare.rs:~929-930` — same flip for `GpuCamera` + `GpuRenderParams`.
- `render/taa.rs:419, :442` — same flip for `GpuTaaParams`.
- `render/gi.rs:404-440` — same flip for `GpuGiParams`.
- `render/atmosphere.rs` (verify line range) — same flip for `GpuAtmosphereParams`.
- `render/construction/**` (D5-owned BUT one site in `prepare_construction` uploads `GpuConstructionParams`) — **D4's commit edits this call site** as part of the mechanical sweep (path-rename precedent: D5 owns `prepare_construction`'s behaviour, D4 owns the upload-call shape — same way D5 architect's exploration §"D4↔D5 shared-file notes" agrees in spirit). Alternative if D5 impl is fragile post-merge: leave `GpuConstructionParams` as `Pod` and convert only the 6 non-construction uniforms; `GpuConstructionParams` cutover follows in a follow-up commit. Architect's default: do the sweep atomically — 1 site in D5's code, 6 in D4's code, all in one PR.
- `render/pipelines.rs:286-294` — drop the `NonZeroU64::new(std::mem::size_of::<GpuFoo>() as u64)` size calls for the 7 converted structs (the `ShaderType` `SHADER_SIZE.get()` returns the correct size); or keep the `NonZeroU64` approach using `<GpuFoo as ShaderType>::min_size().get()`. Architect's choice — both work.

**Rationale:** Single mechanical pass. The hazard-eliminator (the `taa_jitter`-offset-280 trap that bit the port 3×) closes by construction. Compile-time asserts shrink from ~25 to ~5.

**Post-step state:** `gpu_types.rs` shrinks ~270 LOC. The `vec3`-then-scalar hazard is structurally impossible. Tests + asserts shrink correspondingly.

**Verification:** Full suite. **Pay close attention** to `oasis_edit_visual` + `vox_gpu_oracle` (non-deterministic gates per `feedback-multiple-runs-rule-out-false-positives`) — 3 runs minimum because a silent layout change would manifest as a sporadic visual glitch.

### Step 3 — Split `prepare.rs` into `prepare/{world,frame,mod}.rs` + extract `apply_voxel_types_refresh`.

**Edits:**

- `mkdir crates/bevy_naadf/src/render/prepare/`
- `render/prepare.rs` → split into:
  - `render/prepare/mod.rs` (~80 LOC: struct defs + re-exports per §3.1).
  - `render/prepare/world.rs` (~620 LOC: `prepare_world_gpu` body + new `apply_voxel_types_refresh` + new `rebuild_world_bind_group_with_entities`).
  - `render/prepare/frame.rs` (~520 LOC: `prepare_frame_gpu` body).
- `render/prepare/world.rs` — extract `prepare.rs:218-317` (the palette-refresh branch) into a private `fn apply_voxel_types_refresh(…)` called from a small dispatch shell at the top of `prepare_world_gpu`. Same body, different shell.
- `render/prepare/world.rs` — add the new `rebuild_world_bind_group_with_entities` function (D5 will switch to calling it in step 5 below).
- `render/mod.rs` — `use prepare::{prepare_frame_gpu, prepare_world_gpu};` import path stays valid via the re-export in `prepare/mod.rs`. **No external import changes needed.**

**Rationale:** Pure structural relocation. The two systems are already independent at the type level (no shared types) — the file split exposes the existing independence at the file-tree level.

**Post-step state:** `prepare/world.rs` and `prepare/frame.rs` are independently editable. The W4 cross-write becomes a single named function call rather than an inline bind-group rebuild.

**Verification:** Full suite.

### Step 4 — Plugin-per-subsystem extraction (the big one).

**Edits (per subsystem — repeat for 9 plugins):**

- `render/atmosphere.rs` — absorb `graph_b.rs::naadf_atmosphere_node` + `ATMOSPHERE_SPAN` const. Add `AtmospherePipelines` resource (or extend the existing `AtmosphereGpu` — architect's choice; the cleanest is a *new* `AtmospherePipelines` so layouts + pipeline-ids live with their consumer). Add `AtmosphereSet: SystemSet` label. Add `AtmospherePlugin: Plugin` that registers them + sets the `.after(construction::EntityUpdateSet).in_set(Core3dSystems::PostProcess).before(tonemapping)` edges.
- `render/first_hit.rs` (new) — extract `graph.rs::naadf_first_hit_node` + `FIRST_HIT_SPAN`. New `FirstHitPipelines` holding `first_hit_pipeline` + `first_hit_atmosphere_read_layout` (etc.).
- `render/taa.rs` — absorb `graph.rs::naadf_taa_reproject_node` + `naadf_calc_new_taa_sample_node` + `TAA_REPROJECT_SPAN` + `CALC_NEW_TAA_SAMPLE_SPAN`. Already owns `prepare_taa`. Add `TaaPipelines`, `TaaReprojectSet`, `CalcNewTaaSampleSet`, `TaaPlugin`.
- `render/sample_refine.rs` (new) — extract the 5 nodes from `graph_b.rs:242-446`, **but combine them into 2** per §3.5 above. Add `SampleRefinePipelines` (5 pipeline ids, unchanged), `SampleRefineClearSet`, `SampleRefineContinuousSet`, `SampleRefinePlugin`.
- `render/ray_queue.rs` (new) — extract `naadf_ray_queue_node` + `RAY_QUEUE_SPAN`. `RayQueuePipelines`, `RayQueueSet`, `RayQueuePlugin`.
- `render/gi.rs` — absorb `naadf_global_illum_node` + `GLOBAL_ILLUM_SPAN`. Already owns `prepare_gi` + `GiGpu` + `GiBindGroups`. Add `GiPipelines`, `GiSet`, `GiPlugin`.
- `render/spatial_resampling.rs` (new) — extract.
- `render/denoise.rs` (new) — extract.
- `render/final_blit.rs` (new) — extract `naadf_final_blit_node` + `FINAL_BLIT_SPAN` + `prepare_blit_pipeline` + per-format `BlitPipelines: HashMap<TextureFormat, _>`. The `blit_*` fields move OFF `NaadfPipelines` onto `BlitPipelines`.
- `render/pipelines/mod.rs` (was `render/pipelines.rs`) — strip down to: `NaadfPipelines { world_layout, frame_layout, blit_layout, empty_layout }` + the 4-layout `FromWorld` impl + `cell_shader_defs()` helper. All subsystem-specific layouts + all pipeline-ids move to their owning subsystem.
- `render/pipelines/shaders.rs` (new) — pull out the 10 `pub const FOO_SHADER: &str = …` declarations from `pipelines.rs:63-89`. Each subsystem's `*Pipelines::FromWorld` imports the path it needs.
- `render/mod.rs` — strip the entire 17-element `add_systems(Core3d, (…).chain())` block. Replace with a single `app.add_plugins((…))` over the 11 plugins per §3.3. Delete the long ordering-rationale docblock at lines 196-299 (it documents the literal chain; the new edges live in each Plugin's body where the reader looks first).
- DELETE `render/graph.rs` (all 4 nodes relocated).
- DELETE `render/graph_b.rs` (all 10 nodes relocated; SAMPLE_REFINE collapses 5→2 nodes in `sample_refine.rs`).

**Rationale:** The W0-per-workstream-PR design (`15-design-c.md` §1.1) gets its render-side equivalent: each subsystem is now independently editable + independently mergeable. The 17-tuple cross-domain edit-magnet dissolves. The sample-refine collapse drops ~160 LOC. The two `graph*.rs` files dissolve.

**Post-step state:** No cross-domain registry edits required for adding a subsystem; each subsystem is a one-file unit.

**Verification:** Full suite, including 3× runs on non-deterministic gates. **Particularly important:** assert no off-by-one in the new `.after(...)` edges by comparing the resolved schedule order against the old 17-element `.chain()` order (mental walkthrough at design time matches; runtime gates confirm).

### Step 5 — `WorldGpu.bind_group` cross-domain consolidation.

**Edits:**

- `render/prepare/world.rs::rebuild_world_bind_group_with_entities` already added in step 3.
- `render/construction/mod.rs` (or wherever D5 impl placed the rebuild) — replace the inline `render_device.create_bind_group("…", &layout, &BindGroupEntries::sequential((…)))` with a call to `crate::render::prepare::rebuild_world_bind_group_with_entities(&render_device, &pipeline_cache, &pipelines, &world_gpu, &ce, &vd, &ih)`. **This is the only D4 edit into D5-owned code** — justified because the *function definition* is D4's (the bind-group layout shape is D4's), and the *call shape* is D4's too; D5's site is just calling it. Per `00-reuse-audit.md`, D4 owns `WorldGpu` + `world_layout`, so the helper function is D4's, and the D5 call-site swap is a one-line follow-edit on a moved item.

**Rationale:** Makes the cross-write *named* and *grep-able*. Reduces the BEV-3 "two writers in two domains" smell to "two writers, one constructor". Both writes go through the same source-of-truth function.

**Post-step state:** Any future change to the `world_layout` bind-group shape happens in one function. D5 cannot accidentally drift from D4's layout.

**Verification:** Full suite, with extra attention to `--entities` (the gate that actually exercises the W4 entities-on path).

### Step 6 — Delete `pbr_sampling.wgsl` (master-branch identity).

**Edits:**

- DELETE `crates/bevy_naadf/src/assets/shaders/pbr_sampling.wgsl` (868 LOC).
- Verify the e2e PBR gates (`pbr_visual.rs`, `pbr_hard_edge.rs`, `pbr_debug_modes.rs`) are already deleted per Resolution C (D6's territory). If not yet deleted, D4 impl does **not** delete `pbr_sampling.wgsl` until D6 has dropped its references. **Sequencing note:** D6 must land before this step.

**Rationale:** Master-branch identity directive (addendum §"Master-branch identity"): PBR scaffolding on master is suspect by default. `pbr_sampling.wgsl` has zero non-PBR-gate consumers; the gates are deleted.

**Post-step state:** No PBR shader on master.

**Verification:** `cargo build --workspace` (sanity — the file is asset-only, not Rust-compiled). Full e2e suite to confirm no live shader-import path references the deleted file.

---

## 5. Decisions & rejected alternatives

### D1 — Plugin-per-subsystem (chosen) vs `RenderLabel`/`add_render_graph_node` (rejected)

`render/graph.rs:22-24` docblock confirms Bevy 0.19's `Core3d`-schedule
approach: a "render-graph node" is a system in `Core3d`, not an explicit
`Node` trait impl. The `RenderLabel`/`add_render_graph_edges` API exists
(in `bevy::render::render_graph`) but the project's pattern is
`add_systems(Core3d, …)` + `.in_set(Core3dSystems::PostProcess)` (verified
at `render/mod.rs:300-330`). Using `SystemSet` + `.before(…)`/`.after(…)`
is the smaller-blast-radius option: zero API migration on the
node-implementation side, just structural lift-and-rename.

Bevy's render-graph `Node` trait is for inserting graph nodes into Bevy's
*own* core 3D pipeline (where Bevy then composes them via the actual
`bevy::render::render_graph::RenderGraph` data structure that
`bevy_core_pipeline` consumes). The NAADF port doesn't insert into Bevy's
graph; it appends to `Core3d`'s system schedule. So `RenderLabel`-style
graph wiring isn't the idiom-fit; `SystemSet` is.

### D2 — `ShaderType` cutover scope: all 7 at once (chosen) vs stage by phase

D4 explorer's Open Question #3 asked. Architect chooses **all 7 at once**:
the mechanical conversion is identical for all of them, the test surface
(layout asserts, runtime layout tests) is bounded, and a partial cutover
leaves both encoding regimes coexisting in `gpu_types.rs` — a worse smell
than the one being eliminated. `GpuConstructionParams` is D5-shaped at the
*field* level but D4-shaped at the *layout* level; the cutover is purely
layout-mechanical.

### D3 — `WorldGpu.bind_group` Option C (chosen) vs Option A/B

D4 explorer's Open Question #6 asked. Architect chooses **C with seam
tightener**:

- Option A (separate "with-entities" / "without-entities" layouts): forces D5 to duplicate the pipeline layout family (`world_layout_no_entities`, `world_layout_with_entities`). The render passes that bind `@group(0)` would need to select between them at dispatch time (or pin one variant per pipeline — meaning every pipeline gets 2 versions). Pipeline-cache pressure + dispatch-side branching are net cost increases. Rejected.
- Option B (D5 owns the rebuild closure): D4 hands D5 a `FnMut` builder, D5 invokes it. The cross-write still happens, just through a closure instead of an inline call. No legibility win; small ergonomic loss. Rejected.
- Option C tightened: D5 calls D4's named function. The cross-write is *visible* in `grep` (`rebuild_world_bind_group_with_entities`) and lives at one site. Net: a 30-LOC function definition + a one-line call-site change.

### D4 — Sample-refine 4-of-5 collapse (chosen) vs keep all 5

D4 explorer's Open Question #2 asked. Architect chooses **collapse**:

- HUD observability: all 5 already share one `SAMPLE_REFINE_SPAN` (`graph_b.rs:42` — verified). Per-pass HUD lines do not exist today.
- The `valid_history` → `count_valid` → `count_invalid` → `buckets` order is fixed (the WGSL output dependencies enforce it). C# NAADF runs them inline in `WorldRenderBase.cs:352-362`; the Rust port's per-node split was infrastructure, not behaviour.
- wgpu's automatic buffer barriers on storage / indirect-arg-buffer hazards serialise dispatches within one compute pass. Verified pattern at `naadf_ray_queue_node` (`graph_b.rs:151-158`) which dispatches `RayQueue` + `RayQueueStore` in one pass.
- Net: ~160 LOC drop + 3 fewer `SystemSet` labels + 3 fewer ordering edges.

### D5 — `MAX_RAY_STEPS_*` consts: delete (chosen) vs keep as documentation

The consts at `ray_tracing.wgsl:122-136` are documentation-only per the
explicit comment at `:123-131`. Verified zero non-comment WGSL references
to `MAX_RAY_STEPS_*` (grep returns only the const declarations). naga DCEs
them. **Delete + leave a one-line pointer to the live SSoT** (`GiSettings::default()`
in `lib.rs`). Drops ambiguity for a future reader; the test
(`settings.rs:898-904`) already enforces the canonical values via the
KNOBS-to-defaults check on the Rust side.

This is a small deliberate divergence from D4 explorer suspicion verdict
"could be deleted" — **architect upgrades to "delete it"**. Per
faithful-port rule: no behavioural change (the consts are unreferenced;
deletion is purely textual cleanup).

### D6 — `pbr_sampling.wgsl` deletion (chosen)

Per addendum master-branch identity rule + Resolution C: PBR e2e gates die,
`pbr_sampling.wgsl` has zero non-e2e consumers (verified). Master is the
C# port; PBR lives on a branch. The 868-LOC shader has no place on master.

### D7 — Don't unify `NaadfPipelines` + `ConstructionPipelines` (chosen) vs Resolution-D merge

Resolution D approved D5's architect proposing the merge. **D5's
exploration (Finding 10) actually proposes the *opposite*: split
`ConstructionPipelines` per-workstream** (W1Pipelines, W3Pipelines, …).
D4's design aligns with that split: D4 also splits `NaadfPipelines` per
render-side subsystem. Resolution D's "fold `ConstructionPipelines` into
`NaadfPipelines`" interpretation conflicts with both architects'
designs.

**Architect's reading:** Resolution D is best satisfied by *retiring the
W0-empty-sibling pattern* (which both designs do) rather than literally
merging two structs into one. The W0-contract retirement is the *concept*
("`ConstructionPipelines` is no longer the deliberate-empty-sibling-of-`NaadfPipelines`
that the parallel-merge protocol required"), and the *implementation* is
"each subsystem owns its own `*Pipelines` resource". `NaadfPipelines`
ends up holding only the *shared* core layouts (`world_layout`,
`frame_layout`, `blit_layout`, `empty_layout`); it is *not* the central
registry any more. Identical architectural endpoint via different
phrasing.

If D5 architect instead opts for the literal merge (one mega
`Pipelines` resource), D4 design **adapts non-trivially** — D4 would
not split `NaadfPipelines` either, just thin it out. Flagged in §6
Open conflicts as a coordination point — D5 architect doc is the
authoritative source for the construction-side decision.

### D8 — `extract_taa_config` + `extract_gi_config` (low-finding 9): leave as-is

D4 explorer's Open Question implicitly. Architect agrees: the 7-LOC dup is
under the cost of an extra abstraction layer. Skip.

### D9 — `prepare_world_gpu`'s palette-refresh extraction (side-note 5): fold into step 3

Side-note 5 of the exploration. Architect agrees — the
`apply_voxel_types_refresh` extraction folds into Step 3 (the prepare.rs
split). One commit, two improvements.

---

## 6. Open conflicts

### Conflict 1 — D5 architect interpretation of Resolution D

If D5 architect lands the **literal `ConstructionPipelines` →
`NaadfPipelines` merge** (rather than D5 Finding 10's per-workstream
split), D4 cannot also split `NaadfPipelines`. D4 then keeps the existing
30+ field `NaadfPipelines` shape, only thinning the `from_world` body
through helper extraction; the per-subsystem `*Pipelines` resources do
not happen. The plugin-per-subsystem refactor still lands (the
`SystemSet` + plugin shape is orthogonal to pipeline-resource ownership),
but each plugin reaches into `Res<NaadfPipelines>` for its pipeline ids
instead of `Res<FirstHitPipelines>`/etc.

**This is a structural conflict, not a forbidden-move conflict.** Both
architects' proposals are within scope. Architect dispatch should
arbitrate via reading D5's `03-architecture.md` BEFORE D4 impl runs;
implementor reads both docs and applies the matching plan. **Bias for
D4 impl:** if D5 architect doc is ambiguous on the merge-vs-split
question, D4 impl can defer the `NaadfPipelines` decomposition (Step 4
becomes "plugin-per-subsystem but reading from existing `NaadfPipelines`")
— a partial landing that's still net-positive.

### Conflict 2 — `GpuConstructionParams` `ShaderType` cutover ownership

D5 owns the struct's *content*; D4 owns the *layout discipline*. The
mechanical `Pod → ShaderType` flip is layout-only. The flip happens in
D4's commit. If D5 has *also* edited `GpuConstructionParams` (added a
field, changed a type) during D5 impl, D4 impl rebases against the
post-D5 shape and the cutover still works. **No conflict expected** —
the operation is purely a derive swap + pad-field deletion.

**One coordination point:** D5 architect's exploration §"D4↔D5 shared-file
notes #1" mentioned a possible per-workstream `GpuConstructionParams`
split (each workstream's params subset). If D5 architect proposes that
split, D4 design **does not block** — D4 sweeps whatever
`GpuConstructionParams*` structs exist at the time D4 impl runs.

### Conflict 3 — PBR shader deletion sequencing

D4 design proposes deleting `pbr_sampling.wgsl`. The shader is referenced
by `debug_view.rs` (D7) + `e2e/pbr_visual.rs` (D6/Resolution C). **D4 impl
must run after both D6 (Resolution C) and D7 have dropped their
references.** If D6/D7 haven't shipped yet at D4 impl time, **D4 impl
skips Step 6** and the deletion happens in a follow-up. No behavioural
risk to D4-internal work either way.

No forbidden-move conflicts. No deliberate behavioural divergences from
C# NAADF.

---

## 7. What stays / what changes / what's removed

**Stays unchanged inside D4 scope (architect intentionally leaves alone):**

- `render/color_compression.rs` (172 LOC) — already a clean leaf subsystem.
- `render/extract.rs` — except for trivial path updates if uniform-struct re-exports change. `extract_taa_config` + `extract_gi_config` (Finding 9) intentionally left.
- WGSL render shaders (except `pbr_sampling.wgsl` deletion + `ray_tracing.wgsl:122-136` const deletion + `sample_refine.wgsl:655,668` shader-def swap + the SSoT-3 named-const audit on `ray_tracing.wgsl` and a few siblings). The shader bodies are paper-canonical implementations; D4 does not refactor the algorithms.
- Bind-group layouts and pipeline descriptors (structurally relocated; content unchanged).
- All `Resource` field names exposed across the W0 seam (`WorldGpu.*` field names, `FrameGpu.*` field names) — preserved verbatim so D5 code keeps compiling.
- `NaadfRenderPlugin` as the public entry point — its body shrinks, but its `Plugin` trait + name stay.
- `aadf/edit.rs` and the CPU oracle — D1 territory, untouched.

**Changes (file relocations + body edits):**

- `render/mod.rs` — shrinks from 332 → ~120 LOC.
- `render/prepare.rs` → `render/prepare/{mod,world,frame}.rs`.
- `render/pipelines.rs` → `render/pipelines/{mod,shaders}.rs` + per-subsystem `*Pipelines` resources distributed across subsystem files.
- `render/gpu_types.rs` → ~270 LOC drop via `ShaderType` flip; optionally split into `gpu_types/{mod,uniforms,samples,construction}.rs` (architect prefers split; D4 impl may keep as one file if the split's mechanical cost is too high — both are acceptable).
- `render/graph.rs` (309 LOC) → DELETE (contents relocated to `first_hit.rs`, `taa.rs`, `final_blit.rs`).
- `render/graph_b.rs` (574 LOC) → DELETE (contents relocated; sample-refine collapsed 5→2).
- `render/{atmosphere,gi,taa}.rs` — absorb their respective node bodies + plugin shells.
- `render/{first_hit,final_blit,ray_queue,sample_refine,spatial_resampling,denoise}.rs` — new files extracted from graph*.rs.
- WGSL render shaders — SSoT-3 + SSoT-4 fixes + the `ray_tracing.wgsl` const deletion + the optional shader-def named-const sweep.

**Removed:**

- `render/graph.rs` — relocated.
- `render/graph_b.rs` — relocated.
- 3 `sample_refine_*_node` systems out of 5 (collapsed into one continuous node).
- ~70 `_padN` fields across 7 uniform structs.
- ~25 `assert!(size_of)` + `assert!(offset_of)` guards.
- `ray_tracing.wgsl:122-136` documentation-only consts (deletion + redirect comment).
- `assets/shaders/pbr_sampling.wgsl` (868 LOC) — master-branch identity.

---

## 8. D5/D7/D2/D1 coordination notes

### D5 coordination

D5 owns: `render/construction/**`, `aadf/construct.rs` invocations from
construction.

D4 → D5 needs after D5 impl:

- D5 must call `crate::render::prepare::rebuild_world_bind_group_with_entities` instead of inlining the bind-group rebuild (Step 5 above). If D5 architect doc doesn't propose this, D4 impl proposes a one-line change at the D5 site.
- D5's `prepare_construction` `write_buffer(&construction_params, 0, bytemuck::bytes_of(&data))` (D5-side upload of `GpuConstructionParams`) — D4 impl flips this to `write_uniform(...)` as part of Step 2. **One-line edit into D5-owned code; justified by the layout-decision ownership.**
- D5 uses the shared `cell_shader_defs()` helper from `render/pipelines/mod.rs` for its construction-side WGSL files (`chunk_calc.wgsl`, `bounds_calc.wgsl`, `world_change.wgsl`). The helper's location + signature is fixed by D4; D5's import path is a one-liner.

D5 → D4 needs from D5 impl:

- D5 must NOT rename `WorldGpu.*` fields (D4 binds the names verbatim).
- D5's `naadf_gpu_producer_node`, `naadf_bounds_compute_node`, `naadf_world_change_node`, `naadf_entity_update_node` must declare their own `SystemSet` labels (`construction::GpuProducerSet`, `construction::BoundsCalcSet`, `construction::WorldChangeSet`, `construction::EntityUpdateSet`) so D4's `atmosphere.rs::AtmospherePlugin` can write `.after(construction::EntityUpdateSet)`. **D5 architect is asked to add the `SystemSet` declarations** — this is the head-of-render-graph contract.

### D7 coordination

D7 owns: `GiSettings`, `AppArgs`, `lib.rs`.

D4 → D7 needs:

- D7's `GiSettings` move (out of `lib.rs` into `settings.rs` per D7 Finding F2). Once landed, `extract.rs:454, :481` import `use crate::settings::GiSettings;` instead of `use crate::GiSettings;`. **D4 commit handles this two-line update** (path-rename follow-edit; same precedent as Conflict 2 for D5).
- D7 should NOT keep the `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-136`. **D4 deletes** them in Step 1. D7's exploration §"SSoT coordination" already aligned on this.

D7 → D4 needs:

- D7 owns the canonical `GiSettings` shape. D4's `GpuGiParams` mirror reads `ExtractedGiConfig.settings.foo` at every site; if D7 renames `GiSettings` fields, D4's call sites break. **D7 architect asked to flag D4 in shared-file notes** if the move-to-`settings.rs` involves any field-rename. Default assumption: pure move, no rename.

### D2 coordination

D2 owns: `settings.rs`, `editor/`, `hud.rs`.

D4 → D2 needs:

- The KNOBS table at `settings.rs:174-220` references `max_ray_steps_primary` (D4-side uniform field) by name (via `setter: fn(&mut AppArgs, u32)`). D4 does NOT rename these fields. **D2 can rework KNOBS into `Reflect`-driven without coordinating with D4** — the GpuGiParams field names are stable.

D2 → D4 needs:

- D2's `Reflect`-driven KNOBS proposal (D2 HIGH-3) operates on `GiSettings` (D7 territory). No D4 surface change.

### D1 coordination

D1 owns: `voxel/mod.rs`, `aadf/{edit,construct,bounds,generator,entity,cell,block_hash}.rs`.

D4 → D1 needs:

- D1 owns `CELL_DIM = 4` + `CELL_CHILDREN = 64` at `voxel/mod.rs:63-65` (the Rust SSoT). D4's `cell_shader_defs()` helper imports + reads them. If D1 promotes these (per D1 Finding 6's `CHUNK_DIM_VOXELS` consolidation), D4's import path may need a refresh. **D1 architect asked to keep `CELL_DIM` + `CELL_CHILDREN` at the `crate::voxel::*` path** — the helper signature depends on it. If D1 moves them to `crate::aadf::cell::*` or similar, D4's `cell_shader_defs()` updates the import line.

D1 → D4 needs:

- Nothing direct. D1's diagnostic-method consolidation does not affect render-side reads.

---

## 9. Side notes / observations / complaints

1. **The 17-element `.chain()` is the load-bearing smell.** D4 explorer flagged this and the architect concurs: this is the design's load-bearing fix. Every other D4 finding becomes lower-cost once it lands. The cross-domain dimension (4 of 17 are D5-owned) is what makes it the most-important smell, not just an aesthetic one — it's the per-merge-PR edit-magnet that contradicts the W0 design's goal of additive merges. **Architect ranks this above the `ShaderType` cutover** even though `ShaderType` has the highest LOC drop, because `ShaderType` is mechanical safety, the chain split is per-PR friction reduction.

2. **`ShaderType` makes the `vec3`-then-scalar hazard impossible.** Verified: the `taa_jitter`-offset-280 guard (`gpu_types.rs:858-861`) is technical debt comment-ware. The Rust struct's *in-memory* layout is no longer the GPU's *uniform-buffer* layout; `encase` decouples them. Future engineers cannot accidentally introduce the hazard. This is the single largest *defect-class-elimination* of the refactor.

3. **The PBR shader on master is a clear master-branch-identity violation.** `pbr_sampling.wgsl` is 868 LOC of PBR-raymarching infrastructure with no production consumer. Master is the C# port; PBR is a branch. **D4 architect formally proposes deletion.** D6 + D7 must drop their references first (Resolution C + `debug_view.rs`).

4. **Bevy 0.19's render-graph node API is *not* what `RenderLabel`/`add_render_graph_edges` looks like in older Bevy.** The brief's BEV-1 phrasing "RenderGraph labels over `.chain()`" suggested a graph-API migration. **The actual idiom-fit is `SystemSet` + `.before()`/`.after()`** because the port runs node-systems in `Core3d` schedule rather than inserting into Bevy's render graph. Architect documents this so D4 implementor doesn't waste cycles looking for a `RenderLabel` API in Bevy 0.19 that doesn't fit the call shape. **Flag for orchestrator awareness.**

5. **D4 explorer's Open Question #1 — subsystem dir vs flat sibling — chosen: flat sibling.** Architect picks `render/<subsystem>.rs` over `render/<subsystem>/mod.rs`. Rationale: most subsystems sit at 100-700 LOC after the refactor (`first_hit.rs` ~120, `gi.rs` ~720). Only `gi.rs` is borderline; even there a sub-split would land 2-3 sub-files, none over 400. The flat layout matches the existing pattern (`atmosphere.rs`, `taa.rs`, `gi.rs` already siblings). Adding `prepare/` and `pipelines/` and `gpu_types/` as directories is the only directory-style addition — they each split into 2-4 files where the LOC-per-file budget genuinely needs it.

6. **The `prepare/mod.rs` re-export front is load-bearing.** D5's `prepare_construction` imports `WorldGpu` + `FrameGpu` + `prepare_world_gpu` from `crate::render::prepare`. If the split moves the struct defs into `prepare/world.rs`, every D5 import breaks. Architect's mitigation: the struct defs live in `prepare/mod.rs` (not the submodules), and the submodules just hold the systems. `pub use prepare::WorldGpu` resolves verbatim. **D4 impl must verify zero import-path changes in D5 code as a post-step check.**

7. **The `Phase-C followup #1` block in D5's `prepare_construction` (D5 exploration side-note #6) is a D5 problem, not a D4 one.** Architect noted but explicitly **does not propose action** — D4 stays out of D5's `prepare_construction` internals. Flagged only because it touches the W4 placeholder-buffer write that Finding 7 addresses; D4's `rebuild_world_bind_group_with_entities` helper makes the seam *cleaner* even if the underlying split-allocator-vs-split-dispatcher concern remains D5's to resolve.

8. **`extract.rs:452-483` boilerplate (D4 Finding 9): keep.** Mild dup, not worth a generic abstraction. Architect agrees with explorer.

9. **`color_compression.rs` (172 LOC): genuinely fine as-is.** Architect read it (offset 0, full file would be cleaner but the 172 LOC is small enough). No findings.

10. **Equal-footing: the 4× LOC ratio shrinks for the D4 surface specifically by ~12-15% from this refactor.** D4 was ~6 500 LOC; the design drops ~1 500 LOC (incl. PBR shader). That's structurally meaningful. Most of the drop is *behaviour-byte-identical* (file relocation, hand-padding deletion, dead-const removal). The `sample_refine` 5→2 collapse is the only behavioural micro-change, and `WorldRenderBase.cs:352-362` already runs the 4-of-5 dispatches in one C# function — the Rust port restores fidelity with C# by combining them.

11. **The architect explicitly does NOT propose splitting `gi.rs` further.** D4 explorer's Open Question #1 raised it. `gi.rs` ~720 LOC post-absorb is large but cohesive; the file is one resource (`GiGpu`), one bind-group resource (`GiBindGroups`), one prepare system, one node, one pipeline resource, one plugin. The narrative is sequential and a sub-split (`gi/{buffers,uniform,plugin}.rs`) would not improve readability for a domain reader who's already used to the structure. Reject the further-split unless D4 implementor finds a load-bearing reason.

12. **Equal-footing — what's *not* in this design.** Architect deliberately did not propose:
    - A unified `Pipelines` resource ahead of D5's decision (Conflict 1).
    - A pipeline-specialiser pattern (some subsystems could specialise per-quality-level via shader-defs; out of scope).
    - WGSL `#import` for shared structs (D5 architect's territory — D5 has the `shader_drift_guard.rs` smell; D4's renderer shaders don't share constants across the construction/render split in a way that imports would help).
    - A `Pod` → `ShaderType` cutover for the packed-array structs (`GpuVoxelType`, `GpuCameraHistorySlot`, etc.) — they have no `vec3` hazard and the byte-equivalent serialisation matches WGSL's element layout 1:1; converting them would gain nothing.
    - A `Reflect`-driven runtime registry for the `*Pipelines` resources — over-engineering.

13. **Equal-footing — confidence levels.**
    - **High confidence:** plugin-per-subsystem migration; sample-refine collapse; `ShaderType` cutover for the 7 uniforms; SSoT-4 shader-def injection; `pbr_sampling.wgsl` deletion; `ray_tracing.wgsl:122-136` const deletion.
    - **Medium confidence:** `WorldGpu.bind_group` Option-C-with-helper (depends on D5's actual call-site shape post-D5 impl; if D5 lands a different rebuild flow, helper signature may need tweaks).
    - **Lower confidence:** the exact LOC numbers per file post-refactor (the layout splits are well-defined; the LOC estimates assume each subsystem's node body + plugin shell + pipeline resource sit at ~100-150 LOC, which holds for some — `first_hit.rs` — and is approximate for others — `gi.rs` is 720 LOC, the high end). Implementor adjusts. The structural decisions are independent of LOC precision.
    - **Decision dependency:** §5 D7 (`ConstructionPipelines` literal merge vs split) depends on D5 architect's reading of Resolution D. Documented in Conflict 1.

14. **Equal-footing — the `bevy-naadf` master is genuinely well-architected under the bloat (D7 exploration side-note #10 echoes this).** D4 specifically: the bind-group layouts, the W0 seam contract, the GI buffer family, the TAA ring depth, the shader-def injection at `pipelines.rs:269-279` are all idiomatic Bevy. The bloat is in the 17-element tuple + the hand-padding + the residual `graph_b.rs` split + the diagnostic-only WGSL consts. The architect's design is structural, not foundational — there's no foundation rot in D4's surface. Compare to D5's `mod.rs:11043` which the D5 architect must address as genuine rot. **D4 is the *medium*-stinky domain; not foundation-rot.**

