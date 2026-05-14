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

---

## Batch 2 — The 4-plane-bounce first-hit

`09-design-b.md` §11 Batch 2 (steps 6–8). The one risky restructure: the
first-hit pass is rewritten in place from the A-2 single-plane path to the
`base/renderFirstHit.fx` 4-plane-bounce path, the frame layout widens by 2
bindings, a new `@group(2)` read-only atmosphere replaces the A-2 `taa_samples`
ring group, and the final blit is temporarily pointed at `final_color`. Ends ▶
runnable — the image is the 4-plane first-hit with the full multiple-scattering
atmosphere.

### Files changed

| file | change |
|---|---|
| `src/assets/shaders/naadf_first_hit.wgsl` | **REPLACED** — port of `base/renderFirstHit.fx` `calcFirstHit`. The 4-iteration specular-bounce `loop`, the 5-arg `compress_first_hit_data` (real `is_diffuse`), the `first_hit_absorption` + `final_color` writes, `apply_atmosphere` on a ray/volume miss + `add_light_for_direction` along the atmosphere-interaction path. `taa_sample_accum` write **removed** (kept bound at `@group(1) @binding(3)`, touched once so naga retains the binding); `@group(2)` `taa_samples` ring write **removed**; new `@group(2)` = read-only `atmosphere_params` + `atmosphere_comp`. The inline Phase-A sun+ambient term is gone. |
| `src/assets/shaders/atmosphere.wgsl` | bug fix (latent Batch-1 shared-WGSL bug — see below): `atmosphere_oct_index`'s `vec2<u32>(oct.x * f32(...), ...)` constructor → explicit `u32(...)` casts. naga rejects building a `vec2<u32>` from `f32` components; HLSL's `uint2(...)` truncates implicitly. Surfaced only now because Batch 2's first-hit is the first entry shader to actually *call* `atmosphere_oct_index` (naga-oil only validates reachable functions; Batch-1's `naadf_atmosphere.wgsl` has the octahedral decode inline). |
| `src/assets/shaders/naadf_final.wgsl` | the blit source binding renamed `taa_sample_accum` → `blit_source`; the decode branches on `FLAG_BLIT_FINAL_COLOR` — `final_color` packing (raw RGB, no weight: `(.x&0xFFFF, .x>>16, .y&0xFFFF)`) vs. `taa_sample_accum` packing (`weight = .x&0xFFFF`, `rgb / max(1,weight)`). The `taa_sample_accum` branch is dormant in Batch 2; Batch 6 reverts the bind-group source and clears the flag. |
| `src/assets/shaders/render_pipeline_common.wgsl` | += `FLAG_IS_ATMOSPHERE_INTERACTION` (`8u`) + `FLAG_BLIT_FINAL_COLOR` (`16u`) flag consts. |
| `src/render/gpu_types.rs` | += `FLAG_IS_ATMOSPHERE_INTERACTION` (`1<<3`) + `FLAG_BLIT_FINAL_COLOR` (`1<<4`). No struct-size change (the new outputs are storage buffers, not uniform fields). |
| `src/render/pipelines.rs` | `frame_layout` widened by 2 storage bindings (`first_hit_absorption`, `final_color` — slots 4/5). New `atmosphere_read_layout` (`@group(2)` for the first-hit: `atmosphere_params` uniform + `atmosphere_comp` **read-only** storage — distinct from the precompute pass's `atmosphere_layout`, which has it read-write). First-hit pipeline layout `[world, frame, taa]` → `[world, frame, atmosphere_read]`. |
| `src/render/prepare.rs` | `FrameGpu` += `first_hit_absorption` + `final_color` (`vec2<u32>`/pixel, 8 B, created/resized/zero-cleared with `first_hit_data`) + `first_hit_atmosphere_bind_group`. `prepare_frame_gpu` gains `Option<Res<AtmosphereGpu>>`, builds the 6-binding frame bind group + the `@group(2)` atmosphere bind group + the temporary `final_color` blit bind group. `flags` += `FLAG_IS_ATMOSPHERE_INTERACTION` (always — C# default `true`) + `FLAG_BLIT_FINAL_COLOR` (always, this batch). |
| `src/render/graph.rs` | `naadf_first_hit_node` drops the `taa_gpu` resource param + `set_bind_group(2, taa…)`; adds `set_bind_group(2, frame_gpu.first_hit_atmosphere_bind_group)`. `naadf_taa_reproject_node` gets `#[allow(dead_code)]` + a note — kept defined, out of the chain this batch. |
| `src/render/mod.rs` | the `Core3d` chain drops `naadf_taa_reproject_node`: `atmosphere → first_hit → final_blit`. `naadf_taa_reproject_node` no longer imported (Batch 6 re-adds it). |

### Mapping to NAADF source

- `naadf_first_hit.wgsl` is `base/renderFirstHit.fx:28-129`. The `[unroll]`'d
  `for (i = 0; i < 4; ++i)` becomes a WGSL `loop` with `i` declared outside so
  the post-loop `i == 4` test (`:117-121`) reads it; the miss/non-mirror-hit
  `break`s leave `i < 4`, the mirror path increments — so `i == 4u` iff all 4
  planes were mirrors, matching the HLSL. `oldPos` (`:58,114`) is tracked as a
  `vec3<f32>` = `vec3<f32>(cur_pos_int) + cur_pos_frac` (camera-int-relative,
  D1). `applyAtmosphere`'s HLSL body ignores its `pos` arg (only `rayDir`
  matters) — the port fetches `atmosphere_comp[atmosphere_oct_index(dir,…)]`
  itself (the Batch-1 carry-forward: `apply_atmosphere` can't take the storage
  buffer by `ptr`) and folds it in; `i==0` uses `rayDirNoJitter`, else `rayDir`
  (`:73`). `addLightForDirection(oldPos, rayDir, distance(curPos, oldPos), …,
  false, 3, 3)` (`:86`) is gated on `FLAG_IS_ATMOSPHERE_INTERACTION`. The
  non-mirror tail (`:93-108`) sets `is_diffuse = materialBase !=
  SURFACE_SPECULAR_ROUGH`; the mirror tail (`:110-114`) is `absorption *=
  getReflectanceFresnel(ior, cosTheta)` + `reflect`. `#ifdef ENTITIES` blocks
  omitted (Phase B is entity-free — §1). The three output writes are
  `base/renderFirstHit.fx:126-128` verbatim (the `showRayStep ? stepCount :
  voxelTypeRaw` conditional included).
- `isAtmosphereInteraction` is `WorldRenderBase.cs:16,224` (C# default `true`)
  → `FLAG_IS_ATMOSPHERE_INTERACTION`, set unconditionally in `prepare_frame_gpu`.
- The `@group` numbering follows `09-design-b.md` §6.3 ("layout `[world, frame,
  atmosphere]`") — the atmosphere takes the **freed `@group(2)` slot**, NOT
  `@group(3)`. §4.4 and the Batch-2 step-7 prose say "`@group(3)`", but that is
  the stale variant where the `taa_samples` group stays at `@group(2)`; §6.3
  explicitly *removes* that group, so the layout vec has exactly 3 entries and
  the atmosphere is `@group(2)`. (Likewise §5.1's table row still says the
  first-hit "keeps the `taa_samples` ring write from A-2" — that contradicts
  the detailed §6.3 + the Batch-2 step-7 instruction to *remove* it; followed
  §6.3 + step 7, which are the authoritative spec for this batch.)

### Item #2 finding — the `screenPosDistanceSqr` threshold

**The albedo source genuinely says `> 1.0f`; the `base/` source genuinely says
`> 16.0f`. A-2 has NO latent bug. No erratum to `08-review-a2.md` is warranted.**

- `albedo/renderTaaSampleReverse.fx:133-134`: `float screenPosDistanceSqr =
  dot(screenPosDif, screenPosDif); if (screenPosDistanceSqr > 1.0f) continue;`
- `base/renderTaaSampleReverse.fx:138-139`: identical code, but `if
  (screenPosDistanceSqr > 16.0f) continue;`

Phase A-2 ported the **albedo** TAA pipeline (`06-design-a2.md` §1.2.2 — "port
the albedo version's TAA"), so A-2's `screenPosDistanceSqr > 1.0` is a faithful
port of its source. The albedo and `base/` `renderTaaSampleReverse.fx` passes
**genuinely differ** at this line — the `base/` pipeline uses the looser `16.0`
screen-position-similarity gate (the surrounding `base/` code also differs: the
`base/` version has an extra `distMinMax` distance gate at `:128` that the
albedo version lacks). This is a real pipeline divergence, not a transcription
slip. **No A-2 code is touched** (Batch 2 only writes `base/` code) and the
discrepancy does not affect Batch 2 — it lands in **Batch 6**, when `taa.wgsl`'s
`reproject_old_samples` gains the `base/` variant (`09-design-b.md` §5.8.1 / §11
step 17 already calls for the `> 1.0` → `> 16.0` change there). Batch 6 must use
`16.0` for the `base/` reproject pass; A-2's albedo `1.0` stays correct for what
it is.

### The `taa_samples` seam — what was removed/moved, and Batch 6 owns the rewire

Per **Item #1 (ACCEPTED by user)**, the `base/` first-hit restructure removes
the TAA-sample writes from the first-hit pass:

- **Removed from `naadf_first_hit.wgsl`:** (1) the `taa_sample_accum` write (the
  A-2 `pack2x16float(vec2(1.0, light.r))` block) and (2) the `@group(2)`
  `taa_samples` ring write (the A-2 `if (FLAG_IS_TAA)` block). Verified against
  `base/renderFirstHit.fx:126-128` — the `base/` first-hit writes exactly
  `firstHitData` + `firstHitAbsorption` + `finalColor`, no `taaSamples` /
  `taaSampleAccum`. In NAADF's `base/` pipeline those writes moved into
  `base/renderTaaSampleReverse.fx`'s `ReprojectOld` (`taaSampleAccum`) +
  `CalcNewTaaSample` (`taaSamples` ring) passes.
- **`@group(2)` re-homed:** the first-hit pipeline's `@group(2)` was the
  `taa_samples` ring (A-2); it is now the read-only precomputed atmosphere. The
  `taa_layout` descriptor + `TaaGpu.taa_first_hit_bind_group` field stay in the
  tree (still built by `prepare_taa`, now unbound by any node) so Batch 6 can
  re-home them onto the `calc_new_taa_sample` pipeline without a churned diff.
- **Temporary blit source:** because the first-hit no longer writes
  `taa_sample_accum` and `naadf_taa_reproject_node` is out of the chain this
  batch, `naadf_final_blit_node`'s bind group is pointed at `final_color`
  (slot 1) instead of `taa_sample_accum`, and `FLAG_BLIT_FINAL_COLOR` selects
  the matching decode in `naadf_final.wgsl` (`final_color` has no weight field —
  a pure-RGB f16 triple — so the existing `weight`-divide decode would be
  wrong). This keeps the app runnable showing the 4-plane first-hit result
  directly. **This is a deliberate designed seam** — the same kind as Phase A's
  `shaded_color` stand-in.
- **Batch 6 owns the rewire** (`09-design-b.md` §11 steps 17–19): port the
  `base/` `ReprojectOld` (writes `taaDistMinMax`, `screenPosDistanceSqr > 16.0`)
  + `CalcNewTaaSample` (folds `final_color` into the `taa_samples` ring +
  updates `taa_sample_accum`), re-add `naadf_taa_reproject_node` +
  `naadf_calc_new_taa_sample_node` to the chain, **revert** the blit source to
  `taa_sample_accum`, and clear `FLAG_BLIT_FINAL_COLOR`.

### Bug found + fixed during the Batch-2 smoke-run

One blocking issue, in **shared Batch-1 WGSL**, not in new Batch-2 code:

1. **naga rejects `vec2<u32>` built from `f32` components.**
   `atmosphere.wgsl`'s `atmosphere_oct_index` had `vec2<u32>(oct.x *
   f32(texSize-1), oct.y * f32(texSize-1))` — a faithful transcription of the
   HLSL `uint2(...)` which truncates float→uint implicitly, but WGSL's vector
   constructor requires the component types to match. naga's error: *"Vector
   component[0] type Scalar Float, building Scalar Uint"*. This is a **latent
   Batch-1 bug**: `atmosphere_oct_index` lives in the shared `atmosphere.wgsl`
   module, and naga-oil only validates functions actually reachable from an
   entry point — Batch 1's only atmosphere entry (`naadf_atmosphere.wgsl`) does
   the octahedral decode inline and never calls `atmosphere_oct_index`, so it
   was never validated until Batch 2's first-hit became the first real caller.
   Fixed with explicit `u32(...)` casts on both components.

### Verification

- `cargo build --bin bevy-naadf` — clean (pre-existing dead-code warnings only;
  `Atmosphere` / `GI_FLAG_*` / `color_compression.rs` items warn "never used" —
  expected, Batch 3+ consumes them; the two new flag consts are used in
  `prepare.rs`).
- `cargo test --bin bevy-naadf` — **41 passed** (unchanged from Batch 1). Batch 2
  is a pure pipeline restructure — no new CPU-side logic to unit-test; the WGSL
  shader's correctness is the smoke-run + the user's visual review-gate.
- One smoke-run after the `atmosphere.wgsl` fix: launches clean (RTX 5080,
  Vulkan), builds the NAADF test grid, runs the full timeout with **no panic,
  no DeviceLost/Timeout, no naga/WGSL/validation errors**. The render-graph
  chain — atmosphere precompute → 4-plane first-hit → final-blit — compiles and
  dispatches every frame. The image is now the `base/` 4-plane first-hit with
  the full multiple-scattering atmosphere on a miss (no longer the A-2 flat sun
  term); mirror surfaces fill planes 1–3. No GI, no TAA yet. Visual correctness
  is the user's review-gate call.
- No TEMP debug instrumentation was added.

### Notes for the next batch (B3 — `rayQueueCalc` + `globalIllum`)

- **`final_color` is live and written by the first-hit.** B3's `globalIllum`
  and B5's `spatialResampling` thread their result through `FrameGpu.final_color`
  (`09-design-b.md` §3.4). It currently doubles as the temporary blit source —
  so once B5's `spatialResampling` writes into `final_color`, the Batch-2
  temporary blit will *show* the GI directly (as §11 Batch 5 notes). B3 itself
  writes no buffer the blit reads, so the Batch-2 image is unchanged through B3.
- **`first_hit_absorption` is live and written.** B3's `globalIllum` reads it
  (`09-design-b.md` §3.4 / §8.1) — it is in `FrameGpu`, `vec2<u32>`/pixel.
- **The `@group(2)` numbering decision.** Batch 2 put the first-hit's atmosphere
  at `@group(2)` (per §6.3's "`[world, frame, atmosphere]`"). When B3 wires
  `globalIllum`'s `@group(3)` atmosphere (§4.6), note that `globalIllum` *keeps*
  its world `@group(0)` + a GI-specific `@group(1)` + (per §4.6) `@group(3)`
  atmosphere — its group layout is independent of the first-hit's, so there is
  no conflict; just don't assume the first-hit's `@group(2)`-atmosphere
  numbering carries over.
- **`atmosphere_oct_index` is now validated.** The Batch-1 `vec2<u32>`-from-f32
  bug is fixed; B3's `globalIllum` (which also calls `apply_atmosphere` on a
  secondary-ray miss) can rely on `atmosphere_oct_index` + `apply_atmosphere`
  as-is. Watch for the *same class* of bug — HLSL implicit `uint(...)` /
  `int(...)` truncation in vector constructors needs explicit WGSL casts — when
  porting `rayQueueCalc.fx` / `renderGlobalIllum.fx`.
- **`FrameGpu` now has 9 fields + 4 bind groups.** B3's `prepare_gi` /
  `prepare_frame_gpu` changes (§10.3) add `Res<GiGpu>` to `prepare_frame_gpu`
  and a separate `GiBindGroups` resource — `prepare_frame_gpu` already takes
  `Res<AtmosphereGpu>` as of Batch 2, so adding `Res<GiGpu>` is the same
  pattern.
- **`naadf_taa_reproject_node` is `#[allow(dead_code)]` and out of the chain.**
  B6 step 18 re-adds it. The `TaaGpu.taa_first_hit_bind_group` field + the
  `taa_layout` descriptor are likewise dormant-but-present for B6 to re-home.
- **The `taa_sample_accum` binding is still in the first-hit's `frame_layout`**
  (slot 3, touched by a dead `if` so naga keeps it). B6's `CalcNewTaaSample`
  will write `taa_sample_accum` from its own pipeline; the first-hit's slot-3
  binding can stay (harmless) or B6 can drop it from `frame_layout` — designer's
  call, not load-bearing.
