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
