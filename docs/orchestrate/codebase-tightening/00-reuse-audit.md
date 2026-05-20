# Codebase-tightening — Reuse audit

**Author**: re-implementation auditor (delegate orchestration).
**Date**: 2026-05-20.
**Goal**: produce LOC/structural map, propose 4–8 loosely-coupled domains for
parallel /refactor sub-orchestrations, and surface crosscutting reuse / SSoT
violations / Bevy-idiom misfits.

---

## 1 — LOC comparison and structural map

### 1.1 Totals

| side | tool | total lines (source) | counted files |
|---|---|---|---|
| C# reference (`/mnt/archive4/DEV/NAADF/NAADF/`) | `wc -l *.cs` | **9 467** | 64 `.cs` |
| C# reference (`Content/shaders/**/*.fx`) | `wc -l *.fx` | 3 606 | 24 `.fx` |
| C# reference (in-scope `.cs` + shaders) | sum | **13 073** | 88 |
| Rust port (`crates/bevy_naadf/src/**/*.rs`) | `wc -l *.rs` | **52 410** | 73 `.rs` |
| Rust port (`assets/shaders/**/*.wgsl`) | `wc -l *.wgsl` | 8 727 | 26 `.wgsl` |
| Rust port `crates/voxel_noise/src/` (FastNoise2 wrapper — unused) | `wc -l *.rs` | 1 033 | 4 |
| Rust e2e Playwright TS (`e2e/tests/**/*.ts`) | `wc -l *.ts` | 1 638 | 6 |
| Rust workspace + supporting (`Cargo.toml` + `Trunk.toml` + `justfile` + `index.html` + `scripts/`) | rough | ~2 200 | ~10 |
| **Rust port total source surface** | sum | **~66 008** | ~119 |

**Headline ratio (source-to-source)**: Rust port ≈ **4.0×** the C# reference's
`.cs+.fx` line-count (66 008 / 13 073 / 4.0). The user's framing — "the
codebase has grown larger than C#" — is empirically correct, but the
multiplier is concentrated in three areas, not uniform:

- **Rust `render/construction/mod.rs`**: 11 043 LOC. Single file. By itself
  it is 84% of the entire in-scope C# reference. Discussed below.
- **Rust `e2e/`**: 10 292 LOC (Rust) + 1 638 LOC (Playwright). Has no C#
  counterpart at all; the C# reference is verified by the user running the
  windowed app. This is *deliberate* (project-CLAUDE.md verification
  discipline), but it is also where the largest opaque growth lives.
- **Documentation comments + WGSL shader files**: the Rust source carries
  much heavier `//!` and `///` doc-comment headers than the C# source — many
  files are 60-80% comment by mass (e.g. `lib.rs`, `gpu_types.rs`,
  `construction/mod.rs`). The C# uses XML doc comments far more sparingly.
  This is irreducible bloat by file-line-count metric, but it is real
  documentation rather than dead code.

### 1.2 C# reference — top files (in-scope `.cs`)

```
  849 World/Model/ModelData.cs
  757 Libraries/VoxelsCore/MagicaVoxel.cs
  559 World/Data/EntityHandler.cs
  522 World/Data/WorldData.cs
  487 World/Render/Versions/WorldRenderBase.cs
  462 Gui/Internal/ImGuiRenderer.cs              (out of scope — Q1)
  328 Common/PathHandler.cs                       (out of scope — Q1)
  312 World/Data/ChangeHandler.cs
  249 World/Data/EditingHandler.cs
  213 World/Data/BlockHashingHandler.cs
  212 Common/Camera.cs
  190 World/Render/Versions/WorldRenderPathTracer.cs   (deferred — Q1)
  169 World/VoxelTypeHandler.cs
  160 Settings.cs                                       (out of scope)
  157 World/Render/Versions/WorldRenderAlbedo.cs
  151 World/Render/WorldRender.cs
  141 World/Data/WorldBoundHandler.cs
  133 World/Data/EditingTools/EditingToolModel.cs       (deferred — skip)
  132 World/Data/EntityData.cs
  124 Gui/Main/World/WorldEntitiesUi.cs                 (out of scope)
  ...
```

C# top-level dir LOC: `World 4 983 / Gui 1 510 (OOS) / Libraries 1 454 /
Common 1 106 / (root) 414`. **In-scope (Q1) C# core** = `World + Libraries +
Common + (root) ≈ 7 957 LOC + 3 606 LOC `.fx` ≈ 11 563`. Everything else
(`Gui/`, `Settings.cs`, `PathHandler.cs`, `IO.cs`, `obj2voxel`,
`WorldRenderPathTracer.cs`) was either explicitly deferred or out of scope
in the original Q1.

### 1.3 Rust port — top files

```
11043  src/render/construction/mod.rs    <— single largest file, 21% of all Rust
 1956  src/e2e/driver.rs
 1733  src/voxel/vox_import.rs
 1731  src/world/data.rs
 1354  src/voxel/grid.rs
 1207  src/render/prepare.rs
 1165  src/render/construction/world_change.rs
 1146  src/lib.rs
 1055  src/render/gpu_types.rs
 1023  src/e2e/pbr_hard_edge.rs
 1014  src/render/construction/bounds_calc/tests.rs
  940  src/settings.rs
  909  src/render/pipelines.rs
  903  src/editor/hud.rs
  835  src/aadf/bounds.rs
  828  src/aadf/edit.rs
  813  src/e2e/gates.rs
  747  src/e2e/pbr_visual.rs
  711  src/diagnostics.rs
  699  src/e2e/vox_e2e.rs
  696  src/e2e/vox_gpu_oracle.rs
  681  src/e2e/small_edit_visual.rs
  619  src/render/construction/bounds_calc.rs
  618  src/render/gi.rs
  612  src/aadf/block_hash.rs
  608  src/voxel/web_vox.rs
  597  src/editor/tools.rs
  574  src/render/graph_b.rs
  570  src/aadf/construct.rs
  543  src/voxel/cvox_import.rs
  514  src/e2e/framebuffer.rs
  507  src/aadf/generator.rs
  506  src/render/taa.rs
  493  src/e2e/vox_gpu_construction.rs
  ... (39 more files)
```

### 1.4 Rust subdir totals

```
23 344  render/   (incl. 11 043 in construction/mod.rs alone)
10 292  e2e/      (NO C# counterpart)
 4 757  voxel/    (incl. ~2 500 of .vox/.cvox import; C# `MagicaVoxel.cs` is 757)
 4 167  aadf/     (cell + bounds + construct + edit + entity + generator + block_hash)
 3 653  (root)    (lib.rs 1146 + settings.rs 940 + diagnostics.rs 711 + ...)
 2 161  world/    (data.rs 1731 + buffer.rs 399 + mod.rs 31)
 1 836  editor/   (hud.rs 903 + tools.rs 597 + mod.rs 275 + ray.rs 61)
   891  bin/      (e2e_render 481 + diag_compare 314 + bake 96)
   785  texture_array/
   464  camera/
    60  material_set/
```

### 1.5 The 11 043-line file — `src/render/construction/mod.rs`

This module by itself is ~84% the size of the **entire in-scope C# port
target**. It is the single load-bearing piece of structural rot in the port.
Outline (functions/types only — `grep -n "^pub fn\|^fn\|^pub struct\|^pub
enum\|^impl"`):

| line | item | purpose |
|---|---|---|
| 106 | `pub struct ConstructionGpu` | render-world resource holding every Phase-C buffer family (16+ fields) |
| 352 | `pub enum ReadbackStage` | enum for `aadf_*_probe` two-phase staging-buffer machine |
| 376 | `pub struct CpuMirrorReadback` | per-frame readback state + `Arc<AtomicBool>` done flags |
| 414 | `pub struct ConstructionBindGroups` | parallel resource for the bind groups (5+ `Option<BindGroup>`) |
| 482 | `pub struct ConstructionPipelines` | empty sibling of `NaadfPipelines` (Phase-C contract — never edit the parent) |
| 573 | `impl FromWorld for ConstructionPipelines` | ~180 lines registering the W1–W5 compute pipelines |
| 755 | `pub struct ConstructionEvents` | per-frame upload event queue |
| 855 | `pub struct MainWorldEntities` | main-world entity ledger |
| 883 | `pub struct RenderWorldEntityState` | render-world mirror of entity state |
| 910 | `pub fn extract_world_changes` | 180-line extract-system body |
| 1092 | `pub fn populate_cpu_mirror_from_gpu_producer` | 560-line GPU→CPU readback path |
| 1658 | `pub fn prepare_construction` | **1 420-line** monster `Prepare` system — allocates every W1–W5 buffer + builds every bind group |
| 3076 | `pub fn naadf_gpu_producer_node` | 470-line node-system with a 3-way ladder (W5 path / chunk-calc path / CPU fallback) |
| 3547 | `pub struct ConstructionPlugin` | the wiring `Plugin` |
| 3559 | `pub struct AadfDelayedProbe` + `aadf_delayed_probe` (3594) | 270 lines of late-readback diagnostic — sole consumer the wasm-chunk-aadf-determinism investigation |
| 3873 | `pub struct AadfPerCallProbe` + `aadf_per_call_probe` (3915) | similar per-call diagnostic — 170 lines |
| 4088 | `pub struct AadfCpuGpuParity` + `aadf_cpu_gpu_parity*` (4133, 4597) | 600 lines of bit-exact CPU/GPU oracle diff machinery |
| 4727 | `pub fn build_segment_voxel_buffer_from_dense` | CPU↔GPU buffer-encoding helper |
| 4820 | `pub fn build_segment_voxel_buffer` | same again, different shape |
| 4928 | `pub fn validate_gpu_construction` | **e2e CLI gate body — 360 lines, runs MinimalPlugins + RenderPlugin + full Algorithm 1 dispatch** |
| 5290 | `pub fn validate_gpu_construction_scaled` | **scaled variant — 190 lines** |
| 5481 | `fn discover_populated_oasis_voxels` | one-off Oasis-vox investigation helper |
| 5621 | `pub fn validate_gpu_construction_production_scale` | **third variant — ~700 LOC** |
| 6371–8331 | `fn readback_cursor / map_single_u32 / map_single_pair / sample_voxel_readback / render_results_table / run_one_fixture_byte_diff / run_one_fixture_multiseg_byte_diff / run_one_generator_model_byte_diff / run_one_tiled_byte_diff / decode_segment_voxels_into_volume` | **~3 200 lines of test-fixture infrastructure**, all `pub fn` or `fn`, called from `validate_gpu_construction*` |
| 8958+ | various `crate::aadf::generator::ModelData`-typed helpers | leftovers |
| 10 529 | `mod tests` | embedded unit test module |

**Distribution of the 11 043 lines** — by counted segment-class:

| segment | est. LOC | share |
|---|---|---|
| `validate_gpu_construction*` (3 variants) + helpers (`run_one_*_byte_diff` × 4) | **~5 000** | **~45%** |
| `aadf_*_probe` + `aadf_cpu_gpu_parity` diagnostic systems | **~1 100** | ~10% |
| `prepare_construction` system body | **~1 420** | ~13% |
| `naadf_gpu_producer_node` | ~470 | ~4% |
| `populate_cpu_mirror_from_gpu_producer` | ~560 | ~5% |
| Resource structs + `FromWorld` impls | ~700 | ~6% |
| Buffer-encoding helpers + segment_voxel_buffer builders | ~500 | ~5% |
| `mod tests` | ~500 | ~5% |
| Module docs + other glue | ~800 | ~7% |

**So**: roughly **half of the 11 043 lines is test/validation/diagnostic
infrastructure that happens to live in the production module**. The
prepare-system body (1 420 lines) is the next-biggest mass — a single function.

---

## 2 — Domain decomposition proposal

The user wants 4–8 domains, parallelisable, each refactor-able independently.
The codebase's natural seams are:

```
   ┌──────────────────────────────────────────────────────────────┐
   │                       app wiring                              │
   │      lib.rs · main.rs · app_mode.rs · camera/ · hud.rs        │   D7
   └──────────────────────────────────────────────────────────────┘
              │                                              │
              ▼                                              ▼
   ┌──────────────────────┐                       ┌──────────────────────┐
   │   editor + settings  │                       │ diagnostics + texture│
   │   editor/ · settings │                       │      _array + bake   │   D8
   │      D2              │                       │ diagnostics · bin/   │
   └──────────────────────┘                       └──────────────────────┘
              │                                              │
              ▼                                              ▼
   ┌──────────────────────────────────────────────────────────────┐
   │              voxel I/O + grid                                 │
   │ voxel/vox_import · cvox_import · web_vox · async_vox · grid   │   D3
   │ (+ voxel_noise crate — unused)                                │
   └──────────────────────────────────────────────────────────────┘
              │                                              │
              ▼                                              ▼
   ┌──────────────────────┐                       ┌──────────────────────┐
   │   AADF data + CPU    │                       │  e2e + Playwright    │
   │   construction       │                       │      harness         │   D6
   │   aadf/ · world/     │                       │ e2e/ · e2e/tests/    │
   │      D1              │                       │                      │
   └──────────────────────┘                       └──────────────────────┘
              │
              ▼
   ┌──────────────────────────────────────────────────────────────┐
   │       GPU construction (Phase-C, the 11k mod.rs)              │   D5
   │  render/construction/  + assets/shaders/{chunk,bounds,...}    │
   └──────────────────────────────────────────────────────────────┘
              │
              ▼
   ┌──────────────────────────────────────────────────────────────┐
   │  render pipeline (Phase-A/B GI + TAA + first-hit + blit)      │   D4
   │  render/{extract,prepare,gi,taa,graph,graph_b,pipelines,...}  │
   │  + assets/shaders/{naadf_first_hit, naadf_global_illum, taa,  │
   │  sample_refine, spatial_resampling, denoise_split, ...}       │
   └──────────────────────────────────────────────────────────────┘
```

The proposed domains (eight, sized 1 200 – 11 000 LOC):

---

### D1 — `aadf-data-structures` (AADF cell, CPU construction, world container)

- **One-line**: the paper's load-bearing data structure — cell encoding +
  CPU AADF computation + the `WorldData` container, target-agnostic.
- **Paths**: `crates/bevy_naadf/src/aadf/{cell,bounds,construct,edit,entity,generator,block_hash,mod}.rs`,
  `crates/bevy_naadf/src/world/{data,buffer,mod}.rs`,
  `crates/bevy_naadf/src/voxel/mod.rs` (the bit-layout constants live here, dependency root for `aadf/`).
- **LOC**: 4 167 (aadf) + 2 161 (world) + 145 (voxel/mod) ≈ **6 470**.
- **Why its own domain**: this is the canonical paper-port surface — pure
  data structures + pure CPU functions, no Bevy systems, no `RenderApp`,
  no GPU. It is the natural "library core" of the port (the C# `Libraries/
  VoxelsCore/` + `World/Data/` partition). Every other domain depends on
  it; it depends on nothing project-specific (only `bevy::math` + `bytemuck`).
- **Initial suspicion list** (top 2-3 smells):
  1. **`world/data.rs:1731 LOC`** with three near-parallel set-voxel
     entry points (`set_voxel`, `set_voxels_batch`, `set_voxels_batch_oracle`,
     `set_chunks_uniform_batch`) — the docblock literally labels two of
     them `DIAGNOSTIC-ONLY` and notes "production code paths NEVER call
     these methods" (lines 19-34). The diagnostic-only path should move to
     a `#[cfg(test)]` or a `pub(crate) mod oracle` to stop bloating the
     resource type's API.
  2. **`aadf/edit.rs:828 LOC`** is the CPU oracle for the W2 GPU shader.
     Now that the GPU path is the production producer (per E4), this file
     is *only* test infrastructure — but its `pub fn` surface is consumed
     from production paths in `world/data.rs:set_voxel(diagnostic-only)` +
     `render/construction/world_change.rs:476` (a `#[cfg(test)]` mod
     `use crate::aadf::edit::{apply_block_edit_cpu, ...}`). Audit whether
     the public surface is honest or whether some entry points are only
     test fixtures masquerading as `pub`.
  3. **`aadf/bounds.rs:835`** + **`aadf/construct.rs:570`** + **`aadf/generator.rs:507`** —
     three siblings each computing AADF over different layout shapes
     (4³ block, full chunk pipe, per-segment generator). Audit whether
     `compute_aadf_layer` can absorb the generator path or whether the
     three are genuinely different layouts.
- **Estimated tightening surface**: extract diagnostic-only methods out
  of `WorldData`'s API; collapse the 3-4 set-voxel entry points; extract
  `aadf/edit.rs` GPU-oracle helpers behind `#[cfg(test)]` or a sibling
  `oracle` mod; verify the three AADF-computation paths share a core.

---

### D2 — `editor-and-settings-ui` (Bevy-UI editor HUD + settings overlay)

- **One-line**: the in-game editor HUD (paint/cube/sphere tools, palette,
  brush radius) + the Escape settings overlay (the GI/raymarching knob
  panel). Pure Bevy-UI.
- **Paths**: `crates/bevy_naadf/src/editor/{hud,mod,ray,tools}.rs`,
  `crates/bevy_naadf/src/settings.rs`, `crates/bevy_naadf/src/hud.rs`,
  `crates/bevy_naadf/src/app_mode.rs`.
- **LOC**: 1 836 (editor) + 940 (settings) + 245 (hud) + 99 (app_mode) ≈ **3 120**.
- **Why its own domain**: this is the deliberate divergence from the C#
  ImGui tree (per `feature-completeness/01-context.md`). Pure UI surface,
  pure main-world. No GPU, no render-graph. The natural pair: `editor/`
  and `settings/` mutate the same `AppArgs`/`GiSettings`/`EditorState`
  resources and use the same `bevy_ui` 0.19 vocabulary.
- **Initial suspicion list**:
  1. **`settings.rs:940`** contains the `KNOBS: &[Knob]` table — `~30`
     rows of `Knob { label, class, kind: KnobKind::{U32,F32,Bool,Readonly,
     Action} { getter: fn(&GiSettings) -> _, setter: fn(&mut GiSettings, _),
     nudge, big_step, min, max, default } }`. This is a function-pointer
     visitor over `GiSettings` — every new GI knob means a new full row.
     Worth auditing: a `bevy_reflect`-driven impl (`GiSettings: Reflect`
     + a tagging attribute-derived metadata table) could collapse the
     `~30 × 7-field` table into one decl-macro per field. **OR** the
     project deliberately keeps it explicit — this is a Bevy-idiom
     judgment call.
  2. **`editor/hud.rs:903`** vs **`settings.rs:940`** — both author `bevy_ui`
     `Node`-tree spawns ~600 LOC each with very similar idioms (colour
     palette consts, `spawn_h_row`/`spawn_v_row` helpers in editor;
     `setup_settings` builder in settings). Audit whether shared
     `node_*`/`row_*` helpers would deduplicate the two UI builders.
  3. **`editor/tools.rs:597`** has `paint_brush` / `cube_brush` /
     `sphere_brush` — three near-parallel brush implementations each ~120
     LOC. Mirror smell: three "classify chunk vs brush" fns (`brush_aabb`
     / `brush_chunk_aabb` / `sphere_chunk_classify` / `cube_chunk_classify`).
     Some of these could share a `BrushShape` trait.
- **Estimated tightening surface**: reflect-driven settings panel (or
  decl-macro to declare knobs); shared UI helpers across `editor/hud.rs`
  + `settings.rs`; brush trait/visitor pattern.

---

### D3 — `voxel-io-and-grid` (`.vox` / `.cvox` parsers, web async loading, grid builder)

- **One-line**: the `.vox` (MagicaVoxel) + `.cvox` (NAADF native) importers,
  the wasm/native async pump that lands them into the world, the hard-coded
  default-scene grid builder, and the (currently-unused) FastNoise2 wrapper.
- **Paths**: `crates/bevy_naadf/src/voxel/{vox_import,cvox_import,web_vox,
  async_vox,voxel_dispatch,grid}.rs`, `crates/voxel_noise/` (entire crate).
- **LOC**: 4 757 (voxel/, sans mod.rs which went to D1) + 1 033 (voxel_noise) ≈ **5 790**.
- **Why its own domain**: I/O surface — totally separable from rendering
  and from AADF data layout. `voxel_noise` is a sibling crate the original
  context tags as **"NOT yet wired into the renderer"** (`Cargo.toml`
  module docstring). The voxel I/O is also the only domain with a meaningful
  wasm vs native split (`web_vox.rs` vs `async_vox.rs` + dnd listeners).
- **Initial suspicion list**:
  1. **`voxel/vox_import.rs:1733 LOC`** is genuinely large — but it
     hand-rolls the `.vox` scene graph collation against `dot_vox`
     (`Rot3`/`Xform`/`accumulate_world_aabb`/`ChunkBuckets`/
     `collate_voxels_sparse`/`compose_to_sparse_world`/
     `compose_models0_fallback`/`build_constructed_world_sparse`).
     Audit: is the C# `MagicaVoxel.cs` doing all this, or are some of
     these our own additions? The C# reference is 757 LOC.
  2. **`voxel/grid.rs:1354`** has `install_default_embedded_in_fixed_world`
     + `install_vox_in_fixed_world` + (removed) `install_vox_sized_to_model`,
     plus `setup_test_grid` (a `Startup` system that branches into all of
     the above based on `AppArgs.grid_preset`). The branching is documented
     in detail (lines 73-100), suggesting historical churn — worth
     auditing whether the three install paths can collapse now that
     vox-gpu-rewrite Stage 2 consolidated.
  3. **`crates/voxel_noise/`** — 1 033 LOC of FastNoise2 FFI wrapper,
     declared in `Cargo.toml`'s workspace docs as "NOT yet wired into the
     renderer". This is **explicit dead code**: zero callers from
     `bevy_naadf`. Probably worth deleting or fencing behind a Cargo
     feature.
- **Estimated tightening surface**: confirm/remove `voxel_noise`; audit
  whether the `dot_vox` scene-graph collation is really 2× the C#
  `MagicaVoxel.cs` size for good reason; collapse `voxel/grid.rs`'s
  install branches.

---

### D4 — `render-pipeline` (Phase-A/A-2/B render graph: first-hit, TAA, GI, denoise, blit)

- **One-line**: the actual renderer — extract/prepare/graph systems, GI
  pipeline resources, TAA, atmosphere, the WGSL render shaders. **No
  construction code.**
- **Paths**: `crates/bevy_naadf/src/render/{mod,atmosphere,color_compression,
  extract,gi,gpu_types,graph,graph_b,pipelines,prepare,taa}.rs`,
  `crates/bevy_naadf/src/assets/shaders/{naadf_first_hit,naadf_final,
  naadf_atmosphere,naadf_global_illum,sample_refine,spatial_resampling,
  ray_queue_calc,ray_tracing,ray_tracing_common,render_pipeline_common,
  taa,taa_common,denoise_split,color_compression,common,world_data,
  gi_params,atmosphere,pbr_sampling}.wgsl`.
- **LOC**: render/ excluding `construction/` subdir = 23 344 − (sum under
  `render/construction/`) ≈ **7 281 Rust** + WGSL shaders excluding
  construction = 8 727 − (chunk_calc 577 + world_change 579 + bounds_calc
  572 + bounds_common 191 + entity_update 137 + generator_model 160 +
  map_copy 127) ≈ **6 384 WGSL** ≈ **13 665 total**.
- **Why its own domain**: this *is* the headline NAADF port — the paper's
  §4 application. The natural boundary is "everything `Core3d`-graph and
  WGSL render shader, but not construction". The `construction/`
  sub-module already has its own enforced seam (W0 design contract — see
  `15-design-c.md`) so this split is *already structurally enforced*.
- **Initial suspicion list**:
  1. **`render/prepare.rs:1207`** contains both `prepare_world_gpu`
     (build-once world hand-off) AND `prepare_frame_gpu` (per-frame
     camera uniform + bind group + first-hit-data resize). Two systems
     unrelated semantically — load-bearing comments span lines 1-35
     explaining the split. Audit whether splitting the file is structural
     win.
  2. **`render/gpu_types.rs:1055`** + **`render/pipelines.rs:909`** — every
     uniform has a `#[repr(C)]` mirror plus padding fields explicitly
     listed (`_pad0`, `_pad0b`, `_pad1`, ...). Hardcoded layouts are
     idiomatic but the file is 1k LOC of "field, padding, field, padding".
     `bevy::render::render_resource::ShaderType` derives + `encase`
     would auto-handle std140/std430 — Bevy 0.19 ships this. Worth a
     hard look (the project deliberately uses `bytemuck::Pod` instead).
  3. **`render/mod.rs`** lists `add_systems(Core3d, (...)).chain()` with
     **17 named nodes**. That's a tall ladder — the `naadf_sample_refine_*`
     family has 5 separate node systems. Bevy's `RenderGraph` has an
     edges/labels mechanism designed for this; using `.chain()` on a
     17-element tuple is structurally fine but inflexible (any new node
     forces editing this one spot, which the W2/W4 seams explicitly
     wanted to avoid).
- **Estimated tightening surface**: split `prepare.rs`; consider
  `ShaderType` over hand-padded `Pod`; consider `RenderGraph` labels
  instead of the giant `.chain()`. Also audit `gi.rs:51-60` constants
  (4 `pub const`s) — these have to agree with WGSL `2u`/`8u`/`32u`/`8u`
  literals (verified — they don't propagate). SSoT divergence.

---

### D5 — `gpu-construction` (Phase-C: the 11k mod.rs + W0-W5 sub-modules)

- **One-line**: the Phase-C GPU construction sub-graph — chunk_calc,
  bounds_calc, world_change, entity_update, generator_model passes.
  Everything under `render/construction/`.
- **Paths**: `crates/bevy_naadf/src/render/construction/**`,
  `crates/bevy_naadf/src/assets/shaders/{chunk_calc,bounds_calc,
  bounds_common,world_change,entity_update,generator_model,map_copy}.wgsl`.
- **LOC**: 16 062 Rust + 2 343 WGSL ≈ **18 405**. **This is the biggest
  domain by far**, driven by `construction/mod.rs:11 043`.
- **Why its own domain**: Phase-C's W0 design contract (`15-design-c.md`
  §1) already makes this an enforced seam — workstreams W1..W5 each landed
  separately under `render/construction/`. The submodules
  (`bounds_calc.rs`, `chunk_calc.rs`, `change_handler.rs`, `entity_handler.rs`,
  `entity_update.rs`, `generator_model.rs`, `hashing.rs`, `map_copy.rs`,
  `shader_drift_guard.rs`, `world_change.rs`, `config.rs`) are sane sizes;
  the rot is concentrated in `mod.rs`. Refactoring this domain doesn't
  touch the renderer, the world container, or the editor — it's tightly
  scoped.
- **Initial suspicion list**:
  1. **`construction/mod.rs:11 043 LOC`** — see §1.5 above. Five distinct
     concerns mashed into one file: (a) resource definitions, (b) the
     `prepare_construction` 1 420-line system, (c) the `naadf_gpu_producer_node`
     470-line node, (d) **diagnostic probes (~1 100 LOC)** that should
     live behind a `#[cfg(debug_assertions)]` or a `diagnostics` submodule,
     (e) **headless test-validation fixtures (~5 000 LOC)** that should
     either move to `tests/` integration tests or to dedicated
     `validate.rs` sub-modules. Splitting this single file along its
     internal `// === W… ===` section dividers is the single biggest LOC
     reduction available in the port.
  2. **`validate_gpu_construction_production_scale` boots a full
     `RenderApp` from inside the production crate** (line 5621, ~700 LOC).
     This is the inverse-of-idiomatic — production code paths should not
     contain the test fixtures that exercise them. Move to a
     `bevy_naadf::test_fixtures` mod (gated `#[cfg(any(test, e2e))]`)
     or a separate dev-dependency crate.
  3. **`AadfDelayedProbe` / `AadfPerCallProbe` / `AadfCpuGpuParity`** are
     all "investigation residuals" from `wasm-chunk-aadf-nondeterminism`.
     The probes are kept "in case the bug recurs". Worth surveying which
     can move behind a Cargo feature `diagnostic-probes` or be deleted
     wholesale (per the project's `deadcode` skill discipline).
- **Estimated tightening surface**: the **single biggest LOC win** is
  here — splitting `mod.rs` into `mod.rs` (orchestration) +
  `prepare.rs` + `producer.rs` + `validation.rs` + `probes.rs` could
  drop the file from 11k to ~2-3k, with the rest behind `#[cfg]`.

---

### D6 — `e2e-and-playwright` (the entire e2e harness)

- **One-line**: the deterministic e2e harness — bounded-frame driver,
  framebuffer capture, region/SSIM gates, browser Playwright tests.
- **Paths**: `crates/bevy_naadf/src/e2e/**`, `crates/bevy_naadf/src/bin/{e2e_render,diag_compare}.rs`, `e2e/{playwright.config.ts,tests/**}`.
- **LOC**: 10 292 Rust + 1 638 TS + 481 (`bin/e2e_render.rs`) + 314
  (`bin/diag_compare.rs`) ≈ **12 725**.
- **Why its own domain**: the e2e harness has no C# counterpart and is
  entirely the project's own invention to bypass `cargo run --bin
  bevy-naadf` verification (per `CLAUDE.md`). Loosely coupled from
  production — it consumes `build_app(AppConfig::e2e())` as a black box.
  Each gate (`oasis_edit_visual`, `small_edit_visual`, `vox_e2e`,
  `vox_gpu_construction`, `vox_gpu_oracle`, `vox_web_parity`,
  `vox_horizon_parity`, `pbr_*`, ...) is largely independent code in its
  own file.
- **Initial suspicion list**:
  1. **`e2e/driver.rs:1956 LOC`** — single state-machine system. Has 4
     enum phases (`WARMUP`/`MOTION`/`SETTLE`/`SHOOT`/`DRAIN`/`ASSERT`)
     plus the bolted-on `ResizeTestState`. Worth auditing for split.
  2. **9+ gate-specific files** (`oasis_edit_visual.rs:453`,
     `small_edit_repro.rs:376`, `small_edit_visual.rs:681`,
     `vox_e2e.rs:699`, `vox_gpu_construction.rs:493`,
     `vox_gpu_oracle.rs:696`, `vox_horizon_parity.rs:246`,
     `vox_web_parity.rs:428`, `pbr_debug_modes.rs:218`,
     `pbr_hard_edge.rs:1023`, `pbr_visual.rs:747`) — each has its own
     `pin_*_camera` system, its own `State` resource, its own assertions.
     The pattern is consistent; a shared `GateRunner<G: Gate>` trait could
     absorb the boilerplate.
  3. **`e2e/framebuffer.rs:514`** + **`e2e/ssim.rs:229`** + **`e2e/gates.rs:813`** —
     image-diff utilities that probably overlap with existing crate
     functionality (`image`, `dssim`, `image-compare` — already a dep?
     audit `Cargo.toml`).
- **Estimated tightening surface**: gate-runner trait; driver state-
  machine split; shared image-diff helpers; verify Playwright TS doesn't
  re-implement Rust-side gates.

---

### D7 — `app-and-camera` (app wiring, camera, modes, hud diagnostics)

- **One-line**: the `App::new()` orchestration — `lib.rs`, `main.rs`,
  free-camera plug-in, position-split, the press-P diagnostics dump,
  the device-snapshot capture.
- **Paths**: `crates/bevy_naadf/src/{lib,main}.rs`,
  `crates/bevy_naadf/src/camera/{mod,position_split}.rs`,
  `crates/bevy_naadf/src/diagnostics.rs`.
- **LOC**: 1 146 (lib) + 75 (main) + 464 (camera) + 711 (diagnostics) ≈ **2 396**.
- **Why its own domain**: this is the spine — every other module wires
  through `build_app`. Refactoring the app graph (plug-in topology, ordering
  constraints) is its own concern, distinct from any subsystem.
- **Initial suspicion list**:
  1. **`lib.rs:1 146 LOC`** — `build_app_with_args` is a massive
     monolithic system-registration function (lines 638-974, ~340 lines).
     Every plug-in inserts directly into the one place. The Bevy idiom
     would be **a thin `App::new() + DefaultPlugins + plugins!` chain
     with each subsystem owning a `Plugin`** — the project already does
     this for `NaadfRenderPlugin`, `WorldPlugin`, `ConstructionPlugin`,
     `BakedMaterialPlugin`, `TextureArrayPlugin`, `DiagnosticsPlugin`,
     `DeviceSnapshotPlugin` — but the editor / settings / HUD / camera
     wiring is still inline. Pulling those into `EditorPlugin` /
     `SettingsPlugin` / `CameraPlugin` could drop `lib.rs` to ~500 LOC.
  2. **`GiSettings` (lib.rs:109-185)** lives in `lib.rs`, but **the knobs
     table that drives it lives in `settings.rs`**, and the WGSL uniform
     fields (`max_ray_steps_*`, `spatial_iter_count`, `sun_shadow_taps`)
     are duplicated in `render/gpu_types.rs`. Three sources of truth for
     the same set of fields (audit details below in §3).
  3. **`diagnostics.rs:711`** mixes the press-P dump (148 LOC) with the
     **device-snapshot capture submodule (~560 LOC)**. They share zero
     types and zero callers. Worth splitting into `diagnostics/dump.rs` +
     `diagnostics/device_snapshot.rs`.
- **Estimated tightening surface**: pull editor/settings/camera/hud
  inline-`add_systems` blocks into their own `Plugin`s; split
  `diagnostics.rs`; move `GiSettings` to its own file (or to `settings.rs`).

---

### D8 — `asset-pipeline` (texture array bake, baked material loader, bin/bake)

- **One-line**: the offline texture-array baker, the `material.ron` loader,
  the `bake` binary.
- **Paths**: `crates/bevy_naadf/src/texture_array/**`,
  `crates/bevy_naadf/src/baked_material.rs`,
  `crates/bevy_naadf/src/material_set/mod.rs`,
  `crates/bevy_naadf/src/bin/bake.rs`.
- **LOC**: 785 + 220 + 60 + 96 ≈ **1 161**.
- **Why its own domain**: pure asset-pipeline machinery. Zero callers
  from the production renderer (per `lib.rs:752-763` — the texture-array
  plug-in registers loaders but "nothing in the scene consumes a baked
  material yet"). **This is essentially infrastructure-only code** with
  no live consumer.
- **Initial suspicion list**:
  1. **No live consumer**. The `baked_material::MaterialRonLoader`
     registers, but no `Startup` system loads any material. Likely a
     candidate for deletion or feature-gating per `deadcode` skill.
  2. **`material_set/mod.rs:60 LOC`** — what is this for? Tiny module,
     probably dead.
- **Estimated tightening surface**: confirm dead-or-alive status with the
  user; either wire into the renderer or delete.

---

### Domain LOC summary

| domain | Rust | WGSL | TS | total | parallelism rank |
|---|---|---|---|---|---|
| D1 — aadf-data-structures | 6 470 | — | — | 6 470 | low coupling — easy |
| D2 — editor-and-settings-ui | 3 120 | — | — | 3 120 | low coupling — easy |
| D3 — voxel-io-and-grid | 5 790 | — | — | 5 790 | low coupling — easy |
| D4 — render-pipeline | 7 281 | 6 384 | — | 13 665 | medium — shares `gpu_types` |
| D5 — gpu-construction | 16 062 | 2 343 | — | 18 405 | medium — shares `gpu_types` |
| D6 — e2e-and-playwright | 11 087 | — | 1 638 | 12 725 | low — black-box consumer |
| D7 — app-and-camera | 2 396 | — | — | 2 396 | high coupling — touches all |
| D8 — asset-pipeline | 1 161 | — | — | 1 161 | low — dead code |
| **total** | **~53 367** | **8 727** | **1 638** | **~63 732** | |

`(small accounting gap vs the 66k §1 figure is from {Cargo.toml,
Trunk.toml, justfile, scripts/, .cargo/config} that don't slot into any
domain.)`

The D4↔D5 shared seam is `render/gpu_types.rs` + `render/prepare.rs` +
`render/pipelines.rs::NaadfPipelines`. D4 should refactor on those; D5
should respect them as read-only. D7 is best done **last**, after the
other domains stabilise their `Plugin`s.

---

## 3 — Crosscutting reuse audit

### 3.1 SSoT-divergent constants

| ID | constant(s) | locations | severity | domain |
|---|---|---|---|---|
| SSoT-1 | **`max_ray_steps_*` family** (5 fields) — exists as: (a) `GiSettings` fields in `lib.rs:144-184`, (b) `GpuRenderParams.max_ray_steps_primary` in `render/gpu_types.rs:87`, (c) `GpuGiParams.{max_ray_steps_secondary,sun,sun_secondary,visibility}` (`gpu_types.rs`), (d) `KNOBS` table rows in `settings.rs:169-211` (each row repeats the default value as a literal `120`, `100`, `120`, `80`, `60`), (e) WGSL shader `MAX_RAY_STEPS_*` `const`s at `ray_tracing.wgsl:122-126` (kept deliberately per `feature-completeness/01-context.md` ¶ "Do NOT delete `MAX_RAY_STEPS_*` consts"). **5 sources of truth for the same 5 numbers.** Each `120`/`100`/`80`/`60` is hardcoded in three places. | `lib.rs:223-228`, `gpu_types.rs:87`, `settings.rs:174,184,194,202,210`, `ray_tracing.wgsl:122-126` | **3 (foundation)** | crosscutting D4 + D7 + D2 |
| SSoT-2 | **`WORLD_SIZE_IN_CHUNKS = UVec3::new(256, 32, 256)` / `WORLD_SIZE_IN_VOXELS = UVec3::new(4096, 512, 4096)` / `WORLD_SIZE_IN_SEGMENTS = UVec3::new(16, 2, 16)`** — `lib.rs:241,247,257,260`. Derived from one another but written out three times. Test at `lib.rs:tests::fixed_world_size_constants_agree` enforces the relationship. | `lib.rs:241-260` | 1 (nit) | D7 |
| SSoT-3 | **`CELL_DIM = 4` / `CELL_CHILDREN = 64`** in `voxel/mod.rs:63-65` — but every WGSL shader hardcodes `4u` and `64u` literally (e.g. `ray_tracing.wgsl`, `chunk_calc.wgsl`, `bounds_calc.wgsl`, `world_change.wgsl`). No `#define` / shader-def injection of these as paper-canonical constants. Changing `CELL_DIM` would require editing ~25 WGSL files. | `voxel/mod.rs:63-65` + ~25 `.wgsl` files | 2 (worth-fixing — but: the paper hardcodes 4 forever, so the value never changes — this is more "documentation discipline" than risk) | crosscutting D1 + D4 + D5 |
| SSoT-4 | **`VALID_SAMPLE_STORAGE_COUNT = 2` / `INVALID_SAMPLE_STORAGE_COUNT = 8` / `BUCKET_STORAGE_COUNT = 32` / `REFINED_BUCKET_STORAGE_COUNT = 8`** — `render/gi.rs:51-60`. The WGSL shaders that consume these (`naadf_global_illum.wgsl`, `sample_refine.wgsl`, `spatial_resampling.wgsl`) use raw literals (`8u`, `32u`, ...). They are uploaded as fields of `GpuGiParams` so this is *partially* solved — but the WGSL still has bare literals at e.g. `sample_refine.wgsl:655 — (cur_bucket_x >> 18u) * 8u` (the `* 8u` IS this constant). Audit each WGSL literal. | `gi.rs:47-60` + several WGSL files | 2 | D4 |
| SSoT-5 | **`DEFAULT_TAA_RING_DEPTH = 32`** — `lib.rs:274`. Comments at `render/taa.rs` and `render/pipelines.rs` repeat the value in prose ("the 32-deep ring"). Mostly OK — the `#{TAA_SAMPLE_RING_DEPTH}` shader-def + the `AppArgs.taa_ring_depth` + the `TaaRingConfig` resource are SSoT. Audit complete. | `lib.rs:274` | 1 | D4 |
| SSoT-6 | **C# hash-coefficient table** — `BlockHashingHandler.cs:50-55` (a 65-element array of `31^(64-i)` values) is implemented THREE times in the Rust port: (a) `aadf/block_hash.rs::build_polynomial_coefficients` (line 395), (b) `render/construction/hashing.rs::hash_coefficients` (line 241 — referenced from `construction/mod.rs:4950`), (c) hardcoded in WGSL `chunk_calc.wgsl` as `chunk_coefficients` array literal. Audit whether all three agree. | `aadf/block_hash.rs:395`, `render/construction/hashing.rs`, `chunk_calc.wgsl` | 2 | crosscutting D1 + D5 |

### 3.2 Duplicated utilities

| ID | duplication | locations | severity | domain |
|---|---|---|---|---|
| DUP-1 | **3 set-voxel entry points + 2 "build chunk edit window" oracles** on `WorldData`: `set_voxel` (diag), `set_voxels_batch` (prod), `set_voxels_batch_oracle` (diag), `set_chunks_uniform_batch` (prod brush-fast-path), `build_chunk_edit_window_solid_type` + `build_chunk_edit_window_from_world` (`aadf/edit.rs:356,373`). | `world/data.rs:235,721,1099,1181`, `aadf/edit.rs:356,373` | 2 | D1 |
| DUP-2 | **3 brush-shape AABB / classify functions** — `brush_aabb`, `brush_chunk_aabb`, `sphere_chunk_classify`, `cube_chunk_classify` (`editor/tools.rs:47,67,88,108`). Three brushes (`paint_brush`, `cube_brush`, `sphere_brush`) duplicate iteration structure ~60-80 LOC each. | `editor/tools.rs:47-280` | 2 | D2 |
| DUP-3 | **5 `naadf_sample_refine_*_node` systems** in `render/graph_b.rs` — five separate node entries (`_clear`, `_valid_history`, `_count_valid`, `_count_invalid`, `_buckets`) that each have the same prologue (look up pipeline, look up bind group, dispatch). The C# does this with 5 calls to `dispatch(...)` in one function (`WorldRenderBase.cs:227-275`). | `render/graph_b.rs:1-574`, `render/mod.rs:300-326` | 2 | D4 |
| DUP-4 | **3 `validate_gpu_construction*` variants** in `construction/mod.rs:4928,5290,5621` — each boots `MinimalPlugins + RenderPlugin`, runs Algorithm 1, asserts. ~360 + 190 + 700 LOC. They differ only in fixture scale + comparison mode. | `construction/mod.rs:4928,5290,5621` | **3** | D5 |
| DUP-5 | **4 `run_one_*_byte_diff` fixtures** — `run_one_fixture_byte_diff`, `run_one_fixture_multiseg_byte_diff`, `run_one_generator_model_byte_diff`, `run_one_tiled_byte_diff` (`construction/mod.rs:6623,7134,7606,7832`). Each is 500-700 LOC of `boot RenderApp + dispatch + readback + assert`. | `construction/mod.rs:6623-8331` | **3** | D5 |
| DUP-6 | **Press-P diagnostics (`diagnostics.rs:40`) + every e2e gate's pin_camera function** — each writes camera `Transform` + `PositionSplit::from_world` + `camera-history`. The seven `pin_*_camera` systems in `e2e/` repeat ~30 LOC of "compute pose, write `Transform`, write `PositionSplit`" each. | `diagnostics.rs:40-145`, `e2e/oasis_edit_visual.rs:306`, `e2e/vox_gpu_oracle.rs`, `e2e/vox_gpu_construction.rs`, ... | 2 | D6 |
| DUP-7 | **`build_segment_voxel_buffer`** (`construction/mod.rs:4820`) + **`build_segment_voxel_buffer_from_dense`** (`mod.rs:4727`) — two parallel functions doing the same encoding via different input shapes. | `construction/mod.rs:4727,4820` | 1 | D5 |

### 3.3 Patterns that fight Bevy idioms

| ID | smell | locations | severity | domain |
|---|---|---|---|---|
| BEV-1 | **`add_systems(Core3d, (17-element-tuple).chain())`** in `render/mod.rs:300-326` — Bevy provides `RenderLabel` + `add_render_graph_edges` for exactly this case. The `.chain()` is functional but every new node forces editing the single tuple in `render/mod.rs`; the W2/W4 seam idea ("each workstream merges its node in its own PR") is undermined by this central registry. | `render/mod.rs:300-326` | 2 | D4 |
| BEV-2 | **Manual padding fields (`_pad0`, `_pad0b`, `_pad1`) in every `#[repr(C)]` GPU struct** — `gpu_types.rs:43-110+`. Bevy 0.19 ships `bevy::render::render_resource::ShaderType` (an `encase` re-export) that auto-handles std140/std430 padding. The project deliberately chose `bytemuck::Pod + #[repr(C)]` instead. Audit whether the choice is load-bearing or whether `ShaderType` would cut ~300 LOC of padding. | `render/gpu_types.rs:36-...` (~30 structs) | 1-2 (might be deliberate) | D4 |
| BEV-3 | **`Resource` with `Option<Buffer>`, `Option<BindGroup>` fields** — `construction/mod.rs:106-198` (`ConstructionGpu` with 16+ `Option<Buffer>` fields). This is the W0 seam contract (per the docblock at lines 81-104), but the *pattern* is a workaround for not having sub-resources / `EntityCommands.with_inserter`. Each workstream filling its `Some(...)` is opaque — could be modelled as separate `Resource`s (`ChunkCalcGpu`, `BoundsCalcGpu`, etc.) and queried independently. | `construction/mod.rs:106-414` | 2 | D5 |
| BEV-4 | **Function-pointer-based knobs table** — `settings.rs:KNOBS` is `&[Knob { ... kind: KnobKind::U32 { getter: fn(&GiSettings) -> u32, setter: fn(&mut GiSettings, u32), ... }, ... }]`. Bevy ships `bevy_reflect` for exactly this surface; declaring `GiSettings: Reflect` + a `#[knob(nudge=8, big_step=32, min=1, max=512)]` derive-attribute would let the panel iterate fields generically. | `settings.rs:120-378` | 2 | D2 |
| BEV-5 | **No `Added<T>` / `Changed<T>` filters** — `voxel/web_vox.rs::apply_pending_vox` (line 498) polls `PendingVoxParse` every `Update` and short-circuits when `pending.inner.is_none()`. Could be `Added<PendingVoxParse>` or `Changed<PendingVoxParse>` and elide the poll. Multiple `pin_*_camera` systems do the same in e2e/. | `voxel/web_vox.rs:498`, `voxel/async_vox.rs`, multiple e2e systems | 1 | D3 / D6 |
| BEV-6 | **`Option<Res<X>>` / `let Some(x) = x else { return; }` ladder** — `construction/mod.rs::naadf_gpu_producer_node:3076-3127` and `prepare_construction:1658-1700` start with 6-9 sequential `let Some(...) = ... else { return; }` early-bails. Bevy's idiomatic alternative is `.run_if(resource_exists::<X>)` on system registration — but those run-if conditions don't exist here. | `construction/mod.rs:1660-1700,3076-3127`, multiple sites | 1-2 | D5 |

### 3.4 Over-abstractions

| ID | smell | locations | severity | domain |
|---|---|---|---|---|
| OA-1 | **`KnobKind` enum with `getter: fn`/`setter: fn`/`apply: fn` function-pointer payload** (`settings.rs:120-150`) — a built-in reflection mechanism reimplemented from first principles. See BEV-4. | `settings.rs:120` | 2 | D2 |
| OA-2 | **`ConstructionPipelines` as an "empty sibling" of `NaadfPipelines`** (`construction/mod.rs:482`) — the W0 contract bans editing the parent. Two resource types, two `from_world`s, two lookup paths, all to enforce a code-organisation rule. A `pub(crate)` boundary + a `Plugin` would do the same thing without a parallel resource. | `construction/mod.rs:482-754` | 1 | D5 |

### 3.5 Under-abstractions

| ID | smell | locations | severity | domain |
|---|---|---|---|---|
| UA-1 | **`(IVec3, VoxelTypeId)` tuples for "voxel edits"** — `WorldData::set_voxels_batch(edits: &[(IVec3, VoxelTypeId)])` (`world/data.rs:721`) and 4-5 call sites use anonymous tuples. `pub struct VoxelEdit { pos: IVec3, ty: VoxelTypeId }` would be self-documenting. | `world/data.rs:721,1181`, callers in `editor/tools.rs` | 1 | D1 + D2 |
| UA-2 | **WGSL `2u` / `8u` / `32u` literals** that ARE the SSoT-1 / SSoT-4 storage counts — `sample_refine.wgsl:655 — invalid_count = (cur_bucket_x >> 18u) * 8u`. Each literal `8u` here is `INVALID_SAMPLE_STORAGE_COUNT`. No `const NUM_INVALID_SAMPLES: u32 = 8u;` at the top of the shader. | multiple WGSL files | 2 | D4 |
| UA-3 | **"chunk pos packed" raw `u32` with `0x7FF / 0x3FF` masks** — repeated across `aadf/edit.rs:67-69`, `render/construction/world_change.rs`, `chunk_calc.wgsl`. `pack_chunk_pos` / `unpack_chunk_pos` (`aadf/edit.rs:203,208`) is the official helper but several sites bypass it. | `aadf/edit.rs:67-69 (load-bearing)`, then ~10 call sites doing the masking inline. | 2 | D1 + D5 |
| UA-4 | **Six 6-direction `DIR_NEG_X..DIR_POS_Z` indices** (`aadf/cell.rs:28-33`) — direct indices into `Aadf6.d[6]`. An `enum Dir6 { NegX, PosX, ... }` or `[Dir6; 6]` table would make iteration sites type-safe. Low severity because it's used in tight inner loops where the indirection cost matters. | `aadf/cell.rs:28-33`, callers in `aadf/bounds.rs`, `aadf/construct.rs` | 1 | D1 |

---

## Side notes / observations / complaints

1. **The user's framing is correct but partial.** The Rust port really is ~4×
   the C# in LOC. But the bloat is *not uniform*. Half of it is concentrated
   in three places: (a) the 11 043-LOC `render/construction/mod.rs`, (b) the
   10 292-LOC `e2e/` directory that has no C# counterpart, (c) heavy
   doc-comment headers + WGSL+Rust shader-mirror duplication. The first is a
   refactor target (probably the single biggest LOC reduction available).
   The second is **deliberate verification discipline** (CLAUDE.md rules
   forbid the user-style binary smoke run for verification). The third is
   irreducible by the project's faithful-port + verbose-docs ethos. **If
   the goal is "get the port back near C# parity in LOC", focus on (a)
   and prune the e2e harness's dead/unfinished gates.** If the goal is
   "improve IoC and idiom-fit", (a)+(c) get you further than chasing LOC.

2. **`crates/voxel_noise/` is dead code.** Its workspace docstring in the
   root `Cargo.toml` says "NOT yet wired into the renderer". Zero callers
   in `bevy_naadf`. 1 033 LOC + a separate Emscripten Makefile + a
   `voxel_noise/dist/`. This is the easiest win: delete or feature-gate.
   The Rust `/deadcode` skill is the natural fit.

3. **Diagnostics-residual rot.** `AadfDelayedProbe` / `AadfPerCallProbe` /
   `AadfCpuGpuParity` (~1 100 LOC in `construction/mod.rs:3559-4720`),
   `wasm-chunk-aadf-nondeterminism` doc tree, the entire
   `device_snapshot` submodule (560 LOC in `diagnostics.rs`), and the
   `pbr_*` e2e gates (`pbr_debug_modes.rs:218`, `pbr_hard_edge.rs:1023`,
   `pbr_visual.rs:747` — that's 2 000 LOC) all look like "investigation
   residuals kept around in case the bug recurs". Worth a focused
   "delete-or-keep" pass across all of them, gated on `git log` showing
   when they were last load-bearing.

4. **The 4-phase orchestration history shows in the structure.** Phase A
   (albedo first-hit) → Phase A-2 (TAA) → Phase B (GI) → Phase C (GPU
   construction). The Bevy code reflects this in: `render/graph.rs`
   (A passes) vs `render/graph_b.rs` (B passes) — two parallel files
   that should arguably be one; `aadf/construct.rs` (Phase-A CPU
   construction) vs `render/construction/` (Phase-C GPU construction)
   doing the same algorithmic job; `WorldRenderAlbedo.cs` vs
   `WorldRenderBase.cs` already collapsed in NAADF behind a switch in
   `WorldRender.ApplyRenderVersion`, but the port has both paths visible
   in code. **The orchestration left scaffolding behind that the
   "completed" port should retire.**

5. **The proposed D4↔D5 split is tight but not airtight.** Both touch
   `render/gpu_types.rs` (every uniform), `render/prepare.rs::WorldGpu`
   (the world bind group), `render/pipelines.rs::NaadfPipelines`
   (rendering side) vs `construction::ConstructionPipelines`
   (construction side — already split). The W0 seam contract documents
   D4's shared surface as "do NOT touch this from a construction
   workstream"; D5's refactor should respect this and treat `gpu_types`
   as read-only. **If both D4 and D5 land changes to `gpu_types.rs`
   in parallel, the merge will conflict.** Sequence D4's
   `gpu_types`-touching work before D5 or after.

6. **`docs/orchestrate/` rot you'd want to clean alongside.** There are
   13 distinct orchestrate sub-trees (`naadf-bevy-port/`,
   `feature-completeness/`, `oasis-vox-instance-count/`,
   `pbr-raymarching/`, `phase-d-completion/`,
   `refactor-wasm-aadf-postfix-cleanup/`, `streaming-world/`,
   `vox-gpu-rewrite/`, `wasm-chunk-aadf-nondeterminism/`,
   `web-chunks-storage-buffer/`, `web-vox-async-loading/`,
   `web-vox-color-divergence/`, this one). The completed ones probably
   want consolidation into a single `docs/architecture.md` instead of
   the chronological orchestration ledger that's currently there.
   This is doc-side, but it's structurally adjacent: the Phase-A/B/C
   scaffolding that's still in code (#4 above) is mirrored by the doc
   sprawl.

7. **One verdict you didn't ask for but I'll give**: the **single
   highest-leverage refactor in the port** is splitting
   `render/construction/mod.rs` (11 043 → ~2 500 LOC core + ~2 500 LOC
   `validation/` sub-module + ~1 500 LOC `probes/` sub-module + ~1 500
   LOC `prepare.rs` extraction + ~1 500 LOC `producer.rs` extraction).
   No domain boundary crossed, no behavioural change required, dramatic
   readability + LOC-density win. If only one of the 8 domain
   refactors lands, make it **D5**.

8. **One smell you should know but it's not in scope of this audit**:
   `bin/e2e_render.rs:481 LOC` is a tall CLI dispatch — `--baseline`,
   `--validate-gpu-construction`, `--validate-gpu-construction-scaled`,
   `--validate-gpu-construction-production-scale`, `--edit-mode`,
   `--entities`, `--vox`, `--vox-e2e`, `--oasis-edit-visual`,
   `--runtime-edit-mode`, `--small-edit-visual`, `--small-edit-repro`,
   `--vox-gpu-construction`, `--vox-gpu-oracle`, `--vox-web-parity`,
   `--vox-horizon-parity`, `--pbr-debug-modes`, `--pbr-hard-edge`,
   `--pbr-visual`. **18 modes.** Each is a separate gate. Worth
   surveying for dead modes.

9. **Equal-footing complaint**: this audit could not run `tokei` or
   `cloc` (the project's `rtk` shim wraps `find` and may not have those
   tools on PATH). I fell back to `wc -l` which counts every line
   including blanks + comments. The "Rust is 4× the C#" figure should
   probably be re-stated as "Rust source lines (including comments and
   blanks) is 4× the C# source lines" — if a future pass runs `tokei
   --no-blanks` it might land closer to ~3× rather than 4×. The
   *order* of magnitude is right, the multiplier is approximate.
