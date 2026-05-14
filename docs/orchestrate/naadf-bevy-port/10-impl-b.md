# 10 — Phase B Implementation Log

Implements the batched sequence from `09-design-b.md` §11. The orchestrator
dispatches + reviews batch-by-batch. This log grows one batch section at a time.

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch
`feat/phase-b-gi`. Test command: `cargo test --bin bevy-naadf`.

---

## Batch 1 — Shared WGSL + GPU types + the atmosphere subsystem

`09-design-b.md` §11 Batch 1 (steps 1–5). Self-contained, no pipeline
restructure; ends ▶ runnable — the atmosphere precomputes every frame, the rest
of the pipeline is the unchanged A-2 path.

### Files created

| file | what it is | NAADF provenance |
|---|---|---|
| `src/assets/shaders/color_compression.wgsl` | the 5-bit/channel exponential GI-sample colour compression — `COLORS[32]` / `COLOR_DIF_PROB[31]` hard-coded literals, `compress_color`, `refine_comp_color`, `MAX_COLOR_LEVELING`. naga-oil import module; not yet imported by any entry shader. | `render/common/commonColorCompression.fxh` |
| `src/assets/shaders/atmosphere.wgsl` | the multiple-scattering sky model — `add_light_for_direction` (the nested ray-march), `rayleigh` / `phase_function` / `density_at_height` / `ray_sphere` / `scatter_for_densities` / `get_scatter_densities_at_point`, `atmosphere_oct_index` + `apply_atmosphere`, the `AtmosphereParams` struct + the `AtmoLight` in/out value. naga-oil import module. | `render/common/atmosphere/atmosphereRaw.fxh` + `atmospherePrecomputed.fxh` |
| `src/assets/shaders/naadf_atmosphere.wgsl` | the atmosphere precompute compute entry `precompute_atmosphere` — octahedral decode + the `rayDir.y` warp, one quarter of `atmosphere_comp` per frame, packs `(light, absorption)` into the `vec4<u32>` slot. | `render/versions/base/renderAtmosphere.fx` `precomputeAtmosphere` |
| `src/render/atmosphere.rs` | the atmosphere subsystem: the `ATMOSPHERE` sky-constants block (the `UiSkyDebug.cs` defaults), `build_atmosphere_params`, the CPU `Atmosphere::get_light_for_point` (port of `Atmosphere.cs` — needed by Batch 3's `prepare_gi`), the `AtmosphereGpu` render-world resource, `prepare_atmosphere`, the `ATMOSPHERE_TEX_SIZE` / `ATMOSPHERE_WORKGROUP_SIZE` consts. | `World/Render/Atmosphere.cs`, `Gui/Main/UiDebug/UiSkyDebug.cs`, `WorldRenderBase.cs:131-132,205-206` |
| `src/render/graph_b.rs` | the Phase-B render-graph node systems. Batch 1 lands only `naadf_atmosphere_node` (+ `ATMOSPHERE_SPAN`); the other ~10 Phase-B nodes arrive in Batches 2–6. | `WorldRenderBase.cs:205-206` (`renderSky` dispatch) |
| `src/render/color_compression.rs` | CPU recomputation of the `COLORS` / `COLOR_DIF_PROB` tables from the source formula + the `#[test] color_tables_match_wgsl` guard that asserts the hard-coded WGSL literals match (`09-design-b.md` §12 #4), + a `color_table_anchor_points` spot-check. Pure CPU bookkeeping — no GPU resource (option (a) in §5.3). | `commonColorCompression.fxh:7-79` |

### Files changed

| file | change |
|---|---|
| `src/assets/shaders/ray_tracing_common.wgsl` | += the Phase-B VNDF-GGX block (`get_perpendicular_vector`, `get_uniform_hemisphere_sample`, `sample_vndf_isotropic`, `pdf_vndf_isotropic`, `geometry_term` — `commonRayTracing.fxh:65-137`). Now imports `PI` from `common.wgsl`. |
| `src/assets/shaders/common.wgsl` | += the `commonOther.fxh` pure-math helpers `gaussian_f`, `gcd`, `find_coprime`, `next_pow2` (`commonOther.fxh:42-79`). The `addToCounter*` group-shared helpers are NOT ported here (they need `var<workgroup>` at entry-point scope — ported inline per-shader in later batches). |
| `src/assets/shaders/render_pipeline_common.wgsl` | += `SPECULAR_MIRROR_FAC` const, the `FirstHitResult` + `SampleValid` structs, `get_reflectance_fresnel`, `get_specular_normals`, `get_tang`, the full `get_hit_data_from_planes` (the 3-iteration specular-reflection loop + tail, entity branch omitted), `get_screen_pos_projection` / `get_screen_index_projection` (promoted in from `taa.wgsl`) + their `ScreenPosProj` / `ScreenIndexProj` structs. `compress_first_hit_data` gains the `is_diffuse` arg (5-arg `base/` variant — `base/renderFirstHit.fx:18`). The `touch_pi` placeholder is removed. |
| `src/assets/shaders/taa.wgsl` | the A-2-local `get_hit_data_from_planes_a2` / `FirstHitResultA2` / `get_screen_pos_projection` / `get_screen_index_projection` / `ScreenPosProj` / `ScreenIndexProj` are **deleted** — `taa.wgsl` now imports the shared full versions from `render_pipeline_common.wgsl`. The one call site `get_hit_data_from_planes_a2(...)` → `get_hit_data_from_planes(...)`. The `GpuCameraHistorySlot` WGSL struct gains `view_proj_inv: mat4x4<f32>` (96→160 bytes). |
| `src/assets/shaders/naadf_first_hit.wgsl` | the `compress_first_hit_data` call site passes `1u` for the new `is_diffuse` arg (the A-2 single-plane path treats every hit as diffuse; Batch 2's 4-plane first-hit makes it real). |
| `src/render/gpu_types.rs` | += `GpuAtmosphereParams` (128 B), `GpuSampleValid` (`[u32;8]`, 32 B), `GpuBucketInfo` (`[u32;2]`, 8 B), `GpuGiParams` (288 B) + the `GI_FLAG_*` constants. `GpuCameraHistorySlot` gains `view_proj_inv: Mat4` (96→160 B). `GpuRenderParams._pad0` → `tone_mapping_fac: f32` (offset 28; size unchanged at 112 B). New + updated `const _: assert!` size checks. |
| `src/render/pipelines.rs` | `NaadfPipelines` += `atmosphere_layout` (`@group(0)`: `atmosphere_params` uniform + `atmosphere_comp` rw storage) + `atmosphere_pipeline` (cached compute id for `precompute_atmosphere`). New `ATMOSPHERE_SHADER` path const. |
| `src/render/taa.rs` | `CameraHistory` gains `view_proj_inv: [Mat4; 128]`; `update_camera_history` populates it as `view_proj.inverse()` (one extra `.inverse()` per frame — C# `taaSampleCamTransformInvers`). `prepare_taa`'s `GpuCameraHistorySlot` builder gains the `view_proj_inv` field (from `extracted_history.view_proj_inv[i]`). |
| `src/render/extract.rs` | `ExtractedCameraHistory` gains `view_proj_inv: [Mat4; 128]`; `extract_camera_history` copies it from `CameraHistory`. |
| `src/render/prepare.rs` | `GpuRenderParams` literal: `_pad0: 0` → `tone_mapping_fac: 1.0` (the C# `Settings.data.general.toneMappingFac`; consumed by Batch 6's `base/` final blit). |
| `src/render/mod.rs` | `pub mod atmosphere; pub mod color_compression; pub mod graph_b;` declared. `prepare_atmosphere` added to `PrepareResources` (alongside `prepare_world_gpu` / `prepare_taa` — its bind group is self-contained, no `PrepareBindGroups` split needed). `naadf_atmosphere_node` prepended as the *first* node in the `Core3d` `.chain()` (NAADF's dispatch order). |

### Mapping to NAADF source

- The VNDF-GGX block, `commonOther.fxh` helpers, and the specular helpers in
  `render_pipeline_common.wgsl` are straight ports — the `09-design-b.md` §2.2 /
  §5.1 / §5.2 split. Every HLSL `mul(v, M)` is the column-vector `M * v` (the
  `05-review.md` perspective-fix convention); every `#ifdef ENTITIES` block is
  omitted (Phase B is entity-free — §1).
- `get_hit_data_from_planes` is the full `commonRenderPipeline.fxh:154-213` with
  the `:183-203` `ENTITIES` block dropped and the `entityInstancesHistory` /
  `taaIndex` params removed. When planes 1–3 are `HIT_UNDEFINED` (the A-2
  single-plane G-buffer) the loop runs zero iterations and the function reduces
  *exactly* to A-2's deleted `get_hit_data_from_planes_a2` — verified by
  inspection, not a behaviour change for the albedo path.
- `atmosphere.wgsl` is `atmosphereRaw.fxh` + `atmospherePrecomputed.fxh`. The
  HLSL `const bool/int` template-style params on `addLightForDirection` are
  plain runtime args (`include_mie`, `main_iteration_count`,
  `second_iteration_count`); `includeSun` is dropped (both HLSL call sites
  default it to false). The HLSL `inout float3 absorption, light` becomes the
  `AtmoLight` in/out value.
- `atmosphere.rs`'s `ATMOSPHERE` constants are the `UiSkyDebug.cs` field
  initialisers in their *raw* form; the `UiSkyDebug.SetShaderData` scaling
  (`* skySunIntensity`, `* 0.01`, `* 0.000001` — `UiSkyDebug.cs:63-79`) is
  applied at the use sites so the GPU uniform (`build_atmosphere_params`) and
  the CPU `Atmosphere::get_light_for_point` share one source of truth.
  `Atmosphere::get_light_for_point` is a faithful `Atmosphere.cs` port (note:
  the C# `getScatterDensitiesAtPoint` uses a fixed `scatterSteps = 20`, *not*
  `skySubScatterSteps` — ported as-is).
- `naadf_atmosphere.wgsl` mirrors `base/renderAtmosphere.fx`'s `ID = globalID.x
  * 4 + (frameCount % 4)` quarter-per-frame stride; `naadf_atmosphere_node`
  dispatches `ceil((1024² / 4) / 64) = 4096` workgroups (`WorldRenderBase.cs:206`).

### Design-§12 open items handled

- **#3 (`vec3<u32>` / `vec3` alignment).** `atmosphere_comp` is stored as
  `array<vec4<u32>>` (`.w` padding), 64 MiB, fixed-size, zero-cleared on
  creation — exactly §3.3. The atmosphere precompute writes `.w = 0u`.
  Additionally — see the bug below — the `AtmosphereParams` *uniform* hit the
  same `vec3`-followed-by-scalar trap and now carries **explicit** WGSL pad
  fields (the other shared WGSL structs get away without them because their
  `vec3`s are all followed by `vec3`s or are the last field).
- **#4 (`COLORS` / `COLOR_DIF_PROB` as hard-coded literals).** Done — the 63
  literals are pasted into `color_compression.wgsl` with the `09-design-b.md`
  §5.3 formula cited; `src/render/color_compression.rs`'s
  `color_tables_match_wgsl` `#[test]` recomputes from the formula and asserts a
  bit-exact match. The Rust module pastes the same literals (kept honest by the
  test) — if the WGSL is regenerated, both must be updated together.
- **#5 (storage-buffer atomics).** Not yet reached — `sample_counts` /
  `bucket_info` are declared in `gpu_types.rs` as plain `[u32;N]` *byte
  layouts*; their WGSL `atomic<u32>` binding types are a Batch 3/4 concern (the
  byte layout is the same either way). Flagged for B3/B4.

### Bug found + fixed during the Batch-1 smoke-run

The smoke-run surfaced three blocking issues; all three are fixed in this batch
(they block the "▶ compiles & runs" bar):

1. **naga-oil rejects trailing-digit struct field names.** `SampleValid`'s
   `data1` / `data2` (the design's names) triggered "Composable module
   identifiers must not require substitution according to naga writeback rules".
   Renamed to `data_a` / `data_b` (same content; the GPU shaders pack/unpack the
   bitfields directly — the field names are not load-bearing). The
   `AtmosphereParams` pad fields are likewise `pad_cam` / `pad_sun_dir` / … (no
   trailing digits) for the same reason.
2. **WGSL forbids `ptr<storage,…>` function parameters.** `apply_atmosphere`
   was designed to take the `atmosphere_comp` buffer by `ptr` — naga rejects it
   ("a pointer of space Storage … can't be passed into functions"). Split into
   `atmosphere_oct_index(ray_dir, w, h) -> u32` (computes the buffer index) +
   `apply_atmosphere(atmo_comp: vec4<u32>, …)` (takes the already-fetched slot
   value). The caller — which owns the storage binding — does
   `atmosphere_comp[atmosphere_oct_index(...)]` itself. **Note for B2:** the
   4-plane first-hit + the GI passes that call `apply_atmosphere` must fetch the
   slot themselves; the function no longer reaches the buffer.
3. **`GpuAtmosphereParams` uniform layout mismatch → GPU TDR.** The first fixed
   smoke-run hung the GPU (DeviceLost / swapchain Timeout). Root cause: the WGSL
   `AtmosphereParams` initially had **no explicit pad members** (copying the
   `render_pipeline_common.wgsl` convention) — but that convention only holds
   when every `vec3` is followed by another `vec3` or is the last field. A WGSL
   `vec3<f32>` has size 12 / align 16, so a *trailing scalar* (`sky_mie_scatter`
   after `sky_sun_color`) packs into the vec3's 4th slot, whereas the Rust
   `#[repr(C)]` struct has an explicit `_pad4` u32 there. Everything from offset
   76 on shifted by 4 — `sky_main_ray_steps` read a neighbouring float's bit
   pattern as a ~billion-iteration loop bound → the ray-march never terminated →
   TDR. Fixed by giving the WGSL struct explicit `pad_*` fields so it is
   byte-identical to the Rust struct. This is the `09-design-b.md` §12 #3 `vec3`
   gotcha in its *uniform-struct* form — flagged below for the later batches.

### Verification

- `cargo build --bin bevy-naadf` — clean (pre-existing dead-code warnings only;
  `atmosphere.rs`'s `Atmosphere` / `GI_FLAG_IS_ATMOSPHERE_INTERACTION` /
  `GpuGiParams` etc. warn "never used" — expected, Batch 3+ consumes them).
- `cargo test --bin bevy-naadf` — **41 passed** (was 39; +2 from
  `color_compression.rs`: `color_tables_match_wgsl`, `color_table_anchor_points`).
- One smoke-run after the final fix: launches clean, runs ~35 s with **no
  panic, no DeviceLost, no validation/naga/WGSL errors**. The atmosphere
  precompute pipeline compiles and dispatches every frame; the rest of the
  pipeline is the unchanged A-2 path (so the image is the A-2 image — visual
  confirmation is the user's review-gate job).

### Notes for the next batch (B2 — the 4-plane-bounce first-hit)

- **The `vec3`-followed-by-scalar uniform trap (§12 #3) bit `AtmosphereParams`.**
  `GpuGiParams` (already declared in `gpu_types.rs`) has the same shape —
  `vec3` rows (`cam_pos_int`/`cam_pos_frac`/`sky_sun_dir`/`sun_color`) then a
  long scalar tail. Its WGSL counterpart, when written in B2/B3, **must** carry
  explicit `pad_*` fields after each `vec3` (and use non-trailing-digit names —
  naga-oil rejects `_pad0`-style). Do not assume the "no explicit pad"
  convention; verify the WGSL struct is byte-identical to the Rust struct.
- `apply_atmosphere` does **not** read the `atmosphere_comp` buffer — the caller
  must fetch `atmosphere_comp[atmosphere_oct_index(ray_dir, tex_x, tex_y)]` and
  pass the `vec4<u32>` slot in. B2's 4-plane first-hit `apply_atmosphere`-on-miss
  + `add_light_for_direction`-along-segment paths need the `atmosphere_comp`
  storage binding in the first-hit pipeline's `@group(3)`.
- `compress_first_hit_data` is now the 5-arg `base/` variant
  (`dist, norm_tangs, voxel_type_raw, is_diffuse, entity`). The A-2 first-hit
  passes `1u` for `is_diffuse`; B2's 4-plane first-hit computes the real value.
- `get_hit_data_from_planes` (the full version) is in `render_pipeline_common.wgsl`
  — B2's first-hit + the GI passes import it directly. `get_specular_normals` /
  `get_tang` / `get_reflectance_fresnel` / `SPECULAR_MIRROR_FAC` are there too.
- The `view_proj_inv` camera-history ring is **fully plumbed** in Batch 1
  (`CameraHistory` → `update_camera_history` → `ExtractedCameraHistory` →
  `extract_camera_history` → `prepare_taa` upload → the 160-byte
  `GpuCameraHistorySlot`). `09-design-b.md` Batch 3 step 9 also lists this work —
  it is already done; B3 only needs to *consume* `view_proj_inv` in
  `renderSampleRefine`, not plumb it.
- `AppArgs` is unchanged in Batch 1 — `GiSettings` is a Batch 3 (step 9)
  addition, not Batch 1.
- `GpuAtmosphereParams` / `GpuGiParams` size asserts: 128 / 288 bytes. The WGSL
  `AtmosphereParams` is byte-identical (verified by the clean run); when B3
  writes the WGSL `GpuGiParams` counterpart, add a matching reasoning comment.
