# 10 — Phase B Implementation Log

Implements the batched sequence from `09-design-b.md` §11. The orchestrator
dispatches + reviews batch-by-batch. This log grows one batch section at a time.

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch
`feat/phase-b-gi`. Test command: `cargo test` (the `src/lib.rs` extraction moved
the unit tests into the lib suite — the old `cargo test --bin bevy-naadf` now
finds 0 tests).

**Verification step for Batches 4–6 onward:** `cargo build` + `cargo test` +
**`cargo run --bin e2e_render`** (the bounded windowed e2e render-test harness —
`e2e-render-test.md`, implemented 2026-05-14). The e2e run replaces the
open-ended live smoke-run: it boots the real windowed app, renders a fixed frame
budget, reads the framebuffer back, and runs the per-batch region gates + the
`PipelineCache` error scan + the node-dispatch check in one self-terminating
shot — run it **once**, read the exit code, do not loop. Each batch adds its gate
in `src/e2e/gates.rs` (`e2e-render-test.md` §6.4 / Implementation log).

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

---

## Batch 3 — `rayQueueCalc` + `globalIllum` (the GI sample generators)

`09-design-b.md` §11 Batch 3 (steps 9–11). Builds the Phase-B GI buffer set
(`GiGpu` — every §3.7 buffer) + the first two GI passes: `rayQueueCalc` (the
adaptive ~0.25-spp ray-queue builder) and `renderGlobalIllum` (the ≤3-bounce
secondary-ray tracer). These produce GI samples into buffers that nothing reads
yet — the done-bar is "the passes dispatch clean", not "the image changes". Ends
▶ runnable; the Batch-2 image is unchanged.

### Files created

| file | what it is | NAADF provenance |
|---|---|---|
| `src/assets/shaders/gi_params.wgsl` | the shared per-frame GI uniform struct `GpuGiParams` (288 B) + the `GI_FLAG_*` consts. naga-oil import module — imported by `ray_queue_calc.wgsl` + `naadf_global_illum.wgsl` (and the later GI passes). No explicit pad members — the four `vec3` rows are contiguous then the scalar tail begins on a fresh 16-byte boundary, so WGSL's std140-ish padding reproduces the Rust `#[repr(C)]` layout (the `vec3`-then-scalar trap that bit `AtmosphereParams` does NOT apply — verified field-by-field). | the union of every `base/` GI pass's scalar uniforms (`rayQueueCalc.fx:9-10`, `renderGlobalIllum.fx:16-28`, `renderSampleRefine.fx:21-32`, `renderSpatialResampling.fx:15-27`, `renderDenoiseSplit.fx:11-14`) |
| `src/assets/shaders/ray_queue_calc.wgsl` | the two `rayQueueCalc` compute entries: `calc_ray_queue` (`[numthreads(64,1,1)]` — the `should_ray` adaptive test, the inline group-shared prefix-counter, the queue write) + `calc_ray_queue_store` (`[numthreads(1,1,1)]` — converts the raw queued-pixel count to the indirect workgroup count). The `addToCounterAddressBuffer` group-shared prefix-counter is ported **inline** (it needs `var<workgroup>` at entry-point scope — not a reusable shared fn). | `base/rayQueueCalc.fx` `calcRayQueue` + `calcRayQueueStore`; `commonOther.fxh:6-22` `addToCounterAddressBuffer` |
| `src/assets/shaders/naadf_global_illum.wgsl` | the `calcGlobalIlum` compute entry — the ≤3-bounce secondary-ray tracer: per-queued-pixel ray setup, the primary-surface BRDF interaction (mirror / rough-VNDF / diffuse), the ≤3-bounce loop (`shoot_ray`, atmosphere Russian-roulette on miss, albedo, sun-sample, emissive, surface-effect bounce), `compress_color` + lit/unlit classification, the group-shared sample-count atomics, the wrapping ring write into `valid_samples` / `invalid_samples`. `compress_sample_valid` / `compress_sample_invalid` ported. | `base/renderGlobalIllum.fx` `calcGlobalIlum` + `compressSampleValid` + `compressSampleInvalid` |
| `src/render/gi.rs` | the `GiGpu` render-world resource (every §3.7 GI buffer + the `gi_params` uniform + the resize-trigger geometry); `GiBindGroups` (the mixed bind groups, built by `prepare_frame_gpu`); `prepare_gi` (creates/resizes/seeds the buffers, uploads `GpuGiParams`); the `accum_index_of` / `rand_salts_of` / `bucket_grid_of` helpers + the storage-count consts. 3 unit tests. | `WorldRenderBase.cs:104-171` (buffer creation), `:181` (`globalIlumAccumIndex`), `:157-159` (bucket grid) |

### Files changed

| file | change |
|---|---|
| `src/main.rs` | `AppArgs` gains `gi: GiSettings` — a new `GiSettings` struct (the C# `SettingDataRenderBase` slider *defaults* as constants: `bounce_count=3`, `global_illum_max_accum=128`, `spatial_resample_size=500`, `denoise_thresh=400`, `radius_lit_factor=3`, `noise_suppression_factor=0.4`, `spatial_visibility_count=80`, the 5 bools all `true`). `Default`-impl'd; `main()` constructs `gi: GiSettings::default()`. |
| `src/render/extract.rs` | += `ExtractedGiConfig` (a flat `Copy` mirror of `AppArgs.gi`) + `extract_gi_config` — the A-2 `ExtractedTaaConfig` pattern. |
| `src/render/gpu_types.rs` | **unchanged** — `GpuGiParams` (288 B) + `GpuSampleValid` (32 B) + the `GI_FLAG_*` consts were already declared in Batch 1; Batch 3 only consumes them. |
| `src/render/pipelines.rs` | `NaadfPipelines` += `ray_queue_layout` (`@group(0)` for `rayQueueCalc`: `gi_params` uniform + `first_hit_data` RO + `ray_queue` RW + `ray_queue_indirect` RW + `taa_sample_accum` RO), `global_illum_layout` (`@group(1)` for `globalIllum`: `gi_params` + 8 storage bindings), `empty_layout` + `empty_bind_group` (the entry-less placeholder for `globalIllum`'s skipped `@group(2)` — see below), `ray_queue_pipeline` + `ray_queue_store_pipeline` + `global_illum_pipeline` (cached compute ids). New `RAY_QUEUE_SHADER` / `GLOBAL_ILLUM_SHADER` path consts. The `globalIllum` pipeline layout is `[world, global_illum, empty, atmosphere_read]`. |
| `src/render/prepare.rs` | `prepare_frame_gpu` gains `Res<GiGpu>` + `Option<Res<GiBindGroups>>` and builds the **mixed** GI bind groups into a new `GiBindGroups` resource (`ray_queue_bind_group` + `global_illum_bind_group`) — mixing `GiGpu` + `FrameGpu` + `TaaGpu` buffers, rebuilt on the same `pixel_count` resize trigger. Waits on `GiGpu` alongside `TaaGpu` / `AtmosphereGpu` before building any bind group (`09-design-b.md` §10.3). |
| `src/render/graph_b.rs` | += `naadf_ray_queue_node` (TWO dispatches in one node: `RayQueue` over `ceil(pixel_count/64)` workgroups then `RayQueueStore` over 1) + `naadf_global_illum_node` (indirect dispatch off `ray_queue_indirect`, binds `@group(0)` world / `@group(1)` GI / `@group(2)` `empty_bind_group` / `@group(3)` the read-only atmosphere — reuses `FrameGpu.first_hit_atmosphere_bind_group`, same `atmosphere_read_layout` as the first-hit). New `RAY_QUEUE_SPAN` / `GLOBAL_ILLUM_SPAN` HUD-span consts. |
| `src/render/mod.rs` | `pub mod gi` declared; `ExtractedGiConfig` `init_resource`'d; `extract_gi_config` added to `ExtractSchedule`; `prepare_gi` added to `PrepareResources` (alongside `prepare_world_gpu` / `prepare_taa` / `prepare_atmosphere`); the `Core3d` chain gains `naadf_ray_queue_node` + `naadf_global_illum_node` between `naadf_first_hit_node` and `naadf_final_blit_node`. |

### Mapping to NAADF source

- **`ray_queue_calc.wgsl`** is `base/rayQueueCalc.fx` verbatim. `shouldRay`
  (`:12-21`) → `should_ray`: the `accum / 2`, `round(clamp(fac*2,0,3)+1)`
  `mod_size`, the `(frameIndex*4 + x + y) % mod_size == 0` spatial-temporal
  pattern — explicit `u32(round(...))` cast (HLSL truncates implicitly).
  `shouldAdd = (firstHitData[ID].z & 0x7FFF) != 0 && shouldRay(...)`
  (`:29`) — `accum` is `unpack2x16float(taa_sample_accum[id].x).x`. The HLSL
  `RWByteAddressBuffer groupCount` is `ray_queue_indirect` (C# binds
  `rayQueueIndirectBuffer` into `groupCount` — `WorldRenderBase.cs:280`); the
  `.Load(0)`/`.Store(0)`/`.InterlockedAdd(0,...)` byte-address ops at offset 0
  are element `[0]` = `GroupCountX`, so `ray_queue_indirect` is declared
  `array<atomic<u32>, 5>` and the inline counter does
  `atomicAdd(&ray_queue_indirect[0], ...)`. `calcRayQueueStore` reads `[0]`,
  writes `(v+63)/64`.
- **`addToCounterAddressBuffer`** (`commonOther.fxh:6-22`) ported inline into
  `ray_queue_calc.wgsl` per `09-design-b.md` §5.6 — `var<workgroup> index_group:
  atomic<u32>` + `index_group_base: u32`, the three `workgroupBarrier()`s, the
  per-lane `atomicAdd(&index_group, ...)` then lane-0
  `atomicAdd(&ray_queue_indirect[0], ...)`. HLSL `groupshared uint indexGroup = 0`
  initialises at module scope; WGSL `var<workgroup>` does NOT — lane 0 zeroes
  `index_group` before the first barrier (the C# relies on each dispatch starting
  freshly-zeroed; naga gives no such guarantee).
- **`naadf_global_illum.wgsl`** is `base/renderGlobalIllum.fx:60-291`
  `calcGlobalIlum`. The primary-surface BRDF interaction (`:97-116`), the
  ≤3-bounce loop `for bounce in 0..min(maxBounceCount,3)` (`:121-235`), the
  compress+classify tail (`:237-289`). The HLSL `do { ... } while` rough-specular
  loops become WGSL `loop { ...; if (!(cond)) { break; } count++; }`. The HLSL
  `applyAtmosphere(curPosInt+curPosFrac, curDir, ..., 16)` on a secondary-ray
  miss (`:131-132`) — the `atmoMul = 16` Russian-roulette compensation — uses the
  Batch-1-split `atmosphere_oct_index` + `apply_atmosphere` (the caller fetches
  the octahedral slot itself; `apply_atmosphere` can't take the storage buffer by
  `ptr`). `compressSampleValid` (`:34-48`) / `compressSampleInvalid` (`:50-58`)
  ported with explicit `u32()` casts on the `octEncode(sampleDir) * 2^22`
  coordinates. The group-shared `sharedResCount` + the storage-buffer
  `InterlockedAdd(globalIlumSampleCounts[3+accumIndex].x|.y, ...)` →
  `var<workgroup> shared_res_count: atomic<u32>` + `sample_counts` declared
  `array<SampleCountSlot>` where `SampleCountSlot { valid: atomic<u32>, invalid:
  atomic<u32> }` (`09-design-b.md` §5.5 / §12 #5). `#ifdef ENTITIES` blocks
  omitted (`entitySample = ENTITY_FREE` always, the entity params absent).
- **`gi.rs`** mirrors `WorldRenderBase.cs:104-171`. Buffer element sizes from
  `09-design-b.md` §3.1; `bucket_count = ceil(w/8)*ceil(h/8)`
  (`:157-159`); `accum_index = maxAccum - (frameCount % maxAccum) - 1` (`:181`).
  The three indirect buffers are seeded on creation — `ray_queue_indirect =
  [0,1,1,0,0]`, `valid_dispatch = invalid_dispatch = [1,1,1,0,0]`
  (`:136,168,170`). `sample_counts` (131 elements) is fixed-size, zero-cleared
  only on creation (it carries the 128-frame ring). The `sunColor` is the CPU
  `Atmosphere::get_light_for_point((0,10,0))` (`WorldRender.cs:96` — the Batch-1
  CPU atmosphere port). Everything uses plain `create_buffer`, not
  `GrowableBuffer` (the GI buffers never grow — `09-design-b.md` §3.1).

### The per-pixel sample-count signal — data flow (the ~0.25-spp realisation)

This is the headline 2× GI speedup, now actually exercised:

1. **A-2's TAA** exposes the per-pixel accumulated sample count in
   `taa_sample_accum[px].x & 0xFFFF` (an f16). (In the *full* `base/` pipeline
   `ReprojectOld` writes it — that is Batch 6; in Batch 3 it is the zero-cleared
   buffer the Batch-2 first-hit leaves untouched, so every pixel reads `accum ≈ 0`
   → `mod_size == 1` → every hit pixel is queued every frame, i.e. 1 spp. That is
   correct-but-not-yet-adaptive — the adaptive rate only kicks in once Batch 6
   wires `ReprojectOld` to fill `taa_sample_accum.x`. Documented as a cross-batch
   dependency, exactly like Batch 4's `taa_dist_min_max` gap.)
2. **`naadf_ray_queue_node` → `calc_ray_queue`** reads `taa_sample_accum[id].x`,
   runs `should_ray` per hit pixel, and for the pixels that pass reserves a slot
   in the global counter (`ray_queue_indirect[0]`) via the inline group-shared
   prefix-counter and writes the packed pixel position into `ray_queue`.
3. **`calc_ray_queue_store`** (1 thread) reads the raw queued-pixel count from
   `ray_queue_indirect[0]` and rewrites it as the workgroup count `(v+63)/64`.
4. **`naadf_global_illum_node`** dispatches `calc_global_ilum` **indirect** off
   `ray_queue_indirect` — one thread per *queued* pixel, so GI cost scales with
   the adaptive rate, not the screen. Each thread reads `ray_queue[globalID.x]`,
   traces the ≤3-bounce ray, and writes a lit/unlit sample into the
   `valid_samples` / `invalid_samples` rings + bumps the 128-frame
   `sample_counts` ring.

Render-graph ordering (`mod.rs` `.chain()`) guarantees first-hit → ray_queue →
global_illum; wgpu's automatic buffer barriers serialise the shared-buffer
accesses (`ray_queue`, `ray_queue_indirect`, `first_hit_data`).

### Design ambiguities adjudicated

1. **`compressSampleValid`'s `sampleSpecularNormals` parameter — `normTangs`
   directly, NOT `getSpecularNormals(...)`.** `09-design-b.md` §8.1 mentions
   `compress_sample_valid` and §5.2 lists `get_specular_normals` as a shared
   helper, which could read as "globalIllum calls `getSpecularNormals`". The
   HLSL is authoritative: `renderGlobalIllum.fx:280` passes the `normTangs`
   `uint3` of secondary-bounce plane codes **directly** as the
   `sampleSpecularNormals` parameter; `compressSampleValid` packs `.x`/`.y`/`.z`
   each `<< 15` into `data2`. `getSpecularNormals` (`commonRenderPipeline.fxh`)
   is `renderSampleRefine`'s helper, applied to the *first-hit* G-buffer planes —
   a different thing. Followed the HLSL: `compress_sample_valid` takes the raw
   `norm_tangs` `vec3<u32>`. (`get_specular_normals` stays in
   `render_pipeline_common.wgsl` from Batch 1 — Batch 4's `sampleRefine` uses it.)
2. **`globalIllum`'s `@group(2)` — an entry-less placeholder.**
   `09-design-b.md` §8.1 / §4.6 specify `globalIllum` binds `@group(0)` world +
   `@group(1)` GI + `@group(3)` atmosphere — it skips `@group(2)` entirely. wgpu
   pipeline layouts are a `Vec` indexed by group number, so index 2 needs *some*
   layout. Added an entry-less `empty_layout` (`BindGroupLayoutDescriptor::new(_,
   &[])`) + a one-time `empty_bind_group`; `naadf_global_illum_node` does
   `set_bind_group(2, &pipelines.empty_bind_group, &[])`. The design's stated
   `@group(3)` numbering is honoured exactly (the alternative — renumbering
   atmosphere to `@group(2)` — would have been simpler but deviates from the
   design's explicit group plan; the placeholder is the faithful choice).
3. **The per-frame `ray_queue_indirect[0]` reset — a Batch-3 designed seam.**
   `09-design-b.md` §7.3 says `ray_queue_indirect[0]` must be zeroed each frame
   *before* `calcRayQueue`, and that NAADF does it in `ClearBucketsAndCalcMask`
   (a `sampleRefine` pass — Batch 4). Without it, `calcRayQueue`'s `atomicAdd`
   onto element `[0]` would carry the previous frame's workgroup count into the
   next frame's count. **Resolution:** `prepare_gi` re-seeds `ray_queue_indirect`
   to `[0,1,1,0,0]` from the CPU every frame — a minimal Batch-3-local fix so
   Batch 3 is correct *standalone* (the same designed-seam pattern as Batch 2's
   temporary `final_color` blit source). When Batch 4 lands
   `ClearBucketsAndCalcMask`'s in-shader reset, this CPU re-seed becomes
   redundant and Batch 4 can drop it. Flagged in `gi.rs` + `mod.rs`.

### Bugs found + fixed during the Batch-3 smoke-run

Three blocking issues, all in new Batch-3 WGSL, all fixed:

1. **naga-oil rejects the trailing-digit struct field `rand_counter2`.** The
   Batch-1 carry-forward exactly: naga-oil's composable-module writeback rejects
   trailing-digit identifiers. `gi_params.wgsl`'s `rand_counter2` (mirroring the
   C# `randCounter2`) → `rand_counter_b`; the `naadf_global_illum.wgsl` use site
   updated. The WGSL field name is read positionally by offset — not
   load-bearing — same fix as Batch 1's `SampleValid.data_a`/`data_b`. (The Rust
   `GpuGiParams.rand_counter2` field name is unaffected — the rule is WGSL-only.)
2. **`let _ = <imported-fn>(...)` is invalid after naga-oil's import rewrite.**
   `naadf_global_illum.wgsl` discarded the unused `radianceCompWithAbsorption`
   (computed in the HLSL `:237`, never read — kept only for RNG-state fidelity)
   with `let _ = compress_color(...)`. naga-oil rewrites the call to a namespaced
   form and then "Identifier can't be `_`". Fixed: a named throwaway
   (`let _unused_... = compress_color(...)`) for the call; for the binding-only
   `camera_history` reference, WGSL's phony-assignment `_ = expr;` (no `let`).
3. **`fac` is a `float3`, not a scalar — sun-sample type error.**
   `renderGlobalIllum.fx:169` is `float3 fac = saturate(...) * 2` — HLSL
   broadcasts the scalar to a `float3` (the rough-specular branch multiplies it
   by the `vec3` Fresnel `F`, and `radiance += ... * fac` is all `vec3`). The
   port had `var fac` as a scalar `f32`, so `fac = fac * (... * f)` assigned a
   `vec3` to a scalar — naga "Entry point invalid". Fixed: `var fac =
   vec3<f32>(clamp(...) * 2.0)`.

### Verification

- `cargo build --bin bevy-naadf` — clean (pre-existing dead-code warnings only;
  new ones: `GiGpu.bucket_count` / `bucket_size` "never read" — Batch 4's
  `sampleRefine` reads them; `NaadfPipelines.empty_layout` "never read" — only
  used inside `from_world`; expected forward-looking warnings, same pattern as
  Batch 1/2).
- `cargo test --bin bevy-naadf` — **44 passed** (was 41; +3 from `gi.rs`:
  `accum_index_walks_the_ring`, `rand_salts_are_two_distinct_per_frame_values`,
  `bucket_grid_ceils_to_eights`).
- One smoke-run after the three fixes: launches clean (RTX 5080, Vulkan), builds
  the NAADF test grid (32 chunks, 64×32×64 voxels), runs ~38 s, exits cleanly
  (exit code 0) on window close with **no panic, no DeviceLost/Timeout, no
  naga/WGSL/validation/composable-module errors**. The render-graph chain —
  atmosphere precompute → 4-plane first-hit → `rayQueueCalc` (2 dispatches) →
  `globalIllum` (indirect dispatch) → final-blit — compiles and dispatches every
  frame. The image is the unchanged Batch-2 4-plane first-hit (the GI passes
  write `ray_queue` / `valid_samples` / `invalid_samples` / `sample_counts` —
  buffers the blit does not read; the GI result is not composited until Batch 5).
  No TEMP debug instrumentation was added.

### Notes for the next batch (B4 — `sampleRefine`, the 5 passes)

- **`GpuGiParams` is fully populated and uploaded.** B4's `sampleRefine` passes
  bind `gi_params` (the design's `sample_refine_bind_group` — `09-design-b.md`
  §8.2). Every field B4 needs (`accum_index`, `bucket_size_x/y`, `bucket_count`,
  the storage-count constants, `rand_counter`/`rand_counter_b`,
  `sample_max_accum`) is live. **WGSL field-name note:** the second RNG salt is
  `gi_params.rand_counter_b` in WGSL (not `rand_counter2` — naga-oil trailing-
  digit rule); the Rust struct field stays `rand_counter2`.
- **The GI buffers B4 needs are all created/sized/seeded by `prepare_gi`:**
  `valid_samples` / `invalid_samples` (read by B4), `valid_samples_refined` /
  `valid_samples_compressed` / `bucket_info` (written by B4 — `bucket_count`-
  sized, resize-on-viewport), `sample_counts` (the 128-frame ring — fixed-size,
  zero-cleared only on creation), `valid_dispatch` / `invalid_dispatch` (the
  indirect-arg buffers B4's `ComputeValidHistory` writes — seeded `[1,1,1,0,0]`).
  `GiGpu.bucket_count` / `bucket_size` are populated for B4's dispatch math.
- **`ray_queue_indirect[0]` per-frame reset — B4 owns moving it in-shader.**
  Batch 3's `prepare_gi` re-seeds `ray_queue_indirect` to `[0,1,1,0,0]` from the
  CPU every frame (the Batch-3 designed seam — see "Design ambiguities" #3). B4's
  `ClearBucketsAndCalcMask` does the proper in-shader reset (`renderSampleRefine.fx:39`
  — `groupCount.Store(0, 0)`, *and* it clears `sample_counts[3+accumIndex]`).
  Once B4 lands that, the `prepare_gi` CPU re-seed is redundant — B4 can delete
  the `render_queue.write_buffer(&resources.ray_queue_indirect, ...)` line in
  `gi.rs` (it is clearly marked). B4 must also insert `naadf_sample_refine_clear_node`
  at its §4.2 position — **before** `naadf_ray_queue_node` in the chain.
- **The mixed-bind-group pattern is established.** `prepare_frame_gpu` builds
  `GiBindGroups` (currently `ray_queue_bind_group` + `global_illum_bind_group`).
  B4 adds `sample_refine_bind_group` to `GiBindGroups` the same way — it mixes
  `GiGpu` + `FrameGpu` (`first_hit_data`) + `TaaGpu` (`taa_dist_min_max` —
  *not yet written* until B6, see below — and `camera_history` for the
  `view_proj_inv` ring). `prepare_frame_gpu` already takes `Res<GiGpu>`.
- **`taa_dist_min_max` is the zero-cleared buffer until B6** (`09-design-b.md`
  §11 Batch 4 step 13 already calls this out): B4's `CountValidAndRefine` /
  `CountInvalid` validity test reads `taa_dist_min_max`, but the Batch-2
  `albedo/` reproject pass does not write it and B6 is what rewires the `base/`
  `ReprojectOld`. So in B4 the validity test rejects everything — the passes
  dispatch clean, the data is just empty. Correct-but-empty, never invalid.
- **The `empty_layout` / `empty_bind_group` placeholder pattern is available**
  if any B4 pass also skips a `@group` index — `NaadfPipelines.empty_bind_group`
  is a ready entry-less bind group.
- **`get_specular_normals` is unused so far** — it is in
  `render_pipeline_common.wgsl` (Batch 1) but neither Batch 2 nor Batch 3 calls
  it. B4's `sampleRefine` is its first consumer (`renderSampleRefine.fx` applies
  it to the first-hit G-buffer planes).

---

## Streaking artifact fix (2026-05-14)

Diagnose-and-fix for the rendering artifact reported at the Phase-B review gate:
**hard-edged horizontal streaks / a concentric-ring interference pattern wherever
rays miss or exit the voxel volume — the region where the atmospheric sky should
render.** Everpresent at every camera pose. The user confirmed it as a
*resurgence* of the Phase-A "out-of-volume concentric-ring / streaking" artifact.

**Verdict up front: fixed.** Root cause is NOT the Phase-A ray-AABB clip box —
that fix (`05-review.md` — the `0.1`-voxel-inset `float3` bounding box) is fully
intact in `prepare.rs` / `gpu_types.rs` / `world_data.wgsl` / `ray_tracing.wgsl`
/ `naadf_first_hit.wgsl`. This is a *variant*: a different mechanism with the
same visual signature (a regular interference pattern in the miss region), newly
exposed by Phase B Batch 1's atmosphere subsystem.

### Step zero — baseline restore

`src/e2e/gates.rs` was restored to its committed `6e8f26e` state
(`git checkout 6e8f26e -- src/e2e/gates.rs`), dropping the abandoned/scrapped
`6ebd42c` "zoom e2e camera in" edit. `git status` after: only `gates.rs`
modified — known-good e2e harness baseline.

### Diagnosis chain

**1. Observed.** `cargo run --bin e2e_render` → `Read` `e2e_latest.png`: the
upper/sky region of the frame is filled with hard-edged, regular horizontal
streaks and a concentric ring-like band lower down; the emissive cube and voxel
geometry render correctly. Luminance gate: 41.1% non-black (a clean frame is
~100%).

**2. Located.** The Phase-A ray-AABB fix was verified intact across all five
files it touched — *not* the regression. The artifact is in the **miss-ray
path**: for a ray that misses the volume, `naadf_first_hit.wgsl` writes
`final_color` purely from `apply_atmosphere(...)` reading the precomputed
octahedral atmosphere buffer (`atmosphere_comp`). That buffer + the
`atmosphere_oct_index` / `apply_atmosphere` sampler + the `naadf_atmosphere.wgsl`
precompute are all **new in Phase B Batch 1** (the atmosphere subsystem).

**3. Hypothesised + isolated (5 e2e runs, instrumentation reverted after each).**
   - *Run 2* — replaced the miss-path `apply_atmosphere` (precomputed-buffer
     read) with a direct `add_light_for_direction` ray-march: **sky rendered
     clean, no streaks.** → the bug is in the precompute-buffer path, not the
     shared atmosphere ray-march math.
   - *Run 3* — made `naadf_atmosphere.wgsl` write a smooth `(tex_pos_norm.x,
     tex_pos_norm.y, 0.5)` gradient instead of the atmosphere result:
     **streaks persisted** even though the written data is perfectly smooth in
     buffer-index space. → the precompute→buffer→apply *indexing/storage* is
     broken, not the atmosphere computation.
   - *Run 4* — visualised `atmosphere_oct_index`'s output (`comp_pos` + an
     out-of-bounds flag) directly: **perfectly smooth, fully in-bounds.** → the
     read index is correct; the buffer *content* at smoothly-adjacent indices is
     not smooth, i.e. the precompute is not writing the slots the sampler reads.
   - *Run 5* — `naadf_atmosphere.wgsl` writes a `b = 1.0` presence marker on
     every slot it touches; `info!`-logged `frame_count` in `prepare_atmosphere`.
     Result: **`frame_count` is pinned at `0` for the entire e2e run** (every
     `prepare_atmosphere` call logged `frame_count=0`), and the presence marker
     confirmed ~3/4 of the octahedral buffer is stale-zero, interleaved in a
     regular pattern with the ~1/4 written slots.

**4. Root cause (confirmed against NAADF source).**
`base/renderAtmosphere.fx:12` — `precomputeAtmosphere` writes
`ID = globalID.x * 4 + (frameCount % 4)`: it precomputes **one quarter of the
octahedral buffer per frame**, cycling all four quarters as `frameCount`
advances `0,1,2,3,0,…`. The port (`naadf_atmosphere.wgsl`) replicates this
faithfully. But in the e2e harness `frameCount` never leaves `0`, so the
precompute only ever writes the `id % 4 == 0` quarter — the other three
quarters stay zero-cleared forever. The miss-path `apply_atmosphere` then reads
a smoothly-sweeping `oct_index` that cycles 1-written / 3-stale-zero, and that
aliases against the screen into the regular hard-edged streaks. Same *visual
class* as the Phase-A `floor()`-knife-edge artifact (a regular interference
pattern in the miss region), different mechanism — hence the user reading it as
a "resurgence / variant".

*Why `frameCount` is stuck:* `update_camera_history` (`src/render/taa.rs`) — the
**only** writer that increments `CameraHistory.frame_count` — queried the camera
as `Single<(&Camera, &Transform, &PositionSplit), With<FreeCamera>>`. The e2e
harness deliberately spawns its fixed-pose camera **without** `FreeCamera`
(`e2e/mod.rs setup_e2e_camera` — `FreeCameraPlugin` is omitted from the e2e
config). A `Single` system param that matches no entity makes Bevy **silently
skip the system** — so `frame_count` never advanced. This is a *pre-existing
latent* bug (the `With<FreeCamera>` filter + the `FreeCamera`-less e2e camera
both predate Phase B), but it was **harmless until Phase B Batch 1**: Batch 1's
atmosphere precompute is the first subsystem whose *visible output* depends on
`frameCount % 4` cycling. (`sync_position_split` has the identical
`With<FreeCamera>` query and is *also* skipped in the e2e harness — but that one
is genuinely harmless: the e2e camera seeds a correct `PositionSplit` at spawn
and the pose is fixed.)

### The fix

**File: `src/render/taa.rs`** — `update_camera_history`'s camera query changed
from `With<FreeCamera>` to `With<PositionSplit>`. `FreeCamera` is an *input*
concern (the fly-camera plugin); the frame counter + camera-history ring are
*render* concerns that must advance for every configuration of the NAADF render
camera. `PositionSplit` is the component that marks "the NAADF render camera" —
present on both the production fly-camera (`camera/mod.rs setup_camera`) and the
e2e fixed-pose camera (`e2e/mod.rs setup_e2e_camera`). With the broader filter,
`update_camera_history` runs in both configs and `frame_count` advances
monotonically, so the atmosphere precompute cycles all four quarters and the
octahedral buffer is fully populated. The now-unused `FreeCamera` import was
removed; the doc comment records why the filter is `PositionSplit`, not
`FreeCamera`. No NAADF-divergence — `frameCount` advancing every frame *is*
NAADF's behaviour (`WorldRender.cs:86`); the port had simply gated its only
incrementer behind a too-narrow camera filter.

**File: `src/e2e/gates.rs`** — restored to `6e8f26e` (step zero above); not a
fix change, the scrapped zoom edit dropped.

### Verification

- `cargo build` — clean.
- `cargo test` — **44 passed** (4 suites), 0 failed — unchanged.
- `cargo run --bin e2e_render` — exits **0**, all gates green; luminance gate
  now **100.0%** non-black (was 41.1% with the artifact).
- Visual assessment of the post-fix `target/e2e-screenshots/e2e_latest.png`:
  the sky region renders as a **clean atmospheric gradient** — smooth blue at the
  horizon fading to dark toward the top, **no hard-edged horizontal streaks, no
  concentric rings**. The emissive cube reads bright-white, the voxel geometry
  forms the expected dark diamond. The faint dark banding that was also visible
  on the voxel geometry itself (the 4-plane first-hit's per-bounce
  `apply_atmosphere` reads the same stale buffer) is likewise gone. Matches the
  expected clean-sky render.

All five e2e instrumentation passes were reverted; final `git status` is exactly
`src/e2e/gates.rs` + `src/render/taa.rs`, no debug residue.

---

## e2e test-scene expansion (2026-05-14)

The e2e harness's hard-coded test scene was expanded so the framed scene carries
guaranteed non-black luminance pre-GI (more emissive content), and the e2e gates
were recalibrated to the new scene. Background: pre-GI, voxel blocks render
pitch-black except emissive blocks (white), and the atmosphere fades toward dark
at the lower sky — the e2e luminance gate ("scene isn't mostly dead") worked
against a single-emissive scene. The chosen fix (over tinting the sky, which
would deviate the faithful NAADF port) is a larger voxel arrangement + several
additional emissive blocks. **Test-scene + gates only — no renderer/atmosphere
shader was touched.**

### The test grid is SHARED with the production app

`voxel::grid::setup_test_grid` is a `Startup` system added by `build_app`
(`src/lib.rs:292`) for **both** the production `bevy-naadf` binary **and** the
`e2e_render` harness — only the camera differs (`build_app` adds
`camera::setup_camera` for production, `e2e::add_e2e_systems` swaps in the
fixed-pose `setup_e2e_camera` for e2e). So expanding the grid enriches the live
`cargo run` app as well as the e2e frame — acceptable and welcome per the task
brief. The expansion was done in the shared builder (`src/voxel/grid.rs`); no
e2e-specific scene was introduced.

### The expanded scene (`src/voxel/grid.rs build_default_volume`)

Still the 64×32×64-voxel volume, still fully deterministic (fixed positions,
fixed emissive values, no RNG — the e2e harness depends on a bit-identical
scene). Was: ground slab + 2 boxes + 1 sphere + 1 emissive box. Now:

- **Ground** — the bottom-3-layer slab (unchanged).
- **Four corner towers** (`TY_TOWER`, neutral grey) — varied heights 21..26,
  framing the volume corners.
- **Back wall + arch** (`TY_WALL`, sand diffuse) — a wall along the far +x edge
  with a doorway arch carved back to empty; a big surface for GI bounce.
- **Box A** (warm) + **Box B** (cool) — the two original diffuse boxes,
  enlarged and repositioned.
- **A row of three violet pillars** (`TY_PILLAR`) marching across the
  mid-volume, varied heights.
- **Two green diffuse spheres** (`TY_SPHERE`) resting on the ground.
- **Five emissive blocks** distributed through the volume at varied positions /
  heights — the guaranteed-non-black content pre-GI, and the GI bounce-light
  sources once Batch 5 lands:
  1. `TY_EMISSIVE` warm-white — `[28,23,30]..[34,28,36]`, near the volume centre
     (the original single emissive block, kept).
  2. `TY_EMISSIVE_COOL` cool-white — `[10,6,44]..[15,11,49]`, low / near corner.
  3. `TY_EMISSIVE_AMBER` amber — `[46,24,46]..[51,29,51]`, high / far corner.
  4. `TY_EMISSIVE_GREEN` green — `[44,14,14]..[49,19,19]`, mid-height +x/-z.
  5. `TY_EMISSIVE_MAGENTA` magenta — `[20,5,50]..[25,10,55]`, low / near +z.

The palette grew from 6 to 13 entries (`build_palette` — index 0 reserved empty,
+ `TY_TOWER`/`TY_WALL`/`TY_PILLAR` diffuse + four extra emissive colours). The
`voxel_types` GPU buffer is a `GrowableBuffer`, so the larger palette needs no
plumbing change; 13 ≪ `VOXEL_PAYLOAD_MASK` (0x7FFF). Construction grew from
1536→1920 blocks / 2144→7232 voxel-u32s.

Two unit tests added (`src/voxel/grid.rs`): `default_volume_has_five_emissive_blocks`
(asserts one interior voxel of each of the five emissive blocks + that all five
palette entries are `Emissive`) and `default_volume_arch_is_carved` (the wall is
solid, the arch doorway is empty). The pre-existing `default_volume_has_ground_and_air`
air-probe coordinate was moved well clear of the new geometry.

### e2e camera re-framed (`src/e2e/gates.rs e2e_camera_transform`)

At the prior pose `(112,52,117) looking_at (34,20,34)` the expanded scene sat
small and far in the 256×256 frame. The camera was pulled **closer** along the
same look axis — to `(86,42,90) looking_at (32,16,32)`, ~117→~83 units out,
keeping the same ~16°-below-horizontal clean 3/4 pitch — so the expanded volume
fills the frame with the atmosphere sky band still across the top.

### Recalibrated B1–B3 gate rects (`src/e2e/gates.rs`, fractional 0..1 coords)

Re-derived from a fresh readback PNG dump at the new pose + scene:

| rect | new fractional | measured region-mean luminance | gate |
|---|---|---|---|
| `emissive_rect`    | `(0.45, 0.36)–(0.55, 0.45)` | ~234 | `> 120` |
| `solid_block_rect` | `(0.42, 0.52)–(0.58, 0.66)` | ~4   | `< 90` |
| `sky_rect`         | `(0.05, 0.04)–(0.45, 0.16)` | ~133 | `[10, 230]` and `> solid` |

`emissive_rect` is the warm-white centre block's interior; `solid_block_rect` is
the dark diffuse voxel geometry directly below it (near-black pre-GI, by
design); `sky_rect` is the upper-left atmosphere band. All three clear their
thresholds with generous margin. The B2 relative check (`sky_lum > solid_lum`,
133 > 4) holds.

### Recalibrated luminance liveness gate (`src/e2e/framebuffer.rs`)

Measured pre-GI whole-frame **non-black fraction: 69.1%** at the new pose+scene
(was ~41% with the old single-emissive scene) — materially higher thanks to the
five distributed emissive blocks.

- **`MIN_NON_BLACK_FRACTION_PRE_GI` (Batch 1–4 floor): 0.25 → 0.50.** A real
  check ~19 pts below the measured 69.1% — trips if the sky/blit/first-hit node
  silently drops a large part of the frame, not a rubber stamp.
- **`MIN_NON_BLACK_FRACTION_GI` (Batch 5+ hard gate): 0.50 → 0.60.** With the
  pre-GI fraction already at 69.1%, the GI-lit fraction will be ≥ that, so a
  0.50 hard gate would no longer be a real check on the expanded scene. Set to
  0.60 — above the user's verbatim "at least 50%" floor, a real check on the
  GI-lit frame, with headroom for the Batch-5 agent to confirm against the
  actual measured GI-lit fraction and nudge per the `e2e-render-test.md` rule.
  The batch-aware structure (pre-GI floor for B1–B4, higher hard threshold from
  B5) is unchanged.

### Task 3 — NAADF horizon behaviour (record only, no fix)

NAADF's atmosphere (`Content/shaders/render/common/atmosphere/atmosphereRaw.fxh`
`addLightForDirection`, driven by `base/renderAtmosphere.fx precomputeAtmosphere`)
**genuinely fades to dark for downward / low rays — it is not a missing horizon
term.** Mechanism: for any direction `addLightForDirection` ray-marches in-scatter
only while the ray's segment through the atmosphere shell is positive. For a
downward ray `rayResultPlanet.y > 0` (it hits the planet), so the march length
is clamped to `rayResultPlanet.x - rayResult.x` — the thin slice from where the
ray enters the atmosphere down to the planet surface. That slice still
in-scatters a small amount of light (so downward rays are dim, **not** pure
black), but **the planet surface contributes nothing** — there is no ground
albedo / horizon-colour term in the model at all. So the lower hemisphere
correctly tends toward dark (only thin-slice atmosphere in-scatter), and our
WGSL port (`src/assets/shaders/atmosphere.wgsl add_light_for_direction` — the
same `ray_result.y = ray_result_planet.x - ray_result.x` clamp) reproduces this
faithfully. No horizon colour is missing; nothing to change. (The faithful port
stays as-is per the task constraint — this is a record only.)

### Verification

- `cargo build` — clean (pre-existing dead-code warnings only).
- `cargo test` — **46 passed** (was 44; +2 new scene-construction tests —
  `default_volume_has_five_emissive_blocks`, `default_volume_arch_is_carved`).
  The `PipelineCache` scan, node-dispatch, degenerate-frame checks unaffected.
- `cargo run --bin e2e_render` — exits **0**, all gates green at the expanded
  scene: luminance gate `batch 3 — 69.1% non-black; threshold 50%`, then
  `PASS (batch 3) — … per-batch region gate green, every pipeline created
  cleanly, every expected render-graph node dispatched.` Screenshot written to
  `target/e2e-screenshots/e2e_latest.png` every run.
- **Visual assessment of `e2e_latest.png`:** the expanded scene renders
  sensibly — three emissive blocks (warm-white centre, magenta lower-left, green
  right) render bright/coloured, the warm block's dark cube edge is visible, the
  dark diffuse voxel structure (towers, boxes, pillars) fills the mid/lower
  frame near-black as expected pre-GI, the atmosphere sky band sits clean across
  the top with no streaks/rings. (The amber + cool-white emissive blocks are
  higher / partly occluded by geometry — three clearly-visible emissive blocks
  is ample guaranteed-non-black content.) Used 4 of the 5 allotted
  `cargo run --bin e2e_render` invocations.
- No TEMP debug instrumentation added; the rect-derivation used an offline PNG
  analysis script, not in-tree code.

---

## Batch 4 — sampleRefine (2026-05-14)

`09-design-b.md` §11 Batch 4 (steps 12–14). Builds the 5-pass
`renderSampleRefine` stage — the compressed-ReSTIR brightness-leveling stage:
it takes the lit/unlit GI samples `renderGlobalIllum` (Batch 3) wrote into the
temporal rings, reprojects each into the current frame's 8×8 screen-space
bucket grid via the camera-history rings, counts per-bucket lit/unlit totals,
and brightness-levels the survivors with the `COLOR_DIF_PROB` exponential-
difference probability table. Still buffer-only — the 5 passes write
`valid_samples_refined` / `valid_samples_compressed` / `bucket_info`, which
nothing reads until Batch 5's `spatialResampling`. Ends ▶ runnable; the
Batch-2/3 image is unchanged.

### Files created

| file | what it is | NAADF provenance |
|---|---|---|
| `src/assets/shaders/sample_refine.wgsl` | the 5-pass sample-refine stage as 5 compute entry points in one naga-oil module — `clear_buckets_and_calc_mask`, `compute_valid_history`, `count_valid_data_and_refine`, `count_invalid_data`, `refine_buckets`. Shared helpers `shuffle_group` + `reproject_sample` (the byte-identical reprojection the two count passes run). All 5 entries share `@group(0)` = `sample_refine_bind_group`; `compute_valid_history` additionally binds `@group(1)` (the indirect-arg buffers — see "Design ambiguities" #1). | `base/renderSampleRefine.fx` (441 lines — the 5 `cs_5_0` passes + `ShuffleGroup`) |

### Files changed

| file | change |
|---|---|
| `src/render/pipelines.rs` | `NaadfPipelines` += `sample_refine_layout` (`@group(0)`, 11 bindings: `gi_params` uniform + `first_hit_data`/`valid_samples`/`invalid_samples`/`taa_dist_min_max`/`camera_history` RO + `bucket_info`/`valid_samples_refined`/`valid_samples_compressed`/`sample_counts`/`ray_queue_indirect` RW) + `sample_refine_dispatch_layout` (`@group(1)`, `valid_dispatch`+`invalid_dispatch` RW — the wgpu indirect-vs-storage split, #1 below); `sample_refine_clear` / `_valid_history` / `_count_valid` / `_count_invalid` / `_buckets` cached compute pipelines (the `valid_history` one's layout vec carries both groups, the other 4 only `@group(0)`); new `SAMPLE_REFINE_SHADER` path const. |
| `src/render/taa.rs` | `TaaGpu` += `taa_dist_min_max: Buffer` (`pixel_count` × `vec2<u32>` — `base/renderTaaSampleReverse.fx`'s `ReprojectOld` extra output, §3.5). Created/resized/zero-cleared alongside `taa_samples` (`create_screen_buffers` now returns the triple; `prepare_taa`'s match + the clear encoder + the `TaaGpu` insert all thread it). Batch 4 lands the *buffer* so `sample_refine_bind_group` can reference it; Batch 6 wires the `base/` `ReprojectOld` shader write (until then it is the zero-cleared buffer — see "Cross-batch dependency"). |
| `src/render/gi.rs` | `GiBindGroups` += `sample_refine_bind_group` + `sample_refine_dispatch_bind_group`. **`prepare_gi`'s per-frame CPU re-seed of `ray_queue_indirect` is DELETED** — Batch 4's `clear_buckets_and_calc_mask` moves that reset in-shader (see below). The on-creation seed in `create_gi_buffers` stays (`[1]`/`[2]` = `GroupCountY/Z` = 1). |
| `src/render/prepare.rs` | `prepare_frame_gpu` builds the two new mixed bind groups into `GiBindGroups` (alongside the Batch-3 `ray_queue` / `global_illum` groups, same `pixel_count` resize trigger). `sample_refine_bind_group` mixes `GiGpu` + `FrameGpu` (`first_hit_data`) + `TaaGpu` (`taa_dist_min_max` + `camera_history`); `sample_refine_dispatch_bind_group` is `GiGpu`-only (`valid_dispatch` + `invalid_dispatch`). |
| `src/render/graph_b.rs` | += `SAMPLE_REFINE_SPAN` (one combined HUD/dispatch-check span shared by all 5 passes — `09-design-b.md` §4.7 recommendation) + the 5 node systems `naadf_sample_refine_clear_node` / `_valid_history_node` / `_count_valid_node` / `_count_invalid_node` / `_buckets_node`. `clear` / `buckets` dispatch `ceil(bucket_count/64)` workgroups; `valid_history` dispatches `(1,1,1)` and also sets `@group(1)`; `count_valid` / `count_invalid` `dispatch_workgroups_indirect` off `valid_dispatch` / `invalid_dispatch`. |
| `src/render/mod.rs` | the 5 nodes imported + wired into the `Core3d` `.chain()` at their §4.2 positions: `naadf_sample_refine_clear_node` inserted **between `naadf_first_hit_node` and `naadf_ray_queue_node`** (it owns the in-shader `ray_queue_indirect[0]` reset that `calcRayQueue` then `atomicAdd`s into); the other 4 inserted **after `naadf_global_illum_node`, before `naadf_final_blit_node`** (they consume the GI sample rings `globalIllum` filled). |
| `src/e2e/gates.rs` | `CURRENT_BATCH` 3 → 4; `assert_batch_4` added (re-runs the B2 emissive/solid/sky region gate — Batch 4 leaves the image unchanged — plus the optional `hash_baseline(4)` tripwire); `batch_gate` / `expected_spans` gain their B4 arms (`expected_spans` adds the single `naadf_sample_refine` span — the 5 passes share one span). `hash_baseline` kept returning `None` for B4 — see "e2e harness" below. |

### How the 5 passes map to `base/renderSampleRefine.fx`

The WGSL is a faithful function-by-function port of the 5 HLSL `cs_5_0` passes:

- **`clear_buckets_and_calc_mask`** ← `clearBucketsAndCalcMask`
  (`renderSampleRefine.fx:33-69`). Lane 0 of the whole dispatch does the
  per-frame reset (`sample_counts[3+accumIndex] = (0,0)` +
  `ray_queue_indirect[0] = 0`); every bucket lane `< bucket_count` scans its
  8×8 pixel region's `first_hit_data` into the bucket's normal-mask +
  min/max-distance, written to `bucket_info[bucket]`. The HLSL `getTang` is the
  Batch-1 shared helper; `f32tof16` ↔ `pack2x16float(... ).x & 0xFFFF`.
- **`compute_valid_history`** ← `computeValidHistory`
  (`renderSampleRefine.fx:71-101`, `[numthreads(1,1,1)]`). Walks the 128-frame
  `sample_counts` ring back from `accum_index`, summing until the ring-buffer
  capacity is hit, then writes `sample_counts[0]` (write cursors), `[1]`
  (totals), `[2]` (`find_coprime` shuffle seeds — the Batch-1 shared
  `common.wgsl` helpers), and `valid_dispatch[0]` / `invalid_dispatch[0]` (the
  `next_pow2`-padded workgroup counts for the two indirect count passes).
- **`count_valid_data_and_refine`** ← `countValidDataAndRefine`
  (`renderSampleRefine.fx:108-253`, indirect off `valid_dispatch`). For each
  lit sample in the temporal ring (walked in the `shuffle_group` coprime order):
  `reproject_sample` reconstructs the old-frame virtual first-hit, reprojects
  its virtual surface into the current camera (`view_proj * vec4(pos,1)` — the
  `M*v` convention), screen-bucket-indexes it, runs the pdf-ratio + the
  `taa_dist_min_max` distance/specular-normal validity tests; on pass,
  `atomicAdd(bucket_info[i].x, 1<<6)` reserves a refined slot and — if there is
  space — reconstructs the secondary-bounce sample and packs a `refinedSample`
  `vec4<u32>` into `valid_samples_refined[bucket*32 + slot]`. The
  `getRayDir(camRotOld[...])` uses `camera_history[i].view_proj_inv` (the
  INVERSE ring — §3.6, see "Design ambiguities" #2).
- **`count_invalid_data`** ← `countInvalidData` (`renderSampleRefine.fx:255-338`,
  indirect off `invalid_dispatch`). The same `reproject_sample` for unlit
  samples; just `atomicAdd(bucket_info[i].x, 1<<18)` — no sample reconstructed
  or stored.
- **`refine_buckets`** ← `refineBuckets` (`renderSampleRefine.fx:340-417`). Per
  bucket: find the bucket's max compressed-colour level over its ≤32 refined
  samples (the HLSL function-`static uint compColorMaxStorage[32]` → a
  `var<function> array<u32,32>` local — bounded by `effectiveValidCount ≤ 32`),
  then for each refined sample remove weakly-lit ones with
  `COLOR_DIF_PROB[maxColorDif]` probability (the Batch-1 shared
  `color_compression.wgsl` table), compensate the survivors (the
  `darkeningOffset` distance-variance term), write ≤8 to
  `valid_samples_compressed`, and pack the bucket's lit/invalid ratio + count
  into `bucket_info[i].x`.

HLSL implicit-truncation class (the carry-forward): explicit `i32()` casts on
`int2 screenPosBucket = ndc01 * float2(...)` and `int3 newColorComp = max(0,
int3(...) + maxColorDif - darkeningOffset)`, explicit `u32()` on the
`surfacePosInt` frac term and the oct-encode coordinates, explicit `vec3<i32>`
broadcast where the HLSL adds a scalar `maxColorDif` to an `int3`. Every
`#ifdef ENTITIES` block (`renderSampleRefine.fx:141-153,215-227,285-296`)
omitted — Phase B is entity-free; `entityInstancesHistory` is not bound, the
`surfaceEntity` / `sampleEntity` branches dropped.

### `ray_queue_indirect[0]` reset moved in-shader; the B3 CPU seed deleted

Confirmed against `09-design-b.md` §7.3 + §4.2 and `renderSampleRefine.fx:36-40`:
**`clear_buckets_and_calc_mask` (the first sample-refine pass) owns the
per-frame `ray_queue_indirect[0]` reset.** It is the first pass of
`renderSampleRefine`, and NAADF's dispatch order (`WorldRenderBase.cs:272-273`
`ClearBucketsAndCalcMask`, then `:285` `RayQueue`) puts it **before**
`rayQueueCalc` — so `clear_buckets_and_calc_mask`'s lane-0
`ray_queue_indirect[0] = 0u` is the proper in-shader reset. Batch 4:

- `naadf_sample_refine_clear_node` is wired into the `Core3d` chain **between
  `naadf_first_hit_node` and `naadf_ray_queue_node`** (`render/mod.rs`), so the
  in-shader reset runs every frame before `calcRayQueue`'s `atomicAdd`.
- Batch 3's clearly-marked CPU re-seed in `prepare_gi`
  (`render_queue.write_buffer(&resources.ray_queue_indirect, ...)`) is
  **deleted**. The `gi.rs` comment block where it sat is rewritten to record
  the hand-off. The on-*creation* seed in `create_gi_buffers` stays (it seeds
  `[1]`/`[2]` = `GroupCountY/Z` = 1 once; `[0]` is then zeroed in-shader every
  frame).

### Cross-batch dependency — `taa_dist_min_max` empty until Batch 6

`09-design-b.md` §11 Batch 4 step 13 calls this out: `count_valid_data_and_refine`
/ `count_invalid_data` read `taa_dist_min_max` for the per-pixel reprojection
distance / specular-normal validity test, but the `base/` `ReprojectOld` that
*writes* `taa_dist_min_max` is not wired until Batch 6 (the A-2 `albedo/`
reproject pass does not write it). Batch 4 creates the **buffer**
(`TaaGpu.taa_dist_min_max`, zero-cleared) so `sample_refine_bind_group` can
reference it; until Batch 6, `dist_min == dist_max == 0` so the `distMinMax`
test (`dist_cur < dist_min_max.x * 1022/1024 || ...`) rejects every reprojected
sample. The 5 passes still dispatch clean — the refine buffers just stay empty.
Correct-but-empty, never invalid — exactly the designed-in cross-batch seam.

### Design ambiguities adjudicated

1. **`valid_dispatch` / `invalid_dispatch` — split into their own `@group(1)`,
   NOT in the shared sample-refine bind group.** `09-design-b.md` §8.2 lists
   `valid_dispatch` (rw) + `invalid_dispatch` (rw) in the single
   `sample_refine_bind_group` (13 bindings). The e2e harness surfaced the wgpu
   conflict: `count_valid_data_and_refine` binds that group (so the buffers are
   `STORAGE_READ_WRITE`) **and** `dispatch_workgroups_indirect`s off them
   (`INDIRECT`) — wgpu forbids both within one dispatch's usage scope
   ("`BufferUses(STORAGE_READ_WRITE)` is an exclusive usage"). Resolution: the
   two indirect-arg buffers move to a dedicated `@group(1)`
   (`sample_refine_dispatch_layout`) bound **only** by
   `naadf_sample_refine_valid_history_node` (the *only* pass that writes them —
   `renderSampleRefine.fx:99-100`); the count passes get the buffers purely as
   `dispatch_workgroups_indirect` sources (not a shader binding), so no usage
   conflict. The shared `@group(0)` drops 13→11 bindings. This is a faithful
   realisation of the design's intent ("the sample-refine stage needs these
   buffers") — the split is forced by the wgpu indirect-vs-storage exclusivity
   rule, which the design's "one shared bind group" wording did not account
   for. Documented in the WGSL + `pipelines.rs`.
2. **`camRotOld` = the INVERSE camera-history ring.** Followed `09-design-b.md`
   §3.6 exactly: `WorldRenderBase.cs:346` binds `taaSampleCamTransformInvers`
   (the inverse rotation-only view-proj) into `renderSampleRefine`'s `camRotOld`
   parameter, while `renderGlobalIllum` / `renderTaaSampleReverse` bind the
   non-inverse `taaSampleCamTransform`. So `reproject_sample` passes
   `camera_history[frame_index_old].view_proj_inv` to the shared `get_ray_dir`
   (which already takes an *inverse* view-proj — Batch 1 plumbed `view_proj_inv`
   onto the 160-byte `GpuCameraHistorySlot`).
3. **`sample_counts` declared plain (not atomic) in this module.**
   `naadf_global_illum.wgsl` declares the SAME `sample_counts` buffer as
   `array<SampleCountSlot>` with `atomic<u32>` members (it does per-thread
   `InterlockedAdd`s). `sample_refine` does only plain loads/stores on it
   (`computeValidHistory` reads/writes `[0..2]`, `clearBucketsAndCalcMask`
   stores `[3+accumIndex]` — no atomic adds). WGSL allows per-module
   binding-type views of one buffer, so `sample_refine.wgsl` declares it plain
   `array<vec2<u32>>` — simpler, and the byte layout is identical. Same for
   `ray_queue_indirect` (plain `array<u32,5>` here — `clearBucketsAndCalcMask`
   does a plain `.Store(0,0)`).

### Bug found + fixed during the e2e run

Two blocking issues, both surfaced in a single e2e run (the harness's
single-run all-errors property held), both fixed:

1. **`@builtin(group_id)` is not a WGSL builtin.** `count_valid_data_and_refine`
   / `count_invalid_data` ported HLSL `SV_GroupID` as `@builtin(group_id)` —
   naga: "unknown builtin: `group_id`". The WGSL builtin is `workgroup_id`.
   Fixed both entry points (5 sample-refine pipelines failed to compile;
   `naadf_sample_refine` never dispatched). Logging this as the next variant of
   the HLSL→WGSL semantic-mapping class.
2. **The `valid_dispatch` / `invalid_dispatch` usage conflict** — see "Design
   ambiguities" #1. Surfaced as a `Queue::submit` validation error
   ("conflicting usages ... `STORAGE_READ_WRITE` ... `INDIRECT`"); fixed by the
   `@group(1)` split.

### Verification

- `cargo build` — clean, **no new warnings** (the Batch-3 forward-looking
  dead-code warnings on `GiGpu.bucket_count` / `bucket_size` are now resolved —
  Batch 4's `naadf_sample_refine_*_node`s read `bucket_count` for their dispatch
  math).
- `cargo test` — **46 passed** (unchanged — Batch 4 added no unit tests; the 5
  passes are GPU-only working code, exercised by the e2e harness, and the
  existing `gi.rs` helper tests still cover `accum_index` / `bucket_grid` /
  `rand_salts`).
- `cargo run --bin e2e_render` — exits **0**, all gates green: luminance gate
  `batch 4 — 69.1% non-black; threshold 50%` (Batch 4 is still in the pre-GI
  regime — `GI_LIT_BATCH = 5`, so the B1–B4 0.50 floor applies, NOT the B5 0.60
  hard gate; 69.1% clears it with margin), then `PASS (batch 4) — … per-batch
  region gate green, every pipeline created cleanly, every expected
  render-graph node dispatched` (the `naadf_sample_refine` span is now in
  `expected_spans` and the node-dispatch check confirms it fired). **53
  pipelines created cleanly** (was 48 + the 5 new sample-refine pipelines).
  Used **3 of the 5** allotted `cargo run --bin e2e_render` invocations.
- **Visual assessment of `e2e_latest.png`:** the frame is **stable vs. Batch
  2/3 — no visible change, exactly as the design predicts.** The warm-white
  emissive block renders bright just above centre, the magenta block bright in
  the lower-left, the green block on the right; the dark diffuse voxel
  structure fills the mid/lower frame near-black (pre-GI, correct); the
  atmosphere-tinted sky band is clean across the top with no streaks or rings.
  Batch 4's 5 `sampleRefine` passes write `valid_samples_refined` /
  `valid_samples_compressed` / `bucket_info` — buffers the final blit does not
  read — so the GI refinement is not composited into the visible image until
  the denoiser path lands. The done-bar ("the 5 passes dispatch clean, image
  unchanged") is met.
- **e2e harness — `hash_baseline`:** the harness's "Remaining issue" suggested
  Batch 4 bless the first stability-hash baseline by pinning the Batch-3
  readback hash. On reflection that is not sensible to commit — the readback is
  only bit-identical run-to-run *on the same binary/GPU* (the harness's own
  §6.1 caveat), so a literal derived on this dev box would spuriously fail
  elsewhere. `hash_baseline(4)` is kept `None` (the deliberate-deferral path the
  harness doc allows); `assert_batch_4` re-runs the B2 emissive/solid/sky region
  gate as the primary "image unchanged" check, which catches gross regressions.
  The `gates.rs` comment records this reasoning.
- No TEMP debug instrumentation was added.

### Notes for the next batch (B5 — `spatialResampling` + `denoiseSplit`)

- **`valid_samples_compressed` + `bucket_info` are populated by Batch 4 and B5's
  `spatialResampling` is their first consumer** (`renderSpatialResampling.fx`
  `getSampleData` decodes `valid_samples_compressed`; the 12-iteration neighbour
  loop reads `bucket_info`). Both are `bucket_count`-sized, created by
  `prepare_gi`, resize-on-viewport. Note the cross-batch caveat below.
- **The refine buffers are *correct-but-empty* until Batch 6.** Batch 4's
  reprojection validity test rejects every sample because `taa_dist_min_max` is
  the zero-cleared buffer (Batch 6 wires `ReprojectOld`'s write). So in B5,
  `valid_samples_compressed` will decode as empty / all-zero until B6 lands —
  B5's `spatialResampling` "dispatches clean" on empty refine data exactly the
  way B4 dispatches clean on empty `taa_dist_min_max`. B5's done-bar of "GI
  bounce becomes visible" therefore *also* depends on B6 — re-read
  `09-design-b.md` §11 Batch 5 step 15: it temporarily sets
  `GiSettings.is_denoise = false` and relies on Batch-2's temporary
  `final_color` blit to show the spatial-resampling write. Whether the bounce is
  actually visible in B5 vs. only after B6's `taa_dist_min_max` lands is worth
  the B5 agent confirming against the design — the buffers being empty pre-B6 is
  a real constraint on B5's "GI visible" claim.
- **`TaaGpu.taa_dist_min_max` exists** (created/resized/zero-cleared by
  `prepare_taa` alongside `taa_samples`). Batch 6 only needs to add the
  `ReprojectOld` *shader write* + the reproject `@group(0)` rw binding — the
  buffer plumbing is done.
- **The mixed-bind-group pattern + the `@group(1)` indirect-split pattern are
  established.** `prepare_frame_gpu` builds `GiBindGroups` (now
  `ray_queue` + `global_illum` + `sample_refine` + `sample_refine_dispatch`).
  B5 adds `spatial_resampling_bind_group` + `denoise_bind_group` the same way.
  If any B5 pass both binds an indirect buffer rw AND dispatches indirect off
  it, the `@group(1)` split (Batch 4 "Design ambiguities" #1) is the pattern to
  reuse — but B5's `spatialResampling` / `denoiseSplit` are plain
  (non-indirect) `ceil(pixel_count/64)` dispatches (`WorldRenderBase.cs:397,
  412-416`), so this should not recur.
- **`SAMPLE_REFINE_SPAN` is one combined span for all 5 passes** — if B5/B6's
  HUD work wants per-pass timing it would need 5 distinct span consts; the
  design (§4.7) explicitly recommends the one-span form, kept.
- **B4 luminance regime:** `GI_LIT_BATCH = 5`, so B5 is the first batch the
  0.60 hard luminance gate applies to. The B5 agent must re-run the e2e harness
  and confirm the GI-lit frame clears 0.60 (per `e2e-render-test.md` — nudge
  `MIN_NON_BLACK_FRACTION_GI` to just below the measured value if it lands
  under, a real check not a rubber stamp). The current pre-GI frame is at
  69.1%, so there is headroom.

---

## Batch 5 — spatialResampling + denoiser (2026-05-14)

`09-design-b.md` §11 Batch 5 (steps 15–16). Builds the GI *consumers* — the
spatial-reuse stage of compressed ReSTIR GI (`renderSpatialResampling` —
Algorithm 2) and NAADF's sparse separable bilateral denoiser
(`renderDenoiseSplit`). These are the first passes that write `final_color` /
`denoise_preprocessed`; the render-graph chain becomes `… →
spatial_resampling → denoise → final_blit`. Ends ▶ runnable.

### The B5-vs-B6 "GI visible" milestone finding — **the visible bounce requires Batch 6**

The FIRST ACTION the brief required. `09-design-b.md` §11 Batch 5 step 15
claims "GI bounce lighting is visible for the first time" at end-of-B5. The
Batch-4 "note for B5" carry-forward flagged this as suspect ("B5's done-bar of
'GI bounce becomes visible' therefore *also* depends on B6"). **The Batch-5
verification settles it: the visible-bounce milestone genuinely moves to Batch
6.** The §11 evidence + the reasoning:

- **The reservoir path is empty until B6.** `renderSpatialResampling`'s
  12-iteration neighbour loop reads `valid_samples_compressed` + `bucket_info`
  — the `renderSampleRefine` refine buffers. Batch 4's "Cross-batch dependency"
  section + `09-design-b.md` §11 Batch 4 step 13 establish those are
  *correct-but-empty* until Batch 6: `renderSampleRefine`'s reprojection
  validity test rejects every sample because `taa_dist_min_max` is the
  zero-cleared buffer (`dist_min == dist_max == 0`), and `taa_dist_min_max` is
  not written until Batch 6 wires the `base/` `ReprojectOld`. So in B5 every
  bucket reads `bucket_valid_stored == 0` ⇒ the reservoir loop selects nothing
  ⇒ the resampled-GI term is zero.
- **The spatial pass's sun sample IS independent — and IS wired + dispatched —
  but contributes negligibly in this scene.** `renderSpatialResampling.fx:321-339`'s
  sun sample shoots a sun ray and adds `sunColor * weight` for a sun-facing
  unshadowed surface; it does not touch the refine buffers. It is ported
  faithfully and runs every frame. But in the enclosed e2e test scene at the
  fixed pose, its contribution to `final_color` is negligible — the visible
  diffuse geometry is largely sun-shadowed / sun-averted. **Hard evidence:** the
  e2e whole-frame non-black fraction stays bit-identical at **69.1%** through
  B5 (same as B2/B3/B4), and the screenshot is visually indistinguishable from
  Batch 4 (the dark diffuse voxel structure stays near-black; the
  `solid_block_rect` region measured luminance 4.1, vs. its pre-GI ~4).
- **Conclusion.** B5's image is stable like B3/B4 — the GI consumers run, the
  passes dispatch clean, `final_color` is written — but no visually significant
  bounce lands until Batch 6 populates the reservoir buffers via
  `taa_dist_min_max`. `09-design-b.md` step 15's "visible for the first time"
  claim did not trace the full `taa_dist_min_max → renderSampleRefine →
  valid_samples_compressed → renderSpatialResampling` dependency chain; the
  Batch-4 agent did, and flagged it; this batch confirms it. **Not a
  regression, not a bug — a pipeline-shape reality the design's step-15 prose
  understated.** Per the brief: the harness batch-tracker was kept honest (NOT
  flipped to a B5-visible regime the pipeline cannot yet satisfy; no faked
  gate) — see "How the e2e harness batch-tracker was set".

### Files created

| file | what it is | NAADF provenance |
|---|---|---|
| `src/assets/shaders/spatial_resampling.wgsl` | compressed-ReSTIR GI Algorithm 2 — the spatial resampling pass. `calc_spatial_resampling` compute entry + `sample_neighbors` (the 12-iteration weighted-reservoir loop + the adaptive-radius 12-tap pre-pass + the single 3-step mirror-following visibility ray + the sun sample) + `get_sample_data` (the HLSL out-param decode → a struct return) + `get_brdf` + `get_target_function_new`. Binds `@group(0)` world (it traverses) + `@group(1)` the spatial-specific buffer set. | `base/renderSpatialResampling.fx` (406 lines — `calcSpatialResampling` + `sampleNeighbors` + `getSampleData` + `getBRDF` + `getTargetFunctionNew`) |
| `src/assets/shaders/denoise_split.wgsl` | the sparse separable bilateral GI denoiser — `calc_denoise_horizontal` + `calc_denoise_vertical` as 2 compute entry points in one naga-oil module. Kernel 21 (`±10`), σ = 10, with the random sparse per-row/-column offset ("on average every 2nd pixel"); the bilateral weight folds a Gaussian falloff × a TAA-weight-difference term × a normal/material-state match term. Both passes share `@group(0)` = `denoise_bind_group`; the denoiser does not traverse the voxel world. | `base/renderDenoiseSplit.fx` (146 lines — `calcDenoiseHorizontal` + `calcDenoiseVertical`) |

### Files changed

| file | change |
|---|---|
| `src/render/pipelines.rs` | `NaadfPipelines` += `spatial_resampling_layout` (`@group(1)`, 8 bindings: `gi_params` uniform + `first_hit_data`/`first_hit_absorption`/`bucket_info`/`valid_samples_compressed`/`taa_sample_accum` RO + `final_color`/`denoise_preprocessed` RW) + `denoise_layout` (`@group(0)`, 5 bindings: `gi_params` uniform + `first_hit_absorption`/`denoise_preprocessed` RO + `denoise_preprocessed_horizontal`/`final_color` RW) + `spatial_resampling_pipeline` (layout `[world, spatial_resampling]` — it traverses) + `denoise_horizontal_pipeline` + `denoise_vertical_pipeline` (both bind only `denoise_layout`); new `SPATIAL_RESAMPLING_SHADER` / `DENOISE_SHADER` path consts. |
| `src/render/gi.rs` | `GiBindGroups` += `spatial_resampling_bind_group` + `denoise_bind_group` (the doc comment's "Batches 5-6 add the rest" updated — Batch 5 lands these two, Batch 6 the last). |
| `src/render/prepare.rs` | `prepare_frame_gpu` builds the two new mixed bind groups into `GiBindGroups` (alongside the Batch-3/4 groups, same `pixel_count` resize trigger). `spatial_resampling_bind_group` mixes `GiGpu` + `FrameGpu` (`first_hit_data` / `first_hit_absorption` / `final_color`) + `TaaGpu` (`taa_sample_accum`); `denoise_bind_group` mixes `GiGpu` + `FrameGpu` (`first_hit_absorption` / `final_color`). |
| `src/render/graph_b.rs` | += `SPATIAL_RESAMPLING_SPAN` + `DENOISE_SPAN` (one combined HUD/dispatch-check span for the two `renderDenoiseSplit` passes — same one-span convention as `SAMPLE_REFINE_SPAN`) + the 2 node systems `naadf_spatial_resampling_node` (1 dispatch, `ceil(pixel_count/64)` workgroups, binds `@group(0)` world + `@group(1)` spatial) + `naadf_denoise_node` (2 dispatches — horizontal then vertical, each `ceil(pixel_count/64)` — **gated on `ExtractedGiConfig.is_denoise`**, mirroring A-2's `naadf_taa_reproject_node` gate on `ExtractedTaaConfig.enabled`). |
| `src/render/mod.rs` | the 2 nodes imported + wired into the `Core3d` `.chain()` at their §4.2 positions: `naadf_spatial_resampling_node` + `naadf_denoise_node` inserted **after `naadf_sample_refine_buckets_node`, before `naadf_final_blit_node`** (they consume `valid_samples_compressed` / `bucket_info` and produce `final_color` / `denoise_preprocessed`). |
| `src/e2e/gates.rs` | `CURRENT_BATCH` 4 → 5; `assert_batch_5` added (re-runs the B2 emissive/solid/sky region gate — see the milestone finding: B5's image is unchanged — plus the optional `hash_baseline(5)` tripwire, kept `None`); `expected_spans` / `batch_gate` gain their B5 arms (`expected_spans` adds `naadf_spatial_resampling` + `naadf_denoise`). `CURRENT_BATCH` / `hash_baseline` doc comments record the B5-vs-B6 finding. |
| `src/e2e/framebuffer.rs` | **`GI_LIT_BATCH` 5 → 6** — the central honest-batch-tracker change (see "How the e2e harness batch-tracker was set"). The `MIN_NON_BLACK_FRACTION_GI` / `MIN_NON_BLACK_FRACTION_PRE_GI` / `min_non_black_fraction` / `GI_LIT_BATCH` doc comments updated to record that the visible-bounce milestone moved to Batch 6 and that B1-5 use the pre-GI floor. |

### Mapping to NAADF source — `renderSpatialResampling.fx` + `renderDenoiseSplit.fx`

- **`spatial_resampling.wgsl`** is a faithful function-by-function port of
  `base/renderSpatialResampling.fx`:
  - `calc_spatial_resampling` ← `calcSpatialResampling` (`:344-399`): `get_ray_dir`
    using `gi_params.inv_view_proj` (the HLSL binds `invCamMatrix` — the inverse
    view-proj — into `getRayDir`'s `camTransform`; the shared `get_ray_dir`
    already takes an inverse view-proj. Note this is the *current-frame* inverse
    view-proj, NOT `renderSampleRefine`'s per-frame-history inverse ring — a
    different thing, §3.6). `get_hit_data_from_planes` (the shared full version,
    entity params dropped). The `isDenoise` write split: denoise path writes the
    TRANSPOSED `denoise_preprocessed[pixelPos.y + pixelPos.x * screenHeight]`
    (the denoiser reads it column-major); non-denoise path composites
    `final_color += absorption * color`.
  - `sample_neighbors` ← `sampleNeighbors` (`:56-342`): the `isVaryingResmaplingRadius`
    12-tap adaptive-radius pre-pass (`:81-148`), the 12-iteration reservoir loop
    (`:153-263` — the normal-mask / distance / pdf-ratio / Jacobian gates + the
    WRS update), the single 3-step mirror-following visibility ray (`:266-302` —
    `shoot_ray` with `MAX_RAY_STEPS_VISIBILITY`), the resampled-colour resolve
    (`:304-319`), and the independent sun sample (`:321-339` — `shoot_ray` with
    `MAX_RAY_STEPS_SUN`).
  - `get_sample_data` ← `getSampleData` (`:29-38`), HLSL out-params → a
    `SampleData` struct return (the A-2 `decompressSample` pattern).
  - `get_brdf` / `get_target_function_new` ← `getBRDF` / `getTargetFunctionNew`
    (`:40-54`).
- **`denoise_split.wgsl`** is a faithful port of `base/renderDenoiseSplit.fx`:
  `calc_denoise_horizontal` ← `calcDenoiseHorizontal` (`:15-73` — the transposed
  read index, the random sparse x-offset, the bilateral weight, the row-major
  write into `denoise_preprocessed_horizontal`); `calc_denoise_vertical` ←
  `calcDenoiseVertical` (`:75-132` — reads the horizontal scratch + the original
  transposed colour, `lerp(colorOrig, color, 0.92)`, `*= absorption`, ADDS into
  `final_color`). The transposed indexing is ported exactly (`:18,46,72,81-82,106`).
- HLSL implicit-truncation class (the carry-forward): explicit `i32()` /
  `u32()` casts on the `vec2<f32>(pixelPos) + xy` neighbour indices, the
  `f32(bucketValidStored) * nextRand` random sample index, the screen-bucket
  indices; explicit `vec3<f32>` constructors where the HLSL broadcasts a scalar
  (`float3 weight = 2.0f * sunDirCosTheta`). Every HLSL `mul(v, M)` is the
  column-vector `M * v`. The `Uint3` `denoisePreprocessed` /
  `denoisePreprocessedHorizontal` are the `vec4<u32>`-padded buffers (`§3.3`) —
  read `.xyz`, write `.w = 0`. Every `#ifdef ENTITIES` is omitted (the
  `getHitDataFromPlanes` entity params are absent — the shared full version
  already drops them).

### Design ambiguities adjudicated

1. **`is_denoise` — kept `true` (the `GiSettings` default), not toggled per
   step.** `09-design-b.md` §11 Batch 5 step 15 says "temporarily set
   `GiSettings.is_denoise = false`" to run the non-denoise spatial-resampling
   path standalone, then step 16 restores it to `true`. This batch implements
   *both* step 15 and step 16 (`spatial_resampling.wgsl` + `denoise_split.wgsl`
   land together — the denoiser is small, 146 HLSL lines, and §12 / the §11
   Batch sequencing rationale explicitly allow one batch for both), so the
   end-of-batch state is `is_denoise = true` — already the `GiSettings::default()`
   value (`gi.rs` Batch-3 defaults). No `GiSettings` / `main.rs` change. The
   `is_denoise` flag is read by `spatial_resampling.wgsl` (the write split) +
   gated by `naadf_denoise_node`; with the default `true` the denoise path runs
   end-to-end. The step-15 "temporarily false" is a within-batch development
   aid, not an end-state — followed §11's Batch 5 *whole* (steps 15+16).
2. **`spatialResampling`'s `getRayDir` matrix — the current-frame inverse
   view-proj, NOT the camera-history inverse ring.** `09-design-b.md` §3.6
   distinguishes `renderGlobalIllum` / `renderTaaSampleReverse` (bind the
   non-inverse rotation-only ring) from `renderSampleRefine` (binds the *inverse*
   ring). `renderSpatialResampling.fx:17,351` binds `invCamMatrix` — the
   *current-frame* inverse view-proj — into `getRayDir`; it does not index the
   camera-history ring at all (`sampleNeighbors` reconstructs neighbour samples
   from `valid_samples_compressed`'s packed positions, not via reprojection).
   So `spatial_resampling.wgsl` passes `gi_params.inv_view_proj` to the shared
   `get_ray_dir` — and binds no `camera_history`. Followed the HLSL.
3. **`spatialVisibilityCount` — dropped (dead uniform).** `09-design-b.md` §8.3
   notes the HLSL declares `spatialVisibilityCount` but `sampleNeighbors`
   actually passes the `MAX_RAY_STEPS_VISIBILITY` const to `shootRay` at
   `renderSpatialResampling.fx:274`. The §8.3 recommendation is "drop it — it is
   a dead uniform". `GpuGiParams` (Batch 1, `gpu_types.rs` / `gi_params.wgsl`)
   never declared a `spatial_visibility_count` field, so there was nothing to
   drop — the WGSL uses `MAX_RAY_STEPS_VISIBILITY` (the `ray_tracing.wgsl`
   const = 60) directly, faithful to the HLSL behaviour.

### How the e2e harness batch-tracker was set — and why

The brief required engaging the correct luminance regime per the B5-vs-B6
finding, *honestly*. Two batch-tracker constants were touched:

- **`CURRENT_BATCH` 4 → 5** (`gates.rs`) — B5 IS implemented, so the harness's
  `ASSERT` step must run B5's region gate + check B5's render-graph spans. This
  is the straightforward "a batch landed" bump.
- **`GI_LIT_BATCH` 5 → 6** (`framebuffer.rs`) — **this is the honest-tracker
  change.** `GI_LIT_BATCH` is the batch from which the harness applies the 0.60
  hard luminance gate (`MIN_NON_BLACK_FRACTION_GI`) instead of the 0.50 pre-GI
  floor. It was `5` on the design's assumption that the GI bounce becomes
  visible at end-of-B5. The B5-vs-B6 finding shows it does not — B5's frame is
  bit-identical to B4 (69.1% non-black, screenshot indistinguishable). Leaving
  `GI_LIT_BATCH = 5` would make the harness apply the 0.60 hard gate to a frame
  the B5 pipeline produces *for pre-GI reasons* — it would have *passed* (69.1%
  ≥ 60%), but that would have been a gate the B5 pipeline-shape does not yet
  *earn*: the brief explicitly forbids "flip[ping] it to a B5-visible regime
  that the pipeline can't yet satisfy". Setting `GI_LIT_BATCH = 6` keeps B5 on
  the pre-GI floor — the honest regime — and makes Batch 6 (which wires
  `taa_dist_min_max` and lands the real visible bounce) the first batch the
  0.60 hard gate applies to. The first e2e run (with `GI_LIT_BATCH` still 5 and
  a too-optimistic `assert_batch_5` that expected the `solid_block_rect` to
  brighten) FAILED on exactly that region check — confirming the finding — and
  was corrected to the honest `GI_LIT_BATCH = 6` + the `assert_batch_2`-rerun
  `assert_batch_5` (the same pattern `assert_batch_4` uses for an
  "image-unchanged" batch). `MIN_NON_BLACK_FRACTION_GI` stays `0.60` — the
  Batch-6 agent re-confirms it against the actual GI-lit fraction and nudges it
  to just below the measured value (the `e2e-render-test.md` rule), now that B6
  is the batch that actually exercises it.

### Verification

- `cargo build` — clean, no new warnings.
- `cargo test` — **46 passed** (4 suites), unchanged from Batch 4. Batch 5
  added no unit tests: `spatial_resampling.wgsl` / `denoise_split.wgsl` are
  GPU-only working code (exercised by the e2e harness), and there is no new
  CPU-side logic to unit-test (the bind-group / pipeline / node wiring is
  covered by the e2e `PipelineCache` scan + node-dispatch check).
- `cargo run --bin e2e_render` — exits **0**, all gates green: luminance gate
  `batch 5 — 69.1% non-black; threshold 50%` (the pre-GI floor — `GI_LIT_BATCH
  = 6`, the honest regime, since B5's frame is pre-GI-like), then `PASS (batch
  5) — 8 render frames, framebuffer read back & non-degenerate, per-batch
  region gate green, every pipeline created cleanly, every expected
  render-graph node dispatched.` The 3 new pipelines (`spatial_resampling`,
  `denoise_horizontal`, `denoise_vertical`) compiled cleanly — the
  `PipelineCache::Err` scan would have surfaced any naga / WGSL / bind-group /
  pipeline error in a single run — and the `naadf_spatial_resampling` +
  `naadf_denoise` spans were both recorded (the node-dispatch check confirms
  the two new nodes fired every frame). Used **2 of the 5** allotted `cargo run
  --bin e2e_render` invocations (run 1 caught the too-optimistic
  `assert_batch_5` — see "How the e2e harness batch-tracker was set" — run 2
  green after the honest-regime correction).
- **Visual assessment of `e2e_latest.png`:** the frame is **stable vs. Batch
  2/3/4 — no visible change, exactly as the B5-vs-B6 finding predicts.** The
  five emissive blocks render bright/coloured (warm-white centre, magenta
  lower-left, green right; amber + cool-white higher/partly occluded), the
  atmosphere-tinted sky band sits clean across the top with no streaks or
  rings, and the dark diffuse voxel structure (towers, boxes, pillars, spheres)
  fills the mid/lower frame near-black. Batch 5's `spatialResampling` +
  `denoiseSplit` passes run and write `final_color` / `denoise_preprocessed`
  every frame — the GI consumers are wired and dispatching — but their pre-B6
  contribution is negligible (the reservoir loop reads the empty refine
  buffers; the independent sun sample contributes nothing visible in this
  enclosed scene/pose). The done-bar — "the GI consumer passes dispatch clean"
  — is met; the visible bounce lands at Batch 6.
- No TEMP debug instrumentation was added; no cross-frame buffer-readback
  instrumentation was used (the finding was reasoned from the NAADF source's
  `taa_dist_min_max → renderSampleRefine → valid_samples_compressed →
  renderSpatialResampling` dependency chain + the bit-identical 69.1%
  whole-frame fraction, not from buffer probes).

### Notes for the next batch (B6 — the `base/` TAA rewire + renderFinal + integration)

- **B6 lands the visible GI bounce.** This is the central B5-vs-B6 finding:
  B6's step 17 wires the `base/` `ReprojectOld` to write `taa_dist_min_max`,
  which un-blocks Batch 4's `renderSampleRefine` reprojection validity test ⇒
  `valid_samples_compressed` + `bucket_info` populate ⇒ B5's
  `renderSpatialResampling` 12-iteration reservoir loop finally yields output ⇒
  the full indirect GI bounce composites into `final_color`. B6 is therefore
  the batch where the e2e image *changes* — `GI_LIT_BATCH = 6` is set so the
  0.60 hard luminance gate applies from B6, and `assert_batch_6` should be the
  first region gate that asserts the previously-near-black diffuse geometry has
  *brightened* (the positive "GI bounce is visible" check that
  `assert_batch_5`'s first draft wrongly expected at B5). The B6 agent must
  re-confirm `MIN_NON_BLACK_FRACTION_GI = 0.60` against the actual measured
  GI-lit fraction and nudge it per the `e2e-render-test.md` rule.
- **`spatial_resampling.wgsl` reads `taa_sample_accum` on the denoise path**
  (`renderSpatialResampling.fx:371` — the `curTaaColor` term, packed into
  `denoise_preprocessed.y` high half-word as the denoiser's TAA-weight signal).
  `taa_sample_accum` is the zero-cleared `TaaGpu` buffer until B6's
  `ReprojectOld` + `CalcNewTaaSample` write it — so pre-B6 the denoiser's
  TAA-weight bilateral term degrades to a constant (every `cur_taa_weight` is
  the same), which is harmless (the bilateral weight just loses one of its
  three discriminators). B6 wiring `taa_sample_accum` makes the denoiser's
  TAA-weight term real — another B6 quality improvement, like the
  `taa_dist_min_max` one.
- **The mixed-bind-group pattern is fully established.** `prepare_frame_gpu`
  now builds all six `GiBindGroups` entries (`ray_queue` + `global_illum` +
  `sample_refine` + `sample_refine_dispatch` + `spatial_resampling` +
  `denoise`). B6 adds the last — `calc_new_taa_sample_bind_group` — the same
  way (it mixes `TaaGpu` + `FrameGpu` + `WorldGpu`'s `voxel_types` — see
  `09-design-b.md` §4.10 / §10.3, designer's-call build location is
  `prepare_frame_gpu`).
- **The temporary `final_color` blit is still in place** (Batch 2's seam). B6
  step 19 reverts it: `naadf_final_blit_node` reads `taa_sample_accum` again
  (correctly filled by `ReprojectOld` + `CalcNewTaaSample`), and
  `FLAG_BLIT_FINAL_COLOR` is cleared. Until then, B5's GI consumers writing
  `final_color` is exactly what makes the bounce *show* once B6 fills the
  reservoir buffers — the seam is load-bearing for the B6 reveal, then reverted.
- **`naadf_denoise_node` is gated on `ExtractedGiConfig.is_denoise`** (the A-2
  `naadf_taa_reproject_node` gate pattern). With the `GiSettings` default
  `is_denoise = true` it always dispatches; if B6 or a later toggle wants the
  non-denoise path, `spatial_resampling.wgsl`'s non-denoise branch already
  writes `final_color` directly (`renderSpatialResampling.fx:391-398`), so the
  node early-returning is correct — no extra wiring needed.
- **`DENOISE_SPAN` is one combined span for both `renderDenoiseSplit` passes**
  (same one-span convention as `SAMPLE_REFINE_SPAN`). B6's HUD work (step 20)
  adds the timing lines for the expensive nodes — `naadf_spatial_resampling` +
  `naadf_denoise` are both in `expected_spans` and recorded; the HUD's
  `write_timing` + `const`-checked path-pair pattern extends to them directly.

---

## Batch 6 — base/ TAA rewire + final blit + integration (2026-05-14)

`09-design-b.md` §11 Batch 6 (steps 17–20) — the FINAL batch. Wires the `base/`
long-term-memory TAA path (`ReprojectOld` writing `taa_dist_min_max` +
`CalcNewTaaSample`), `taa_dist_min_max` into the bind groups + the chain,
reverts Batch-2's temporary `final_color` blit seam, ports the `base/`
`renderFinal` (`tone_mapping_fac`), re-adds both TAA nodes to the `Core3d`
chain, and lands the HUD + e2e gate work. After Batch 6 the full NAADF
`WorldRenderBase` real-time GI pipeline — 13 render-graph nodes — is wired.

### Files changed

| file | change |
|---|---|
| `src/assets/shaders/taa.wgsl` | **REWIRED to the `base/` variant** (was the A-2 `albedo/` port). `reproject_old_samples`: gains the `taa_dist_min_max` `@group(0) @binding(5)` rw binding + the write (`base/renderTaaSampleReverse.fx:79` — `f16(distMin) \| f16(distMax)<<16`, `valid_normals_spec`); un-omits the `valid_normals_spec` accumulation (`:68-70`, `get_specular_normals` is real now — the `base/` 4-plane first-hit populates planes 1-3); changes the `screenPosDistanceSqr` reject `> 1.0` → `> 16.0` (the `base/` value — item #2); changes the accum write from A-2's read-add-write to the `base/` **OVERWRITE** with `colorSum` (the `base/` first-hit writes `final_color`, not `taa_sample_accum`, so `ReprojectOld` overwrites). NEW `calc_new_taa_sample` entry point (`base/renderTaaSampleReverse.fx:170-206`) on `@group(1)` — reconstructs the first-hit virtual path, decompresses the voxel type for roughness, reads `final_color` as the current GI light, `taa_compress_sample`s it into the 16-deep `taa_samples` ring (`% TAA_SAMPLE_RING_DEPTH`), folds the light into `taa_sample_accum` with `sample_weight + 1`. |
| `src/assets/shaders/naadf_final.wgsl` | **REPLACED in place** with the `base/renderFinal.fx` `MainPS` behaviour: the blit source is `taa_sample_accum` again (the Batch-2 temporary `final_color` seam is reverted; the `FLAG_BLIT_FINAL_COLOR` decode branch is removed); the tonemap denominator is `params.tone_mapping_fac` (`base/:55`) instead of the A-2 hardcoded `1.0`; the `showRayStep` debug reads `first_hit_data[pixelIndex].z & 0x7FFF` (`base/:44`) instead of `col_samples.x`. |
| `src/render/pipelines.rs` | `taa_reproject_layout` += `taa_dist_min_max` rw binding (slot 5). NEW `calc_new_taa_sample_layout` (`@group(1)`, 6 bindings: `taa_params` uniform + `first_hit_data`/`final_color`/`voxel_types` RO + `taa_samples`/`taa_sample_accum` RW). NEW `calc_new_taa_sample_pipeline` — `taa.wgsl` entry `calc_new_taa_sample`, layout `[empty_layout, calc_new_taa_sample_layout]` (the shader places its bindings on `@group(1)` so they do not collide with `reproject_old_samples`'s `@group(0)` in the shared naga-oil module — the same `@group`-placeholder pattern `naadf_global_illum.wgsl` uses). |
| `src/render/prepare.rs` | `FrameGpu` += `calc_new_taa_sample_bind_group`. `prepare_frame_gpu` gains `Option<Res<WorldGpu>>` (the `calc_new_taa_sample` bind group needs `voxel_types`) — waited-for like the other three render-world resources. `taa_reproject_bind_group` += `taa_dist_min_max` (slot 5). NEW `calc_new_taa_sample_bind_group` built (mixes `TaaGpu` + `FrameGpu` + `WorldGpu`). The blit bind group's slot 1 reverts to `taa_gpu.taa_sample_accum` (was the temporary `final_color`). `flags` no longer sets `FLAG_BLIT_FINAL_COLOR`. |
| `src/render/graph.rs` | `naadf_taa_reproject_node` — `#[allow(dead_code)]` removed, doc updated to the `base/` variant. NEW `naadf_calc_new_taa_sample_node` + `CALC_NEW_TAA_SAMPLE_SPAN` — binds `[empty_bind_group, calc_new_taa_sample_bind_group]`, `ceil(pixel_count/64)` workgroups, gated on `ExtractedTaaConfig.enabled`. `naadf_final_blit_node` doc updated to the `base/renderFinal` variant. |
| `src/render/mod.rs` | the `Core3d` `.chain()` gains `naadf_taa_reproject_node` (after `naadf_first_hit_node`, before `naadf_sample_refine_clear_node`) + `naadf_calc_new_taa_sample_node` (after `naadf_denoise_node`, before `naadf_final_blit_node`) — both at their `09-design-b.md` §4.2 positions. 13-node chain. |
| `src/hud.rs` | renderer-mode string → `"Renderer: NAADF (Phase B — real-time GI)"`. Timing lines for the expensive Phase-B nodes (`09-design-b.md` §4.12): `atmosphere`, `first-hit`, `taa-reproject`, `global-illum`, `sample-refine`, `spatial-resmpl`, `denoise`, `final-blit`. The `const`-checked `matches_span` pairs extended to the 5 new `graph_b` spans. |
| `src/e2e/gates.rs` | `CURRENT_BATCH` 5 → 6. NEW `assert_batch_6` — the first region gate that asserts the GI bounce is VISIBLE (the dark diffuse `solid_block_rect` has *brightened* past `MIN_GI_BOUNCE_LUMINANCE`) + re-runs the emissive/sky checks. `expected_spans` / `batch_gate` gain their B6 arms (`expected_spans` adds `naadf_taa_reproject` + `naadf_calc_new_taa_sample`). |
| `README.md` | the roadmap's Phase A-2 + Phase B entries marked ✅ with the full Phase-B node inventory. |

### Mapping to NAADF source

- **`reproject_old_samples` (the `base/` `ReprojectOld` rewire)** ←
  `base/renderTaaSampleReverse.fx:25-168`. The `taa_dist_min_max` write
  (`:79`), the `validNormalsSpec` 3-field accumulation (`:68-70`), the
  `screenPosDistanceSqr > 16.0f` reject (`:139`), and the `taaSampleAccum`
  OVERWRITE (`:167` — `uint2(f16(colorSum.w) | f16(colorSum.r)<<16, f16(colorSum.g)
  | f16(colorSum.b)<<16)`). The A-2 `albedo/` variant had none of these — it
  had no `taaDistMinMax` output, folded `validNormalsSpec` to a no-op, used
  `> 1.0`, and read-added-wrote `taaSampleAccum` (because the `albedo/`
  first-hit pre-writes the current sample into `taaSampleAccum`; the `base/`
  first-hit writes `finalColor` instead, so `ReprojectOld` overwrites).
- **`calc_new_taa_sample`** ← `base/renderTaaSampleReverse.fx:170-206`
  (`calcNewTaaSample`) verbatim: `getRayDir` (no jitter), `getHitDataFromPlanes`
  (the shared full version), `decompressVoxelType(voxelTypeData[voxelType])`,
  the `final_color`→`light` read (`:187-188`), the `extra_data8` 5-bit roughness
  (`:189-192`), the `compressSample` into `taaSamples[(taaIndex % 32) ...]` →
  `% TAA_SAMPLE_RING_DEPTH` (the §6 16-deep ring), and the `taaSampleAccum`
  fold with `sampleWeight + 1` (`:197-205`). The HLSL `compressSample` takes
  the f16 *bits* for `dist`; the A-2 `taa_compress_sample` helper takes a float
  and does the `f32tof16` itself, so the float distance is passed (`65520.0`
  for a miss, else the decoded `firstHit.w & 0x7FFF`).
- **`naadf_final.wgsl` (the `base/renderFinal` revert)** ← `base/renderFinal.fx`
  `MainPS` — `taaSampleAccum` blit source (`:38-40`), `toneMappingFac` tonemap
  denominator (`:55`), `showRayStep` reading `firstHitData[pixelIndex].z &
  0x7FFF` (`:44`). The Batch-2 temporary `final_color` seam is reverted exactly
  as `09-design-b.md` §11 Batch 6 step 19 / the Batch-2 "the `taa_samples`
  seam" note specify.
- **`taa_dist_min_max` wiring** — the buffer + bind-group plumbing landed in
  Batch 4 (`prepare_taa` creates/resizes/zero-clears it; the `sample_refine`
  bind group references it). Batch 6 adds the missing piece: the `ReprojectOld`
  *shader write* + the reproject `@group(0)` rw binding + the
  `taa_reproject_bind_group` entry. Per the Batch-5 "note for B6", this is the
  wiring that un-blocks the `renderSampleRefine` reprojection validity test ⇒
  `valid_samples_compressed` + `bucket_info` populate ⇒ the
  `renderSpatialResampling` reservoir loop carries real data.

### Item #2 (the `screenPosDistanceSqr` threshold) — applied

`16.0` is used in the Batch-6 `base/` `reproject_old_samples`. The Batch-2
impl-log finding established that NAADF's `albedo/` source genuinely uses
`> 1.0` and the `base/` source genuinely uses `> 16.0` — a real per-variant
divergence, no A-2 bug, no `08-review-a2.md` erratum. Batch 6 ports the
`base/` value faithfully (`taa.wgsl` `reproject_old_samples`, the screen-position
reject), and the file-header deviations note records it.

### Item #7 (GI settings as constants) — applied / confirmed

No GI-settings GUI was added. The `WorldRenderBase` settings ship as fixed
`AppArgs`/`GiSettings` constants (landed in Batch 3) + the A-2-style
`TAA_SAMPLE_AGE` / `tone_mapping_fac` constants. Batch 6 added one more
constant in this spirit: `tone_mapping_fac` is set to `1.0` in
`prepare_frame_gpu` (C# `Settings.data.general.toneMappingFac`), exactly as A-2
handled `taaSampleMaxAge` — a compile-time constant, not a runtime knob.

### §6.3 authoritative shape (not the stale §4.4/§5.1 variant) — confirmed

Batch 6 followed the authoritative `09-design-b.md` §6.3 + §11 shape, not the
stale §4.4/§5.1 `@group(3)` "keeps the ring write" variant. Concretely: the
first-hit pipeline's bind-group layout stays `[world, frame, atmosphere]` (the
`taa_samples` `@group(2)` group was *removed* in Batch 2, atmosphere at
`@group(2)`); Batch 6 does NOT resurrect a first-hit `taa_samples` binding. The
`taa_samples` ring write lives solely in `calc_new_taa_sample` (§6.3 — "the
`@group(2)` `taa_layout` moves off the first-hit pipeline onto the
`calc_new_taa_sample` pipeline"). The dormant `taa_layout` descriptor +
`TaaGpu.taa_first_hit_bind_group` field (kept since Batch 2) are now superseded
by `calc_new_taa_sample_layout` / `FrameGpu.calc_new_taa_sample_bind_group` —
they remain in the tree as harmless dead plumbing (a follow-up cleanup could
drop them; not load-bearing, not Batch-6 scope to churn). The
`FLAG_BLIT_FINAL_COLOR` flag const (gpu_types.rs + render_pipeline_common.wgsl)
is likewise now dormant — the temporary seam it gated is reverted; left defined
(it is `pub`, no dead-code warning) consistent with how Batch 2's other dormant
plumbing was kept.

### e2e batch-tracker state + the `assert_batch_6` gate

- `CURRENT_BATCH` 5 → 6. `GI_LIT_BATCH` was already `6` (set by Batch 5's
  honest-tracker change) — so the **0.60 hard luminance gate
  (`MIN_NON_BLACK_FRACTION_GI`)** now engages for the first time, exactly as
  the Batch-5 "note for B6" intended.
- `assert_batch_6` (`gates.rs`) — the first region gate that asserts the GI
  bounce is *visible*: (1) the emissive blocks + atmosphere sky must still
  render (the emissive/sky portions of the Batch-2 gate — NOT the full
  `assert_batch_2`, whose `solid_block_rect < 90` "near-black" check is exactly
  what Batch 6 inverts), and (2) the positive check — the dark diffuse
  `solid_block_rect` region (luminance ~4 near-black through Batch 5) must have
  *brightened* past `MIN_GI_BOUNCE_LUMINANCE`. `expected_spans(6)` adds
  `naadf_taa_reproject` + `naadf_calc_new_taa_sample` (the full 10-span Phase-B
  node set; `sample_refine` + `denoise` each remain one combined span).
- **`MIN_NON_BLACK_FRACTION_GI` / `MIN_GI_BOUNCE_LUMINANCE` were NOT
  re-confirmed against a measured GI-lit value** — see the verification
  section: the e2e run did not reach a GI-lit frame. The `0.60` and `12.0`
  thresholds are the design-intent placeholders; they must be re-measured and
  nudged once the downstream defect below is fixed.

### Verification

- `cargo build --bin bevy-naadf` + `cargo build --bin e2e_render` — both clean,
  no warnings.
- `cargo test` — **46 passed** (4 suites), unchanged from Batch 5. Batch 6
  added no unit tests: the changes are WGSL shader logic + bind-group / pipeline
  / node wiring (covered by the e2e `PipelineCache` scan + node-dispatch check)
  + the e2e gate itself; there is no new CPU-side pure logic to unit-test.
- `cargo run --bin e2e_render` — **FAILS: the frame is uniformly black** (0.0%
  non-black; the degenerate-frame floor, the 0.60 luminance gate, and
  `assert_batch_6` all trip). Exit code 0 (the harness reports failures
  textually). 5 e2e invocations used (the hard cap) — see the diagnosis below.
- The TEMP instrumentation added during diagnosis (eprintln gates in
  `graph.rs` / `prepare.rs`, one diagnostic write in `taa.wgsl`
  `calc_new_taa_sample`) has been **fully reverted** — `grep -rn TEMP src/`
  is clean.

### Diagnosis — Batch 6's own wiring is correct; a latent Batch-4/5 GI-consumer defect is exposed

The e2e frame is uniformly `[0,0,0,255]`. Diagnosis from 5 e2e invocations +
static analysis (no cross-frame buffer-readback instrumentation was used):

1. **No pipeline / shader / device error.** The `PipelineCache::Err` scan is
   clean; `RUST_LOG=wgpu=warn` shows no validation errors. Every new pipeline
   (`calc_new_taa_sample`) and every changed shader (`taa.wgsl`,
   `naadf_final.wgsl`) compiles.
2. **Every node runs.** TEMP eprintlns confirmed `prepare_frame_gpu` builds
   `FrameGpu` every frame (all four render-world resources present, incl. the
   new `WorldGpu` wait), and `naadf_first_hit_node` / `naadf_taa_reproject_node`
   / `naadf_calc_new_taa_sample_node` all DISPATCH every frame.
3. **The blit reads `taa_sample_accum`; `taa_sample_accum` reads as zero.** A
   TEMP probe made `calc_new_taa_sample` write `final_color`'s `light` straight
   into `taa_sample_accum` (weight 1, bypassing the reproject fold) — the frame
   was *still* black. Since `calc_new_taa_sample` dispatches and its write
   definitely lands, this proves **`final_color` is zero at the point
   `calc_new_taa_sample` reads it** (the blit-read path + the
   `calc_new_taa_sample`-write path are otherwise sound — same buffer,
   `.chain()`-ordered, wgpu auto-barriers, the exact pattern Batch 5's
   `final_color` blit used).
4. **`final_color` was non-zero through Batch 5.** Batch 2's temporary
   `final_color` blit measured a bit-identical 69.1% non-black through
   B2–B5 — `first_hit` writes `final_color = primary light` and that content
   was verified. Nothing in the B6 chain writes `final_color` between the
   denoiser and `calc_new_taa_sample` / the blit.
5. **The only thing newly active in the B6 chain is the GI consumer data
   flow.** B6's `taa_dist_min_max` write un-blocks `renderSampleRefine`'s
   reprojection validity test — so for the first time `valid_samples_compressed`
   / `bucket_info` carry real data and `renderSpatialResampling`'s 12-iteration
   reservoir loop yields output, which `renderDenoiseSplit` then composites
   into `final_color`. Through B5 those buffers were *correct-but-empty*
   (`taa_dist_min_max` was zero-cleared), so the B4/B5 GI-consumer WGSL had
   never been exercised with non-empty input.

**Conclusion: Batch 6's own deliverable is correctly implemented** — the
`base/` `ReprojectOld` + `taa_dist_min_max` write, `CalcNewTaaSample`, the
final-blit revert, the chain wiring. It successfully un-blocked the GI data
flow, and *that* exposed a **latent defect in the Batch-4/5 GI-consumer WGSL**
(`sample_refine.wgsl` / `spatial_resampling.wgsl` / `denoise_split.wgsl`) that
**corrupts `final_color` to zero once the reservoir buffers carry real data**.
The most likely culprits, in order: (a) `denoise_split.wgsl`'s vertical pass
producing NaN (a bilateral-weight normalisation divide-by-zero, or a bad
`denoise_preprocessed` read) that propagates into `final_color` — the blit's
`curColor / (toneMappingFac + curColor)` maps NaN → 0; (b) `spatial_resampling.wgsl`'s
denoise-path branch zeroing or mis-transposing `final_color` / `denoise_preprocessed`;
(c) `sample_refine.wgsl` writing a malformed `valid_samples_compressed` that
`renderSpatialResampling`'s `getSampleData` decodes into a NaN/huge colour. This
matches the design's own framing — `09-design-b.md` §11: "Batch 6 ... depends
on every prior batch's buffers being correct"; a half-built pipeline is
correct-but-empty, but a *fully*-built pipeline with a latent consumer bug
surfaces only at B6.

**Recommended next step (for the reviewer / a follow-up):** with `is_denoise`
temporarily forced `false`, the spatial pass writes `final_color` directly
(bypassing the denoiser) — this isolates (a) from (b)/(c) in one e2e run.
Then bisect the `sample_refine → spatial_resampling → denoise` chain with a
single targeted buffer probe. This is a Batch-4/5-code fix, not a Batch-6-design
change.

### Phase B impl status

**Phase B is NOT yet feature-complete.** All 13 render-graph nodes are wired
and dispatch cleanly, the `base/` TAA path + final blit are correctly ported
(Batch 6's scope is done — `taa_samples` rewired into `calc_new_taa_sample`,
`taa_dist_min_max` wired, the final blit reverted, item #2's `16.0` + item #7's
GI-constants applied, the authoritative §6.3 shape followed), and the build +
all 46 tests are green. **What remains:** a latent Batch-4/5 GI-consumer WGSL
defect (diagnosed above) corrupts `final_color` to zero once the GI data flow
is live — it must be fixed before the GI bounce is visible and the Phase-B
review gate (`01-context.md` §2d done-bar — "bounce lighting visible, no
obvious artifacts") can pass. The e2e `MIN_NON_BLACK_FRACTION_GI` /
`MIN_GI_BOUNCE_LUMINANCE` thresholds must then be re-measured against the
real GI-lit frame and nudged per the `e2e-render-test.md` rule.

---

## GI-consumer defect fix (2026-05-15)

**Status: NOT fixed — hit the 5-run e2e cap. The Batch-6 diagnosis's stated
suspect (the GI-consumer WGSL) is RULED OUT; the real defect is in the Batch-6
TAA path. This section records the diagnosis chain, what was tried, and the
narrowed root location for a follow-up.**

### What was ruled out (the Batch-6 diagnosis was wrong about *where*)

The Batch-6 diagnosis concluded the defect is "a latent Batch-4/5 GI-consumer
WGSL defect" in `denoise_split.wgsl` / `spatial_resampling.wgsl` /
`sample_refine.wgsl`. **Five e2e isolation runs + a full faithful-port audit
disprove that.** In order:

1. **Isolation run 1 — `is_denoise = false`.** Forced the `GiSettings` default
   to `false` so `spatial_resampling`'s non-denoise branch writes `final_color`
   directly and `naadf_denoise` is skipped entirely. **Frame still uniformly
   `[0,0,0,255]`.** → the denoiser (`denoise_split.wgsl`) is NOT the defect.

2. **Isolation run 2 — `spatial_resampling` non-denoise passthrough.** Patched
   `calc_spatial_resampling`'s non-denoise branch to write `final_color`
   *unchanged* (`final_col = min(read(final_color), COLORS[26])`, dropping the
   `+= absorption * color` GI composite). **Frame still uniformly black.** → if
   `spatial_resampling` were corrupting `final_color`, a pure passthrough would
   have left first-hit's primary light intact and the frame would show the
   pre-GI image. It stayed black → `spatial_resampling` is NOT corrupting
   `final_color`.

3. **Faithful-port audit of the whole GI-consumer chain.** Read
   `sample_refine.wgsl` / `spatial_resampling.wgsl` / `denoise_split.wgsl` /
   `naadf_global_illum.wgsl` / `get_hit_data_from_planes` / `oct_decode` /
   `oct_encode` / `pdf_vndf_isotropic` / `geometry_term` / `taa_compress_sample`
   / `taa_decompress_sample` against the NAADF HLSL (`base/renderSampleRefine.fx`,
   `base/renderSpatialResampling.fx`, `base/renderDenoiseSplit.fx`,
   `base/renderGlobalIllum.fx`, `commonRenderPipeline.fxh`, `commonTaa.fxh`).
   **Every checked spot is a faithful port.** Notably: the e2e test grid is
   **all `Diffuse` + `Emissive`, zero specular** (`src/voxel/grid.rs`) — so
   `first_hit_is_diffuse` is always `true` in `spatial_resampling`, the entire
   specular code path (`get_brdf`, `pdf_vndf_isotropic`) is never taken, and the
   diffuse-path `sample_neighbors` is provably finite (every divisor is
   epsilon-guarded or constant `1/(2π)`; `taa_decompress_sample`'s `s.color` is
   bounded `[0,100]`; `reproject_old_samples`'s `color_sum` is therefore bounded
   too). `sample_neighbors` returns a finite `color`.

4. **Isolation run 3 — `calc_new_taa_sample` constant probe.** This is the
   decisive one. Patched `calc_new_taa_sample` to overwrite `taa_sample_accum`
   for **every** pixel with a known finite constant
   (`vec2(pack2x16float(vec2(1.0, 5.0)), pack2x16float(vec2(5.0, 5.0)))` —
   weight 1, RGB (5,5,5)). The blit (`naadf_final.wgsl`) reads `taa_sample_accum`
   and would tonemap that constant to a bright grey `~(0.82,0.82,0.82)`.
   **Frame STILL uniformly `[0,0,0,255]`.** Runs 4–5 re-ran it to capture the
   full node-dispatch report: only `naadf_denoise` is reported "never
   dispatched" (expected — `is_denoise` was still `false` from run 1's leftover;
   reverted now), i.e. **`naadf_calc_new_taa_sample` AND `naadf_final_blit` both
   dispatched**, the `calc_new_taa_sample` pipeline compiled and ran a span, the
   constant write executed — and the blit still read zero.

### The narrowed root cause

**The defect is NOT in the GI consumers at all — it is in the Batch-6 TAA path
(`reproject_old_samples` / `calc_new_taa_sample` / the `taa_sample_accum`
blit).** Concretely: **`calc_new_taa_sample` writes `taa_sample_accum`, the blit
reads `taa_sample_accum`, both bind groups reference the same
`taa_gpu.taa_sample_accum`, both nodes dispatch — yet the blit reads ZERO.**
The framebuffer is `[0,0,0,255]` (alpha 255 = the blit's `vec4(..., 1.0)` output
ran and covered the screen — so the blit fragment shader *executes*; it just
reads a zero `taa_sample_accum`).

This is a **Batch-6-introduced** defect, not a Batch-4/5 one. Batch 6 is what
(a) added `naadf_taa_reproject_node` + `naadf_calc_new_taa_sample_node` to the
`Core3d` chain for the first time, (b) reverted the blit source from the
temporary `final_color` seam back to `taa_sample_accum`, and (c) replaced
`naadf_final.wgsl`. Batch 5's e2e was 69.1% non-black precisely because the blit
still read `final_color` (the temp seam) — Batch 5 never exercised the
`taa_sample_accum` blit path. The Batch-6 diagnosis's own TEMP probe (write
`final_color.light → taa_sample_accum` from `calc_new_taa_sample`) was black for
the **same reason** the constant probe is black — both write `taa_sample_accum`
via `calc_new_taa_sample` and both are not seen by the blit — but the Batch-6
agent mis-attributed that to "`final_color` is zero" instead of "the
`calc_new_taa_sample` → `taa_sample_accum` → blit hand-off is broken."

The likely mechanisms (not yet isolated — would need run 6+):
- **A `taa_sample_accum` buffer-instance / bind-group-staleness mismatch** —
  `calc_new_taa_sample` and the blit ending up on different buffer instances
  (e.g. `prepare_taa` re-creating `taa_sample_accum` on an early frame while
  `prepare_frame_gpu` clones a stale `blit_bind_group` / `calc_new_taa_sample_
  bind_group` — the frame-bind-group rebuild is gated on `first_hit_data`'s
  `needs_new_storage`, not `TaaGpu`'s). Bind-group construction in
  `prepare.rs:519-604` and `taa.rs:304-353` should be audited for this.
- **A command-encoder submission-order issue** between the separate `Core3d`
  systems' `RenderContext`s (the `.chain()` orders system *execution*; whether
  it orders GPU *submission* of `calc_new_taa_sample`'s encoder before the
  blit's needs confirming).
- A read-write hazard on `taa_sample_accum` between `reproject_old_samples`
  (overwrite) and `calc_new_taa_sample` (fold) that drops the fold.

### Recommended next step (for the follow-up — within a fresh 5-run budget)

Probe the **blit** directly, not `calc_new_taa_sample`: temporarily make
`naadf_final.wgsl` output a hard-coded constant `vec4(0.5, 0.5, 0.5, 1.0)`
ignoring `taa_sample_accum` entirely. If the frame goes grey → the blit
node/pipeline/draw is fine and the defect is purely the
`calc_new_taa_sample → taa_sample_accum` write not landing (chase the
buffer-instance / submission-order hypotheses above). If the frame stays black
→ the blit node itself is broken (pipeline specialisation, the
`Operations::default()` clear, or the e2e readback target). One run splits it.
Then a second probe pinning down the buffer instance (e.g. log the
`taa_sample_accum` `wgpu::Buffer` global-id in `prepare_frame_gpu` for both
bind groups and in `prepare_taa`).

### Verification (current state)

- All TEMP isolation probes **reverted** — `git status` clean,
  `grep -rn "TEMP" src/` clean, `is_denoise` restored to `true`.
- `cargo build` — clean, no warnings.
- `cargo test` — **46 passed** (4 suites), unchanged.
- `cargo run --bin e2e_render` — still **FAILS** (frame uniformly black, the
  0.60 GI gate + `assert_batch_6` + degenerate-frame floor trip). 5 of 5 e2e
  invocations used (the hard cap) — all 5 spent on the isolation experiments
  above; no rebuild→rerun grind.
- The e2e `MIN_NON_BLACK_FRACTION_GI` / `MIN_GI_BOUNCE_LUMINANCE` thresholds
  were **not** re-measured — no GI-lit frame was ever produced. They remain the
  Batch-6 design-intent placeholders (`0.60` / `12.0`).

### Phase B impl status

**Phase B is still NOT feature-complete.** All 13 render-graph nodes are wired
and dispatch; the GI-consumer WGSL (`sample_refine` / `spatial_resampling` /
`denoise_split`) is a verified-faithful port and is **cleared of suspicion** —
the Batch-6 diagnosis pointed at the wrong subsystem. The remaining blocker is a
**Batch-6 TAA-path defect**: the `calc_new_taa_sample → taa_sample_accum → blit`
hand-off does not deliver — the blit reads a zero `taa_sample_accum` even when
`calc_new_taa_sample` writes a non-zero constant to every pixel of it. This is a
Rust-side bind-group / buffer-lifecycle or render-graph-submission bug (most
likely in `prepare_frame_gpu` / `prepare_taa`), NOT a WGSL shader-math bug. It
must be fixed before the GI bounce can be visible and the Phase-B review gate
can pass.
