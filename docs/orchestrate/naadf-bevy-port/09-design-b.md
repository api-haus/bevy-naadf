# 09 — Phase B Architecture Design (real-time GI pipeline)

## delegate-architect findings — Phase B (2026-05-14)

Phase B ports NAADF's **`WorldRenderBase` real-time raytraced GI pipeline** — research-doc
§4.2–4.3, `02-research.md` §1.2.1 / §1.2.3 / §1.2.4 / §1.2.5 — onto the completed Phase-A-2 base
(albedo first-hit + 16-frame long-term TAA, both review-gated). This is the implementable design:
an `impl` agent executes §10's batched sequence without further architectural decisions.

All choices sit inside `01-context.md` §2d's binding scope decision (NAADF's `WorldRenderBase`
fully — reference pathtracer + DLSS-RR explicitly OUT), Q1–Q4, D1–D5, and the A-2 decisions
(16-deep `taaSamples` ring, 128-deep camera-history ring, `M*v` matrix convention, the
`taa_sample_accum.x` per-pixel sample-count signal). They are cited, not relitigated.

**Every HLSL path / line and every worktree path / line below is verified by Read/Grep against
the source on disk** — nothing is invented. Worktree = `/mnt/archive4/DEV/bevy-naadf/.claude/
worktrees/phase-b-gi`; NAADF HLSL/C# = `/mnt/archive4/DEV/NAADF/NAADF` (read-only reference).

Source read directly for this design:
`Content/shaders/render/versions/base/{renderFirstHit,rayQueueCalc,renderAtmosphere,renderGlobalIllum,renderSampleRefine,renderSpatialResampling,renderDenoiseSplit,renderTaaSampleReverse,renderFinal}.fx`,
`render/common/{commonRenderPipeline,commonRayTracing,commonColorCompression,commonOther,commonTaa,commonEntities,common,commonConstants}.fxh`,
`render/common/atmosphere/{atmosphereRaw,atmospherePrecomputed}.fxh`;
C# `World/Render/Versions/WorldRenderBase.cs`, `World/Render/WorldRender.cs`,
`World/Render/Atmosphere.cs`, `Gui/Main/UiDebug/UiSkyDebug.cs`; and the entire worktree
`src/render/**` + `src/assets/shaders/**` Phase-A/A-2 surface.

---

## 0. Scope of this document

| section | covers | brief item |
|---|---|---|
| §1 | What Phase B is / is NOT — non-scope statement | brief 11 |
| §2 | `src/` module layout deltas — new files + changed files | brief 1 |
| §3 | Data structures — GI sample structs, bucket-info, ray queue, uniforms | brief 2 |
| §4 | The render-graph plan — every new node, pipelines, bind groups, edges | brief 3 |
| §5 | The WGSL ports — each `base/` shader → its WGSL file + provenance table | brief 4 |
| §6 | The 4-plane-bounce first-hit | brief 5 |
| §7 | `rayQueueCalc` + the adaptive ~0.25-spp data flow | brief 6 |
| §8 | Compressed ReSTIR GI — globalIllum / sampleRefine / spatialResampling | brief 7 |
| §9 | The sparse bilateral denoiser + the atmosphere precompute | brief 8 |
| §10 | extract / prepare changes | brief 9 |
| §11 | The numbered, BATCHED Phase-B implementation sequence | brief 10 |
| §12 | Open items the orchestrator must surface before implementation begins | — |

---

## 1. What Phase B is — and explicitly is NOT (brief item 11)

**Phase B IS:** the full NAADF `WorldRenderBase` real-time GI pipeline ported to WGSL render-graph
nodes on the Phase-A-2 base. Concretely (`02-research.md` §5.4, `WorldRenderBase.cs`):

- The **4-plane-bounce first-hit** — a Phase-B variant of `naadf_first_hit.wgsl` that fills
  G-buffer planes 1–3 (specular bounces), writes `firstHitAbsorption` + `finalColor`, and applies
  the precomputed atmosphere (`base/renderFirstHit.fx`).
- The **atmosphere precompute** — the full multiple-scattering sky model
  (`base/renderAtmosphere.fx` + `atmosphereRaw.fxh` + `atmospherePrecomputed.fxh`).
- **`rayQueueCalc`** — the adaptive sampler; reads `taa_sample_accum.x` and produces the
  ~0.25-spp ray queue (`base/rayQueueCalc.fx`).
- **Compressed ReSTIR GI** — `renderGlobalIllum` (≤3-bounce secondary rays, lit/unlit
  separation, 5-bit/channel colour compression), `renderSampleRefine` (5 passes: clear-buckets,
  valid-history, count-valid-and-refine, count-invalid, refine-buckets — the `COLOR_DIF_PROB`
  brightness-leveling), `renderSpatialResampling` (Algorithm 2 — the 12-iteration spatial pass
  with a single visibility check).
- The **sparse bilateral denoiser** (`base/renderDenoiseSplit.fx` — separable horizontal/vertical
  kernel 21, σ=10, sparse).
- The **Phase-B `renderTaaSampleReverse`** — the `base/` variant: `ReprojectOld` additionally
  writes `taaDistMinMax`; a new `CalcNewTaaSample` pass folds the denoised GI result into the
  16-frame TAA history (`02-research.md` §1.2.2 cross-check, `06-design-a2.md` §7.6).
- The **Phase-B `renderFinal`** — the `base/` variant of the final blit.
- Reuse of the Phase-A `shoot_ray` AADF traversal **unchanged** for all GI secondary rays + sun
  rays + visibility rays; reuse of the A-2 16-frame TAA infrastructure + the `taa_sample_accum`
  signal; reuse of the render-world plumbing.

**Phase B is NOT** (do not design or implement any of these in Phase B):

- **The reference pathtracer** (`WorldRenderPathTracer` / `Content/shaders/render/versions/
  pathTracer/**`) — `01-context.md` §2d binding scope decision: future work, do not port.
- **DLSS / DLSS-RR** — `01-context.md` §2d: future work; the `dlss` / `force_disable_dlss` Cargo
  plumbing stays dormant exactly as Phase A/A-2 left it; do not wire the GI pipeline to DLSS, do
  not add G-buffer extensions for it.
- **Phase C** — GPU world construction (`chunkCalc.fx`), the background chunk-AADF queue
  (`boundsCalc.fx` / `WorldBoundHandler`), flood-fill edit invalidation (`worldChange.fx` /
  `ChangeHandler`), editing tools. The CPU-built static test grid from Phase A is the producer;
  the traversal shader is producer-agnostic.
- **Entities.** Phase A/A-2 are entity-free (`02-research.md` §1.1.7, `03-design.md` §7.5,
  `06-design-a2.md` §1). Phase B stays entity-free: every `#ifdef ENTITIES` block in the `base/`
  shaders is **omitted**, exactly as Phase A omitted the `ENTITIES` traversal branch. `entity` is
  always `ENTITY_FREE` (`0x3FFF`); `entitySample` always `ENTITY_FREE`; `entityPosChange` always
  `(0,0,0)`; `entityInstancesHistory` is never bound; the entity-offset terms in
  `getHitDataFromPlanes` / `renderSampleRefine` / `renderSpatialResampling` are simply absent.
- **The non-A-2 `05-review.md` §4 secondary issues** — `prepare_world_gpu`-runs-every-frame and
  the zeroed `GpuRenderParams.bounding_box_*` fields — UNLESS one actively blocks the GI pipeline
  (it does not — see §10.1). Leave them.
- **The `WorldRenderBase` GUI** (`SettingDataRenderBase.RenderImGui`) — every ImGui slider in
  `WorldRenderBase.cs:26-52` becomes a **compile-time / `AppArgs` constant** in the port, not a
  runtime knob (Phase A established this — `06-design-a2.md` §7.1, §13.4). The settings
  themselves (`bounceCount=3`, `globalIllumMaxAccum=128`, `spatialResampleSize=500`, etc.) ARE
  ported as the constant values; the *slider UI* is not.

---

## 2. `src/` module layout deltas (brief item 1)

Phase B is an **extension**, not a restructure — no Phase-A/A-2 module is reorganised. It is
large, so it adds several new modules. New files and changed files:

### 2.1 New Rust files

```
src/render/
  atmosphere.rs   NEW  AtmosphereGpu resource; the atmosphere `octahedral` buffer + the
                       GpuAtmosphereParams uniform + the sky-param constants (from
                       UiSkyDebug.cs's defaults); prepare_atmosphere system; the
                       atmosphere render node. ~250 lines.
  gi.rs           NEW  GiGpu render-world resource (every Phase-B GI buffer — see §3.7);
                       the GpuGiParams uniform; prepare_gi (creates + resizes the GI
                       buffers, uploads the per-frame GI uniforms, builds the GI bind
                       groups); the 8x8 bucket-grid geometry helper; the
                       global_illum_max_accum / accum-ring index helpers; the
                       FrameCounters helper (rand-salt derivation, accum index, taa
                       index — all derived from the A-2 frame counter). ~600 lines.
  graph_b.rs      NEW  the seven new Phase-B render-graph node systems + their span-name
                       consts (atmosphere, the 4-plane first-hit is the EXISTING node
                       extended — see §6 — not a new one; rayQueueCalc, globalIllum,
                       sampleRefine ×5, spatialResampling, denoiseSplit×2). ~500 lines.
                       (Alternatively these can land in the existing `graph.rs`; a
                       separate file keeps the A-2 graph readable — designer's call,
                       graph_b.rs recommended.)
```

`src/render/taa.rs` is **extended in place** (not split): it gains `CalcNewTaaSample` wiring,
the `taaDistMinMax` buffer on `TaaGpu`, and the camera-history-ring `view_proj_inv` field
(`renderSampleRefine` needs the *inverse* ring too — §3.6). `src/render/{extract,prepare,
pipelines,gpu_types,mod}.rs` are extended (see §2.3).

### 2.2 New WGSL files

```
src/assets/shaders/
  color_compression.wgsl  NEW  port of commonColorCompression.fxh — the COLORS[32] /
                               COLOR_DIF_PROB[31] tables, compress_color (5-bit/channel
                               exponential), refine_comp_color. Phase-B-only
                               (02-research.md §5.1 tags commonColorCompression.fxh
                               Phase B). naga-oil import module.
  atmosphere.wgsl         NEW  port of atmosphereRaw.fxh + atmospherePrecomputed.fxh —
                               add_light_for_direction (ray-marched sky), the
                               Rayleigh/Mie phase fns, density_at_height, ray_sphere,
                               apply_atmosphere (samples the precomputed octahedral
                               buffer). naga-oil import module.
  naadf_atmosphere.wgsl   NEW  compute entry — port of base/renderAtmosphere.fx
                               `precomputeAtmosphere`. Writes one quarter of the
                               octahedral atmosphere buffer per frame.
  ray_queue_calc.wgsl     NEW  compute entry — port of base/rayQueueCalc.fx
                               `calcRayQueue` + `calcRayQueueStore`. Builds the adaptive
                               ray queue from taa_sample_accum.
  naadf_global_illum.wgsl NEW  compute entry — port of base/renderGlobalIllum.fx
                               `calcGlobalIlum`. The ≤3-bounce secondary-ray tracer.
  sample_refine.wgsl      NEW  compute — the 5 passes of base/renderSampleRefine.fx
                               (clear_buckets_and_calc_mask, compute_valid_history,
                               count_valid_data_and_refine, count_invalid_data,
                               refine_buckets) as 5 entry points in one module.
  spatial_resampling.wgsl NEW  compute entry — port of base/renderSpatialResampling.fx
                               `calcSpatialResampling` (Algorithm 2).
  denoise_split.wgsl      NEW  compute — base/renderDenoiseSplit.fx
                               `calcDenoiseHorizontal` + `calcDenoiseVertical` as 2
                               entry points.
  naadf_final_b.wgsl      NEW  fragment — port of base/renderFinal.fx `MainPS`. Nearly
                               identical to naadf_final.wgsl but adds the `toneMappingFac`
                               uniform term — see §5.9. (Could be a flag on the existing
                               naadf_final.wgsl; a separate file is cleaner — §5.9.)
```

The B-only functions get **added to the existing shared WGSL modules**:
- `ray_tracing_common.wgsl` gains the VNDF/GGX functions (`commonRayTracing.fxh:65-137` —
  `get_perpendicular_vector`, `get_uniform_hemisphere_sample`, `sample_vndf_isotropic`,
  `pdf_vndf_isotropic`, `geometry_term`). These are the Phase-B "splits" `02-research.md` §5.5
  flagged. (`compress_quaternion`/`decompress_quaternion` stay un-ported — entity-only.)
- `render_pipeline_common.wgsl` gains: the **full** `get_hit_data_from_planes` (the 3-iteration
  specular-reflection loop + `SPECULAR_MIRROR_FAC` LUT — `commonRenderPipeline.fxh:154-213`,
  entity branch omitted); `get_reflectance_fresnel`; `get_specular_normals`; `get_tang`; the full
  `get_screen_pos_projection` / `get_screen_index_projection` (these already exist in `taa.wgsl`
  as A-2-local — Phase B promotes them here so `renderSampleRefine` / `renderSpatialResampling`
  can share them — §5.2); the `FirstHitResult` struct. `SPECULAR_MIRROR_FAC` is a new `const`.
- `common.wgsl` gains the `commonOther.fxh` helpers Phase B needs: `gaussian_f`, `gcd`,
  `find_coprime`, `next_pow2`. (The `addToCounter*` group-shared counter helpers are NOT ported
  as shared functions — they use `groupshared` + `GroupMemoryBarrierWithGroupSync`, which port
  fine into the specific compute entries that need them (`ray_queue_calc.wgsl`,
  `naadf_global_illum.wgsl`) but are not reusable cross-module — port them inline per §7 / §8.1.)

### 2.3 Changed files

| file | change |
|---|---|
| `src/render/gpu_types.rs` | add `GpuAtmosphereParams`, `GpuGiParams`, `GpuSampleValid` (the 32-byte lit-sample struct — but see §3.2: it is a raw `[u32;8]`, not a fielded struct), `GpuBucketInfo` (`[u32;2]`), the refined/compressed/invalid sample types are raw `vec4<u32>` so need no Rust struct. Extend `GpuTaaParams` with the `cam_pos_int_old` / `accum_index` fields the `base/` TAA pass needs (§3.6) — OR add a small `GpuTaaParamsB` (designer's call — §3.6 recommends extending). Add the new size asserts. `GpuRenderParams` is unchanged. |
| `src/render/extract.rs` | `ExtractedCameraData` gains `view_proj_inv_history`-ringing (no — the inverse ring goes on `ExtractedCameraHistory`, see §10.2). `ExtractedCameraHistory` gains `view_proj_inv: [Mat4; 128]` (the *inverse* rotation-only view-proj ring — C# `taaSampleCamTransformInvers[128]` — `renderSampleRefine` binds it as `camRotOld`). Add `ExtractedGiConfig` (mirrors the `AppArgs`-promoted `WorldRenderBase` settings — §3.8) + `extract_gi_config`. |
| `src/render/taa.rs` | `CameraHistory` gains `view_proj_inv: [Mat4; 128]`, populated in `update_camera_history` from `rotation_only_view_proj(...).inverse()` (one extra `.inverse()` per frame — cheap). `TaaGpu` gains `taa_dist_min_max: Buffer` (the `base/` `ReprojectOld` extra output — §3.5). `prepare_taa` creates/resizes it, uploads the inverse ring into `camera_history` slots (the slot struct gains `view_proj_inv` — §3.6). The TAA reproject node now also runs the new `CalcNewTaaSample` pass — see §4.9, §5.8. |
| `src/render/prepare.rs` | `prepare_frame_gpu` — `FrameGpu` gains `first_hit_absorption: Buffer` + `final_color: Buffer` (the two `base/` first-hit outputs — §3.4). The first-hit `@group(1)` layout widens by 2 bindings; the bind-group build adds them. (`final_color` is the GI working buffer threaded through globalIllum/spatialResampling/denoise/CalcNewTaaSample.) |
| `src/render/pipelines.rs` | add ~13 new bind-group-layout descriptors + ~12 new `CachedComputePipelineId`s + one new blit-pipeline-per-format cache for `naadf_final_b` (or reuse — §5.9); extend the first-hit pipeline's `@group(1)` layout. Add the new shader-path consts. |
| `src/render/mod.rs` | `pub mod atmosphere; pub mod gi; pub mod graph_b;` declared; register the new render-world resources (`AtmosphereGpu`, `GiGpu`, `ExtractedGiConfig`); add `extract_gi_config` to `ExtractSchedule`; add `prepare_atmosphere` + `prepare_gi` to `PrepareResources`; rebuild the `Core3d` `.chain()` with all the new nodes in NAADF's dispatch order (§4.2). |
| `src/main.rs` | `AppArgs` gains the GI config fields (or a nested `GiSettings` struct) — §3.8. The `CameraHistory` resource init is unchanged (it gains a field but keeps its `Default`). |
| `src/hud.rs` | add timing lines for the new render nodes (§4.10). Update the renderer-mode string to "Phase B — GI". |
| `src/assets/shaders/naadf_first_hit.wgsl` | **replaced in place** by the 4-plane-bounce variant (§6). This is the single biggest WGSL change. It is an *extension* of the existing file — the Phase-A/A-2 single-plane path becomes the `i==0` iteration of a 4-iteration loop. |
| `src/assets/shaders/render_pipeline_common.wgsl` | gains the B-only functions (§2.2). `compress_first_hit_data` gains the `is_diffuse` parameter (the `base/` variant — `base/renderFirstHit.fx:18` — has 5 args, the `albedo/` one had 4: `.y = isDiffuse | ...` instead of `.y = 1 | ...`). |
| `src/assets/shaders/ray_tracing_common.wgsl` | gains the VNDF/GGX functions (§2.2). |
| `src/assets/shaders/common.wgsl` | gains `gaussian_f`, `gcd`, `find_coprime`, `next_pow2` (§2.2). |
| `src/assets/shaders/taa.wgsl` | the A-2-local `get_hit_data_from_planes_a2` / `get_screen_pos_projection` / `get_screen_index_projection` are **replaced by imports** of the now-shared full versions from `render_pipeline_common.wgsl` (§5.2). The `base/renderTaaSampleReverse.fx` adds the `taaDistMinMax` write to `ReprojectOld` and the new `CalcNewTaaSample` entry point — see §5.8. |

`src/world/`, `src/aadf/`, `src/voxel/`, `src/camera/`, `src/assets/shaders/{ray_tracing,
world_data,naadf_final}.wgsl` are **untouched** (`naadf_final.wgsl` stays as the A-2 final blit;
Phase B's `renderFinal` is the new `naadf_final_b.wgsl` — but see §5.9, it may just replace it).

---

## 3. Data structures (brief item 2)

All bit layouts below are **derived from the verified NAADF HLSL** (`WorldRenderBase.cs` buffer
creation + the `base/` shaders' compress/decompress functions + `commonRenderPipeline.fxh`'s
`SampleValid`). The C# `Uint2/3/4/8` element types map to `vec2/3/4<u32>` on the GPU and
`[u32;N]` (or fielded `#[repr(C)]` structs) on the CPU side.

### 3.1 GPU buffer / texture inventory (Phase B)

From `WorldRenderBase.cs:104-171` — every `StructuredBuffer` `WorldRenderBase` creates, sized:

| C# resource | C# element / size | Bevy Phase-B resource | element / size |
|---|---|---|---|
| `rayQueueBuffer` | `uint`, `w·h + 1` | `GiGpu.ray_queue` | `array<u32>`, `pixel_count + 1` |
| `rayQueueIndirectBuffer` | `DispatchComputeArguments` (5×`u32`) | `GiGpu.ray_queue_indirect` | `array<u32>`, 5 — `INDIRECT \| STORAGE \| COPY_DST` |
| `firstHitData` | `Uint4`, `w·h` | **`FrameGpu.first_hit_data`** (exists — A-2) | `array<vec4<u32>>`, `pixel_count` |
| `firstHitAbsorption` | `Uint2`, `w·h` | `FrameGpu.first_hit_absorption` (NEW) | `array<vec2<u32>>`, `pixel_count` |
| `finalColor` | `Uint2`, `w·h` | `FrameGpu.final_color` (NEW) | `array<vec2<u32>>`, `pixel_count` |
| `atmosphereComp` | `Uint3`, `1024·1024` | `AtmosphereGpu.atmosphere_comp` | `array<vec3<u32>>` (see §3.3 note), `1024·1024` |
| `globalIlumValidSamples` | `Uint8` (32 B), `w·h·2` | `GiGpu.valid_samples` | `array<GpuSampleValid>` (`[u32;8]`), `pixel_count·2` |
| `globalIlumInvalidSamples` | `Uint4` (16 B), `w·h·8` | `GiGpu.invalid_samples` | `array<vec4<u32>>`, `pixel_count·8` |
| `globalIlumValidSamplesRefined` | `Uint4`, `bucketCount·32` | `GiGpu.valid_samples_refined` | `array<vec4<u32>>`, `bucket_count·32` |
| `globalIlumValidSamplesCompressed` | `Uint4`, `bucketCount·8` | `GiGpu.valid_samples_compressed` | `array<vec4<u32>>`, `bucket_count·8` |
| `globalIlumBucketInfo` | `Uint2`, `bucketCount` | `GiGpu.bucket_info` | `array<vec2<u32>>`, `bucket_count` |
| `globalIlumSampleCounts` | `Uint2`, `128+3` | `GiGpu.sample_counts` | `array<vec2<u32>>`, `131` |
| `globalIlumValidDispatch` / `globalIlumInvalidDispatch` | indirect (5×`u32`) | `GiGpu.valid_dispatch` / `invalid_dispatch` | `array<u32>`, 5 each — `INDIRECT \| STORAGE \| COPY_DST` |
| `denoisePreprocessed` / `denoisePreprocessedHorizontal` | `Uint3`, `w·h` | `GiGpu.denoise_preprocessed` / `_horizontal` | `array<vec3<u32>>`, `pixel_count` |
| `taaDistMinMax` | `Uint2`, `w·h` | `TaaGpu.taa_dist_min_max` (NEW) | `array<vec2<u32>>`, `pixel_count` |

`bucket_count = ((w + 7) / 8) · ((h + 7) / 8)` (`WorldRenderBase.cs:157-159`). `atmosphereTexSizeX
= atmosphereTexSizeY = 1024` (`WorldRenderBase.cs:131-132`) — **a compile-time constant** in the
port (`ATMOSPHERE_TEX_SIZE = 1024`).

All `pixel_count`- and `bucket_count`-sized buffers **resize on viewport change** (same trigger
as `first_hit_data` — `prepare.rs:356-370`). The `atmosphere_comp` buffer + the indirect buffers
+ `sample_counts` (131 elements, fixed) are fixed-size. The GI sample-list buffers
(`valid_samples`, `invalid_samples`, `valid_samples_refined`, `valid_samples_compressed`,
`bucket_info`) are `pixel_count`/`bucket_count`-derived → resize on viewport change.

**Buffer creation pattern:** none of these grow at runtime — they are all fixed-per-resolution.
Use plain `render_device.create_buffer` (the `prepare.rs` pattern), **not** `GrowableBuffer`
(`GrowableBuffer` is for the world buffers that the editing/Phase-C path grows; the GI buffers
never grow — `WorldRenderBase.CreateScreenTextures` allocates them once per resolution). All get
`STORAGE | COPY_DST`; the three indirect buffers also get `INDIRECT`; everything is
**zero-cleared on creation** via the `encoder.clear_buffer` pattern (`prepare.rs:397-403`,
`taa.rs:310-317`).

### 3.2 The lit sample — `GpuSampleValid` (32 bytes / `Uint8`)

Derived from `commonRenderPipeline.fxh:38-42` (`struct SampleValid { uint4 data1; uint4 data2; }`)
and the verified packing in `renderGlobalIllum.fx:34-48` (`compressSampleValid`). It is **8 raw
`u32`s** — the shader packs/unpacks the bitfields directly; there is no benefit to a fielded Rust
struct (the CPU never reads or writes individual samples — they are GPU-only working data).

```rust
// src/render/gpu_types.rs
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuSampleValid {
    /// data1 (uvec4) + data2 (uvec4), packed exactly as compressSampleValid
    /// (renderGlobalIllum.fx:34-48). GPU-only working data — never CPU-read.
    pub data: [u32; 8],
}
const _: () = assert!(std::mem::size_of::<GpuSampleValid>() == 32);
```

WGSL side (`render_pipeline_common.wgsl` — promote `SampleValid` from a header concept):
```wgsl
struct SampleValid { data1: vec4<u32>, data2: vec4<u32> }
```
The bit layout, for the WGSL `compress_sample_valid` / the decode in `sample_refine.wgsl` (from
`renderGlobalIllum.fx:34-48`):
- `data1.x = firstHit.x` (entity | normTang0<<15)
- `data1.y = pixelPos.x | (firstHit.y & 0xFFFF8000)` — pixel-X in low 15 bits, plane-1 code high
- `data1.z = pixelPos.y | (firstHit.z & 0xFFFF8000)` — pixel-Y in low 15 bits, plane-2 code high
- `data1.w = taaIndex | (roughness << 7) | (firstHit.w & 0xFFFF8000)` — `taaIndex` low 7 bits,
  8-bit roughness at bit 7, plane-3 code high
- `data2.x = entitySample | (isFirst << 14) | (sampleSpecularNormals.x << 15)`
- `data2.y = compColor | (sampleSpecularNormals.y << 15)` — the 15-bit 5-bit/channel colour
- `data2.z = (sampleDirOct.y >> 10) | (sampleSpecularNormals.z << 15)`
- `data2.w = (sampleDirOct.y & 0x3FF) | (sampleDirOct.x << 10)` — octahedral sample dir,
  `octEncode(sampleDir) * 2^22`

### 3.3 The unlit / refined / compressed / invalid samples — raw `vec4<u32>`

- **`invalid_samples`** (`globalIlumInvalidSamples`, 16 B / `Uint4`) — `compressSampleInvalid`
  (`renderGlobalIllum.fx:50-58`): `.x = firstHit.x`, `.y = pixelPos.x | (firstHit.y & 0xFFFF8000)`,
  `.z = pixelPos.y | (firstHit.z & 0xFFFF8000)`, `.w = taaIndex | (roughness<<7) | (firstHit.w &
  0xFFFF8000)`. Raw `array<vec4<u32>>` — no Rust struct.
- **`valid_samples_refined`** (`globalIlumValidSamplesRefined`, `Uint4`) — packed in
  `renderSampleRefine.fx:236-249` (`refinedSample`): surface-Y / colour, sample-dist /
  sample-normal-oct, surface-X / sample-dir-oct / material, surface-Z / sample-dir-oct /
  material-state. Raw `vec4<u32>`.
- **`valid_samples_compressed`** (`globalIlumValidSamplesCompressed`, `Uint4`) — `refineBuckets`
  rewrites the colour field of a refined sample (`renderSampleRefine.fx:401-404`). Same `Uint4`
  layout as refined, with the colour field re-leveled. Raw `vec4<u32>`. `spatial_resampling.wgsl`
  decodes it via `getSampleData` (`renderSpatialResampling.fx:29-38`).
- **`atmosphere_comp`** (`Uint3`) — `vec3<u32>` per octahedral texel
  (`renderAtmosphere.fx:27-31`): `.x = f16(light.r) | f16(light.g)<<16`, `.y = f16(light.b) |
  f16(absorption.r)<<16`, `.z = f16(absorption.g) | f16(absorption.b)<<16`.
  **WGSL `vec3<u32>` in a storage `array` has 16-byte stride** (alignment 16) — but the C#
  `Uint3` `StructuredBuffer` has 12-byte stride. **This is a layout mismatch.** Resolution: store
  `atmosphere_comp` as `array<vec4<u32>>` (16-byte, `.w` unused/padding) in the WGSL **and** size
  the Rust buffer at `1024·1024·16` bytes. The atmosphere precompute writes `atmo_comp.w = 0u`;
  `apply_atmosphere` reads `.xyz`. Document this as a deliberate WGSL-alignment port deviation
  (the C# DX11 `StructuredBuffer<Uint3>` packs tight; WGSL/std430 does not). `atmosphere_comp` is
  fixed-size — `1024·1024·16` = 64 MiB. (Verify against `device.limits().max_buffer_size` — at
  the wgpu 256 MiB default this is fine; a `debug_assert!` keeps it visible — same as
  `GrowableBuffer`.) Same `vec3→vec4` treatment applies to `denoise_preprocessed` /
  `denoise_preprocessed_horizontal` (`Uint3`): store as `array<vec4<u32>>`, `.w` unused, sized
  `pixel_count·16`.

### 3.4 `FrameGpu` additions — `first_hit_absorption` + `final_color`

`base/renderFirstHit.fx:6-8` declares three RW outputs: `firstHitData`, `firstHitAbsorption`,
`finalColor`. Phase A/A-2 only had `firstHitData` (in `FrameGpu`) + `taa_sample_accum` (in
`TaaGpu`). Phase B adds the other two to `FrameGpu`:

```rust
// src/render/prepare.rs — FrameGpu gains:
pub first_hit_absorption: Buffer,  // array<vec2<u32>>, pixel_count — base/renderFirstHit.fx firstHitAbsorption
pub final_color: Buffer,           // array<vec2<u32>>, pixel_count — base/ finalColor (the GI working colour buffer)
```

Both `pixel_count`-sized, `STORAGE | COPY_DST`, zero-cleared on creation, resized with
`first_hit_data`. `final_color` is the buffer the GI passes thread their result through:
`first_hit` writes the primary-ray light into it (`renderFirstHit.fx:128`); `spatialResampling`
adds the resampled GI to it (non-denoise path) or `denoiseSplit` does (denoise path —
`renderDenoiseSplit.fx:128-131`); `CalcNewTaaSample` reads it as `light` and folds it into the
TAA history (`renderTaaSampleReverse.fx:187-188`).

### 3.5 `TaaGpu` addition — `taa_dist_min_max`

`base/renderTaaSampleReverse.fx:9` declares `RWStructuredBuffer<uint2> taaDistMinMax`, written by
`ReprojectOld` (`:79`) and read by `renderSampleRefine`'s `CountValidAndRefine` / `CountInvalid`
(`:182`, `:325`). A-2's `albedo/` reproject pass did NOT have this (`06-design-a2.md` §7.6). Phase
B adds it:
```rust
// src/render/taa.rs — TaaGpu gains:
pub taa_dist_min_max: Buffer,  // array<vec2<u32>>, pixel_count
```
`pixel_count`-sized, `STORAGE | COPY_DST`, zero-cleared, resized with `taa_samples`. Element
layout (`renderTaaSampleReverse.fx:79`): `.x = f16(distMin) | f16(distMax)<<16`, `.y =
validNormalsSpec` (the packed specular-normal validity mask — `:68-70`).

### 3.6 `GpuTaaParams` extension + the camera-history inverse ring

The `base/` GI passes (`renderGlobalIllum`, `renderSampleRefine`, `renderSpatialResampling`,
`renderTaaSampleReverse`) need camera state the A-2 `GpuTaaParams` does not carry:

- `renderGlobalIllum.fx:24-28` needs `camRotOld[128]` (the **non-inverse** rotation-only ring —
  already on `camera_history` as `view_proj`), `taaOldCamPosFromCurCamInt[128]` (already on
  `camera_history` as `cam_pos_from_cur_int`), `taaJitterOld[128]` (already as `jitter`),
  `accumIndex`, `taaIndex`, `randCounter`, `randCounter2`, `maxBounceCount`.
- `renderSampleRefine.fx:24-25` needs `camRotOld[128]` — but **note**: `WorldRenderBase.cs:346`
  binds `taaSampleCamTransformInvers` (the **inverse** ring) into `renderSampleRefine`'s
  `camRotOld` parameter, while `renderGlobalIllum.fx` / `renderTaaSampleReverse.fx` bind the
  **non-inverse** `taaSampleCamTransform`. The same shader parameter name, two different
  matrices, depending on the pass — `renderSampleRefine` calls `getRayDir(camRotOld[...], ...)`
  so it needs the **inverse** (`invCamMatrix`-style). **So the camera-history slot struct must
  carry BOTH** the rotation-only view-proj and its inverse.

**`GpuCameraHistorySlot` gains `view_proj_inv`** (`renderSampleRefine`'s `camRotOld`):
```rust
// src/render/gpu_types.rs — GpuCameraHistorySlot, now 160 bytes:
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCameraHistorySlot {
    pub view_proj: Mat4,                  // C# camRotOld[i] (rotation-only view-proj) — globalIllum/taaReverse
    pub view_proj_inv: Mat4,              // C# taaSampleCamTransformInvers[i] — sampleRefine's camRotOld
    pub cam_pos_from_cur_int: Vec3,
    pub _pad0: u32,
    pub jitter: Vec2,
    pub _pad1: Vec2,
}
const _: () = assert!(std::mem::size_of::<GpuCameraHistorySlot>() == 64 + 64 + 16 + 16);
```
This is a **layout change** to `GpuCameraHistorySlot` (96 → 160 bytes). The A-2 `taa.wgsl` struct
decl + the A-2 `prepare_taa` upload + `CameraHistory` ring all gain the field. `update_camera_
history` computes `view_proj_inv = view_proj.inverse()` (one extra `.inverse()` per frame).

**`GpuTaaParams` extension** vs. a new `GpuGiParams` — there is a clean separation. Decision:
- **`GpuTaaParams`** stays the TAA pass's uniform. The `base/` `renderTaaSampleReverse` needs the
  same fields A-2 already gave it (`inv_view_proj`, `view_proj`, `cam_pos_int/frac`,
  `screen_width/height`, `frame_count`, `taa_index`, `sample_age`) — **no `GpuTaaParams` change
  for the TAA pass itself.** The new `CalcNewTaaSample` pass needs `taa_index` + cam pos +
  `inv_view_proj` — all already present.
- **`GpuGiParams`** (NEW — §3.8) carries everything the GI passes (`rayQueueCalc`, `globalIllum`,
  `sampleRefine`, `spatialResampling`) need that is NOT in `GpuTaaParams`: `accum_index`,
  `rand_counter` / `rand_counter2`, `max_bounce_count`, `bucket_size_x/y`, `bucket_count`,
  `sun_color`, `sky_sun_dir`, `skip_samples`, `is_denoise`, `is_sample_leveling`,
  `is_varying_resampling_radius`, `spatial_resample_size`, `radius_lit_factor`,
  `spatial_visibility_count`, `noise_suppression_factor`, `denoise_thresh`, `sample_max_accum`,
  the storage-count constants, `frame_count`/`frame_index`, `screen_width/height`,
  `cam_pos_int/frac`, `inv_view_proj`, `view_proj` (`camMatrix`). It is bound by every GI pass
  (one shared uniform — simplest).

### 3.7 `GiGpu` render-world resource

```rust
// src/render/gi.rs
#[derive(Resource)]
pub struct GiGpu {
    // ray-queue (rayQueueCalc) — §7
    pub ray_queue: Buffer,            // array<u32>, pixel_count + 1
    pub ray_queue_indirect: Buffer,   // 5×u32, INDIRECT — the indirect dispatch args for globalIllum
    // GI sample lists (globalIllum / sampleRefine) — §8
    pub valid_samples: Buffer,        // array<GpuSampleValid>, pixel_count·2
    pub invalid_samples: Buffer,      // array<vec4<u32>>, pixel_count·8
    pub valid_samples_refined: Buffer,    // array<vec4<u32>>, bucket_count·32
    pub valid_samples_compressed: Buffer, // array<vec4<u32>>, bucket_count·8
    pub bucket_info: Buffer,          // array<vec2<u32>>, bucket_count
    pub sample_counts: Buffer,        // array<vec2<u32>>, 131 — the 128+3 accumulation ring + 3 header slots
    pub valid_dispatch: Buffer,       // 5×u32, INDIRECT — sampleRefine CountValidAndRefine indirect args
    pub invalid_dispatch: Buffer,     // 5×u32, INDIRECT — sampleRefine CountInvalid indirect args
    // denoiser (spatialResampling writes / denoiseSplit reads) — §9.1
    pub denoise_preprocessed: Buffer,        // array<vec4<u32>> (Uint3 padded — §3.3), pixel_count
    pub denoise_preprocessed_horizontal: Buffer, // array<vec4<u32>>, pixel_count
    // per-frame GI uniform
    pub gi_params: Buffer,            // GpuGiParams
    // geometry the resize trigger / bucket-grid math needs
    pub pixel_count: u32,
    pub bucket_count: u32,
    pub bucket_size: UVec2,
    // bind groups — §4
    pub ray_queue_bind_group: BindGroup,
    pub global_illum_bind_group: BindGroup,
    pub sample_refine_bind_group: BindGroup,
    pub spatial_resampling_bind_group: BindGroup,
    pub denoise_bind_group: BindGroup,
}
```

`sample_counts` is the `globalIlumSampleCounts` `Uint2[128+3]` — the **accumulation ring**: slots
`[0]` = current sample write cursors `(valid, invalid)`, `[1]` = total counts, `[2]` = the
coprime shuffle seeds, `[3+accumIndex]` for `accumIndex ∈ [0,128)` = the per-frame `(validCount,
invalidCount)` ring (`renderGlobalIllum.fx:264-265`, `renderSampleRefine.fx:38,79,89`). It is a
**128-frame ring of GI sample counts** — the GI analogue of the TAA `taaSamples` ring (it is how
"up to 64 past frames of GI samples" — `02-research.md` §1.2.3 — is realised). Fixed-size, NOT
resized; but it must **NOT be zero-cleared every frame** (it carries the ring) — zero-clear only
on creation/resize. The `clearBucketsAndCalcMask` pass clears slot `[3+accumIndex]` per frame
(`renderSampleRefine.fx:36-40`) — that is the ring's per-frame slot reset, done in-shader.

**Indirect dispatch:** wgpu supports `dispatch_workgroups_indirect`. `globalIllum` is dispatched
indirect off `ray_queue_indirect` (`WorldRenderBase.cs:323`); `sampleRefine`'s `CountValidAndRefine`
and `CountInvalid` are dispatched indirect off `valid_dispatch` / `invalid_dispatch`
(`WorldRenderBase.cs:356,359`). The indirect args are written by `rayQueueCalc`'s
`calcRayQueueStore` pass and `sampleRefine`'s `ValidHistory` pass respectively — see §7, §8.2.

### 3.8 `GpuGiParams` + the `AppArgs` GI config

The `WorldRenderBase` ImGui sliders (`WorldRenderBase.cs:14-25`, `SettingDataRenderBase`) become
`AppArgs` constants (no GUI — §1). Defaults from `SettingDataRenderBase`:

```rust
// src/main.rs — AppArgs gains a nested struct (or flat fields):
pub struct GiSettings {
    pub bounce_count: u32,                  // 3
    pub global_illum_max_accum: u32,        // 128
    pub spatial_resample_size: f32,         // 500.0
    pub spatial_visibility_count: u32,      // 80  (MAX_RAY_STEPS_VISIBILITY-ish; HLSL passes it but the WGSL caps via the const — see §8.3)
    pub denoise_thresh: f32,                // 400.0
    pub radius_lit_factor: f32,             // 3.0
    pub noise_suppression_factor: f32,      // 0.4
    pub skip_samples: bool,                 // true  — the 1↔0.25 spp toggle
    pub is_denoise: bool,                   // true
    pub is_sample_leveling: bool,           // true
    pub is_varying_resampling_radius: bool, // true
    pub is_atmosphere_interaction: bool,    // true
}
```
Note `taaSampleMaxAge` is NOT here — it stays the A-2 `TAA_SAMPLE_AGE = 16` const (`taa.rs:223`;
the C# `base/` default is 32 but A-2's §6 lever caps the ring at 16, so `sample_age` stays
clamped to 16 — `06-design-a2.md` §7.1).

`GpuGiParams` `#[repr(C)]` mirrors the union of every GI pass's scalar uniforms (verified against
`rayQueueCalc.fx:9-10`, `renderGlobalIllum.fx:16-28`, `renderSampleRefine.fx:21-32`,
`renderSpatialResampling.fx:15-27`, `renderDenoiseSplit.fx:11-14`):

```rust
// src/render/gpu_types.rs
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuGiParams {
    pub inv_view_proj: Mat4,   // C# invCamMatrix — getRayDir in globalIllum / spatialResampling
    pub view_proj: Mat4,       // C# camMatrix — sampleRefine's reproject; the rotation-only view-proj
    pub cam_pos_int: IVec3,
    pub _pad0: u32,
    pub cam_pos_frac: Vec3,
    pub _pad1: u32,
    pub sky_sun_dir: Vec3,
    pub _pad2: u32,
    pub sun_color: Vec3,        // C# sunColor = Atmosphere.GetLightForPoint — see §9.2
    pub _pad3: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub frame_count: u32,       // C# frameCount / frameIndex
    pub taa_index: u32,
    pub accum_index: u32,       // globalIllumMaxAccum - (frameCount % globalIllumMaxAccum) - 1
    pub rand_counter: u32,      // per-frame RNG salt — see §10.3
    pub rand_counter2: u32,
    pub max_bounce_count: u32,  // GiSettings.bounce_count (3)
    pub bucket_size_x: u32,
    pub bucket_size_y: u32,
    pub bucket_count: u32,
    pub sample_max_accum: u32,  // GiSettings.global_illum_max_accum (128)
    pub valid_sample_storage_count: u32,    // 2
    pub invalid_sample_storage_count: u32,  // 8
    pub bucket_storage_count: u32,          // 32
    pub refined_bucket_storage_count: u32,  // 8
    pub spatial_resample_size: f32,         // 500.0
    pub radius_lit_factor: f32,             // 3.0
    pub noise_suppression_factor: f32,      // 0.4
    pub denoise_thresh: f32,                // 400.0
    pub spatial_visibility_count: u32,      // 80 (informational; the WGSL uses the const cap — §8.3)
    pub flags: u32,             // packed skip_samples / is_denoise / is_sample_leveling / is_varying_resampling_radius / is_atmosphere_interaction
    pub _pad4: u32,
    pub _pad5: u32,
}
```
Lay it out 16-byte-aligned; add a compile-time size assert. The `flags` bits:
`GI_FLAG_SKIP_SAMPLES = 1<<0`, `GI_FLAG_IS_DENOISE = 1<<1`, `GI_FLAG_IS_SAMPLE_LEVELING = 1<<2`,
`GI_FLAG_IS_VARYING_RADIUS = 1<<3`, `GI_FLAG_IS_ATMOSPHERE_INTERACTION = 1<<4`.

### 3.9 `GpuAtmosphereParams`

Mirrors the `atmosphereRaw.fxh:6-19` sky uniforms (from `UiSkyDebug.cs`'s defaults + the
`SetShaderData` scaling at `UiSkyDebug.cs:63-79`) + `renderAtmosphere.fx:7`'s `camPos`,
`atmosphereTexSizeX/Y`, `frameCount`:

```rust
// src/render/gpu_types.rs
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuAtmosphereParams {
    pub cam_pos: Vec3,                 // camPos.toVector3() — only .y is used (renderAtmosphere.fx:25)
    pub _pad0: u32,
    pub sky_sun_dir: Vec3,
    pub _pad1: u32,
    pub sky_rayleigh_scatter: Vec3,    // (5.802, 13.558, 33.1)
    pub _pad2: u32,
    pub sky_ozone_absorb: Vec3,        // (0.650, 1.881, 0.085)
    pub _pad3: u32,
    pub sky_sun_color: Vec3,           // skySunColor * skySunIntensity = (1,1,1)*10
    pub _pad4: u32,
    pub sky_mie_scatter: f32,          // 2.5
    pub sky_sphere_radius: f32,        // 50000.0 * 100
    pub sky_atmosphere_thickness: f32, // 50000.0
    pub sky_atmosphere_density: f32,   // 14.0 * 0.01  (UiSkyDebug.cs:73 scaling)
    pub sky_absorb_intensity: f32,     // 3.0
    pub sky_scatter_intensity: f32,    // 1.35 * 0.000001  (UiSkyDebug.cs:75 scaling)
    pub sky_mie_factor: f32,           // 0.85
    pub sky_main_ray_steps: u32,       // 24
    pub sky_sub_scatter_steps: u32,    // 6
    pub atmosphere_tex_size_x: u32,    // 1024
    pub atmosphere_tex_size_y: u32,    // 1024
    pub frame_count: u32,
}
```
The sky parameters are **constants** (from `UiSkyDebug.cs`'s field initialisers — there is no GUI
in the port). `cam_pos` + `frame_count` are per-frame. `sky_sun_dir` is per-frame: NAADF rotates
the sun (`WorldRender.cs:91` — `Vector3.Transform((0,1,0), RotZ(sunAngle)·RotY(30°))`). Phase A's
`prepare_frame_gpu` already computes a fixed `sky_sun_dir` (`prepare.rs:281-288`); Phase B uses
the **same `sky_sun_dir` for both `GpuAtmosphereParams` and `GpuGiParams`** — extract it once.
Note: `skyAtmosphereAveragePoint` is in `UiSkyDebug` but the shaders never read it — omit. The
C# `Atmosphere.cs` mirrors `densityAtHeight` etc. on the CPU for `GetLightForPoint` (the GI sun
colour) — that is §9.2.

### 3.10 Bind-group plan summary

Phase B keeps the Phase-A `@group(0)` world layout (`chunks`/`blocks`/`voxels`/`voxel_types`/
`world_meta`) — every GI pass that traverses (`first_hit`, `globalIllum`, `spatialResampling`)
binds it. The TAA / atmosphere / rayQueue / sampleRefine / denoise passes do **not** traverse the
voxel world, so they bind no `@group(0)` world layout. Each new pass gets its own bind-group
layout (§4 enumerates them per node). Because Phase B has many passes sharing many buffers, the
**recommended layout convention**: `@group(0)` = world data (for traversing passes only),
`@group(1)` = per-pass-specific buffers + the pass's uniform. This keeps each pipeline's layout
list short and avoids one mega-layout. The bind groups are built in `prepare_gi` /
`prepare_atmosphere` (after the buffers they reference exist — §10.2 sequences this).

---

## 4. The render-graph plan (brief item 3)

### 4.1 The node set

Phase A-2's graph is the three-node chain `naadf_first_hit → naadf_taa_reproject →
naadf_final_blit` in `Core3dSystems::PostProcess`, before `tonemapping` (`render/mod.rs:89-99`).
Phase B expands it to NAADF's full deferred pipeline. Each new node is a `Core3d`-schedule system
(the Phase-A "render-graph node = a system that records commands" pattern — `graph.rs:1-22`),
wrapped in a `time_span` for the HUD, skipping silently until its resources + pipeline exist.

NAADF's dispatch order is **verified from `WorldRenderBase.cs:197-443`**:

```
atmosphere precompute       (renderSky        — WorldRenderBase.cs:205-206)
first_hit (4-plane)         (firstHitEffect   — :228-229)
TAA reproject (ReprojectOld) (renderTaaSample — :252-253)        [the existing A-2 node, base/ variant]
sampleRefine: ClearBucketsAndCalcMask  (sampleRefineEffect — :272-273)
rayQueueCalc: RayQueue + RayQueueStore (rayQueueEffect    — :285-288)
globalIllum (indirect)      (globalIllumEffect — :322-323)
sampleRefine: ValidHistory             (:352-353)
sampleRefine: CountValidAndRefine (indirect) (:355-356)
sampleRefine: CountInvalid (indirect)        (:358-359)
sampleRefine: RefineBuckets            (:361-362)
spatialResampling           (spatialResamplingEffect — :396-397)
denoiseSplit: CalcDenoiseHorizontal    (denoiseEffect — :412-413)   [if is_denoise]
denoiseSplit: CalcDenoiseVertical      (:415-416)                    [if is_denoise]
TAA: CalcNewTaaSample       (renderTaaSample — :421-422)
renderFinal (fullscreen)    (renderFinal — :440-443)
```

**The `ClearBucketsAndCalcMask` pass runs BEFORE `rayQueueCalc`** (it shares the `groupCount`
indirect buffer that `rayQueueCalc` then fills — `WorldRenderBase.cs:270` binds
`rayQueueIndirectBuffer` into `sampleRefine`'s `groupCount`, and `ClearBucketsAndCalcMask` zeroes
it at `:39`). And the `sampleRefine` 5 passes are **interleaved** with `rayQueueCalc` and
`globalIllum`, not a contiguous block. The render-graph node grouping must respect this exact
order.

### 4.2 The render-graph `.chain()` (brief: "specify the graph edges")

`render/mod.rs` rebuilds the `Core3d` chain. Each entry below is one `Core3d`-schedule system;
`.chain()` gives the render-graph edges; wgpu's automatic buffer barriers serialise the
shared-buffer accesses (the Phase-A/A-2 pattern — `mod.rs:88-89`). NAADF's `sampleRefine` 5
passes split across the timeline, so they are **5 separate node systems** (not one node with 5
dispatches — the order interleaves with other effects). Final chain:

```rust
.add_systems(Core3d, (
    naadf_atmosphere_node,                   // §9.2
    naadf_first_hit_node,                    // §6 — the EXISTING node, now 4-plane (its system body grows)
    naadf_taa_reproject_node,                // §5.8 — the EXISTING A-2 node, now base/ variant (writes taaDistMinMax)
    naadf_sample_refine_clear_node,          // §8.2 pass 1: ClearBucketsAndCalcMask
    naadf_ray_queue_node,                    // §7  — RayQueue + RayQueueStore (2 dispatches in one node)
    naadf_global_illum_node,                 // §8.1 — indirect dispatch off ray_queue_indirect
    naadf_sample_refine_valid_history_node,  // §8.2 pass 2: ValidHistory (1,1,1 dispatch)
    naadf_sample_refine_count_valid_node,    // §8.2 pass 3: CountValidAndRefine (indirect)
    naadf_sample_refine_count_invalid_node,  // §8.2 pass 4: CountInvalid (indirect)
    naadf_sample_refine_buckets_node,        // §8.2 pass 5: RefineBuckets
    naadf_spatial_resampling_node,           // §8.3 — Algorithm 2
    naadf_denoise_node,                      // §9.1 — CalcDenoiseHorizontal + CalcDenoiseVertical (2 dispatches; gated on is_denoise)
    naadf_calc_new_taa_sample_node,          // §5.8 — folds the GI result into the TAA history
    naadf_final_blit_node,                   // §5.9 — the EXISTING node, base/ variant
)
    .chain()
    .in_set(Core3dSystems::PostProcess)
    .before(tonemapping))
```

13 distinct node systems; `naadf_first_hit_node` / `naadf_taa_reproject_node` /
`naadf_final_blit_node` are the **existing** A-2 systems whose *bodies* change (more bind groups,
the base/ shader); the other 10 are new. (NAADF's `WorldRenderBase` has ~25 dispatches per frame
— `02-research.md` §4.11 — these 13 node systems issue them: most nodes do one dispatch, the
ray-queue / denoise / sample-refine-clear nodes do 1–2.)

### 4.3 `naadf_atmosphere_node` (§9.2)

- Resources: `Option<Res<AtmosphereGpu>>`, `Res<NaadfPipelines>`, `Res<PipelineCache>`.
- Pipeline: `atmosphere_pipeline` (`naadf_atmosphere.wgsl`, entry `precompute_atmosphere`).
- Bind group: `atmosphere_bind_group` (`@group(0)`): `atmosphere_params` uniform (binding 0,
  read), `atmosphere_comp` storage (binding 1, **read_write** — it writes one quarter per frame).
- Dispatch: `ceil((ATMOSPHERE_TEX_SIZE·ATMOSPHERE_TEX_SIZE / 4) / 64)` workgroups
  (`WorldRenderBase.cs:206` — `(1024·1024/4 + 63)/64`).
- Span: `"naadf_atmosphere"`.

### 4.4 `naadf_first_hit_node` — EXISTING node, body extended (§6)

Stays the same node *system*. Its compute pass now binds three groups: `@group(0)` world,
`@group(1)` frame (widened — `first_hit_data` + `taa_sample_accum` + `first_hit_absorption` +
`final_color` + `camera` + `render_params`), `@group(2)` taa-samples ring (unchanged), **plus a
new `@group(3)` atmosphere** (`atmosphere_comp` + `atmosphere_params` — the 4-plane first-hit
applies the precomputed atmosphere on a ray miss — `base/renderFirstHit.fx:73,124`). Dispatch
unchanged (`ceil(pixel_count/64)`). The pipeline layout grows from `[world, frame, taa]` to
`[world, frame, taa, atmosphere]` and the `@group(1)` `frame_layout` widens by 2 bindings.

### 4.5 `naadf_ray_queue_node` (§7)

- Resources: `Option<Res<FrameGpu>>` (for `first_hit_data`), `Option<Res<TaaGpu>>` (for
  `taa_sample_accum`), `Option<Res<GiGpu>>` (for `ray_queue`, `ray_queue_indirect`, `gi_params`).
- Two dispatches in one node (NAADF's `RayQueue` then `RayQueueStore` passes —
  `WorldRenderBase.cs:285-288`): `ray_queue_pipeline` (`ceil(pixel_count/64)` workgroups) then
  `ray_queue_store_pipeline` (`1` workgroup — `[numthreads(1,1,1)]`, `rayQueueCalc.fx:36`).
- Bind group `ray_queue_bind_group` (`@group(0)`): `gi_params` uniform, `first_hit_data` (read),
  `ray_queue` (rw), `ray_queue_indirect` (rw), `taa_sample_accum` (read).
- Span: `"naadf_ray_queue"`.

### 4.6 `naadf_global_illum_node` (§8.1)

- Resources: `@group(0)` world, `@group(1)` GI-specific (see §8.1 for the binding list),
  `@group(3)` atmosphere (it calls `applyAtmosphere` on a secondary-ray miss —
  `renderGlobalIllum.fx:132`).
- Dispatch: **`dispatch_workgroups_indirect(ray_queue_indirect, 0)`** —
  `WorldRenderBase.cs:323` `DispatchComputeIndirect`. The indirect args were written by
  `naadf_ray_queue_node`'s `RayQueueStore` pass.
- Span: `"naadf_global_illum"`.

### 4.7 The five `naadf_sample_refine_*_node`s (§8.2)

One node system per `renderSampleRefine.fx` pass, in the §4.2 order. Each binds
`sample_refine_bind_group` (a single shared bind group — every `sampleRefine` pass binds the same
buffer set; `WorldRenderBase.cs` re-binds the same effect 5×). Dispatches:
- `naadf_sample_refine_clear_node`: `ceil(bucket_count/64)` workgroups
  (`WorldRenderBase.cs:273`).
- `naadf_sample_refine_valid_history_node`: `1` workgroup (`[numthreads(1,1,1)]` —
  `renderSampleRefine.fx:71`, `WorldRenderBase.cs:353`).
- `naadf_sample_refine_count_valid_node`: `dispatch_workgroups_indirect(valid_dispatch, 0)`
  (`WorldRenderBase.cs:356`).
- `naadf_sample_refine_count_invalid_node`: `dispatch_workgroups_indirect(invalid_dispatch, 0)`
  (`WorldRenderBase.cs:359`).
- `naadf_sample_refine_buckets_node`: `ceil(bucket_count/64)` workgroups
  (`WorldRenderBase.cs:362`).
- Spans: `"naadf_sample_refine_clear"`, `"_valid_history"`, `"_count_valid"`, `"_count_invalid"`,
  `"_buckets"` — or one combined `"naadf_sample_refine"` span wrapping all five (the HUD has a
  fixed budget — one span is cleaner; designer's call, one span recommended).

### 4.8 `naadf_spatial_resampling_node` (§8.3)

- Resources: `@group(0)` world (it traverses for visibility rays + sun rays), `@group(1)`
  spatial-specific bindings (§8.3).
- Dispatch: `ceil(pixel_count/64)` workgroups (`WorldRenderBase.cs:397`).
- Span: `"naadf_spatial_resampling"`.

### 4.9 `naadf_denoise_node` (§9.1)

- Two dispatches: `denoise_horizontal_pipeline` then `denoise_vertical_pipeline`, each
  `ceil(pixel_count/64)` workgroups (`WorldRenderBase.cs:412-416`).
- **Gated on `GiSettings.is_denoise`** (`WorldRenderBase.cs:400`): when off, the node
  early-returns (the spatial-resampling pass already wrote `final_color` directly in its
  non-denoise branch — `renderSpatialResampling.fx:391-398`). Mirror A-2's `naadf_taa_reproject_
  node` gate on `ExtractedTaaConfig.enabled` (`graph.rs:127-129`) — extract `is_denoise` into the
  render world (it is in `ExtractedGiConfig` — §10.2) and gate on it.
- Bind group `denoise_bind_group` (`@group(0)`): `gi_params`, `first_hit_absorption` (read),
  `denoise_preprocessed` (read), `denoise_preprocessed_horizontal` (rw), `final_color` (rw).
- Span: `"naadf_denoise"`.

### 4.10 `naadf_calc_new_taa_sample_node` (§5.8)

- The second pass of `base/renderTaaSampleReverse.fx` (`CalcNewTaaSample` — `:170-206`). Folds
  the denoised GI result (`final_color`) into the 16-frame TAA history (writes one `taa_samples`
  ring slot + updates `taa_sample_accum`).
- Resources: `Option<Res<TaaGpu>>`, `Option<Res<FrameGpu>>` (for `first_hit_data` + `final_color`).
- Pipeline: `calc_new_taa_sample_pipeline` (`taa.wgsl`, entry `calc_new_taa_sample`).
- Bind group: a new `calc_new_taa_sample_bind_group` — `taa_params` uniform, `first_hit_data`
  (read), `final_color` (read), `voxel_types` (read — it calls `decompressVoxelType` for the
  roughness — `renderTaaSampleReverse.fx:184`), `taa_samples` (rw), `taa_sample_accum` (rw).
  **Note:** this needs `voxel_types` from `@group(0)` — so either bind `@group(0)` world OR add
  `voxel_types` to this bind group. It does NOT traverse, so binding only `voxel_types` (not the
  whole world layout) is cleaner — add it to the pass's own group.
- Dispatch: `ceil(pixel_count/64)` workgroups.
- **Gated on `ExtractedTaaConfig.enabled`** — same as `naadf_taa_reproject_node`. When TAA is off,
  this node is skipped and `final_color` is blitted directly... no — actually when TAA is off the
  `renderFinal` reads `taa_sample_accum`, which `CalcNewTaaSample` is what fills with the GI
  result. **For Phase B, TAA is always on** (the A-2 done-bar; `AppArgs.taa` default `true` —
  `main.rs:51`). Keep the gate for the runtime toggle, but note: with TAA off in Phase B the GI
  result would not reach the blit. Document: the `AppArgs.taa`-off path is "first-hit albedo
  only, no GI in the final image" — acceptable for the A/B toggle, the GI passes still run but
  their result is not folded in. (This matches NAADF: `CalcNewTaaSample` is the *only* path GI
  → `taaSampleAccum` → `renderFinal`.)
- Span: `"naadf_calc_new_taa_sample"`.

### 4.11 `naadf_final_blit_node` — EXISTING node, base/ variant (§5.9)

Structurally unchanged (fullscreen pass, reads `taa_sample_accum`, tonemaps). The only change is
the fragment shader: `naadf_final_b.wgsl` adds the `toneMappingFac` uniform term (`base/
renderFinal.fx:55` — `tv = curColor / (toneMappingFac + curColor)` vs. the A-2
`naadf_final.wgsl:58` hardcoded `1.0`). `toneMappingFac` is a new field in `GpuRenderParams` OR
the blit reuses `GpuGiParams` — §5.9 decides (recommend: add `tone_mapping_fac: f32` to
`GpuRenderParams`, replacing one of its `_pad` slots — it is the cleanest, no layout-size change
if it takes a pad slot; verify the pad accounting). Span unchanged: `"naadf_final_blit"`.

### 4.12 HUD timing lines

`src/hud.rs`: the HUD has a fixed per-node line budget. Phase B adds ~10 nodes — too many to list
individually. **Decision:** add lines for the *expensive* nodes only — `atmosphere`, `first-hit`,
`global-illum`, `sample-refine` (one combined span), `spatial-resampling`, `denoise`,
`taa-reproject`, `final-blit`. The cheap nodes (`ray-queue`, `calc-new-taa-sample`,
`sample-refine-clear`) fold into nothing or share a span. Each line follows the existing
`write_timing` + `const`-checked path-pair pattern (`hud.rs:34-63,170-190`). Update the
renderer-mode string to `"Renderer: NAADF (Phase B — real-time GI)"`.

---

## 5. The WGSL ports — provenance tables (brief item 4)

Each WGSL file names the HLSL `.fx`/`.fxh` it derives from, extending the `03-design.md` §5.5 /
`06-design-a2.md` provenance-table style. Every matrix multiply uses `M*v` + the perspective
`w`-divide (the `05-review.md` perspective-fix convention — `06-design-a2.md` §4.2; every HLSL
`mul(v, M)` becomes `M * v`). Every `#ifdef ENTITIES` block is **omitted** (§1).

### 5.1 Phase-B WGSL file → HLSL provenance

| WGSL file | derives from (HLSL) | contents |
|---|---|---|
| `naadf_first_hit.wgsl` (REPLACED) | `base/renderFirstHit.fx` `calcFirstHit` | 4-plane-bounce first-hit (§6): the 4-iteration specular-bounce loop, `compress_first_hit_data` (the 5-arg `base/` variant), `firstHitAbsorption` + `finalColor` writes, `applyAtmosphere` on a miss, the `taa_samples` ring write (kept from A-2). |
| `naadf_atmosphere.wgsl` | `base/renderAtmosphere.fx` `precomputeAtmosphere` | the compute entry: octahedral decode, `addLightForDirection` (ray-marched), write one quarter of `atmosphere_comp` per frame. |
| `ray_queue_calc.wgsl` | `base/rayQueueCalc.fx` `calcRayQueue` + `calcRayQueueStore` | the two compute entries: `shouldRay` (the `taa_sample_accum`-driven adaptive test), the group-shared counter (`addToCounterAddressBuffer` ported inline — §7), the queue write, the indirect-args store. |
| `naadf_global_illum.wgsl` | `base/renderGlobalIllum.fx` `calcGlobalIlum` | the ≤3-bounce secondary-ray tracer; `compress_sample_valid` / `compress_sample_invalid`; the group-shared sample-count atomics; the lit/unlit split write into the sample-list ring. |
| `sample_refine.wgsl` | `base/renderSampleRefine.fx` (5 passes) | `clear_buckets_and_calc_mask`, `compute_valid_history`, `count_valid_data_and_refine`, `count_invalid_data`, `refine_buckets` — 5 compute entries; the `COLOR_DIF_PROB` brightness-leveling. |
| `spatial_resampling.wgsl` | `base/renderSpatialResampling.fx` `calcSpatialResampling` | Algorithm 2: `sampleNeighbors` (the 12-iteration reservoir loop + the adaptive-radius 12-tap pre-pass), the Jacobian, `getTargetFunctionNew`, the single 3-step visibility ray, the sun sample, the denoise-vs-final write split. |
| `denoise_split.wgsl` | `base/renderDenoiseSplit.fx` | `calc_denoise_horizontal` + `calc_denoise_vertical` — 2 compute entries; the sparse separable bilateral filter. |
| `naadf_final_b.wgsl` | `base/renderFinal.fx` `MainPS` | fullscreen tonemap fragment — the `base/` variant with `toneMappingFac`. |
| `color_compression.wgsl` (NEW shared) | `commonColorCompression.fxh` | `COLORS[32]`, `COLOR_DIF_PROB[31]`, `compress_color` (5-bit/channel exponential), `refine_comp_color`. |
| `atmosphere.wgsl` (NEW shared) | `atmosphereRaw.fxh` + `atmospherePrecomputed.fxh` | `add_light_for_direction`, `rayleigh`, `phase_function`, `density_at_height`, `ray_sphere`, `scatter_for_densities`, `get_scatter_densities_at_point`, `apply_atmosphere`. |
| `ray_tracing_common.wgsl` (EXTENDED) | `commonRayTracing.fxh:65-137` | gains `get_perpendicular_vector`, `get_uniform_hemisphere_sample`, `sample_vndf_isotropic`, `pdf_vndf_isotropic`, `geometry_term`. |
| `render_pipeline_common.wgsl` (EXTENDED) | `commonRenderPipeline.fxh` | gains the full `get_hit_data_from_planes` (entity branch omitted), `get_reflectance_fresnel`, `get_specular_normals`, `get_tang`, `get_screen_pos_projection`, `get_screen_index_projection`, the `FirstHitResult` + `SampleValid` structs, the `SPECULAR_MIRROR_FAC` const. `compress_first_hit_data` gains the `is_diffuse` arg. |
| `common.wgsl` (EXTENDED) | `commonOther.fxh:42-79` | gains `gaussian_f`, `gcd`, `find_coprime`, `next_pow2`. |
| `taa.wgsl` (EXTENDED) | `base/renderTaaSampleReverse.fx` | `reproject_old_samples` gains the `taa_dist_min_max` write; new `calc_new_taa_sample` entry; the A-2-local plane/projection helpers replaced by imports of the now-shared full versions. |

### 5.2 The shared-helper promotion (`render_pipeline_common.wgsl`)

A-2 deliberately put `get_hit_data_from_planes_a2` / `get_screen_pos_projection` /
`get_screen_index_projection` **local to `taa.wgsl`** (`06-design-a2.md` §7.3 — "this belongs in
`taa.wgsl`... not in `render_pipeline_common.wgsl` (which is where the full Phase-B version will
eventually go)"). Phase B executes that plan:

1. **`get_hit_data_from_planes`** — port the **full** `commonRenderPipeline.fxh:154-213` into
   `render_pipeline_common.wgsl`: the 3-iteration specular-reflection loop (`:164-181`), the
   `SPECULAR_MIRROR_FAC[7]` LUT, the tail (`:205-211`). The `#ifdef ENTITIES` block (`:183-203`)
   is **omitted**. The function signature drops the `entityInstancesHistory` + `taaIndex` params
   (entity-only). `taa.wgsl`'s `get_hit_data_from_planes_a2` is **deleted** — `taa.wgsl` imports
   the full version. Because planes 1–3 are now actually populated (4-plane first-hit — §6), the
   loop runs real iterations: this is correct, not a behaviour change vs. A-2 — A-2's single-plane
   reduction was *exactly* the loop with zero iterations, and the full version reduces to it when
   planes 1–3 are `HIT_UNDEFINED`.
2. **`get_screen_pos_projection` / `get_screen_index_projection`** — the A-2 `taa.wgsl` versions
   (`taa.wgsl:154-203`) are already faithful ports of `commonRenderPipeline.fxh:133-152` with the
   `M*v` convention. **Move them verbatim** into `render_pipeline_common.wgsl`; `taa.wgsl`,
   `sample_refine.wgsl` import them. (`renderSampleRefine.fx:158-160` does the same `mul(...,
   camMatrix)` + NDC test inline — port it onto the shared `get_screen_pos_projection`.)
3. **`get_reflectance_fresnel`, `get_specular_normals`, `get_tang`** — straight ports of
   `commonRenderPipeline.fxh:81-85, 107-117, 119-131`. `get_specular_normals` was hardcoded `0u`
   in A-2 (`naadf_first_hit.wgsl:171` — "always 0 in A-2"); Phase B's 4-plane first-hit makes it
   real — `naadf_first_hit.wgsl` + the TAA/GI passes call the real function.

### 5.3 `color_compression.wgsl` — provenance

Port `commonColorCompression.fxh`. The `COLORS[32]` / `COLOR_DIF_PROB[31]` tables are **compile-
time constants** in HLSL (`pow` expressions) — WGSL `const` arrays cannot hold `pow()` results
(WGSL `const` requires const-expressions; `pow` is not const-evaluable in WGSL). **Resolution:**
compute the 32 + 31 values **on the CPU** (in Rust, in `gi.rs` or a `color_compression` helper)
and either (a) hard-code them as literal `const` arrays in the WGSL (a code-gen-once step — write
a Rust unit test that prints them, paste the literals), or (b) upload them as a small uniform/
storage buffer. **Recommendation: (a)** — they are 63 fixed `f32`s, deterministic, never change;
hard-code the literals into `color_compression.wgsl` with a comment citing the formula
(`COLOR_EXP = 2^0.6`, `COLOR_START = 1/64`, `COLORS[i] = COLOR_START · COLOR_EXP^(i-1)` for
`i≥1`, `COLORS[0]=0`; `COLOR_DIF_PROB[i] = 1 - COLOR_EXP^(-i)`). A Rust `#[test]` that
recomputes them and asserts the WGSL literals match keeps them honest (the same discipline as
`gpu_types.rs`'s size asserts). `compress_color` uses `firstbithigh` — WGSL's equivalent is
`firstLeadingBit` (returns the bit index of the highest set bit; `firstbithigh` HLSL semantics
match `firstLeadingBit` for non-zero inputs — the HLSL guards with `max(1, ...)`, so port that
guard). `refine_comp_color` is straightforward.

### 5.4 `atmosphere.wgsl` — provenance + the `const`-array problem

Port `atmosphereRaw.fxh` + `atmospherePrecomputed.fxh`. No `const` arrays here — all per-call
math. The `addLightForDirection` HLSL signature has `const bool` / `const int` template-style
params (`includeMie`, `mainIterationCount`, `secondIterationCount`, `includeSun`) — WGSL has no
default args and no `const`-param specialisation; port them as plain runtime args (the call sites
pass `gi_params`/`atmosphere_params` values). The two call sites: `precomputeAtmosphere` passes
`(true, skyMainRaySteps, skySubScatterSteps)` (`renderAtmosphere.fx:25`); `applyAtmosphere`
samples the precomputed buffer (`atmospherePrecomputed.fxh:9-22`). `octEncode`/`octDecode` are
already in `ray_tracing_common.wgsl` — `atmosphere.wgsl` imports them.

### 5.5 `naadf_global_illum.wgsl` — the group-shared atomics

`renderGlobalIllum.fx` uses `groupshared uint sharedResCount` + `InterlockedAdd` +
`GroupMemoryBarrierWithGroupSync` (`:30-32, 254-268`). WGSL: `var<workgroup> shared_res_count:
atomic<u32>;`, `atomicAdd`, `workgroupBarrier()`. The `globalResCountValid` /
`globalResCountInvalid` are also `groupshared` — port as `var<workgroup>`. The per-thread
`InterlockedAdd(globalIlumSampleCounts[3+accumIndex].x, ...)` is a **storage-buffer atomic** —
WGSL `atomicAdd(&sample_counts[...].x, ...)` requires the buffer element to be `atomic<u32>`.
**This is a constraint:** `sample_counts` must be declared `array<vec2<atomic<u32>>>` (or a
struct of two `atomic<u32>`) in every shader that atomically writes it (`naadf_global_illum.wgsl`,
`sample_refine.wgsl`). The shaders that only *read* `sample_counts` (also `sample_refine.wgsl`'s
later passes) read `atomicLoad`. WGSL allows a storage buffer to be `array<atomic<u32>>` in one
binding and the same buffer non-atomic in another binding only if declared consistently per
pipeline — declare it atomic everywhere it is bound to be safe. **Flag for impl:** the
`sample_counts` buffer's WGSL type is `array<SampleCountSlot>` where `struct SampleCountSlot {
valid: atomic<u32>, invalid: atomic<u32> }`.

### 5.6 `ray_queue_calc.wgsl` — `addToCounterAddressBuffer` port

`rayQueueCalc.fx`'s `calcRayQueue` uses `addToCounterAddressBuffer` (`commonOther.fxh:6-22`) — a
group-shared prefix-counter that atomically reserves a contiguous range in the
`RWByteAddressBuffer groupCount`. Port it **inline** into `ray_queue_calc.wgsl` (it is not a
reusable shared function — it uses `var<workgroup>` + `workgroupBarrier()` which must be at the
entry-point scope): `var<workgroup> index_group: atomic<u32>;`, `var<workgroup> index_group_base:
u32;`, the three `workgroupBarrier()`s, `atomicAdd(&index_group, ...)`, and
`atomicAdd(&ray_queue_indirect[...], ...)` for the global counter (the `RWByteAddressBuffer
groupCount` is `ray_queue_indirect` — `WorldRenderBase.cs:280` binds `rayQueueIndirectBuffer` into
`groupCount`; the `.Load(0)`/`.Store(0)` byte-address ops at offset 0 are element `[0]` of the
indirect buffer = `GroupCountX`). `calcRayQueueStore` (`[numthreads(1,1,1)]`) reads
`ray_queue_indirect[0]`, computes `(v + 63) / 64`, writes it back — that converts the raw pixel
count into the workgroup count for the indirect `globalIllum` dispatch.

### 5.7 `sample_refine.wgsl` / `spatial_resampling.wgsl` — the heavy ports

These are the two biggest WGSL files (`renderSampleRefine.fx` = 441 lines, `renderSpatialResampling.fx`
= 406 lines). Port faithfully, function-by-function, with named provenance comments per block
(the §5.1 table covers the file-level provenance; the WGSL adds per-function comments). Key
WGSL-specific port notes:
- **`static uint compColorMaxStorage[32]`** (`renderSampleRefine.fx:359`) — HLSL function-scope
  `static` array; WGSL has no function-`static`. It is per-thread scratch sized 32 — port as a
  `var<function> comp_color_max_storage: array<u32, 32>;` local. (It is bounded by
  `effectiveValidCount ≤ bucketStorageCount = 32` — fits.)
- **`InterlockedAdd` on `globalIlumBucketInfo`** (`renderSampleRefine.fx:195, 337`) —
  `bucket_info` must be `array<atomic<u32>>` (or a struct with one `atomic<u32>` + one plain
  `u32`) where atomically written. Same constraint-flag as §5.5.
- **`ShuffleGroup`** + `findCoprime`/`nextPow2` — pure functions, port straight (`find_coprime` /
  `next_pow2` go in `common.wgsl` per §2.2).
- `spatial_resampling.wgsl` traverses (`shootRay` for visibility + sun) — it imports
  `ray_tracing.wgsl` and binds `@group(0)` world.
- The `getSampleData` out-param decode (`renderSpatialResampling.fx:29-38`) → a WGSL struct
  return (the A-2 `taa.wgsl` pattern for `decompressSample`).

### 5.8 `taa.wgsl` — the base/ TAA pass changes (§4.2, §4.10)

A-2 ported the `albedo/renderTaaSampleReverse.fx` (`06-design-a2.md` §7.6 — "A-2 ports the
`albedo/` version... no `taaDistMinMax` output, no `CalcNewTaaSample`"). Phase B switches to the
**`base/` variant**:
1. **`reproject_old_samples`** (the existing `taa.wgsl` entry) gains the `taa_dist_min_max` write
   — `base/renderTaaSampleReverse.fx:79`: `taa_dist_min_max[pixel_index] = vec2<u32>(f16(distMin)
   | f16(distMax)<<16, valid_normals_spec)`. The `valid_normals_spec` accumulation (`:68-70`) was
   "folded to a no-op" in A-2 (`taa.wgsl:280-281` — "always 0"); Phase B's 4-plane first-hit
   makes the specular-normals real, so the `valid_normals_spec` accumulation is **un-omitted** —
   port `:68-70` for real (it uses `get_specular_normals`, now a real shared function — §5.2).
   The `taa.wgsl` reproject `@group(0)` layout gains a `taa_dist_min_max` rw binding.
   - The HLSL `screenPosDistanceSqr > 16.0` reject (`base/renderTaaSampleReverse.fx:139`) vs.
     A-2's `> 1.0` (`taa.wgsl:354`) — the A-2 design (`06-design-a2.md` §7.4) used `> 1.0`
     ("within 1 pixel"); the `base/` HLSL actually uses `> 16.0`. **Port the `base/` value
     `16.0`** — Phase B follows the `base/` shader faithfully. (Flag this as an A-2→B
     reconciliation: A-2 read it as `1.0`; the `base/` source says `16.0`. The `base/` source is
     authoritative for Phase B.)
2. **`calc_new_taa_sample`** (NEW entry — `base/renderTaaSampleReverse.fx:170-206`): port
   `calcNewTaaSample`. It reconstructs the first-hit virtual path (`get_hit_data_from_planes` —
   the now-shared full version), decompresses the voxel type for roughness, reads `final_color`
   as `light`, computes `extra_data` (the 5-bit roughness — `:189-192`), `taa_compress_sample`s
   it into `taa_samples[(taa_index % 16) · w·h + pixel_index]` (the **16**-ring — the §6 lever;
   the HLSL `% 32` → `% TAA_SAMPLE_RING_DEPTH`), and folds `light` into `taa_sample_accum`
   (`sampleWeight + 1`, RGB accumulated — `:197-205`). **This is now the primary path the
   per-pixel sample-count signal is maintained for the GI loop** — the A-2 first-hit
   `taa_samples` write becomes redundant in the `base/` pipeline? **No** — verify: in the `base/`
   pipeline, `base/renderFirstHit.fx` does **NOT** write `taa_samples` (it writes `firstHitData`,
   `firstHitAbsorption`, `finalColor` — `:126-128` — but no `taaSamples` write; the `if(isTAA)`
   block is `albedo/`-only). So **the A-2 first-hit `taa_samples` ring write must be REMOVED**
   for Phase B — `calc_new_taa_sample` is the sole `taa_samples` writer in the `base/` pipeline.
   This is a §6 change: the 4-plane `naadf_first_hit.wgsl` drops the `if (FLAG_IS_TAA)` ring write
   (and the `@group(2)` taa-samples binding moves off the first-hit pipeline onto the
   `calc_new_taa_sample` pipeline). The `taa_sample_accum` write by `first_hit` **also changes**:
   in the `base/` pipeline `first_hit` writes `finalColor` (not `taaSampleAccum`); `taaSampleAccum`
   is written by `ReprojectOld` (the reprojected history) and then `CalcNewTaaSample` (history +
   new GI). So the `base/` `naadf_first_hit.wgsl` writes `final_color`, NOT `taa_sample_accum`.
   **This is the central restructure of the first-hit pass — see §6.3.**

### 5.9 `naadf_final_b.wgsl` vs. extending `naadf_final.wgsl`

`base/renderFinal.fx` differs from `albedo/renderFinal.fx` only in: (a) the `toneMappingFac`
uniform in the tonemap (`base/:55` vs `albedo/` hardcoded `1.0` — the A-2 `naadf_final.wgsl:58`
hardcodes `1.0`); (b) the `showRayStep` debug reads `firstHitData[pixelIndex].z & 0x7FFF`
directly (`base/:44`) — A-2's reads `col_samples.x` (`naadf_final.wgsl:48`). **Decision:** add
`tone_mapping_fac: f32` to `GpuRenderParams` (replacing a `_pad` slot — `GpuRenderParams` has
6 pad fields; `_pad0` at offset 28 is a clean slot — verify the 112-byte layout holds) and
**replace `naadf_final.wgsl` in place** with the `base/` behaviour (the `albedo/` and `base/`
final blits are 95% identical; a second file is gold-plating). The A-2 final blit reading
`taa_sample_accum` is unchanged — `CalcNewTaaSample` keeps `taa_sample_accum` as the blit source.
`tone_mapping_fac` default = `1.0` (NAADF's `Settings.data.general.toneMappingFac`; not exposed —
a constant in `prepare_frame_gpu`).

---

## 6. The 4-plane-bounce first-hit (brief item 5)

`base/renderFirstHit.fx` (137 lines) is the Phase-B first-hit. It is an **extension of the
existing `naadf_first_hit.wgsl`**, not a separate file — `naadf_first_hit.wgsl` is replaced in
place (the Phase-A/A-2 single-plane path becomes the `i==0` iteration of the 4-iteration loop).
This is the single biggest WGSL change in Phase B.

### 6.1 The 4-iteration specular-bounce loop

`base/renderFirstHit.fx:65-115` — a `[unroll]`'d `for (i = 0; i < 4; ++i)` loop:
- Each iteration: `shootRay` (the existing `shoot_ray`, unchanged); `normTangs[i] =
  rayResult.normalComp`.
- On a **miss**: `applyAtmosphere(oldPos, rayDir, absorption, light)` then `break`
  (`:71-75`). (`oldPos` is the ray origin before the bounce; `rayDirNoJitter` for `i==0`,
  `rayDir` otherwise — `:73`.)
- On a **hit**: advance the ray to the surface; if `isAtmosphereInteraction`, `addLightForDirection`
  along the segment travelled (`:84-86` — the ray-marched atmosphere, the `atmosphere.wgsl`
  function); decompress the voxel type.
- If the surface is **not `SURFACE_SPECULAR_MIRROR`**: apply albedo (unless `SURFACE_SPECULAR_ROUGH`),
  add emissive, set `distanceRay` / `voxelTypeRaw` / `isDiffuse`, `break` — this is the
  *terminating* hit (`:93-108`).
- If the surface **is `SURFACE_SPECULAR_MIRROR`**: `absorption *= getReflectanceFresnel(...)`;
  `rayDir = reflect(rayDir, normal)`; `oldPos` updated; loop continues to the next plane
  (`:110-114`).
- If the loop runs all 4 iterations without a non-mirror hit (`i == 4`): `normTangs[3] = 0x1FFFF`
  (the `HIT_NOTHING` marker), `distanceRay = -1` (`:117-121`).

WGSL: a `for (var i = 0u; i < 4u; i = i + 1u)` loop (WGSL has no `[unroll]` — naga unrolls
small constant loops; the loop body is identical). `norm_tangs` is a `vec4<u32>`; index it
`norm_tangs[i]` (WGSL allows dynamic indexing of a `vec4` via a `var` — but to be safe, use a
`var norm_tangs: array<u32, 4>` then assemble the `vec4` at the end). The `reflect` builtin
exists in WGSL.

### 6.2 The G-buffer pack — the 5-arg `base/` `compressFirstHitData`

`base/renderFirstHit.fx:18-26` — `compressFirstHitData(dist, normTangs, voxelTypeRaw, isDiffuse,
entity)`: `.y = isDiffuse | (normTangs.y << 15)` (the `albedo/` variant had `.y = 1 | ...` — the
A-2 `compress_first_hit_data` hardcodes `1u`). Phase B's `compress_first_hit_data` gains the
`is_diffuse` parameter (`render_pipeline_common.wgsl` change — §2.3). The `showRayStep` debug
stuffs `rayResult.stepCount` into `voxelTypeRaw` (`:126`) — port that conditional.

### 6.3 What changes vs. the A-2 first-hit (the central restructure)

The A-2 `naadf_first_hit.wgsl` writes `first_hit_data` + `taa_sample_accum` + (when `FLAG_IS_TAA`)
one `taa_samples` ring slot (`naadf_first_hit.wgsl:158-202`). The `base/` first-hit writes
`first_hit_data` + `first_hit_absorption` + `final_color` — **and NOT `taa_sample_accum`, NOT
`taa_samples`** (verified: `base/renderFirstHit.fx:126-128` writes exactly those three; there is
no `taaSamples` / `taaSampleAccum` write in the `base/` first-hit — that moved to
`base/renderTaaSampleReverse.fx`'s two passes).

So the Phase-B `naadf_first_hit.wgsl`:
- **Keeps** the `@group(0)` world + `@group(1)` camera/params + the `first_hit_data` write.
- **Adds** `first_hit_absorption` + `final_color` writes to `@group(1)` (the 2 new `FrameGpu`
  buffers — §3.4). `firstHitAbsorption[id] = uint2(f16(absorption.x)|f16(absorption.y)<<16,
  f16(absorption.z))`; `finalColor[id] = uint2(f16(light.x)|f16(light.y)<<16, f16(light.z))`
  (`base/renderFirstHit.fx:127-128`).
- **Adds** `@group(3)` atmosphere (`atmosphere_comp` + `atmosphere_params`) — `applyAtmosphere`
  and `addLightForDirection` need it.
- **Removes** the `taa_sample_accum` write (the A-2 `pack2x16float(vec2(1.0, light.r))` block —
  `naadf_first_hit.wgsl:189-202`). `taa_sample_accum` is now written by `ReprojectOld` +
  `CalcNewTaaSample`.
- **Removes** the `@group(2)` `taa_samples` ring write (the A-2 `if (FLAG_IS_TAA)` block —
  `naadf_first_hit.wgsl:170-187`). `taa_samples` is now written by `CalcNewTaaSample`.
- The `@group(2)` `taa_layout` (`taa_samples`) **moves off** the first-hit pipeline onto the
  `calc_new_taa_sample` pipeline (§4.10) — `pipelines.rs` change.

**Consequence for `prepare`/`graph`:** `prepare_taa`'s `taa_first_hit_bind_group` is renamed/
re-purposed to `taa_calc_new_sample_bind_group` (bound by `naadf_calc_new_taa_sample_node`, not
`naadf_first_hit_node`). The `naadf_first_hit_node` body drops `pass.set_bind_group(2,
&taa_gpu.taa_first_hit_bind_group, ...)` (`graph.rs:94`) and gains `pass.set_bind_group(3,
&atmosphere_gpu.bind_group, ...)`. The first-hit pipeline layout goes `[world, frame, taa]` →
`[world, frame, atmosphere]` (taa group removed, atmosphere group added). **Flag this for impl:**
it is a non-trivial rewire of the A-2 first-hit node — the A-2 first-hit `taa_samples` write was
correct for the A-2 *albedo* pipeline but the `base/` pipeline restructures it.

### 6.4 VNDF/GGX in the first-hit

The 4-plane loop only does **mirror** reflections (`reflect` + Fresnel — `base/renderFirstHit.fx:110-113`);
it does NOT do rough-specular VNDF sampling (that is in `renderGlobalIllum` — `:101-113`). So the
first-hit pass needs `get_reflectance_fresnel` (new shared fn) but **not** `sample_vndf_isotropic`.
The VNDF/GGX functions are needed by `naadf_global_illum.wgsl` + `spatial_resampling.wgsl` (§8).
Be precise: §2.2's "VNDF/GGX into `ray_tracing_common.wgsl`" is correct, but the *first-hit*
consumer of that module is only `get_reflectance_fresnel` (which is actually in
`commonRenderPipeline.fxh`, not `commonRayTracing.fxh` — so it goes in
`render_pipeline_common.wgsl`). The first-hit pass does **not** import the VNDF functions.

---

## 7. `rayQueueCalc` — the adaptive ~0.25-spp data flow (brief item 6)

This is the headline 2× speedup. The data flow, verified end-to-end against `rayQueueCalc.fx` +
`WorldRenderBase.cs:276-288` + the A-2 `taa_sample_accum` signal:

### 7.1 The signal source

A-2 exposes the per-pixel accumulated sample count in `taa_sample_accum[px].x & 0xFFFF` as an f16
(`06-design-a2.md` §2.2; verified in `08-review-a2.md` §1 — `pack2x16float(vec2(sample_weight +
color_sum.a, ...))`). After `ReprojectOld` runs (and before `rayQueueCalc`), `taa_sample_accum[px].x
& 0xFFFF` holds `color_sum.a` = the count of accepted reprojected history samples for that pixel
this frame (the `base/` `ReprojectOld` writes `taaSampleAccum[id] = uint2(f16(colorSum.w) |
f16(colorSum.r)<<16, ...)` — `base/renderTaaSampleReverse.fx:167` — `colorSum.w` is the accepted
count). **Render-graph ordering guarantees this**: `naadf_taa_reproject_node` runs before
`naadf_ray_queue_node` in the §4.2 chain (NAADF's order — `WorldRenderBase.cs:252` ReprojectOld,
`:285` RayQueue).

### 7.2 `calcRayQueue` — the adaptive test

`rayQueueCalc.fx:12-34`:
```
shouldRay(pos, accum):
    if !skipSamples: return true
    fac = accum / 2.0
    modSize = round(clamp(fac * 2, 0, 3) + 1)        // modSize ∈ {1,2,3,4}
    return ((frameIndex * 4 + pos.x + pos.y) % modSize) == 0
```
`accum` is `f16tof32(taaSampleAccum[ID].x & 0xFFFF)` (`rayQueueCalc.fx:29`). A well-converged
pixel (high `accum`) gets a large `modSize` → rayed only every 4th frame on a spatial-temporal
pattern → **~0.25 spp**. A freshly-disoccluded pixel (`accum` near 0) gets `modSize == 1` → rayed
every frame → 1 spp. The `shouldAdd` gate is `(firstHitData[ID].z & 0x7FFF) != 0 && shouldRay(...)`
— `firstHitData.z & 0x7FFF` is the `voxelTypeRaw` (`0` = miss → no GI ray needed). When
`skipSamples` is false, every hit pixel is rayed (1 spp) — the `GI_FLAG_SKIP_SAMPLES` toggle is
the 1↔0.25-spp switch (`WorldRenderBase.cs:48` "Toggles between 1spp and 0.25 spp"); default on.

WGSL: `should_ray` is a plain function reading `gi_params.flags & GI_FLAG_SKIP_SAMPLES` and
`gi_params.frame_count` (the C# `frameIndex`). `accum` = `unpack2x16float(taa_sample_accum[id].x).x`.

### 7.3 The queue + indirect args

`calcRayQueue` uses the group-shared prefix-counter (`addToCounterAddressBuffer` — §5.6) to
atomically reserve a slot in the global counter at `ray_queue_indirect[0]` and writes
`pixelsToRender[index] = pixelPos.x | (pixelPos.y << 16)` (`rayQueueCalc.fx:30-33`).
`calcRayQueueStore` (`[numthreads(1,1,1)]`) reads `ray_queue_indirect[0]`, sets it to
`(count + 63) / 64` (`rayQueueCalc.fx:36-41`) — the workgroup count. `globalIllum` then dispatches
**indirect** off `ray_queue_indirect` (`WorldRenderBase.cs:323`), so it only launches one thread
per *queued* pixel — the GI cost scales with the adaptive rate, not the screen.

**`ray_queue_indirect` layout:** it is a `DispatchComputeArguments` (`WorldRenderBase.cs:135-136`)
= `{GroupCountX, GroupCountY, GroupCountZ}` packed as 3 (or 5 — the C# allocates 5) `u32`s. The C#
seeds `{0, 1, 1}` (`:136`). The port: `ray_queue_indirect` is a 5-`u32` buffer (`INDIRECT |
STORAGE | COPY_DST`), seeded `[0, 1, 1, 0, 0]` on creation (zero-clear then write `[1]=1, [2]=1`
— or write the full `[0,1,1,0,0]` once). `calcRayQueue` `atomicAdd`s into `[0]`;
`calcRayQueueStore` reads `[0]`, writes `[0] = (v+63)/64`. The `naadf_global_illum_node` calls
`pass.dispatch_workgroups_indirect(&gi_gpu.ray_queue_indirect, 0)`.

**Per-frame reset:** `ray_queue_indirect[0]` must be zeroed each frame *before* `calcRayQueue`
runs. NAADF does this in `ClearBucketsAndCalcMask` (`renderSampleRefine.fx:39` —
`groupCount.Store(0, 0)`), which runs before `RayQueue` in the §4.2 order. So
`naadf_sample_refine_clear_node` (the first sample-refine node) zeroes `ray_queue_indirect[0]` —
the port keeps that: `clear_buckets_and_calc_mask` writes `ray_queue_indirect[0] = 0u` when
`global_id.x == 0` (`renderSampleRefine.fx:36-40`).

---

## 8. Compressed ReSTIR GI (brief item 7)

### 8.1 `renderGlobalIllum` — the secondary-ray tracer

`base/renderGlobalIllum.fx` (299 lines). Port `calcGlobalIlum`:
- **Input:** `pixelsToRender[globalID.x]` (the ray queue — unpacked to `pixelPos`),
  `firstHitData`, the camera-history rings, the `gi_params` uniform, `@group(0)` world,
  `@group(3)` atmosphere.
- Reconstruct the first-hit virtual path with `get_hit_data_from_planes` (the shared full version).
- Compute the **primary-surface BRDF interaction** (`:97-116`): mirror → `reflect`;
  rough-specular → `sample_vndf_isotropic` loop (the new shared VNDF fn) + `geometry_term` +
  Fresnel; diffuse → `get_uniform_hemisphere_sample`.
- **The ≤3-bounce loop** (`:121-235`): `for bounce in 0..min(maxBounceCount, 3)`: `shootRay`
  (the existing `shoot_ray`, `MAX_RAY_STEPS_SECONDARY`); on miss, `applyAtmosphere` with
  probability `1/16` (Russian roulette — `:131-132`); on hit, apply albedo, sun-sample (with the
  rough-specular BRDF weighting — `:155-187`), emissive, then the surface-effect bounce
  (mirror/rough/diffuse — `:197-225`). Tracks `sampleDir` / `sampleDist` / `sampleNormalComp` /
  `normTangs` / `isFirstDiffuseHit` / `hitEmitterDirectly`.
- **Compress + classify** (`:237-289`): `compressColor(radiance, rand)` (the 5-bit/channel
  exponential — `color_compression.wgsl`); a **lit** sample (`radianceComp > 0`) →
  `compressSampleValid` → `globalIlumValidSamples`; an **unlit** sample → with probability `7/8`
  *skipped* (the "every 8th unlit sample stored, weighted ×8" — `02-research.md` §1.2.3;
  `:251-252` — `isSkip = !isValid && nextRand > 1/8`), else `compressSampleInvalid` →
  `globalIlumInvalidSamples`. The group-shared atomics (`sharedResCount`,
  `globalResCountValid/Invalid`) + the `globalIlumSampleCounts[3+accumIndex]` storage atomic
  count the per-frame lit/unlit totals (§5.5). The sample is written into the **ring** at
  `(samplesStartIndex + maxSampleCount + index - ...) % maxSampleCount` — a wrapping write into
  the 2-frame (lit) / 8-frame (unlit) sample-list ring.
- **Bind group `global_illum_bind_group`** (`@group(1)`): `gi_params`, `first_hit_data` (read),
  `first_hit_absorption` (rw — `renderGlobalIllum.fx:7` declares it RW but the shader does not
  appear to write it... verify; bind as rw to be safe), `valid_samples` (rw), `invalid_samples`
  (rw), `sample_counts` (rw atomic), `final_color` (rw — `renderGlobalIllum.fx:12` declares it
  but `calcGlobalIlum` does not write it; bind for layout stability), `ray_queue` (read),
  `camera_history` (read). `@group(0)` = world; `@group(3)` = atmosphere.

### 8.2 `renderSampleRefine` — the 5 passes, temporal resampling into 8×8 regions

`base/renderSampleRefine.fx` (441 lines). 5 compute entry points (§4.7 = 5 nodes). All bind the
single `sample_refine_bind_group`. The `globalIlumBucketInfo` is the **8×8 screen-space region**
data (`02-research.md` §1.2.3 — "8×8 disjoint screen-space pixel regions").
- **`clearBucketsAndCalcMask`** (`:33-69`): clears `sampleCounts[3+accumIndex]` +
  `ray_queue_indirect[0]` (the per-frame ring-slot + queue-counter reset — §7.3); per 8×8 bucket,
  scans its 64 pixels' `firstHitData`, computes the normal-mask + min/max-distance + min/max-tang,
  writes `globalIlumBucketInfo[bucket]`.
- **`computeValidHistory`** (`:71-101`, `[numthreads(1,1,1)]`, 1 dispatch): walks the
  `sampleCounts` ring back from `accumIndex`, sums lit/unlit counts until the ring buffer
  capacity (`validSampleStorageCount·w·h` / `invalidSampleStorageCount·w·h`) is hit — this is "up
  to 64 past frames" (`02-research.md` §1.2.3). Writes `sampleCounts[0]` (start indices),
  `sampleCounts[1]` (total counts), `sampleCounts[2]` (the coprime shuffle seeds via
  `findCoprime`/`nextPow2`), and the two **indirect dispatch arg buffers** `globalIlumValidDispatch`
  / `globalIlumInvalidDispatch` (`:99-100`) — `count_valid_data_and_refine` / `count_invalid_data`
  dispatch indirect off these.
- **`countValidDataAndRefine`** (`:108-253`, indirect dispatch): for each lit sample in the
  temporal ring, reproject it into the current 8×8 bucket grid via the camera-history (it uses
  the **inverse** ring `camRotOld` = `view_proj_inv` — §3.6), the distance/specular-normal
  validity test against `taaDistMinMax`, then `InterlockedAdd` into the bucket's stored-count and
  writes a `refinedSample` into `globalIlumValidSamplesRefined[bucket·32 + slot]` (up to 32 per
  bucket — `bucketStorageCount`).
- **`countInvalidData`** (`:255-338`, indirect dispatch): same reprojection for unlit samples,
  just `InterlockedAdd`s the bucket's invalid count (no sample stored).
- **`refineBuckets`** (`:340-417`): per bucket — the **`COLOR_DIF_PROB` brightness-leveling**
  (`02-research.md` divergence #10): compares each of the ≤32 refined samples to the bucket's max
  brightness, removes weakly-lit ones with `COLOR_DIF_PROB[maxColorDif]` probability, compensates
  the survivors, writes ≤8 (`refinedBucketStorageCount`) to `globalIlumValidSamplesCompressed`,
  packs the bucket's lit/invalid ratio + count into `globalIlumBucketInfo`.
- **Bind group `sample_refine_bind_group`** (`@group(0)`): `gi_params`, `first_hit_data` (read),
  `bucket_info` (rw atomic), `valid_samples` (read), `valid_samples_refined` (rw),
  `valid_samples_compressed` (rw), `invalid_samples` (read), `sample_counts` (rw atomic),
  `taa_dist_min_max` (read), `valid_dispatch` (rw), `invalid_dispatch` (rw),
  `ray_queue_indirect` (rw — the clear pass zeroes it), `camera_history` (read). That is 13
  bindings — within the `maxBindingsPerBindGroup` default (640+); fine as one group.

### 8.3 `renderSpatialResampling` — Algorithm 2

`base/renderSpatialResampling.fx` (406 lines). Port `calcSpatialResampling` + `sampleNeighbors` +
the helpers. This is **Algorithm 2** (`02-research.md` §1.2.3, paper lines 341–367):
- `sampleNeighbors` (`:56-342`): the **12-iteration** neighbour loop (`for i in 0..sampleCount`,
  `sampleCount = 12` — `WorldRenderBase.cs` passes `12` at `renderSpatialResampling.fx:359`). Per
  iteration: pick a neighbouring 8×8 bucket within the **adaptive per-pixel radius** (the
  `isVaryingResmaplingRadius` 12-tap pre-pass — `:81-148` — estimates `radiusFac`); retrieve the
  bucket info; skip if normal-mask / distance-invalid; pick a random lit sample from the bucket;
  decode it (`getSampleData`); skip if geometry-invalid (the pdf-ratio test — `:218-223`); compute
  the **Jacobian** `jacobianNow/jacobianNeighbor` (`:227-237`); compute the target function
  (`getTargetFunctionNew` — `:49-54`); merge into the reservoir (`:251-262` — the WRS update).
- After the loop: the **single visibility check** (`:266-302`) — a 3-step `shootRay` chain
  (mirror-following — `MAX_RAY_STEPS_VISIBILITY`); `isVisible = totalHitLength² -
  selectedLengthToSampleSquaredNow >= 0`; if not visible, `sumWeight = 0`.
- Then the sun sample (`:321-339`) — another `shootRay` against the sun.
- **Write split** (`:365-398`): if `isDenoise` → write `denoisePreprocessed[pixel]` (the
  transposed-index `pixelPos.y + pixelPos.x · screenHeight` — note the **transpose**, the
  denoiser reads it column-major — `:389`); else add directly into `finalColor` (`:393-397`).
- The `spatialVisibilityCount` uniform (`renderSpatialResampling.fx:24`) — note: the HLSL
  declares it but `sampleNeighbors` actually passes `MAX_RAY_STEPS_VISIBILITY` (the const) to
  `shootRay` at `:274`, NOT `spatialVisibilityCount`. **Port faithfully:** the visibility
  `shoot_ray` uses `MAX_RAY_STEPS_VISIBILITY` (the `ray_tracing.wgsl` const = 60). The
  `spatial_visibility_count` field in `GpuGiParams` (§3.8) is bound for layout fidelity but
  unused by the WGSL — or just drop it from `GpuGiParams`. (Recommend: drop it — it is a dead
  uniform; one fewer field.)
- **Bind group `spatial_resampling_bind_group`** (`@group(1)`): `gi_params`, `first_hit_data`
  (read), `first_hit_absorption` (read), `bucket_info` (read), `valid_samples_compressed` (read),
  `taa_sample_accum` (read — the denoise-path TAA-colour read at `:371`), `final_color` (rw),
  `denoise_preprocessed` (rw). `@group(0)` = world (it traverses).

### 8.4 The lit/unlit/compressed colour formats — restating the bit layouts

For the implementer, the three GI sample colour fields, **all 5-bit/channel exponential**
(`color_compression.wgsl` `compressColor` — `commonColorCompression.fxh:93-106`):
`compColor = compColorR | (compColorG << 5) | (compColorB << 10)` — a 15-bit field. It is stored:
- in **lit samples**: `SampleValid.data2.y` low 15 bits (`renderGlobalIllum.fx:44`).
- in **refined/compressed samples**: `refinedSample.x & 0x7FFF` (`renderSampleRefine.fx:245,
  401-404`).
Decoded back via `COLORS[compColor & 0x1F]` etc. (`renderSpatialResampling.fx:241`). The
`COLORS[32]` LUT is the 5-bit-index → f32-colour table (§5.3).

The **TAA** sample colour (the A-2 `taa_common.wgsl`) is a *different* compression — 8-bit/channel
exponential (`commonTaa.fxh` `compressSample`). The two coexist: `taa_common.wgsl` (8-bit, TAA
samples) and `color_compression.wgsl` (5-bit, GI samples) — `06-design-a2.md` §13.1 already
flagged this distinction; Phase B now adds the second one.

---

## 9. The sparse bilateral denoiser + the atmosphere precompute (brief item 8)

### 9.1 The sparse bilateral denoiser — `renderDenoiseSplit`

`base/renderDenoiseSplit.fx` (146 lines). Two compute entries, dispatched in sequence (§4.9),
gated on `is_denoise`:
- **`calcDenoiseHorizontal`** (`:15-73`): reads `denoisePreprocessed` (column-major index —
  `pixelPos = uint2(globalID.x / screenHeight, globalID.x % screenHeight)` — the transpose
  `renderSpatialResampling.fx` wrote); for `y in -10..=10` (kernel 21 — `02-research.md` §1.2.4),
  with a **random sparse x-offset** (`int x = nextRand(rand) < 0.5 && y != 0 ? 1 : 0` — "on
  average every 2nd pixel"); the bilateral weight = `gaussianF(y, 10)` (the `common.wgsl`
  `gaussian_f` — σ=10) × a TAA-weight-difference term (`rcp(1 + |ΔtaaWeight| · denoiseThresh)`)
  × a normal/state-match term; writes `denoisePreprocessedHorizontal` (row-major).
- **`calcDenoiseVertical`** (`:75-132`): reads `denoisePreprocessedHorizontal`; same kernel for
  `x in -10..=10`; `lerp(colorOrig, color, 0.92)`; multiplies by `firstHitAbsorption`; **adds
  into `finalColor`** (`:128-131`).
- WGSL port notes: `rcp(x)` → `1.0 / x`; `gaussianF` → the new `common.wgsl` `gaussian_f`; the
  `nextRand` sparse offset uses `init_rand` + `next_rand` (the existing
  `ray_tracing_common.wgsl`). The transposed index is faithful — port it exactly (the
  `denoisePreprocessed` buffer is written column-major by `spatialResampling`, read column-major
  by `denoiseHorizontal`, written row-major into `denoisePreprocessedHorizontal`).
- **No SVGF** — `02-research.md` divergence #11: NAADF ships only the sparse bilateral; there is
  no SVGF shader to port.
- **Bind group `denoise_bind_group`** (`@group(0)`): `gi_params`, `first_hit_absorption` (read),
  `denoise_preprocessed` (read), `denoise_preprocessed_horizontal` (rw), `final_color` (rw). Both
  passes share it.

### 9.2 The atmosphere precompute — `renderAtmosphere` + the full `Atmosphere` model

Phase A/A-2 used only the inline sun+ambient term in `naadf_first_hit.wgsl:132-153`
(`03-design.md` §5.2 — "albedo inlines a simple sun term"). Phase B needs the full
multiple-scattering model (`02-research.md` §1.2.5, divergence #7):
- **`naadf_atmosphere.wgsl`** ports `base/renderAtmosphere.fx` `precomputeAtmosphere`: each
  invocation handles `ID = globalID.x · 4 + (frameCount % 4)` — **one quarter of the
  octahedral buffer per frame** (`renderAtmosphere.fx:12` — amortised over 4 frames). It
  octahedral-decodes the texel to a ray direction, applies the `rayDir.y` warp (`:20-21`), runs
  `addLightForDirection` (the ray-marched sky — `atmosphere.wgsl`), packs `(light, absorption)`
  into the `atmosphere_comp` `vec3<u32>` slot (`vec4<u32>` with `.w` padding — §3.3).
- **`atmosphere.wgsl`** ports `atmosphereRaw.fxh` (`addLightForDirection` — the nested ray-march:
  `mainIterationCount` outer steps, `secondIterationCount` scatter steps, Rayleigh/Mie/Ozone
  density, `raySphere` intersections) + `atmospherePrecomputed.fxh` (`applyAtmosphere` — samples
  the precomputed `atmosphere_comp` via `octEncode`).
- **The GI sun colour** — `WorldRenderBase.cs:96` passes `sunColor` (a `Vector3`) to
  `renderGlobalIllum` + `renderSpatialResampling`; it is `Atmosphere.GetLightForPoint((0,10,0))`
  (`WorldRender.cs:93`) — the **CPU** atmosphere model (`Atmosphere.cs`). Port `Atmosphere.cs`'s
  `GetLightForPoint` to a small **Rust** function in `atmosphere.rs` (it is ~40 lines:
  `DensityAtHeight`, `RaySphere`, `ScatterForDensities`, `getScatterDensitiesAtPoint`,
  `GetLightForPoint` — all pure math, verified in `Atmosphere.cs`). `prepare_gi` calls it once
  per frame to compute `gi_params.sun_color`. (`prepare_frame_gpu`'s current fixed `sun_color`
  `(1.0, 0.95, 0.85)` — `prepare.rs:330` — is the Phase-A placeholder; Phase B replaces it with
  the computed value for `GpuGiParams`, and the inline sun term in the first-hit is *removed*
  since the 4-plane first-hit uses `applyAtmosphere` instead.)
- **Sky parameters as constants:** §3.9 — the `GpuAtmosphereParams` sky fields come from
  `UiSkyDebug.cs`'s defaults with the `SetShaderData` scaling (`UiSkyDebug.cs:63-79`); the Rust
  `Atmosphere::get_light_for_point` uses the same constants. Define them once in `atmosphere.rs`
  as a `const ATMOSPHERE: AtmosphereConstants` block so the GPU uniform and the CPU function
  share the source of truth.

---

## 10. extract / prepare changes (brief item 9)

### 10.1 What does NOT change (the §1 non-scope, restated for prepare)

- `prepare_world_gpu` is **not** touched — the `05-review.md` §4 "runs every frame"
  inefficiency does not block GI (the early-out at `prepare.rs:107` works; the world buffers are
  built once). The GI pipeline reads `WorldGpu` exactly as Phase A does.
- The `world/`, `aadf/`, `voxel/` modules and the world-construction path are untouched (Phase C).
- `GpuRenderParams.bounding_box_*` stay zeroed (the traversal reads `world_meta` — `05-review.md`
  §4, `06-design-a2.md` §1); Phase B adds no dependency on them. (`GpuRenderParams` only gains
  `tone_mapping_fac` in a pad slot — §5.9.)

### 10.2 extract changes (`src/render/extract.rs`)

- **`ExtractedCameraHistory`** gains `view_proj_inv: [Mat4; 128]` — the inverse rotation-only
  view-proj ring (`renderSampleRefine`'s `camRotOld` — §3.6). `extract_camera_history` copies it
  from the main-world `CameraHistory.view_proj_inv` (which `update_camera_history` populates).
- **`ExtractedGiConfig`** (NEW) — mirrors `AppArgs.GiSettings` into the render world (the GI
  config the prepare/graph systems need). Like A-2's `ExtractedTaaConfig` (`extract.rs:207-222`).
  `extract_gi_config` copies it each frame. Carries: `bounce_count`, `global_illum_max_accum`,
  `spatial_resample_size`, `denoise_thresh`, `radius_lit_factor`, `noise_suppression_factor`,
  the bools. `naadf_denoise_node` gates on `ExtractedGiConfig.is_denoise`.
- `ExtractedCameraData` already carries `view_proj` (the non-inverted rotation-only — A-2 added
  it, `extract.rs:67`) and `inv_view_proj` — Phase B's `GpuGiParams` reuses both. No new
  `ExtractedCameraData` field.

### 10.3 prepare changes — `prepare_atmosphere` + `prepare_gi` + the existing systems

**`prepare_atmosphere`** (NEW, `src/render/atmosphere.rs`, `RenderSystems::PrepareResources`):
- Creates `AtmosphereGpu` once: the `atmosphere_comp` buffer (`1024·1024·16` bytes — §3.3,
  fixed-size, zero-cleared on creation), the `atmosphere_params` uniform. The atmosphere buffer
  is **not resized** on viewport change (it is octahedral, resolution-independent).
- Every frame: uploads `GpuAtmosphereParams` (the sky constants + per-frame `cam_pos` +
  `frame_count` + `sky_sun_dir`).
- Builds `atmosphere_bind_group`.

**`prepare_gi`** (NEW, `src/render/gi.rs`, `RenderSystems::PrepareResources`):
- Creates `GiGpu` once / resizes the `pixel_count`- and `bucket_count`-sized buffers on viewport
  change (the §3.7 buffer list; same resize-trigger pattern as `prepare_taa` — `taa.rs:264-305`).
  `bucket_count` / `bucket_size` are derived from the viewport: `bucket_size = ((viewport + 7) /
  8)`, `bucket_count = bucket_size.x · bucket_size.y` (`WorldRenderBase.cs:157-159`).
- The indirect buffers (`ray_queue_indirect`, `valid_dispatch`, `invalid_dispatch`) are seeded on
  creation: `ray_queue_indirect = [0,1,1,0,0]`, `valid_dispatch = invalid_dispatch = [1,1,1,0,0]`
  (`WorldRenderBase.cs:136,168,170`).
- `sample_counts` (131 elements) is fixed-size, zero-cleared **only on creation** (it carries the
  128-frame ring — §3.7).
- Every frame: builds `GpuGiParams` (§3.8) — the per-frame fields are `frame_count` /
  `frame_index` (= `extracted_history.frame_count`), `taa_index` (`extracted_history.taa_index`),
  `accum_index` = `global_illum_max_accum - (frame_count % global_illum_max_accum) - 1`
  (`WorldRenderBase.cs:181` — port as a `gi.rs` helper), `rand_counter` / `rand_counter2` (the
  RNG salts — `WorldRenderBase` indexes `randValues[randCounter++]` 7 times per frame; A-2's
  simplification — `06-design-a2.md` §4.1 — uses the frame counter as the salt; Phase B extends:
  use `frame_count` and `frame_count ^ 0x9E3779B9` (or `frame_count * 2 + 1`) as two distinct
  per-frame salts — the load-bearing property is two distinct per-frame-varying salts, exactly
  the A-2 reasoning), `cam_pos_int/frac` + `inv_view_proj` + `view_proj` (from
  `ExtractedCameraData`), `sun_color` (the CPU `Atmosphere::get_light_for_point` — §9.2),
  `sky_sun_dir` (shared with the atmosphere — extract once), the `flags`, and the constants.
  Uploads it.
- Builds the GI bind groups. **Ordering:** `prepare_gi` runs in `PrepareResources`; the bind
  groups that mix `GiGpu` + `FrameGpu` + `TaaGpu` (`ray_queue_bind_group` needs
  `FrameGpu.first_hit_data` + `TaaGpu.taa_sample_accum`; `sample_refine_bind_group` needs
  `TaaGpu.taa_dist_min_max`; `global_illum_bind_group` needs `FrameGpu.first_hit_data/absorption/
  final_color` + `TaaGpu.camera_history`; etc.) must be built **after** `FrameGpu` + `TaaGpu`
  exist. **Mirror the A-2 split exactly** (`06-design-a2.md` §5.5): `prepare_gi`
  (`PrepareResources`) creates `GiGpu`'s buffers + uploads `gi_params`; the **mixed bind groups
  are built in `prepare_frame_gpu`** (`PrepareBindGroups`, after `FrameGpu` + `TaaGpu` + `GiGpu`
  all exist). So `prepare_frame_gpu` gains `Res<GiGpu>` + `Res<AtmosphereGpu>` and builds
  `ray_queue_bind_group`, `global_illum_bind_group`, `sample_refine_bind_group`,
  `spatial_resampling_bind_group`, `denoise_bind_group`, `calc_new_taa_sample_bind_group` — all
  in one place, all rebuilt on the shared `needs_new_storage` trigger. (This is exactly how A-2
  put `taa_reproject_bind_group` in `prepare_frame_gpu` — `prepare.rs:435-445`.) Move the
  bind-group *fields* off `GiGpu` onto `FrameGpu` if it is cleaner — designer's call; recommend
  keeping them on `GiGpu` but built by `prepare_frame_gpu` (it already takes `Res<TaaGpu>` and
  builds `TaaGpu`-referencing bind groups — same pattern; but `GiGpu` is `Res`, not `ResMut` —
  so the bind groups go on a separate render-world resource OR `prepare_frame_gpu` inserts a
  `GiBindGroups` resource. **Recommendation:** a separate `GiBindGroups` render-world resource
  built by `prepare_frame_gpu`, holding all the mixed bind groups — cleanest, no `ResMut<GiGpu>`
  needed.)

**`prepare_frame_gpu`** (existing — `prepare.rs:256-465`):
- `FrameGpu` gains `first_hit_absorption` + `final_color` (§3.4) — created/resized with
  `first_hit_data`, zero-cleared on creation.
- The `@group(1)` `frame_layout` widens by 2 bindings (`first_hit_absorption`, `final_color`) —
  `pipelines.rs` change; the frame bind-group build adds them.
- Gains `Res<GiGpu>`, `Res<AtmosphereGpu>` and builds the `GiBindGroups` resource (the mixed
  bind groups — see above) + the atmosphere/first-hit `@group(3)` bind group.
- Sets `GpuRenderParams.tone_mapping_fac = 1.0` (§5.9).

**`prepare_taa`** (existing — `taa.rs:241-384`):
- `TaaGpu` gains `taa_dist_min_max` (§3.5) — created/resized with `taa_samples`, zero-cleared.
- The `camera_history` upload now also writes the `view_proj_inv` slot field (§3.6) — the
  `GpuCameraHistorySlot` build loop (`taa.rs:333-342`) gains the `view_proj_inv` line from
  `extracted_history.view_proj_inv[i]`.
- `taa_first_hit_bind_group` is renamed `taa_calc_new_sample_bind_group` and now binds the
  `calc_new_taa_sample` pipeline's `@group(...)` (it needs `taa_samples` + `voxel_types` —
  §4.10; `voxel_types` comes from `WorldGpu` — so this bind group mixes `TaaGpu` + `WorldGpu`,
  built in `prepare_frame_gpu` like the other mixed groups, OR `prepare_taa` takes `Res<WorldGpu>`).
  Cleanest: build it in `prepare_frame_gpu` with the other mixed groups.

**`update_camera_history`** (existing — `taa.rs:156-186`): gains the `view_proj_inv[slot] =
view_proj.inverse()` line (one extra `.inverse()` per frame).

### 10.4 The `mod.rs` wiring

`render/mod.rs` registers `AtmosphereGpu`-related extract/prepare is N/A (it is render-world-only,
created by `prepare_atmosphere`), inits `ExtractedGiConfig`, adds `extract_gi_config` to
`ExtractSchedule`, adds `prepare_atmosphere` + `prepare_gi` to `PrepareResources` (before
`prepare_frame_gpu` in `PrepareBindGroups` — same ordering rule as `prepare_taa`), and rebuilds
the `Core3d` `.chain()` per §4.2.

---

## 11. The numbered, BATCHED Phase-B implementation sequence (brief item 10)

Phase B is the biggest phase. The impl is designed as **6 batches**, each ending at a buildable
(and where marked ▶, runnable) state. The orchestrator dispatches and reviews batch-by-batch.
Each batch's steps end at a compiling state; ▶ = smoke-runnable. Test command:
`cargo test --bin bevy-naadf` (binary-only crate — `07-impl-a2.md`).

**Batch boundaries are chosen so each batch is independently reviewable and the app keeps
rendering *something* throughout** — the pipeline is built back-to-front-ish where possible so a
half-built pipeline still produces an image. The hard ordering constraint: the GI passes consume
each other's buffers, so the *middle* batches build buffers + WGSL that do not yet feed the blit
— those batches' done-bar is "compiles + the new pass dispatches without validation errors", not
"the image changes". The image only fully changes at Batch 6.

### Batch 1 — Shared WGSL + GPU types + the atmosphere subsystem

Self-contained, no pipeline restructure. Ends ▶ runnable (atmosphere precomputes; the rest of the
pipeline is still the A-2 path, unchanged).

1. **Shared WGSL extensions.** `ray_tracing_common.wgsl` += VNDF/GGX (`get_perpendicular_vector`,
   `get_uniform_hemisphere_sample`, `sample_vndf_isotropic`, `pdf_vndf_isotropic`,
   `geometry_term`). `render_pipeline_common.wgsl` += `get_reflectance_fresnel`,
   `get_specular_normals`, `get_tang`, the full `get_hit_data_from_planes` (entity branch
   omitted), `get_screen_pos_projection` / `get_screen_index_projection` (moved from `taa.wgsl`),
   `SPECULAR_MIRROR_FAC`, the `FirstHitResult` + `SampleValid` structs; `compress_first_hit_data`
   gains the `is_diffuse` arg (update the A-2 `naadf_first_hit.wgsl` call site to pass `1u` for
   now). `common.wgsl` += `gaussian_f`, `gcd`, `find_coprime`, `next_pow2`. `taa.wgsl`'s local
   plane/projection helpers replaced by imports. **Compiles** (naga validation; the A-2 render
   path still runs unchanged — `get_hit_data_from_planes` with planes 1–3 = `HIT_UNDEFINED`
   reduces to the A-2 single-plane behaviour).
2. **`color_compression.wgsl`** (NEW) — the `COLORS[32]`/`COLOR_DIF_PROB[31]` literals (generated
   + a Rust `#[test]` that recomputes + asserts), `compress_color`, `refine_comp_color`. Not yet
   imported by any entry shader. **Compiles + test.**
3. **`atmosphere.wgsl`** (NEW) — `add_light_for_direction` + the phase/density/sphere helpers +
   `apply_atmosphere`. Not yet imported. **Compiles.**
4. **GPU types.** `gpu_types.rs` += `GpuAtmosphereParams`, `GpuGiParams`, `GpuSampleValid`,
   `GpuBucketInfo` (or note `bucket_info` is raw `vec2<u32>`); `GpuCameraHistorySlot` gains
   `view_proj_inv` (96→160 bytes — update the size assert + the `taa.wgsl` struct decl + the A-2
   `prepare_taa` upload loop + `CameraHistory` ring + `update_camera_history`'s `.inverse()`);
   `GpuRenderParams` gains `tone_mapping_fac` in a pad slot. Size asserts. **Compiles + tests.**
5. **The atmosphere subsystem.** `atmosphere.rs` (NEW) — `AtmosphereGpu`, the sky constants
   (from `UiSkyDebug.cs`), the CPU `Atmosphere::get_light_for_point` (port of `Atmosphere.cs`),
   `prepare_atmosphere`. `naadf_atmosphere.wgsl` (NEW) — the precompute entry. `pipelines.rs` +=
   `atmosphere_pipeline` + `atmosphere_layout`. `graph_b.rs` (NEW) — `naadf_atmosphere_node`.
   `mod.rs` — register `prepare_atmosphere` in `PrepareResources`, add `naadf_atmosphere_node` as
   the *first* node in the `Core3d` chain. **▶ Compiles & runs** — the atmosphere precomputes
   every frame (verify: no WGSL/pipeline errors; the A-2 render path is otherwise unchanged so
   the image is identical to Phase A-2).

### Batch 2 — The 4-plane-bounce first-hit

Restructures the first-hit pass. Ends ▶ runnable — the image changes (the first-hit now applies
the full atmosphere instead of the inline sun term, and fills planes 1–3 for mirror surfaces).

6. **`FrameGpu` buffers + the frame layout.** `prepare.rs` — `FrameGpu` += `first_hit_absorption`
   + `final_color` (created/resized/zero-cleared with `first_hit_data`). `pipelines.rs` — widen
   the `@group(1)` `frame_layout` by 2 bindings. **Compiles.**
7. **The 4-plane `naadf_first_hit.wgsl`** (REPLACED) — port `base/renderFirstHit.fx` (§6): the
   4-iteration bounce loop, the 5-arg `compress_first_hit_data`, the `first_hit_absorption` +
   `final_color` writes, `apply_atmosphere` on a miss + `add_light_for_direction` on the
   atmosphere-interaction path. **Remove** the `taa_sample_accum` write + the `@group(2)`
   `taa_samples` ring write (§6.3). Add `@group(3)` atmosphere bindings. `pipelines.rs` — the
   first-hit pipeline layout `[world, frame, taa]` → `[world, frame, atmosphere]`. `graph.rs` /
   `graph_b.rs` — `naadf_first_hit_node` drops `set_bind_group(2, taa...)`, adds
   `set_bind_group(3, atmosphere...)`. **Build the atmosphere `@group(3)` bind group** in
   `prepare_frame_gpu`.
8. **The TAA pass restructure for the `base/` pipeline.** Because the first-hit no longer writes
   `taa_sample_accum`/`taa_samples`, the A-2 graph is temporarily broken — `naadf_taa_reproject_node`
   reads `taa_sample_accum` (now never written by first-hit) and `naadf_final_blit_node` reads it.
   **Minimal fix for this batch:** point `naadf_final_blit_node` at `final_color` *temporarily*
   (a one-line bind-group change — `final_color` is the `vec2<u32>` light buffer the first-hit
   now writes, same element format as `taa_sample_accum`'s colour fields). This makes the app
   runnable at end-of-Batch-2 showing the 4-plane first-hit result directly (no TAA, no GI yet).
   The proper `ReprojectOld` + `CalcNewTaaSample` rewire is Batch 6. Document this as a
   **deliberate temporary blit-source** (it gets reverted in Batch 6 — the same kind of designed
   seam as Phase A's `shaded_color` stand-in). `naadf_taa_reproject_node` + `naadf_calc_new_taa_
   sample_node` are **not in the chain yet** in this batch.
   **▶ Compiles & runs** — the image shows the 4-plane-bounce first-hit with the full atmosphere;
   mirror surfaces show one reflection (planes 1–3). No GI, no TAA. (User-visible: the sky is now
   the real multiple-scattering model, not the flat sun term.)

### Batch 3 — `rayQueueCalc` + `globalIllum` (the GI sample generators)

Builds the GI buffers + the first two GI passes. These produce GI samples into buffers but
nothing reads them yet — the done-bar is "the passes dispatch without validation errors", not "the
image changes". **Compiles & runs** (the Batch-2 image is unchanged — the GI passes write buffers
the blit does not read).

9. **`GiGpu` + `prepare_gi` + `GpuGiParams`.** `gi.rs` (NEW) — `GiGpu` (all the §3.7 buffers,
   created/resized/seeded per §10.3), the `accum_index` / RNG-salt / bucket-grid helpers,
   `prepare_gi` (creates the buffers + uploads `gi_params`). `extract.rs` — `ExtractedGiConfig` +
   `extract_gi_config`; `ExtractedCameraHistory` += `view_proj_inv` ring. `taa.rs` —
   `CameraHistory` + `update_camera_history` populate `view_proj_inv`; `prepare_taa` uploads it
   into the `camera_history` slots. `main.rs` — `AppArgs` += `GiSettings`. `mod.rs` — register
   `prepare_gi`, `extract_gi_config`. **Compiles.**
10. **`ray_queue_calc.wgsl` + `naadf_ray_queue_node`.** Port `rayQueueCalc.fx` (§7) — the two
    entries, the inline group-shared counter, the `should_ray` adaptive test, the indirect-args
    store. `pipelines.rs` += `ray_queue_pipeline` + `ray_queue_store_pipeline` + the layout.
    `graph_b.rs` += `naadf_ray_queue_node`. Build `ray_queue_bind_group` in `prepare_frame_gpu`.
    Add the node to the chain (after first-hit / before — actually after the temporary blit, its
    position in the final chain is §4.2; for this batch insert it after `naadf_first_hit_node`).
    **Compiles & runs** — `ray_queue` + `ray_queue_indirect` populate (verify: no validation
    errors; the indirect buffer's `[0]` is a sane non-zero workgroup count).
11. **`naadf_global_illum.wgsl` + `naadf_global_illum_node`.** Port `renderGlobalIllum.fx` (§8.1)
    — the ≤3-bounce tracer, `compress_sample_valid`/`compress_sample_invalid`, the group-shared
    atomics, the lit/unlit ring writes. `pipelines.rs` += `global_illum_pipeline` + the layout.
    `graph_b.rs` += `naadf_global_illum_node` (**indirect dispatch** off `ray_queue_indirect`).
    Build `global_illum_bind_group`. Add to the chain. **▶ Compiles & runs** — `globalIllum`
    dispatches indirect, writes the GI sample lists (verify: no validation errors, no panic; the
    image is still the Batch-2 first-hit — GI samples are generated but not yet resampled/blitted).

### Batch 4 — `sampleRefine` (the 5 passes — temporal resampling into 8×8 regions)

Builds the 5-pass sample-refine block. Still buffer-only — the done-bar is "the 5 passes dispatch
clean". **Compiles & runs** (image unchanged).

12. **`sample_refine.wgsl` — passes 1+2.** Port `clearBucketsAndCalcMask` (incl. the per-frame
    `ray_queue_indirect[0]` + `sample_counts[3+accumIndex]` reset — §7.3) and `computeValidHistory`
    (the ring-walk + the `valid_dispatch`/`invalid_dispatch` indirect-args write). `pipelines.rs`
    += the two pipelines + the shared `sample_refine_layout`. `graph_b.rs` +=
    `naadf_sample_refine_clear_node` + `naadf_sample_refine_valid_history_node`. Build
    `sample_refine_bind_group`. Insert the clear node at its §4.2 position (before `ray_queue`).
    **Compiles & runs.**
13. **`sample_refine.wgsl` — passes 3+4 (indirect).** Port `countValidDataAndRefine` +
    `countInvalidData` — the reprojection into the 8×8 bucket grid (using `view_proj_inv`), the
    `taa_dist_min_max` validity test, the `bucket_info` atomics, the `valid_samples_refined`
    write. `pipelines.rs` += the two pipelines. `graph_b.rs` += the two nodes (**indirect
    dispatch** off `valid_dispatch`/`invalid_dispatch`). **Compiles & runs.** *(Note:
    `taa_dist_min_max` is not yet written — the A-2 reproject pass does not write it; until
    Batch 6 rewires the TAA pass, `taa_dist_min_max` is the zero-cleared buffer, so the
    validity test rejects everything. That is fine — the pass dispatches clean; the data is just
    empty. Document this cross-batch dependency.)*
14. **`sample_refine.wgsl` — pass 5.** Port `refineBuckets` — the `COLOR_DIF_PROB` brightness-
    leveling, the ≤8-survivor write into `valid_samples_compressed`. `pipelines.rs` +=
    `refine_buckets_pipeline`. `graph_b.rs` += `naadf_sample_refine_buckets_node`. Add to the
    chain. **▶ Compiles & runs** — the 5-pass sample-refine block dispatches clean (image
    unchanged — `valid_samples_compressed` is written but not yet read by the blit).

### Batch 5 — `spatialResampling` + `denoiseSplit` (the GI consumers)

Builds Algorithm 2 + the denoiser. These write `final_color` / `denoise_preprocessed`. **▶
Runnable — the image starts to show GI** (if Batch 2's temporary blit still points at
`final_color`, the spatial-resampling write into `final_color` is now visible).

15. **`spatial_resampling.wgsl` + `naadf_spatial_resampling_node`.** Port
    `renderSpatialResampling.fx` (§8.3) — `sampleNeighbors` (the 12-iteration reservoir loop, the
    adaptive-radius pre-pass, the Jacobian, the single visibility ray, the sun sample), the
    denoise-vs-final write split. `pipelines.rs` += `spatial_resampling_pipeline` + the layout
    (binds `@group(0)` world — it traverses). `graph_b.rs` += `naadf_spatial_resampling_node`.
    Build `spatial_resampling_bind_group`. Add to the chain. **▶ Compiles & runs** — with
    `is_denoise` off (temporarily set `GiSettings.is_denoise = false` for this step), the spatial
    pass writes GI directly into `final_color`, and Batch-2's temporary `final_color` blit shows
    it — **GI bounce lighting is visible** for the first time (noisy — no denoiser yet).
16. **`denoise_split.wgsl` + `naadf_denoise_node`.** Port `renderDenoiseSplit.fx` (§9.1) — the
    two separable sparse-bilateral passes. `pipelines.rs` += the two pipelines + the layout.
    `graph_b.rs` += `naadf_denoise_node` (2 dispatches, gated on `ExtractedGiConfig.is_denoise`).
    Build `denoise_bind_group`. Add to the chain. Set `GiSettings.is_denoise = true`. **▶
    Compiles & runs** — with denoise on, the spatial pass writes `denoise_preprocessed`, the
    denoiser filters it into `final_color` — the GI is denoised (image: bounce lighting,
    medium-frequency noise filtered).

### Batch 6 — The `base/` TAA rewire + `renderFinal` + the final integration

Wires the proper `base/` TAA path (`ReprojectOld` + `CalcNewTaaSample`), reverts Batch-2's
temporary blit source, ports the `base/` `renderFinal`, and lands the full pipeline. **▶
Runnable — THE PHASE B DELIVERABLE.**

17. **`taa.wgsl` — the `base/` `ReprojectOld` + `taaDistMinMax`.** `taa.wgsl`'s
    `reproject_old_samples` gains the `taa_dist_min_max` write (§5.8.1) — un-omit the
    `valid_normals_spec` accumulation (now `get_specular_normals` is real), change the
    `screenPosDistanceSqr` reject from `> 1.0` to `> 16.0` (the `base/` value — §5.8.1). `TaaGpu`
    += `taa_dist_min_max` (created/resized/cleared with `taa_samples` — `prepare_taa` change).
    The reproject `@group(0)` layout gains the `taa_dist_min_max` rw binding. **Compiles & runs**
    — `taa_dist_min_max` now populates, so Batch-4's `sampleRefine` validity test starts
    accepting samples (the GI quality improves — the sample-refine block now has real data).
18. **`taa.wgsl` — `CalcNewTaaSample`.** Add the `calc_new_taa_sample` entry (§5.8.2) — folds
    `final_color` (the denoised GI) into the 16-frame `taa_samples` ring + updates
    `taa_sample_accum`. `pipelines.rs` += `calc_new_taa_sample_pipeline` + its layout (binds
    `taa_samples` rw, `taa_sample_accum` rw, `first_hit_data` read, `final_color` read,
    `voxel_types` read, `taa_params` uniform). `graph_b.rs` += `naadf_calc_new_taa_sample_node`
    (gated on `ExtractedTaaConfig.enabled`). Build the bind group in `prepare_frame_gpu`. Add
    both `naadf_taa_reproject_node` and `naadf_calc_new_taa_sample_node` to the chain at their
    §4.2 positions. **Compiles & runs.**
19. **`naadf_final.wgsl` — the `base/` variant + revert the temporary blit.** Update
    `naadf_final.wgsl` to the `base/renderFinal.fx` behaviour (the `tone_mapping_fac` term —
    §5.9; the `showRayStep` read from `first_hit_data.z`). **Revert** Batch-2's temporary
    `final_color` blit source — `naadf_final_blit_node` reads `taa_sample_accum` again (now
    correctly filled by `ReprojectOld` + `CalcNewTaaSample`). `prepare_frame_gpu` sets
    `tone_mapping_fac = 1.0`. **▶ Compiles & runs — THE PHASE B DELIVERABLE:** the full NAADF
    real-time GI pipeline — 4-plane first-hit → atmosphere → adaptive ~0.25-spp GI via
    `rayQueueCalc` → compressed ReSTIR GI (globalIllum + sampleRefine + spatialResampling) →
    sparse bilateral denoiser → 16-frame TAA (ReprojectOld + CalcNewTaaSample) → final blit.
    Bounce lighting visible, temporally stable via the TAA, no obvious artifacts.
20. **HUD + polish.** `hud.rs` — the new render-node timing lines (§4.12), the renderer-mode
    string. Verify the `const`-checked path/span pairs. Update `README.md` roadmap. **▶ Compiles
    & runs** — HUD shows the Phase-B pass timings.

**Phase-B review gate:** after step 20, Phase B is reviewed and user interactive re-test confirms
GI is rendering (bounce lighting visible, temporally stable, no obvious artifacts) before Phase B
closes (`01-context.md` §2d done-bar).

### Batch sequencing rationale

- **Batch 1** is pure infrastructure (shared WGSL + types + atmosphere) — independently
  reviewable, ends runnable, zero pipeline-restructure risk.
- **Batch 2** is the one risky restructure (the first-hit rewire + the temporary blit seam) —
  isolated in its own batch so the reviewer can focus on it; ends runnable with a visible image
  change (the real atmosphere).
- **Batches 3–4** build the GI sample *generators* + *refiners* — buffer-only, the done-bar is
  "dispatches clean", the image does not change. Two batches because `sampleRefine` is 5 passes /
  441 HLSL lines — too large for one batch with `globalIllum`.
- **Batch 5** builds the GI *consumers* — the image starts showing GI. Splitting spatial-
  resampling and the denoiser into one batch is fine (the denoiser is small — 146 lines).
- **Batch 6** is the integration batch — the `base/` TAA rewire + reverting the temporary blit +
  the final blit. It is where the pipeline becomes whole; kept last because it depends on every
  prior batch's buffers being correct.

The cross-batch buffer dependencies (Batch 4's `sampleRefine` needs Batch 6's `taa_dist_min_max`;
Batch 5's spatial pass needs Batch 4's `valid_samples_compressed`) are **called out in the steps
above** — a half-built pipeline is correct-but-empty, never invalid, so each batch's "dispatches
clean" done-bar holds.

---

## 12. Open items the orchestrator must surface before Phase-B implementation begins

These are decisions/risks the orchestrator should put to the user (or confirm) before dispatching
Batch 1 — they are within the binding scope but the implementer should not have to guess:

1. **The first-hit restructure (§6.3) is a non-trivial rewire of A-2 code.** The `base/` first-hit
   does NOT write `taa_sample_accum`/`taa_samples` — those move to `base/renderTaaSampleReverse.fx`'s
   two passes. This means Batch 2 temporarily re-points the final blit at `final_color` and
   Batch 6 reverts it. This is a designed seam (like Phase A's `shaded_color` stand-in) but the
   orchestrator should be aware the A-2 first-hit `taa_samples` write — which `08-review-a2.md`
   verified as correct — is *removed* in Phase B. It was correct *for the albedo pipeline*; the
   `base/` pipeline restructures it. **Not a regression — a pipeline-shape change.** Confirm the
   orchestrator/user accepts this.

2. **`screenPosDistanceSqr` reject threshold: A-2 used `1.0`, the `base/` HLSL says `16.0`
   (§5.8.1).** `06-design-a2.md` §7.4 specified `> 1.0` ("within 1 pixel"); the actual
   `base/renderTaaSampleReverse.fx:139` reject is `> 16.0`. The A-2 `albedo/` shader
   (`albedo/renderTaaSampleReverse.fx`) — re-verify which value the *albedo* variant uses; if the
   albedo variant also says `16.0`, then A-2's `1.0` is an A-2 *bug* the Phase-B port should fix
   (and flag for an A-2 erratum). If the albedo variant genuinely says `1.0` and only `base/`
   says `16.0`, then it is a deliberate per-variant difference and Phase B correctly uses `16.0`.
   **The implementer must Read `albedo/renderTaaSampleReverse.fx` to settle this** — the design
   says "port the `base/` value `16.0` for Phase B" regardless, but whether A-2 needs an erratum
   depends on the albedo source. Flag for the orchestrator.

3. **`vec3<u32>` storage-buffer alignment (§3.3).** The C# `Uint3` `StructuredBuffer`s
   (`atmosphereComp`, `denoisePreprocessed`, `denoisePreprocessedHorizontal`) have 12-byte
   stride; WGSL `array<vec3<u32>>` has 16-byte stride. The design resolves this by storing them
   as `array<vec4<u32>>` (`.w` padding) and sizing the Rust buffers accordingly. This is a
   deliberate, faithful-enough port deviation (the bit *contents* are identical, only the stride
   differs) — but it is a deviation the orchestrator should note exists, and it makes
   `atmosphere_comp` 64 MiB (verify against `max_buffer_size`).

4. **`COLORS[32]` / `COLOR_DIF_PROB[31]` as hard-coded literals (§5.3).** WGSL `const` arrays
   cannot hold `pow()` results. The design hard-codes the 63 computed `f32` literals into
   `color_compression.wgsl` with a Rust `#[test]` guarding them. This is the cleanest option but
   it means a small generated-literals block in a WGSL file. Confirm acceptable (vs. uploading a
   uniform — also viable but adds a bind-group entry to every GI pass).

5. **Storage-buffer atomics (§5.5, §5.7).** `sample_counts` and `bucket_info` must be declared
   `array<...atomic<u32>...>` in WGSL for the `InterlockedAdd`s. This is a hard wgpu requirement
   — flag it so the implementer declares these buffers' WGSL types as atomic from the start (not
   discovered mid-port).

6. **Phase B is large — 13 render nodes, ~12 pipelines, 9 new WGSL entry-point files, ~1900
   lines of HLSL ported.** The 6-batch breakdown keeps each batch reviewable, but the orchestrator
   should expect Phase B to take meaningfully longer than A or A-2 and budget the
   dispatch/review cycles accordingly. Batches 3 and 4 (the GI sample generators/refiners) are
   the heaviest — `renderGlobalIllum.fx` + `renderSampleRefine.fx` together are ~740 HLSL lines.

7. **The `WorldRenderBase` GUI sliders → `AppArgs` constants (§1, §3.8).** Every
   `SettingDataRenderBase` slider becomes a fixed `GiSettings` value (the slider *defaults*). If
   the user wants any of these tunable at runtime later, that is a post-Phase-B addition — Phase B
   ships them as constants (matching how A-2 handled `taaSampleMaxAge` — `06-design-a2.md` §13.4).
   Confirm the user does not want a Phase-B GI settings GUI (the brief's "no editor" / Q1 implies
   not, but the GI sliders are arguably "render settings" not "editor" — worth a one-line
   confirm).
