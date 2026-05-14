# 06 — Phase A-2 Architecture Design (long-term-memory TAA)

## delegate-architect findings — Phase A-2 (2026-05-14)

Phase A-2 ports NAADF's **long-term-memory TAA** — research-doc §4.1, `02-research.md`
§1.2.1–§1.2.2 — onto the completed Phase-A albedo render path. This is the implementable
design: an `impl` agent executes §11's numbered sequence without further architectural
decisions.

All choices sit inside the binding constraints from `01-context.md` §2c — the §6 decision
(16-deep `taaSamples` ring, 128-deep camera-history ring, NAADF's own TAA, DLSS deferred), the
0.25-spp Phase-B feed constraint, and the carried `frame_count`/`rand_counter` fix. They are
cited, not relitigated. Every HLSL path / line and every Bevy-port path / line below is
verified against the source on disk — nothing here is invented.

Source HLSL read directly for this design:
`Content/shaders/render/versions/albedo/renderTaaSampleReverse.fx`,
`renderFirstHit.fx`, `renderFinal.fx`, `common/taa/commonTaa.fxh`,
`common/commonColorCompression.fxh`, `common/commonRenderPipeline.fxh`;
C# `World/Render/Versions/WorldRenderAlbedo.cs`, `World/Render/WorldRender.cs`,
`Common/Camera.cs`.

---

## 0. Scope of this document

| section | covers | brief item |
|---|---|---|
| §1 | What A-2 is / is not — non-scope statement | brief 9 |
| §2 | TAA data structures — `taaSamples` 16-ring, `taaSampleAccum`, camera-history ring | brief 1 |
| §3 | The 64-bit TAA sample format — derived exactly from `commonTaa.fxh` | brief 1 |
| §4 | `gpu_types.rs` deltas — Rust `#[repr(C)]` types + WGSL struct decls | brief 1 |
| §5 | Bind-group plan deltas | brief 1 |
| §6 | The first-hit change — adding the `taaSamples`-ring write | brief 3 |
| §7 | The reprojection + accumulation pass — `renderTaaSampleReverse.fx` → WGSL | brief 2 |
| §8 | The render-graph change — the TAA node + the final-blit swap | brief 4 |
| §9 | Extract / prepare changes — frame counter, camera-history ring, jitter, buffers | brief 5 |
| §10 | `src/` module layout deltas | brief 6 |
| §11 | HUD | brief 7 |
| §12 | Numbered Phase-A-2 implementation sequence | brief 8 |
| §13 | Open items the orchestrator must surface before implementation | — |

---

## 1. What Phase A-2 is — and explicitly is NOT (brief item 9)

**Phase A-2 IS:** the albedo-path long-term-memory TAA. Phase A's first-hit compute pass gains
the `taaSamples`-ring write the Phase-A port omitted (`04-impl.md` step 10 logged this omission);
a new TAA-reproject compute node slots between `naadf_first_hit` and `naadf_final_blit`; the
Phase-A `shaded_color` blit-source stand-in is replaced by the real `taaSampleAccum` buffer; and
`prepare.rs`'s `frame_count`/`rand_counter` misuse is fixed to a real monotonic frame counter.

**Phase A-2 is NOT** (do not design or implement any of these in A-2):

- **Phase B GI / ReSTIR / denoiser.** No `rayQueueCalc`, `renderGlobalIllum`,
  `renderSampleRefine`, `renderSpatialResampling`, `renderDenoiseSplit`, no atmosphere
  precompute, no `base/renderTaaSampleReverse.fx` `CalcNewTaaSample` second pass. A-2 ports the
  **albedo** TAA only (`renderTaaSampleReverse.fx`, the `albedo/` tree — `01-context.md` §2c).
- **The DLSS / DLSS-RR evaluation.** The `dlss` / `force_disable_dlss` Cargo plumbing stays
  dormant exactly as Phase A left it (`01-context.md` §2c — "Phase A-2 does **not** depend on
  DLSS"). A-2 must not wire the NAADF TAA to DLSS, must not add G-buffer extensions for it.
- **The non-A-2 `05-review.md` §4 secondary issues.** Specifically: `prepare_world_gpu`
  re-running every frame (`05-review.md` §5's "Noted in passing" — the `existing.is_some() &&
  !extracted.dirty` early-out not tripping) is **out of A-2 scope** — leave it. The zeroed
  `GpuRenderParams.bounding_box_*` fields are **out of A-2 scope** — leave them zeroed; A-2 adds
  no dependency on them (the traversal reads `world_meta` instead — `05-review.md` §4). The
  `rayAABB` f32-precision faithful-port note is **not touched**.
- **Specular / 4-plane G-buffer reconstruction.** Phase A's first-hit fills only plane 0
  (`03-design.md` §5.3); A-2 keeps it that way. The TAA reproject's `getHitDataFromPlanes`
  reduces to a single-plane reconstruction (§7.3) — the specular-path `getHitDataFromPlanes`,
  `getSpecularNormals`, the `SPECULAR_MIRROR_FAC` LUT, `pdf_vndf_isotropic`, and the rough-
  specular `extraData` reweight in `renderTaaSampleReverse.fx:138-148` are Phase B.
- **Entities.** `02-research.md` §1.1.7 / `03-design.md` §7.5 keep entities a deferred Phase-A
  sub-feature; A-2 stays entity-free. The `#ifdef ENTITIES` blocks in
  `renderTaaSampleReverse.fx` (lines 76-84, 96-104) and the `entityInstancesHistory` bind are
  **omitted**, exactly as Phase A omitted the `ENTITIES` traversal branch.
- **The 32-deep sample ring.** The §6 VRAM lever fixes the ring at **16-deep**. Do not make it
  32. Do not make it configurable beyond what §2.1 specifies.

---

## 2. TAA data structures (brief item 1)

NAADF's albedo TAA owns three GPU resources plus a CPU-side camera-history ring
(`WorldRenderAlbedo.cs:32-65`). The mapping to the Bevy port:

| NAADF (C#) | NAADF size | Bevy A-2 resource | A-2 size |
|---|---|---|---|
| `taaSamples` `StructuredBuffer<Uint2>` (`WorldRenderAlbedo.cs:57`) | `screenW·screenH·32` | `TaaGpu.taa_samples : Buffer` (`array<vec2<u32>>`) | `screenW·screenH·16` |
| `taaSampleAccum` `StructuredBuffer<Uint2>` (`:58`) | `screenW·screenH` | `TaaGpu.taa_sample_accum : Buffer` (`array<vec2<u32>>`) | `screenW·screenH` |
| `taaSampleCamTransform[128]` + `taaSampleCamTransformInvers[128]` + `oldCamPositions[128]` + `taaSampleJitter[128]` + `taaOldCamPosFromCurCamInt[128]` (`:36-40`, `:60-64`) | 128-deep CPU rings | `CameraHistory` main-world `Resource` (CPU rings) → uploaded into `TaaGpu` per-frame uniform/storage | **128-deep** (unchanged — §6 lever is the *sample* ring only) |

### 2.1 The `taaSamples` ring — 16-deep (the §6 VRAM lever)

`taaSamples` is a flat storage buffer of `vec2<u32>` (64 bits/sample — §3). NAADF lays it out
**slot-major**: `taaSamples[(taaIndex % 32) * screenW * screenH + pixelIndex]`
(`renderFirstHit.fx:116`, `renderTaaSampleReverse.fx:113`). A-2 keeps the slot-major layout but
the ring is **16 slots, not 32**:

```
taa_samples[slot * pixel_count + pixel_index]      slot ∈ [0, 16)
```

- Element count: `pixel_count * 16`. At 1440p (`2560·1440 = 3_686_400` px) that is
  `58_982_400` elements × 8 bytes ≈ **472 MB** — consistent with `01-context.md` §2c's
  "~501 MB @1440p" estimate (the difference is the camera-history ring + `taa_sample_accum`).
  The 32-deep ring would be ~944 MB — the lever saves ~470 MB.
- A single named constant carries the depth — **`TAA_SAMPLE_RING_DEPTH = 16`** — declared once
  in Rust (`render/taa.rs`) and once in WGSL (`taa.wgsl`); every `% 32` and `* 32` in the HLSL
  becomes `% TAA_SAMPLE_RING_DEPTH` / `* TAA_SAMPLE_RING_DEPTH`. **Implementer note:** the HLSL
  has `(taaIndex % 32)` in `renderFirstHit.fx:116` and `(taaIndex + i) % 32` in
  `renderTaaSampleReverse.fx:91` — *both* become `% 16`. Do not miss the second one. The
  camera-history index `(taaIndex + i) % 128` (`renderTaaSampleReverse.fx:90`) stays `% 128`.
- The buffer needs `STORAGE | COPY_DST` (the first-hit pass writes into it; the reproject pass
  reads from it). It must be **zero-cleared on creation** (like Phase A's `first_hit_data` /
  `shaded_color` — `prepare.rs:355-362`) so the first ~16 frames, before the ring is full, read
  zeroed (rejected) history rather than garbage.
- It is **resized on viewport resize** (same trigger as `first_hit_data` — §9.4).

### 2.2 The `taa_sample_accum` buffer — the real blit source

`taaSampleAccum` is `vec2<u32>` per pixel (`pixel_count` elements, 8 bytes each). It is the
per-pixel accumulated colour + accumulated sample weight. **This buffer replaces Phase A's
`shaded_color` stand-in** — the Phase-A `shaded_color` was deliberately built to the
`taaSampleAccum` element format (`03-design.md` §5.3, verified: Phase A's `naadf_first_hit.wgsl`
writes `pack2x16float(vec2(1.0, light.r))` / `pack2x16float(vec2(light.g, light.b))` — exactly
`renderFirstHit.fx:120-121`'s `taaSampleAccum` write). So this is a **rename + re-home**, not a
format change:

- `shaded_color` (in `FrameGpu`, §2.5 of `03-design.md`) → renamed `taa_sample_accum`, moved
  into the new `TaaGpu` render-world resource (§5). The first-hit pass still writes it; the TAA
  reproject pass reads-modify-writes it; the final blit reads it.
- Element format unchanged: `.x = f16(weight) | (f16(color.r) << 16)`,
  `.y = f16(color.g) | (f16(color.b) << 16)` — the `02-research.md` §1.2.2 / `renderFinal.fx:36-39`
  layout.
- **The per-pixel accumulated sample count is `weight` — `f16tof32(taa_sample_accum[px].x &
  0xFFFF)`.** This is the binding 0.25-spp constraint (`01-context.md` §2c): the first-hit pass
  writes weight `1.0` for the current frame's sample; `renderTaaSampleReverse.fx:169` accumulates
  `sampleWeight + colorSum.a` where `colorSum.a` is the count of accepted reprojected history
  samples. After the reproject pass, `taa_sample_accum[px].x & 0xFFFF` (as f16) is the per-pixel
  count of frames currently contributing to that pixel — the exact signal Phase B's adaptive
  sampler reads. **A-2 must preserve this `.a` accumulation faithfully (§7.5).** Record:
  **0.25 spp is the Phase-B GI sampling target; the A-2 TAA's per-pixel `weight` accumulation is
  the signal that drives it.** A-2 itself does not consume the count (no GI sampler yet), but it
  must be intact and exposed in `taa_sample_accum`.
- Also zero-cleared on creation, resized on viewport resize.

### 2.3 The 128-deep camera-history ring

NAADF keeps **128**-deep CPU rings of per-frame camera state (`WorldRenderAlbedo.cs:36-40`,
`:60-64`), indexed by `taaIndex = 128 - (frameCount % 128) - 1` (`WorldRender.cs:88`). The §6
decision keeps this at NAADF's depth — it is tiny in VRAM (128 × a few matrices). Per frame, at
slot `taaIndex`, NAADF stores (`WorldRenderAlbedo.cs:76-84`):

| C# field | what it is | used by the reproject pass as |
|---|---|---|
| `oldCamPositions[taaIndex] = camPos` | the frame's `PositionSplit` (int+frac world pos) | source for the *derived* `taaOldCamPosFromCurCamInt` (below) |
| `taaSampleCamTransform[taaIndex] = camera.viewProjTransform` | the frame's view-proj matrix (translation-free, origin-based — `Camera.cs:201`) | `camRotOld[curHistoryIndex]` — projects a virtual pos into that past frame's screen |
| `taaSampleCamTransformInvers[taaIndex] = camera.invViewProjTransform` | the inverse (the Phase-A `inv_view_proj`) | **stored but the albedo reproject pass does not bind `camRotOldInv`** — verified: `renderTaaSampleReverse.fx` binds `camRotOld`, `invCamMatrix`, `camMatrix`, not an inverse-array. So A-2 does **not** need to ring this; see §9.2. |
| `taaSampleJitter[taaIndex] = taaJitter` | the frame's Halton jitter (`taaJitterOld[]`) | `curTaaJitter` — negated and passed as the pixel offset of the reprojection |

Plus the **per-frame-derived** array, recomputed *every frame for all 128 slots* against the
*current* camera int position (`WorldRenderAlbedo.cs:81-84`):

```
taaOldCamPosFromCurCamInt[i] = (oldCamPositions[i] - camPos).toVector3()   // for i in 0..128
```

i.e. each past frame's camera position **expressed relative to the current frame's camera
integer position** — a `PositionSplit` subtraction then `.toVector3()`. This is the
camera-relative-rendering trick (D1): the reproject math (`renderTaaSampleReverse.fx:109,
120-121`) works in *current-camera-int-relative* space, never in absolute world space, so f32
precision holds for large worlds.

**A-2's `CameraHistory` resource** (main world) is therefore four CPU rings:

```rust
// src/render/taa.rs  (or a camera/ submodule — §10)
pub const CAMERA_HISTORY_DEPTH: usize = 128;

#[derive(Resource)]
pub struct CameraHistory {
    /// Per-frame camera PositionSplit (C# oldCamPositions[128]).
    pub positions: [PositionSplit; CAMERA_HISTORY_DEPTH],
    /// Per-frame translation-free view-proj matrix (C# taaSampleCamTransform[128]).
    pub view_proj: [Mat4; CAMERA_HISTORY_DEPTH],
    /// Per-frame Halton jitter (C# taaSampleJitter[128]).
    pub jitter: [Vec2; CAMERA_HISTORY_DEPTH],
    /// Monotonic frame counter (C# WorldRender.frameCount).
    pub frame_count: u32,
}
```

`taaIndex` is **derived**, not stored: `taa_index = CAMERA_HISTORY_DEPTH - (frame_count %
CAMERA_HISTORY_DEPTH) - 1` (`WorldRender.cs:88`). It is computed in the update system (§9.1) and
in `prepare_taa` (§9.2) — keep the formula in **one** helper fn so it cannot drift.

The `taaOldCamPosFromCurCamInt` array is **not stored in `CameraHistory`** — it is derived each
frame in `prepare_taa` from `positions` minus the current camera's `PositionSplit`, and uploaded
as part of the per-frame TAA uniform/storage (§9.2).

---

## 3. The 64-bit TAA sample format (brief item 1)

Derived **exactly** from `commonTaa.fxh` `compressSample` / `decompressSample` (read directly,
lines 30-53) — and cross-checked against `02-research.md` §1.2.2's C# cross-check note. The
sample is a `uint2` = 64 bits, laid out:

### 3.1 `.x` — distance (low 16) + hash (high 16)

```
sampleComp.x = distComp | ((hash & 0xFFFF) << 16)
```

- **bits 0–15: `distComp`** — `f32tof16(dist)` (a 16-bit half-float distance). At the
  `compressSample` call site (`renderFirstHit.fx:115`) the caller passes
  `f32tof16(voxelTypeRaw == 0 ? 65520 : distanceRay)` — a hit's primary-ray distance, or `65520`
  (≈ f16 max) for a miss. `decompressSample` reads `f16tof32(sampleComp.x & 0x7FFF)` — note the
  `& 0x7FFF`, masking the f16 *sign* bit off (distance is always positive).
- **bits 16–31: `hash`** — the low 16 bits of `getHashFromData(isDiffuse, specularNormals,
  entity)` (`commonTaa.fxh:20-28`). For Phase A's plane-0-only, entity-free, all-diffuse world:
  `isDiffuse = 1`, `specularNormals = 0`, `entity = ENTITY_FREE = 0x3FFF`. So
  `hash = getHashFromData(1, 0, 0x3FFF)` — **a constant** in A-2 (every pixel that hits geometry
  has the same hash; misses too). The hash machinery is still ported faithfully (it is cheap and
  Phase B needs it varying), but in A-2 it collapses to one value. See §7.4 for why this still
  matters for the reproject reject test.

### 3.2 `.y` — exponential-compressed colour (24) + normal (3) + extraData (5)

```
sampleComp.y = colorComp.x | (colorComp.y << 8) | (colorComp.z << 16)
             | (normalComp << 24) | (extraData << 27)
```

- **bits 0–23: `colorComp` (8 bits/channel R,G,B)** — the **exponential colour compression**.
  `compressSample` (`commonTaa.fxh:33-35`):
  ```
  maxColorChannel = max(color.r, max(color.g, color.b));
  if (maxColorChannel > 100) color *= (100.0 / maxColorChannel);   // clamp to [0,100]
  colorComp = 12 * log2(color + pow(2, -255.0/12.0) * 100) + (255.0 - 12.0 * log2(100.0));
  ```
  `decompressSample` (`commonTaa.fxh:48-49`):
  ```
  colComp = uint3(sampleComp.y & 0xFF, (sampleComp.y >> 8) & 0xFF, (sampleComp.y >> 16) & 0xFF);
  color = float4(100.0 * pow(2, (colComp - 255.0) / 12.0), 1);   // .a := 1
  ```
  This is `02-research.md` §1.2.2's `f(x) = 12·log₂(x/100 + 2^(-255/12)) + 255` (algebraically
  equal — `12·log2(x + 2^(-255/12)·100) + 255 - 12·log2(100) = 12·log2((x/100) + 2^(-255/12)) +
  255`). **Implementer note:** `colorComp` is computed as a `uint3` in HLSL — the float result
  is *truncated* to integer per channel on assignment, then re-masked `& 0xFF`. WGSL has no
  implicit float→uint truncation; the port must `u32(...)` explicitly and `& 0xFFu` each channel
  (a `255+`-valued result must clamp into 8 bits — HLSL's `uint` cast wraps, but for in-range
  `[0,100]` colours the result is in `[0,255]`; still `min(255u, ...)` to be safe). The
  decompressed `color.a` is **always set to `1.0`** — that `.a = 1` is the per-sample
  "this sample counts as 1" weight that the accumulation sums (§7.5). **This is the load-bearing
  bit for the 0.25-spp signal — do not drop the `.a`.**
- **bits 24–26: `normalComp` (3 bits)** — the 3-bit normal index. `compressSample` is called
  with `firstHitNormalTang & 0x7` (`renderFirstHit.fx:115`) — the low 3 bits of the plane-0
  normal-tang code, i.e. the `NORMAL[]` LUT index. `decompressSample` reads
  `(sampleComp.y >> 24) & 0x7`. Used by the rough-specular reweight (Phase B); in A-2 it is
  written and read but only consumed by the §7.3-deferred specular branch — keep it for format
  fidelity.
- **bits 27–31: `extraData` (5 bits)** — material roughness, 5-bit. `renderFirstHit.fx:115`
  passes `0` for the albedo path (no rough-specular surfaces). `decompressSample` reads
  `sampleComp.y >> 27`. In A-2 it is always `0` ⇒ the `extraData != 0` branch in
  `renderTaaSampleReverse.fx:138-148` is dead — see §7.3.

### 3.3 Rust mirror

There is **no `#[repr(C)]` struct** for the sample — it is two raw `u32`s read/written by the
WGSL with bit ops. The buffer is `array<vec2<u32>>` on the GPU and `Buffer` of `2·u32` elements
on the CPU side (sized by element count). Provide WGSL helper fns `taa_compress_sample` /
`taa_decompress_sample` in `taa_common.wgsl` (§10) that are line-faithful ports of
`commonTaa.fxh` — that is the only place the layout is encoded.

---

## 4. `gpu_types.rs` deltas — Rust types + WGSL struct decls (brief item 1)

### 4.1 `GpuRenderParams` — fix `frame_count`, populate `taa_index`, set `is_taa` (no layout change)

The existing `GpuRenderParams` (`gpu_types.rs:55-102`) already has `frame_count`, `rand_counter`,
`taa_index`, `flags` (with `FLAG_IS_TAA` already defined, `gpu_types.rs:110`), and `taa_jitter`.
**A-2 changes the *values written*, not the struct layout:**

- `frame_count` — currently `time.elapsed().as_millis()` (`prepare.rs:278`). A-2: a **real
  monotonic `u32` frame counter** (§9.1). This is the carried `05-review.md` §4 fix.
- `rand_counter` — currently `elapsed_secs * 1000` (`prepare.rs:279`). NAADF's `randCounter`
  indexes a `randValues[32]` table refilled per frame (`WorldRender.cs:82-86`,
  `WorldRenderAlbedo.cs:94`). A-2's faithful-enough port: `rand_counter` = the frame counter (a
  monotonic per-frame salt is what the RNG needs — `initRand` in `naadf_first_hit.wgsl` uses it
  only as RNG salt). Do **not** port the full `randValues[32]` table — that is gold-plating; a
  monotonic counter is the load-bearing property. (Document this as a deliberate simplification
  in the `prepare.rs` comment.)
- `taa_index` — currently always `0` (`prepare.rs:280`). A-2: the derived
  `CAMERA_HISTORY_DEPTH - (frame_count % CAMERA_HISTORY_DEPTH) - 1` (§2.3).
- `flags` — A-2 sets `FLAG_IS_TAA` when `AppArgs.taa` is true (it is wired but `false` in Phase
  A — `main.rs:45,51`; A-2 flips the default to `true`, see §9.5). The first-hit pass branches
  on it (§6).
- `taa_jitter` — currently `Vec2::ZERO` (`prepare.rs:289`). A-2: the per-frame Halton jitter
  (§9.3) when `AppArgs.taa` && jitter enabled, else zero.

No new fields, no size change — `GpuRenderParams` stays `16 * 7` bytes; the compile-time assert
(`gpu_types.rs:226`) still holds. The WGSL `GpuRenderParams` struct
(`render_pipeline_common.wgsl:104-120`) is unchanged.

### 4.2 New `GpuTaaParams` uniform — the TAA reproject pass's own uniform

`renderTaaSampleReverse.fx` binds a uniform set that overlaps `renderFirstHit.fx`'s but is not
identical (it adds `camMatrix`, `sampleAge`, the `[128]` arrays). Rather than widen
`GpuRenderParams`, A-2 adds a **dedicated `GpuTaaParams`** `#[repr(C)]` uniform for the TAA node,
mirroring `renderTaaSampleReverse.fx:10-21`'s scalar uniforms:

```rust
// src/render/gpu_types.rs  (new)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuTaaParams {
    /// Rotation-only inverse view-proj (C# invCamMatrix) — for get_ray_dir.
    /// Same matrix Phase A puts in GpuCamera.inv_view_proj.
    pub inv_view_proj: Mat4,
    /// Translation-free view-proj of the CURRENT frame (C# camMatrix) — projects
    /// a reprojected virtual pos into the current screen for the 1-px reject test.
    pub view_proj: Mat4,
    /// Current camera integer position (C# camPosInt) — base for the
    /// camera-relative reprojection space.
    pub cam_pos_int: IVec3,
    pub _pad0: u32,
    /// Current camera fractional position (C# camPosFrac).
    pub cam_pos_frac: Vec3,
    pub _pad1: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    /// Monotonic frame counter (C# frameCount).
    pub frame_count: u32,
    /// taaIndex = CAMERA_HISTORY_DEPTH - (frame_count % CAMERA_HISTORY_DEPTH) - 1.
    pub taa_index: u32,
    /// How many past frames to walk (C# sampleAge / taaSampleMaxAge).
    /// Clamped to [1, TAA_SAMPLE_RING_DEPTH] in A-2 — see §7.1.
    pub sample_age: u32,
    pub _pad2: u32,
    pub _pad3: u32,
    pub _pad4: u32,
}
```

Layout: `mat4 (64) + mat4 (64) + (ivec3+pad) (16) + (vec3+pad) (16) + 4×u32 (16) + 4×u32 (16)` =
**192 bytes**, 16-byte aligned throughout. Add a compile-time `assert!(size_of::<GpuTaaParams>()
== 192)`.

**Implementer note — `camMatrix` (`view_proj`):** `renderTaaSampleReverse.fx:127` does
`mul(float4(oldVirtualPos, 1), camMatrix)` to project a virtual position into the *current*
frame's screen. `oldVirtualPos` is in **current-camera-int-relative** space (§2.3). NAADF's
`camera.viewProjTransform` (`Camera.cs:201`) is the **translation-free, origin-based** view-proj
— so projecting a camera-int-relative position with it is correct. The Bevy port must therefore
upload, as `view_proj`, the **same rotation-only view-proj** that `extract_camera` already builds
the *inverse* of for `GpuCamera.inv_view_proj` (`extract.rs:117-119` —
`clip_from_view * world_from_view_rot.inverse()`, before the final `.inverse()`). i.e.
`view_proj = clip_from_view * world_from_view_rot.inverse()` and `inv_view_proj =
view_proj.inverse()`. **This is the §5.x perspective-fix lineage — A-2 must reuse it, not
re-derive it.** See §9.2.

### 4.3 New `GpuCameraHistory` storage — the 128-deep ring, GPU side

The reproject shader indexes `camRotOld[128]`, `taaOldCamPosFromCurCamInt[128]`,
`taaJitterOld[128]`. A `mat4[128]` does not fit a uniform's 64 KiB limit comfortably alongside
the rest (128 × 64 B = 8 KiB for matrices alone — actually fine), but the cleanest, layout-stable
choice is a **read-only storage buffer** of a per-slot struct:

```rust
// src/render/gpu_types.rs  (new)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCameraHistorySlot {
    /// Past frame's translation-free view-proj (C# camRotOld[i]).
    pub view_proj: Mat4,
    /// Past frame's camera pos, relative to the CURRENT camera int position
    /// (C# taaOldCamPosFromCurCamInt[i] = (oldCamPositions[i] - camPos).toVector3()).
    /// Recomputed every frame in prepare_taa.
    pub cam_pos_from_cur_int: Vec3,
    pub _pad0: u32,
    /// Past frame's Halton jitter (C# taaJitterOld[i]).
    pub jitter: Vec2,
    pub _pad1: Vec2,
}
```

Layout: `mat4 (64) + (vec3+pad) (16) + (vec2+vec2pad) (16)` = **96 bytes/slot**, 16-byte
aligned. The buffer is `array<GpuCameraHistorySlot, 128>` — `128 · 96 = 12_288` bytes. Bound as
a **read-only storage buffer** (it is small but a fixed-size uniform array of this struct is more
fragile across naga than a runtime-sized storage array; storage is the safe call and matches how
NAADF's `[128]` arrays are effectively just buffers). Created **once** (it is fixed-size, not
resized on viewport change), rewritten every frame by `prepare_taa`.

### 4.4 WGSL struct decls

`taa.wgsl` declares `GpuTaaParams` and `GpuCameraHistorySlot` mirroring the Rust layout, with the
**same no-explicit-`_pad`-members convention** Phase A established (`04-impl.md` step 11
deviation #3 — naga-oil's composable-module round-trip rejects explicit padding members; WGSL's
`vec3`→16-byte and `vec2`→8-byte slotting reproduces the padded `#[repr(C)]` layout). Document
the field offsets in the WGSL comment, exactly as `render_pipeline_common.wgsl:84-95,99-103` does
for `GpuCamera` / `GpuRenderParams`.

`taa_common.wgsl` declares no structs — only the `taa_compress_sample` / `taa_decompress_sample`
helpers and the `taa_hash_from_data` / `taa_neighbor_offsets` constants (§10).

---

## 5. Bind-group plan deltas (brief item 1)

Phase A has three bind-group layouts in `NaadfPipelines` (`pipelines.rs:66-87`): `world_layout`
(`@group(0)` — `chunks`/`blocks`/`voxels`/`voxel_types`/`world_meta`), `frame_layout`
(`@group(1)` — `camera`/`render_params`/`first_hit_data`/`shaded_color`), `blit_layout`
(`first_hit_data`/`shaded_color`/`render_params`). A-2's changes:

### 5.1 `frame_layout` — rename binding 3, keep the layout shape

Binding 3 of `@group(1)` is currently `shaded_color` (`storage_buffer_sized(false, None)` — rw —
`pipelines.rs:136`). A-2 renames it to `taa_sample_accum` (same wgsl type `array<vec2<u32>>`,
same `read_write` access — the first-hit pass still writes it). **Layout shape unchanged** — it
is a pure rename so the first-hit pass keeps writing the accum buffer exactly as Phase A did.
`naadf_first_hit.wgsl`'s `@group(1) @binding(3)` decl is renamed; its write site is unchanged
(§6).

### 5.2 New `taa_layout` — `@group(2)` for the first-hit pass's TAA-sample write

The first-hit pass, when `is_taa`, additionally writes `taaSamples`
(`renderFirstHit.fx:116`). That needs a new binding. **Add a third bind group `@group(2)` to the
first-hit pipeline**, a new `taa_layout`:

- `@group(2) @binding(0)` — `taa_samples : array<vec2<u32>>`, **read-write** storage
  (`storage_buffer_sized(false, None)`), `ShaderStages::COMPUTE`.

That is the *only* TAA resource the first-hit pass touches (it writes one slot of the ring; it
does not touch `taa_sample_accum` via this group — that stays in `@group(1)`, nor the camera
history). Keeping it a separate one-binding group means: (a) the first-hit pipeline layout is
`[world_layout, frame_layout, taa_layout]`; (b) when `AppArgs.taa` is off the group can still be
bound (the shader's `if (is_taa)` guards the write) — but see §6 for the simpler "always bind"
approach.

### 5.3 New `taa_reproject_layout` — the TAA node's bind group(s)

The reproject pass (`renderTaaSampleReverse.fx`) reads `firstHitData`, `taaSamples`, reads-writes
`taaSampleAccum`, and reads the camera-history arrays + scalar uniforms. One bind group
`taa_reproject_layout` (`ShaderStages::COMPUTE`):

| binding | resource | wgsl type | access |
|---|---|---|---|
| 0 | `taa_params` | `GpuTaaParams` uniform | uniform |
| 1 | `camera_history` | `array<GpuCameraHistorySlot, 128>` storage | read |
| 2 | `first_hit_data` | `array<vec4<u32>>` storage | read |
| 3 | `taa_samples` | `array<vec2<u32>>` storage | read |
| 4 | `taa_sample_accum` | `array<vec2<u32>>` storage | **read_write** |

The reproject pass does **not** need `@group(0)` world data — it does not traverse the voxel
world (it only reprojects history; verified: `renderTaaSampleReverse.fx` has no `shootRay` call).
So the TAA node binds **only `taa_reproject_layout` as `@group(0)`** of its own pipeline. Simple,
self-contained.

### 5.4 `blit_layout` — rename binding 1

`blit_layout` binding 1 is `shaded_color` (`pipelines.rs:151`). Rename to `taa_sample_accum`
(same type/access — read-only storage). `naadf_final.wgsl`'s `@group(0) @binding(1)` decl
renamed; the rest of `naadf_final.wgsl` is unchanged (it already reads the `taaSampleAccum`
element format — `03-design.md` §5.3, verified against `renderFinal.fx:36-39`).

### 5.5 Bind-group construction (`prepare.rs` / `prepare_taa`)

- `prepare_frame_gpu` (`prepare.rs:240-400`): `bind_group` (the `@group(1)` frame group) now
  binds `taa_sample_accum` (from `TaaGpu`) at slot 3 instead of the local `shaded_color`
  (§9.4 moves `taa_sample_accum` ownership to `TaaGpu`). `blit_bind_group` likewise binds
  `taa_sample_accum` at slot 1. **Ordering note:** `prepare_frame_gpu` runs in
  `PrepareBindGroups` (`render/mod.rs:60-62`); `TaaGpu` must exist by then — `prepare_taa` (which
  creates `TaaGpu`) must run in `PrepareResources`, before `prepare_frame_gpu` — see §9.4, §12.
- A new `taa_first_hit_bind_group` (the `@group(2)` for the first-hit pass) and
  `taa_reproject_bind_group` (the TAA node's group) are built — either in `prepare_taa` or
  `prepare_frame_gpu`; put them in `prepare_taa` since it owns `TaaGpu` and runs in
  `PrepareResources`. But `taa_reproject_bind_group` also references `first_hit_data` (owned by
  `FrameGpu`, created in `prepare_frame_gpu`/`PrepareBindGroups`) — so `taa_reproject_bind_group`
  must be built in `prepare_frame_gpu` (after both `TaaGpu` and `first_hit_data` exist). **Net:**
  `prepare_taa` (PrepareResources) creates `TaaGpu`'s buffers + uploads `camera_history` +
  `taa_params`; `prepare_frame_gpu` (PrepareBindGroups) builds *all* the bind groups that mix
  `FrameGpu` and `TaaGpu` resources. This mirrors Phase A's existing split exactly.

---

## 6. The first-hit change — adding the `taaSamples`-ring write (brief item 3)

Phase A's `naadf_first_hit.wgsl` omitted the HLSL `if (isTAA)` block entirely (`04-impl.md` step
10, logged). The HLSL block (`renderFirstHit.fx:109-117`):

```hlsl
if (isTAA)
{
    uint4 firstHit = compressFirstHitData(distanceRay, normTangs, voxelTypeRaw, entity);
    firstHitData[globalID.x] = firstHit;
    uint specularNormals = getSpecularNormals(firstHit);
    uint2 sampleComp = compressSample(f32tof16(voxelTypeRaw == 0 ? 65520 : distanceRay),
                                      light, firstHitNormalTang & 0x7, true, specularNormals, 0, entity);
    taaSamples[(taaIndex % 32) * screenWidth * screenHeight + globalID.x] = sampleComp;
}
```

### 6.1 What A-2 changes in `naadf_first_hit.wgsl`

1. **Add the `@group(2)` binding** (§5.2):
   `@group(2) @binding(0) var<storage, read_write> taa_samples: array<vec2<u32>>;`
2. **Import the sample-compress helper:** `#import "shaders/taa_common.wgsl"::{taa_compress_sample}`.
3. **Add the ring write**, gated on `FLAG_IS_TAA`. Phase A already writes `first_hit_data`
   *unconditionally* (`04-impl.md` step 10 deviation — the HLSL only writes it inside
   `if(isTAA)`; Phase A made it unconditional so plane 0 is always populated). **A-2 keeps
   `first_hit_data` unconditional** — that is correct and the reproject pass needs it. A-2 only
   adds the *`taa_samples` write* inside the `is_taa` guard:
   ```wgsl
   if ((params.flags & FLAG_IS_TAA) != 0u) {
       // specularNormals = 0 in A-2 (plane-0-only, entity-free — §3.1)
       let specular_normals = 0u;
       // dist for the sample: hit → distance_ray; miss (voxel_type_raw == 0) → 65520
       let sample_dist = select(distance_ray, 65520.0, voxel_type_raw == 0u);
       let sample = taa_compress_sample(
           sample_dist, light, norm_tangs.x & 0x7u,
           1u /*isDiffuse*/, specular_normals, 0u /*extraData*/, entity,
       );
       let slot = params.taa_index % TAA_SAMPLE_RING_DEPTH;
       taa_samples[slot * (params.screen_width * params.screen_height) + pixel_index] = sample;
   }
   ```
   - `norm_tangs.x` is the plane-0 normal-tang code (already computed —
     `naadf_first_hit.wgsl:93`); `& 0x7u` is the 3-bit normal index, as `renderFirstHit.fx:115`'s
     `firstHitNormalTang & 0x7`.
   - `light` is the already-computed shaded colour (the same value written to `taa_sample_accum`
     — `naadf_first_hit.wgsl:151-152`). `compressSample` takes the *colour*; the
     exponential-compress happens inside the helper (§3.2).
   - `entity = ENTITY_FREE` (already in scope — `naadf_first_hit.wgsl:81`).
   - `TAA_SAMPLE_RING_DEPTH = 16u` — a WGSL const in `taa_common.wgsl` (or `taa.wgsl`),
     re-exported into `naadf_first_hit.wgsl` via import.
4. **`compressFirstHitData` provenance note:** the HLSL also calls `getSpecularNormals(firstHit)`
   — in A-2 that is always 0 (plane-0-only), so the port hardcodes `specular_normals = 0u`
   rather than porting `getSpecularNormals` (that fn is Phase B — `02-research.md` §5.1). Note
   this as a deliberate A-2 simplification in the WGSL comment.

The `taa_sample_accum` write at the end of `calc_first_hit` (currently `shaded_color`,
`naadf_first_hit.wgsl:143-157`) is **unchanged in content** — only the binding *name* changes
(§5.1). It still writes `f16(1.0)` weight + the f16 RGB.

**When `AppArgs.taa` is off** (not the A-2 default — §9.5 — but it stays wired): `FLAG_IS_TAA` is
clear, the `if` is skipped, `taa_samples` is never written. The `@group(2)` bind group is still
*bound* (with a valid `taa_samples` buffer — `TaaGpu` always creates it) so the pipeline layout
is satisfied; the shader just does not write it. No branchless-pipeline-variant needed —
matching how Phase A handles `FLAG_CHECK_SUN` / `FLAG_SHOW_RAY_STEP` at runtime, not via pipeline
specialisation.

### 6.2 Pipeline layout change

`NaadfPipelines.first_hit_pipeline`'s layout (`pipelines.rs:162`) goes from
`[world_layout, frame_layout]` to `[world_layout, frame_layout, taa_layout]`. The
`naadf_first_hit_node` (`graph.rs:44-78`) gains `pass.set_bind_group(2, &taa_gpu.taa_first_hit_bind_group, &[])`.

---

## 7. The reprojection + accumulation pass — `renderTaaSampleReverse.fx` → WGSL (brief item 2)

The new shader **`taa.wgsl`** is the faithful WGSL port of
`render/versions/albedo/renderTaaSampleReverse.fx`'s `reprojectOldSamples`
(`[numthreads(64,1,1)]`). Compute entry point name: `reproject_old_samples`. HLSL provenance is
named per-block in the WGSL comments, extending `03-design.md` §5.5's provenance style.

It depends on shared WGSL: `taa_common.wgsl` (`taa_decompress_sample`, the 3×3 neighbour
offsets, `taa_hash_from_data`), `render_pipeline_common.wgsl` (`get_ray_dir`, `NORMAL[]`,
`HIT_UNDEFINED`, `ENTITY_FREE`), and `common.wgsl` (`flatten_index` — not actually needed; the
reproject pass does not flatten 3D — verify and drop the import if unused).

### 7.1 `sample_age` — clamped to the 16-ring

`renderTaaSampleReverse.fx:88` loops `for (i = 1; i < sampleAge; ++i)` where `sampleAge` is the
C# `taaSampleMaxAge` (`WorldRenderAlbedo.cs:122`, an ImGui slider 1..32 —
`WorldRenderAlbedo.cs:21`). The §6 decision caps the **ring** at 16; therefore `sample_age` must
be **clamped to `[1, TAA_SAMPLE_RING_DEPTH]` = `[1, 16]`** — walking more than 16 past frames is
meaningless when only 16 slots exist (slot indices would alias). A-2 has no ImGui; `sample_age`
is a constant set in `prepare_taa` (§9.2) — **default it to `16`** (full 16-frame history — the
A-2 done-bar wants the full ported behaviour). Record it in `GpuTaaParams.sample_age`, clamped.
The loop in `taa.wgsl` is `for (var i = 1u; i < params.sample_age; i = i + 1u)`.

### 7.2 Phase 1 — the 3×3 neighbourhood precompute (`renderTaaSampleReverse.fx:32-75`)

For the pixel and its 8 neighbours (the `neighborOffsets[9]` from `commonTaa.fxh:6-18` — port as
a WGSL `const taa_neighbor_offsets : array<vec2<i32>, 9>` in `taa_common.wgsl`):

```
for i in 0..9:
    cur_pixel_pos = pixel_pos + taa_neighbor_offsets[i]
    cur_first_hit = first_hit_data[cur_pixel_pos.x + cur_pixel_pos.y * screen_width]
    cur_first_hit_result = get_hit_data_from_planes(...)        // §7.3 — single-plane reduction
    cur_first_hit_entity = cur_first_hit.x & 0x3FFFu
    cur_first_hit_is_diffuse = cur_first_hit.y & 0x1u
    cur_dist = f16tof32(cur_first_hit.w & 0x7FFFu)
    if ((cur_first_hit.z & 0x7FFFu) == 0u): cur_dist = 65520.0   // no voxel type ⇒ miss
    if cur_dist < first_hit_dist:                                // track the CLOSEST neighbour
        first_hit_dist = cur_dist
        first_hit_entity = cur_first_hit_entity
        first_hit_pos = cur_first_hit_result.pos
        first_hit_mirror_fac = cur_first_hit_result.normal_mirror_fac   // = (1,1,1) in A-2
    dist_min_max.x = min(dist_min_max.x, cur_dist)
    dist_min_max.y = max(dist_min_max.y, cur_dist)
    // specular-normals accumulation — A-2: all 0, see §7.3; still port the line shape but it
    // folds to a no-op since cur_first_hit_specular_normals == 0
    cur_hash = taa_hash_from_data(cur_first_hit_is_diffuse, cur_first_hit_specular_normals,
                                  cur_first_hit_entity) & 0xFFFFu
    if i == 0: valid_hash_center = cur_hash
    else:      valid_hashes_comp[(i-1)/2] |= cur_hash << (16 * ((i-1) % 2))
```

- `valid_hashes_comp` is `array<u32, 4>` (8 neighbour hashes packed 2-per-u32). `valid_hash_center`
  is the centre pixel's hash.
- **Edge pixels:** `cur_pixel_pos` can go out of bounds at screen edges. The HLSL relies on
  out-of-bounds `firstHitData[...]` reads returning something benign (DX11 SRV returns 0). WGSL
  storage reads out of bounds are **undefined**. A-2 must clamp: `cur_pixel_pos = clamp(pixel_pos
  + offset, vec2(0), vec2(screen_width-1, screen_height-1))` before indexing. This is a
  port-correctness deviation — log it (same kind as `04-impl.md`'s `shoot_ray`
  `any(cur_cell < bounding_box_min)` explicit-bounds deviation). Clamping to the edge pixel is
  benign for the 3×3 min/max/hash precompute.
- `ENTITIES` block at `renderTaaSampleReverse.fx:76-84` — **omitted** (§1).

`rayDir` for this pass: `get_ray_dir(params.inv_view_proj, pixel_pos, screen_width, screen_height,
vec2(0.0))` — **no jitter** (verified: `renderTaaSampleReverse.fx:30` calls `getRayDir` with no
jitter arg, default `(0,0)`). Reuse the *exact* `get_ray_dir` from `render_pipeline_common.wgsl`
(the perspective-fixed one — `05-review.md`).

### 7.3 `get_hit_data_from_planes` — the single-plane A-2 reduction

`getHitDataFromPlanes` (`commonRenderPipeline.fxh:154-213`) reconstructs the virtual-path hit
position from up to 4 stored planes. **In A-2 only plane 0 is filled** (`03-design.md` §5.3 —
Phase A's first-hit only sets `normTangs[0]`; planes 1–3 are `HIT_UNDEFINED`). So the loop at
`commonRenderPipeline.fxh:164-181` runs **zero iterations** (it breaks immediately on
`nextNormalTang == HIT_UNDEFINED` at `:167-168`, since `firstHit.y >> 15 == HIT_UNDEFINED`), and
the function reduces to the tail (`:205-211`):

```wgsl
// A-2 single-plane reduction of getHitDataFromPlanes (commonRenderPipeline.fxh:205-211).
// Planes 1-3 are HIT_UNDEFINED in the albedo path, so the specular-reflection loop
// runs zero iterations and this is the whole function.
fn get_hit_data_from_planes_a2(
    first_hit: vec4<u32>, cam_pos_int: vec3<i32>, cam_pos_frac: vec3<f32>, ray_dir: vec3<f32>,
) -> FirstHitResultA2 {
    var r: FirstHitResultA2;
    let normal_tang = first_hit.x >> 15u;          // plane-0 normal-tang code
    r.normal = NORMAL[normal_tang & 0x7u];
    let ray_dir_comp_for_normal = abs(dot(ray_dir, r.normal));
    let dist_to_tang = abs(
        dot(cam_pos_frac, abs(r.normal))
        - (f32((normal_tang >> 3u)) - dot(vec3<f32>(cam_pos_int), abs(r.normal)))
    );
    let dist_fac = dist_to_tang / ray_dir_comp_for_normal;
    r.pos = cam_pos_frac + ray_dir * dist_fac;     // virtual hit pos, camera-int-relative
    r.dist = dist_fac;
    r.normal_mirror_fac = vec3<f32>(1.0, 1.0, 1.0); // no specular bounces ⇒ identity
    r.ray_dir = ray_dir;
    return r;
}
```

- `FirstHitResultA2` is a small WGSL struct: `pos: vec3<f32>`, `normal: vec3<f32>`,
  `normal_mirror_fac: vec3<f32>`, `dist: f32`, `normal_tang: u32`, `ray_dir: vec3<f32>` — the
  A-2 subset of HLSL `FirstHitResult` (`commonRenderPipeline.fxh:44-50`).
- `r.pos` is in **current-camera-int-relative** space (the HLSL builds it from `camPosFrac` only,
  never adding `camPosInt` — that is the D1 camera-relative trick).
- **Deliberate A-2 simplification — log it:** the full `getHitDataFromPlanes` (the 3-iteration
  specular loop, `SPECULAR_MIRROR_FAC`, the `ENTITIES` block) is Phase B. A-2 ports the
  single-plane reduction. This belongs in `taa.wgsl` (it is TAA-pass-specific in A-2) — not in
  `render_pipeline_common.wgsl` (which is where the *full* Phase-B version will eventually go).
  Name the provenance: "single-plane reduction of `getHitDataFromPlanes`,
  `commonRenderPipeline.fxh:205-211`".

### 7.4 Phase 2 — the reprojection loop (`renderTaaSampleReverse.fx:86-161`)

```
pos_virtual = ray_dir * first_hit_dist                          // virtual pos of THIS pixel's hit
color_sum = vec4(0,0,0,0)                                       // .rgb = accumulated colour, .a = count
for i in 1 .. sample_age:                                       // sample_age ≤ 16
    cur_history_index = (taa_index + i) % 128                   // camera-history ring slot
    cur_taa_index     = (taa_index + i) % TAA_SAMPLE_RING_DEPTH  // sample ring slot — 16, NOT 32

    cur_pos_virtual = pos_virtual                               // (+ entity offset — omitted, §1)
    cur_taa_jitter  = camera_history[cur_history_index].jitter

    // reproject into past frame's screen
    let reproject_pos = cur_pos_virtual - camera_history[cur_history_index].cam_pos_from_cur_int
    screen_index = get_screen_index_projection(
        screen_width, screen_height, reproject_pos,
        camera_history[cur_history_index].view_proj, -cur_taa_jitter)
    if !valid: continue

    // fetch + decompress the past sample
    cur_samp = taa_samples[screen_index + cur_taa_index * screen_width * screen_height]
    (sample_dist, color /*.a = 1*/, normal_comp, extra_data, hash) = taa_decompress_sample(cur_samp)

    // distance reject (noise-insensitive — the long-term-TAA core)
    ray_dir_old   = normalize(cur_pos_virtual - camera_history[cur_history_index].cam_pos_from_cur_int)
    old_virtual_pos = camera_history[cur_history_index].cam_pos_from_cur_int
                      + ray_dir_old * sample_dist                // (- entity offset — omitted)
    dist_cur = distance(old_virtual_pos, vec3(0.0))
    if dist_cur < dist_min_max.x * (1022.0/1024.0)
       || dist_cur > dist_min_max.y * (1026.0/1024.0)
       || sample_dist > dist_min_max.y * 2.0:
        continue

    // 1-pixel screen-position reject — project old virtual pos into the CURRENT screen
    screen_projection_new = params.view_proj * vec4(old_virtual_pos, 1.0)   // M*v — see note
    ndc_new = screen_projection_new.xyz / screen_projection_new.w
    ndc_new.y *= -1.0
    ndc01_new = ndc_new.xy * 0.5 + 0.5
    screen_pos_new = ndc01_new * vec2(screen_width, screen_height)
    screen_pos_dif = screen_pos_new - vec2<f32>(pixel_pos)
    if dot(screen_pos_dif, screen_pos_dif) > 1.0: continue

    // rough-specular reweight (renderTaaSampleReverse.fx:138-148) — DEAD in A-2:
    // extra_data is always 0 (§3.2), so `if (extra_data != 0)` is never taken.
    // Port the `if` shell for fidelity but it folds away; pdf_vndf_isotropic is Phase B.

    // hash reject — must match the centre or one of the 8 neighbour hashes
    if hash != valid_hash_center:
        is_hash_valid = false
        for h in 0..8:
            is_hash_valid = is_hash_valid
                || hash == ((valid_hashes_comp[h/2] >> (16*(h%2))) & 0xFFFF)
        if !is_hash_valid: continue

    color_sum += color                          // color.a == 1 ⇒ color_sum.a counts accepted samples
```

**Implementer notes:**

- `% TAA_SAMPLE_RING_DEPTH` — the **second** `% 32` in the HLSL (`:91`) becomes `% 16`. The
  `% 128` at `:90` stays `% 128`.
- **`get_screen_index_projection`** — port `getScreenIndexProjection` +
  `getScreenPosProjection` (`commonRenderPipeline.fxh:133-152`) into `taa.wgsl`. It does
  `screen_projection = transformation * vec4(pos, 1.0)` (M*v — the column-vector convention, per
  the `05-review.md` perspective fix; the HLSL `mul(v, M)` against NAADF's row-major matrix is
  the column-vector `M*v` against a glam matrix — **A-2 must use `M*v` consistently**, same as
  `get_ray_dir` was fixed to). NDC reject: `ndc.x/y ∈ [-1,1]`, `ndc.z ∈ [0,1]`. Then
  `ndc.y *= -1`, `ndc01 = (ndc.xy + 1) * 0.5`, `screen_pos = ndc01 * vec2(w,h)`,
  `screen_pos_int = clamp(screen_pos + pixel_offset, vec2(0), vec2(w-1, h-1))`,
  `screen_index = screen_pos_int.x + screen_pos_int.y * w`. Returns `(valid, screen_index)`.
  **WGSL has no `out` params and no default args** — return a small struct
  `struct ScreenProj { valid: bool, screen_index: u32 }` and make `pixel_offset` an explicit
  arg.
- **The `view_proj` multiply** (`renderTaaSampleReverse.fx:127` `mul(float4(oldVirtualPos,1),
  camMatrix)`): in WGSL this is `params.view_proj * vec4(old_virtual_pos, 1.0)` — `M*v`. The
  comment in `taa.wgsl` must cite the `05-review.md` perspective-fix reasoning so a future reader
  does not "fix" it back to `v*M`.
- **`distance(old_virtual_pos, vec3(0.0))`** — WGSL `distance(a, b)` exists; `distance(p,
  vec3(0.0))` is `length(p)`. Either is fine; keep `distance(.., vec3(0.0))` to match the HLSL
  `distance(oldVirtualPos, 0)` line-for-line.
- The `ENTITIES` block at `:96-104` and the `entityPosChange` term — **omitted**; wherever the
  HLSL adds `entityPosChange` (which is `(0,0,0)` without entities), the A-2 port simply does not
  have the term. `curPosVirtual` is just `posVirtual`; `oldVirtualPos` has no `- entityPosChange`.
- The rough-specular `if (extraData != 0)` block (`:138-148`) — `extra_data` is always `0` in
  A-2 (§3.2). Port the `if (extra_data != 0u) { ... }` shell so the WGSL is structurally
  faithful, but its body (`pdf_vndf_isotropic`, the `reflect`, the `color *= fac`) references
  Phase-B functions. **Decision:** since the branch is provably dead in A-2, **do not port the
  body** — leave a one-line `// extra_data is always 0 in the albedo path (§3.2); the
  rough-specular reweight is Phase B` comment in place of the `if`. Porting a dead branch that
  pulls in `pdf_vndf_isotropic` (a Phase-B function) would force a premature Phase-B WGSL port —
  do not. Log this as a deliberate A-2 omission.

### 7.5 Phase 3 — accumulation into `taa_sample_accum` (`renderTaaSampleReverse.fx:163-171`)

```wgsl
let taa_color_comp = taa_sample_accum[global_id.x];               // the first-hit-written current sample
let sample_weight = unpack2x16float(taa_color_comp.x).x;           // f16(weight) in low 16 — the CURRENT count (1.0)
var taa_color = vec3<f32>(
    unpack2x16float(taa_color_comp.x).y,                           // R
    unpack2x16float(taa_color_comp.y).x,                           // G
    unpack2x16float(taa_color_comp.y).y);                          // B
taa_color = taa_color + color_sum.rgb;                             // add accumulated history colour

var new_color_comp = vec2<u32>(0u, 0u);
new_color_comp.x = pack2x16float(vec2<f32>(sample_weight + color_sum.a, taa_color.r));
new_color_comp.y = pack2x16float(vec2<f32>(taa_color.g, taa_color.b));
taa_sample_accum[global_id.x] = new_color_comp;
```

- **This is the load-bearing 0.25-spp signal.** `sample_weight` is the current frame's count
  (`1.0`, written by the first-hit pass — §6); `color_sum.a` is the count of accepted reprojected
  history samples (each accepted `color.a == 1`). The result `sample_weight + color_sum.a` is the
  **per-pixel accumulated sample count**, stored back into `taa_sample_accum[px].x & 0xFFFF` (as
  f16). `naadf_final.wgsl` divides RGB by `max(1, weight)` (`renderFinal.fx:39`) to get the
  averaged colour. Phase B's `rayQueueCalc.fx` reads this same `weight` to decide which pixels
  need GI rays (`02-research.md` §1.2.3 — `shouldRay` uses `taaSampleAccum` accum count). **A-2
  must keep this accumulation exactly — do not simplify it, do not drop `color_sum.a`.**
- **Read-modify-write hazard:** `taa_sample_accum` is read at `global_id.x` and written at
  `global_id.x` in the *same* dispatch — each thread only touches its own pixel, so there is no
  cross-thread hazard. But the *first-hit pass* writes `taa_sample_accum[px]` and the *reproject
  pass* reads-then-writes it — they are **separate dispatches with a render-graph edge between
  them** (§8), so wgpu's automatic buffer barriers serialise them. No manual barrier needed (same
  as Phase A's first-hit → blit edge).
- **`global_id.x` vs `pixel_index`:** the HLSL uses `globalID.x` for the `taaSampleAccum` index
  and `pixelPos = uint2(globalID.x % screenWidth, globalID.x / screenWidth)` for the 3×3 / ray
  dir. A-2's `taa.wgsl` does the same: `let pixel_index = global_id.x;` then `let pixel_pos =
  vec2<u32>(pixel_index % screen_width, pixel_index / screen_width);`. Guard
  `if (pixel_index >= screen_width * screen_height) { return; }` first (HLSL `:26-27`).

### 7.6 What the reproject pass does NOT do (A-2 vs the Phase-B `base/` version)

`02-research.md` §1.2.2's cross-check notes that the `base/renderTaaSampleReverse.fx` *also*
writes `taaDistMinMax` and has a `CalcNewTaaSample` pass. **A-2 ports the `albedo/` version** —
it has neither: no `taaDistMinMax` output buffer, no second `CalcNewTaaSample` pass. The albedo
reproject pass is a *single* compute pass (`ReprojectOld`). Do not add the `base/` extras.

---

## 8. The render-graph change — the TAA node + the final-blit swap (brief item 4)

### 8.1 The new node

A third `Core3d`-schedule node system **`naadf_taa_reproject_node`** (in `render/graph.rs` —
extends the existing `naadf_first_hit_node` / `naadf_final_blit_node` pattern, `graph.rs:44-142`).
It is a compute pass — same shape as `naadf_first_hit_node` (`graph.rs:44-78`):

- Resources: `Option<Res<TaaGpu>>`, `Option<Res<FrameGpu>>`, `Res<NaadfPipelines>`,
  `Res<PipelineCache>`. Skip silently if any is missing or the pipeline is not compiled (the
  Phase-A `let Some(...) else { return };` pattern).
- Dispatch: `ceil(pixel_count / 64)` workgroups (`FIRST_HIT_WORKGROUP_SIZE = 64` — reuse it; the
  HLSL is also `[numthreads(64,1,1)]` — `renderTaaSampleReverse.fx:22`).
- Binds `@group(0) = taa_reproject_bind_group` (§5.3 — the TAA pass's only bind group).
- Wrapped in `time_span("naadf_taa_reproject")` for the HUD (§11).

### 8.2 Graph edges — slot it between first-hit and final blit

Phase A's graph (`render/mod.rs:66-72`) is `(naadf_first_hit_node, naadf_final_blit_node).chain()`
in `Core3dSystems::PostProcess`, before `tonemapping`. A-2 inserts the TAA node in the middle:

```rust
.add_systems(
    Core3d,
    (naadf_first_hit_node, naadf_taa_reproject_node, naadf_final_blit_node)
        .chain()
        .in_set(Core3dSystems::PostProcess)
        .before(tonemapping),
)
```

This is NAADF's "TAA placed unusually early" (`02-research.md` §1.2.1) — the order is
`first_hit → TAA → final` (in Phase B, `rayQueueCalc`/GI/denoise go *after* TAA, before final).
The `.chain()` gives the render-graph edges; wgpu's automatic buffer barriers serialise the
shared-buffer accesses (`first_hit_data`, `taa_sample_accum`, `taa_samples`).

**When `AppArgs.taa` is off:** the TAA node still runs but is a no-op-equivalent — the reproject
pass with `is_taa` off... actually no: the reproject pass *always* reads `taa_sample_accum` and
writes it back. If `is_taa` is off, the first-hit pass never wrote `taa_samples`, so every
reprojected fetch reads zeroed/stale samples. **Decision:** gate the *node dispatch* on
`AppArgs.taa` — extract `AppArgs.taa` into a render-world resource (or read it off
`GpuTaaParams`/a flag) and have `naadf_taa_reproject_node` early-return when TAA is off. With TAA
off, `taa_sample_accum` is exactly Phase A's `shaded_color` (first-hit writes it, final blit
reads it) — the node skipping leaves it untouched, so the off-path is bit-identical to Phase A.
This keeps the `AppArgs.taa` toggle meaningful and the A-2 done-bar (TAA on) is the default.

### 8.3 The final-blit swap

`naadf_final_blit_node` (`graph.rs:91-142`) is **structurally unchanged** — it still binds
`frame_gpu.blit_bind_group` and draws the fullscreen triangle. The only change is upstream: the
`blit_bind_group` (built in `prepare_frame_gpu`) now binds `taa_sample_accum` (from `TaaGpu`)
instead of the local `shaded_color` at binding 1 (§5.4, §5.5). `naadf_final.wgsl` is unchanged
except the binding *name* `shaded_color` → `taa_sample_accum` (§5.4) — its tonemap math already
reads the `taaSampleAccum` element format (`03-design.md` §5.3 — the Phase-A stand-in was built
to this format precisely so this is a zero-logic-change swap).

### 8.4 Pipelines

`NaadfPipelines` (`pipelines.rs:64-87`) gains:
- `taa_layout: BindGroupLayoutDescriptor` — `@group(2)` for the first-hit pass (§5.2).
- `taa_reproject_layout: BindGroupLayoutDescriptor` — the TAA node's group (§5.3).
- `taa_reproject_pipeline: CachedComputePipelineId` — queued from `taa.wgsl`, entry point
  `reproject_old_samples`, layout `[taa_reproject_layout]`.
- `first_hit_pipeline`'s layout extended to `[world_layout, frame_layout, taa_layout]` (§6.2).

`FIRST_HIT_SHADER` / `FINAL_BLIT_SHADER` consts gain a `TAA_REPROJECT_SHADER =
"shaders/taa.wgsl"`. All built in `NaadfPipelines::from_world` (`pipelines.rs:89-188`) — the same
`FromWorld` / `RenderStartup` path. The blit pipeline's per-format caching (`pipelines.rs:190-235`)
is **untouched** — the TAA node is a compute pass, format-agnostic.

---

## 9. Extract / prepare changes (brief item 5)

### 9.1 The monotonic frame counter (the carried `05-review.md` §4 fix)

NAADF's `frameCount` is incremented once per rendered frame (`WorldRender.cs:86`, inside the
`P`-key guard — in the port there is no such guard, just increment every frame). `taaIndex`
derives from it (`WorldRender.cs:88`).

**A-2 adds a main-world frame counter.** Cleanest: it lives on the `CameraHistory` resource
(§2.3 — `frame_count: u32`), incremented by the per-frame camera-history update system (§9.3).
Then:
- `extract_camera` (`extract.rs:104-130`) is extended (or a new `extract_camera_history` system
  is added) to copy `CameraHistory` (the rings + `frame_count`) into a render-world
  `ExtractedCameraHistory` resource.
- `prepare_frame_gpu` (`prepare.rs:275-280`) sets `GpuRenderParams.frame_count` =
  `extracted.frame_count` (the real counter), `rand_counter` = `extracted.frame_count` (the RNG
  salt — §4.1), `taa_index` = the derived index. **Replaces** the `time.elapsed()` lines
  (`prepare.rs:278-279`) — the `Res<Time>` param on `prepare_frame_gpu` is removed (it was only
  used for the bogus counters; verify nothing else uses it — grep confirms `time` is used only at
  `:248,278,279`).

**Why main-world, not render-world:** the camera-history ring must be updated in the main world
(it is logically part of camera state, and the §9.3 update needs the camera `Transform`), and
the counter must be consistent between "which ring slot we write this frame" and "what
`frame_count` the shader sees". One counter, owned by `CameraHistory`, incremented once per
frame in the main-world update system, extracted into the render world.

### 9.2 `prepare_taa` — the new prepare system (`PrepareResources`)

A new system in `render/prepare.rs` (or `render/taa.rs` — §10), running in
`RenderSystems::PrepareResources` (alongside `prepare_world_gpu`):

1. **Create `TaaGpu` once** (the render-world resource owning the TAA buffers — §9.4). On the
   first frame with a valid extracted camera: create `taa_samples` (`pixel_count * 16` ×
   `vec2<u32>`), `taa_sample_accum` (`pixel_count` × `vec2<u32>`), `camera_history`
   (`128` × `GpuCameraHistorySlot`), `taa_params` (`GpuTaaParams` uniform). Zero-clear
   `taa_samples` + `taa_sample_accum` on creation (the `encoder.clear_buffer` pattern from
   `prepare.rs:355-362`).
2. **Resize on viewport change** — if `pixel_count` changed (track it on `TaaGpu` like
   `FrameGpu.pixel_count` — `prepare.rs:71,306-307`), re-create `taa_samples` +
   `taa_sample_accum` (zero-cleared). `camera_history` + `taa_params` are *not* resized (fixed
   size). **Resize note:** on resize the entire `taa_samples` ring is discarded — the next ~16
   frames rebuild it from zeroed (rejected) history. That is correct and unavoidable (the ring is
   screen-space); it produces a brief ~16-frame re-converge after a resize, which is acceptable
   (NAADF does the same — `CreateScreenTextures` reallocates `taaSamples`,
   `WorldRenderAlbedo.cs:50-58`).
3. **Upload `camera_history` every frame.** Build the `[128]` `GpuCameraHistorySlot` array from
   `ExtractedCameraHistory`:
   - `slot[i].view_proj = extracted.view_proj[i]`
   - `slot[i].jitter = extracted.jitter[i]`
   - `slot[i].cam_pos_from_cur_int = (extracted.positions[i] - current_camera_position_split).to_world()`
     — the `PositionSplit` subtraction then `.to_world()`, i.e. NAADF's
     `(oldCamPositions[i] - camPos).toVector3()` (`WorldRenderAlbedo.cs:83`). `current_camera_position_split`
     is `extracted_camera.position_split` (the Phase-A `ExtractedCameraData` — `extract.rs:50,126`).
   `queue.write_buffer` the whole array.
4. **Upload `taa_params` every frame.** Build `GpuTaaParams` (§4.2):
   - `inv_view_proj` = `extracted_camera.inv_view_proj` (the rotation-only inverse — already
     built by `extract_camera`, `extract.rs:119`).
   - `view_proj` = the **non-inverted** rotation-only view-proj. **`extract_camera` currently only
     keeps the inverse** (`extract.rs:117-119` computes `clip_from_view_rot` then immediately
     `.inverse()`s it; only `inv_view_proj` is stored on `ExtractedCameraData`). **A-2 must add
     `view_proj` (the pre-inverse `clip_from_view_rot`) to `ExtractedCameraData`** so
     `prepare_taa` has it. This is a one-field addition to `extract.rs:50-64` + storing
     `clip_from_view_rot` before the `.inverse()` at `:118-119`. (Alternative: re-invert
     `inv_view_proj` in `prepare_taa` — but storing it directly avoids a redundant inverse and a
     potential precision wobble. Store it.)
   - `cam_pos_int` / `cam_pos_frac` = `extracted_camera.position_split.pos_int` / `.pos_frac`.
   - `screen_width` / `screen_height` = the viewport.
   - `frame_count` = `extracted.frame_count`.
   - `taa_index` = the derived index (one shared helper fn — §2.3).
   - `sample_age` = `16` (clamped to `[1, TAA_SAMPLE_RING_DEPTH]` — §7.1).
5. **Build `taa_first_hit_bind_group`** (the `@group(2)` for the first-hit pass — just
   `taa_samples`). Built here since `TaaGpu` owns `taa_samples`. Rebuild only when `taa_samples`
   is (re-)created.

The `taa_reproject_bind_group` is built in `prepare_frame_gpu` (it mixes `TaaGpu` +
`FrameGpu.first_hit_data` — §5.5).

### 9.3 The camera-history-ring update + jitter (main world, `Update`)

A new main-world `Update` system **`update_camera_history`** (in `src/camera/` or `src/render/`
— §10) — it must run **after** `sync_position_split` (which updates the camera's `PositionSplit`
from the `Transform` — `position_split.rs:102-107`), so the ring stores the *current* frame's
camera state:

```
fn update_camera_history(
    camera: Single<&PositionSplit, With<FreeCamera>>,
    mut history: ResMut<CameraHistory>,
    // + whatever is needed to compute the view-proj — see note
) {
    let taa_index = taa_index_of(history.frame_count);     // shared helper (§2.3)
    history.positions[taa_index] = *camera;
    history.view_proj[taa_index] = /* rotation-only view-proj of THIS frame */;
    history.jitter[taa_index]    = /* this frame's Halton jitter, or ZERO if jitter disabled */;
    history.frame_count = history.frame_count.wrapping_add(1);
}
```

- **`taa_index` write-then-increment:** NAADF writes `oldCamPositions[taaIndex] = camPos` *with
  the current `taaIndex`* (`WorldRenderAlbedo.cs:77-80`), and `taaIndex` is computed from
  `frameCount` *before* the frame (`WorldRender.cs:88`, in `Update`, before `Render`). So: in
  `update_camera_history`, compute `taa_index` from the *current* `frame_count`, write the rings
  at that slot, *then* increment `frame_count`. The render-side `prepare_taa` must use the **same
  `taa_index`** (from the *same, pre-increment* `frame_count`) — which is why `frame_count` is
  extracted and `taa_index` is re-derived render-side with the identical helper. **Ordering
  subtlety:** `update_camera_history` increments `frame_count` in `Update`; `extract` runs after
  `Update`; so the extracted `frame_count` is *already incremented*. The render side must
  therefore derive `taa_index` from `extracted.frame_count - 1` (the value the ring was written
  with) — OR `update_camera_history` increments *first* then writes... no. **Cleanest:** do not
  increment in `update_camera_history`. Instead: `update_camera_history` reads `frame_count`,
  derives `taa_index`, writes the rings. A *separate* tiny system (or the tail of
  `update_camera_history`) increments `frame_count` — and the render side derives `taa_index`
  from the **same `frame_count` value the rings were written with**. To make this
  unambiguous: **store `taa_index` itself on `CameraHistory`** (computed once in
  `update_camera_history`, before the increment) and extract *that*, rather than re-deriving it
  render-side. This removes the off-by-one trap entirely. Revised `CameraHistory`: add
  `pub taa_index: u32` — set in `update_camera_history`, extracted alongside `frame_count`, used
  directly by `prepare_taa` and written into `GpuTaaParams.taa_index` + `GpuRenderParams.taa_index`.
  **This is a small but load-bearing design call — flag it for the implementer: compute
  `taa_index` exactly once per frame, in `update_camera_history`, store it, and never re-derive
  it.**
- **The view-proj for the ring:** the ring stores the *rotation-only* view-proj
  (`Camera.cs:201` — built from `CreateLookAt(Vector3.Zero, ...)`). The main-world
  `update_camera_history` does not have easy access to the same matrix `extract_camera` builds
  (that runs in `ExtractSchedule` with `Camera` + `GlobalTransform`). **Two options:**
  (a) `update_camera_history` also queries `Camera` + `GlobalTransform` and rebuilds the
  rotation-only view-proj with the *same* formula as `extract_camera` (`clip_from_view *
  Mat4::from_quat(rotation).inverse()`); (b) move the camera-history ring update into the
  *render world* extract step. **Recommendation: (a)** — `update_camera_history` queries
  `(&Camera, &GlobalTransform, &PositionSplit, With<FreeCamera>)` and builds the matrix with a
  **shared helper fn** `rotation_only_view_proj(camera, global_transform) -> Mat4` that both
  `update_camera_history` and `extract_camera` call (extract the formula at `extract.rs:117-118`
  into this helper, in a `camera/` or `render/` module). One formula, one place — no drift.
  Keeping the ring update in the main world also keeps `frame_count` / `taa_index` ownership
  consistent with the main-world `CameraHistory` resource.
- **Jitter (`TemporalJitter`):** NAADF's jitter is `getJitter(frameCount)` =
  `Halton2D((frame % 32) + 1, (3,7)) - 0.5` (`WorldRender.cs:137-140,113-135`), gated on the
  `isTAAJitter` setting (`WorldRenderAlbedo.cs:73`). A-2 ports this as a plain function
  `halton_jitter(frame: u32) -> Vec2` in `src/render/taa.rs` (or `src/camera/`): the 2D Halton
  (base 3, base 7) of `(frame % 32) + 1`, minus `0.5`. **Do NOT use Bevy's `TemporalJitter`
  component** — that is DLSS-coupled (`camera/mod.rs:28` — `TemporalJitter` is part of the
  `DlssRrComponents` tuple) and DLSS is dormant in A-2. The jitter is NAADF's own Halton,
  computed in `update_camera_history`, written into `CameraHistory.jitter[taa_index]`, and *also*
  passed to the first-hit pass via `GpuRenderParams.taa_jitter` (§4.1) so the primary ray is
  jittered. `get_ray_dir` already takes a `pixel_offset` arg (`render_pipeline_common.wgsl:145-157`,
  `naadf_first_hit.wgsl:53-59`) — Phase A passes `params.taa_jitter` (currently always zero).
  A-2 just makes `taa_jitter` non-zero. **Gating:** add an `AppArgs` field or reuse `AppArgs.taa`
  — simplest is: jitter on iff `AppArgs.taa` is on (NAADF has a separate `isTAAJitter` toggle,
  but A-2 does not need the extra knob — TAA-on implies jitter-on is a fine A-2 default; note it
  as a deliberate simplification). When `AppArgs.taa` is off, `taa_jitter = ZERO` and
  `CameraHistory.jitter[*] = ZERO`.
  - **Jitter consistency:** the first-hit pass jitters the primary ray with
    `taa_jitter`; the reproject pass's `get_screen_index_projection` un-jitters with
    `-camera_history[slot].jitter` (`renderTaaSampleReverse.fx:107,109` — `-curTaaJitter`). So
    the jitter written into `CameraHistory.jitter[taa_index]` *this* frame **must equal** the
    `GpuRenderParams.taa_jitter` used by *this* frame's first-hit pass. Both derive from
    `halton_jitter(frame_count)` with the *same* `frame_count` — keep that single source of
    truth. (`prepare_frame_gpu` computes `GpuRenderParams.taa_jitter = halton_jitter(extracted.frame_count)`
    — but `update_camera_history` already computed it for the ring. To guarantee they match,
    **store the frame's jitter on `CameraHistory` too** — or just trust that `halton_jitter` is
    pure and both call it with the extracted `frame_count`. The pure-function-called-twice
    approach is fine *if* the `frame_count` is identical; since `taa_index` is being stored on
    `CameraHistory` anyway to avoid the off-by-one, store the frame's `jitter` there too as
    `current_jitter: Vec2` and have `prepare_frame_gpu` read it from the extracted history.
    One value, computed once.)

### 9.4 `TaaGpu` render-world resource + buffer ownership

```rust
// src/render/taa.rs  (or render/prepare.rs)
#[derive(Resource)]
pub struct TaaGpu {
    /// The 16-deep sample ring — pixel_count * 16 × vec2<u32>.
    pub taa_samples: Buffer,
    /// The per-pixel accumulated colour + count — pixel_count × vec2<u32>.
    /// This is the real `taaSampleAccum`; replaces Phase A's `shaded_color`.
    pub taa_sample_accum: Buffer,
    /// The 128-deep camera-history ring — 128 × GpuCameraHistorySlot, fixed size.
    pub camera_history: Buffer,
    /// The TAA reproject pass's scalar uniform.
    pub taa_params: Buffer,
    /// Pixel count the screen-space buffers are sized for (resize trigger).
    pub pixel_count: u32,
    /// @group(2) for the first-hit pass — just `taa_samples`.
    pub taa_first_hit_bind_group: BindGroup,
}
```

- **`taa_sample_accum` moves out of `FrameGpu` into `TaaGpu`.** Phase A's `FrameGpu.shaded_color`
  (`prepare.rs:69`) is *deleted*; `TaaGpu.taa_sample_accum` replaces it. `prepare_frame_gpu` no
  longer creates `shaded_color` (`prepare.rs:306-329` — the `shaded_color` half of the storage-
  buffer creation is removed; `first_hit_data` creation stays). `prepare_frame_gpu`'s bind-group
  builds (`prepare.rs:365-389`) reference `taa_gpu.taa_sample_accum` instead of the local
  `shaded_color`. **`prepare_frame_gpu` therefore gains `Res<TaaGpu>` as a param** (it must run
  after `prepare_taa` — both already in the right order if `prepare_taa` is `PrepareResources`
  and `prepare_frame_gpu` is `PrepareBindGroups`).
- **Resize coherence:** `first_hit_data` (in `FrameGpu`) and `taa_samples` + `taa_sample_accum`
  (in `TaaGpu`) all resize on the *same* viewport-change trigger. `prepare_taa`
  (`PrepareResources`) and `prepare_frame_gpu` (`PrepareBindGroups`) each independently check
  `pixel_count` against their own stored value. They will agree because they read the same
  `extracted_camera.viewport_size`. The bind groups that mix them (`taa_reproject_bind_group`,
  the frame `bind_group`) are all rebuilt in `prepare_frame_gpu` *after* both resizes have
  happened — correct.

### 9.5 `AppArgs.taa` default flip

`main.rs:51` sets `taa: false` (Phase A default). A-2's done-bar is "temporally stable, TAA on"
(`01-context.md` §2c). **Flip the default to `taa: true`.** Keep the field (it stays a wired
runtime toggle — useful for A/B comparison and for the off-path being bit-identical to Phase A,
§8.2). The `GridPreset` enum and the rest of `AppArgs` are untouched.

---

## 10. `src/` module layout deltas (brief item 6)

A-2 is an **extension**, not a restructure — no Phase-A module is reorganised. New files and
changed files:

### 10.1 New files

```
src/render/
  taa.rs              NEW  TaaGpu resource; CameraHistory resource; prepare_taa system;
                           the camera-history-ring update system (update_camera_history);
                           halton_jitter(); taa_index_of(); the rotation_only_view_proj()
                           shared helper; TAA_SAMPLE_RING_DEPTH / CAMERA_HISTORY_DEPTH consts.
src/assets/shaders/
  taa.wgsl            NEW  the TAA reproject compute pass — port of
                           albedo/renderTaaSampleReverse.fx `reprojectOldSamples`;
                           get_hit_data_from_planes_a2 (single-plane reduction);
                           get_screen_index_projection / get_screen_pos_projection;
                           the GpuTaaParams + GpuCameraHistorySlot struct decls.
  taa_common.wgsl     NEW  port of common/taa/commonTaa.fxh — taa_compress_sample /
                           taa_decompress_sample (the 64-bit sample format, §3),
                           taa_hash_from_data, taa_neighbor_offsets[9],
                           the TAA_SAMPLE_RING_DEPTH const.
```

**`color_compression.wgsl` is NOT created in A-2.** The brief lists it as a *possible* new file,
but the verified source shows the TAA sample's exponential colour compression lives *inside*
`commonTaa.fxh` (`compressSample` / `decompressSample` — the 8-bit/channel exponential, §3.2),
**not** in `commonColorCompression.fxh`. `commonColorCompression.fxh` (read directly) is the
**5-bit/channel** ReSTIR-GI sample compression (`COLORS[32]`, `COLOR_DIF_PROB[31]`,
`compressColor`, `refineCompColor`) — that is **Phase B** (`02-research.md` §5.1 explicitly tags
`commonColorCompression.fxh` Phase B). So A-2's colour compression goes in `taa_common.wgsl`
(from `commonTaa.fxh`); `color_compression.wgsl` from `commonColorCompression.fxh` is a Phase-B
file. **Flag this to the orchestrator** — the brief's file list anticipated `color_compression.wgsl`;
the verified source says A-2 does not need it.

### 10.2 Changed files

| file | change |
|---|---|
| `src/render/gpu_types.rs` | add `GpuTaaParams` (§4.2), `GpuCameraHistorySlot` (§4.3) + their size asserts; `GpuRenderParams` *values* change but not the struct (§4.1) — no edit to the struct itself. |
| `src/render/extract.rs` | add `view_proj` (rotation-only, non-inverted) to `ExtractedCameraData` (§9.2); add `ExtractedCameraHistory` resource + `extract_camera_history` system (copies `CameraHistory` rings + `frame_count` + `taa_index` + `current_jitter`). |
| `src/render/prepare.rs` | `prepare_taa` system (§9.2) — *may* live in `render/taa.rs` instead, cleaner; `prepare_frame_gpu` loses `Res<Time>`, gains `Res<TaaGpu>`, sets the real `frame_count`/`rand_counter`/`taa_index`/`taa_jitter` (§4.1, §9.1), binds `taa_sample_accum` instead of `shaded_color`, builds `taa_reproject_bind_group`; `FrameGpu.shaded_color` field deleted. |
| `src/render/pipelines.rs` | add `taa_layout`, `taa_reproject_layout`, `taa_reproject_pipeline`; extend `first_hit_pipeline` layout to 3 groups; add `TAA_REPROJECT_SHADER` const (§8.4). |
| `src/render/graph.rs` | add `naadf_taa_reproject_node` + `TAA_REPROJECT_SPAN` const; `naadf_first_hit_node` binds `@group(2)` (§6.2). `naadf_final_blit_node` unchanged. |
| `src/render/mod.rs` | declare `pub mod taa;`; register `CameraHistory` (main world — actually inserted by a startup system or `WorldPlugin`); init `ExtractedCameraHistory`; add `extract_camera_history` to `ExtractSchedule`; add `prepare_taa` to `PrepareResources`; insert `naadf_taa_reproject_node` into the `Core3d` chain (§8.2). |
| `src/main.rs` | `AppArgs.taa` default `false` → `true` (§9.5); add `update_camera_history` to the `Update` schedule, after `sync_position_split`; insert/seed the `CameraHistory` resource (a `Startup` system, or `Default` + `WorldPlugin` registration). |
| `src/hud.rs` | add the `naadf_taa_reproject` timing line (§11). |

`src/world/`, `src/aadf/`, `src/voxel/`, `src/camera/position_split.rs` are **untouched**.
(`src/camera/mod.rs` is untouched unless `update_camera_history` is placed there instead of
`render/taa.rs` — designer's call; `render/taa.rs` is recommended to keep camera-history with the
TAA code it serves.)

### 10.3 WGSL `#import` graph

`taa.wgsl` imports: `taa_common.wgsl` (sample (de)compress, neighbour offsets, ring depth),
`render_pipeline_common.wgsl` (`get_ray_dir`, `NORMAL`, `HIT_UNDEFINED`, `ENTITY_FREE`),
`world_data.wgsl` is **not** imported (the reproject pass binds no `@group(0)` world data — §5.3).
`naadf_first_hit.wgsl` gains an import of `taa_common.wgsl` (`taa_compress_sample`,
`TAA_SAMPLE_RING_DEPTH`). `naadf_final.wgsl` imports are unchanged.

---

## 11. HUD (brief item 7)

Phase A established the pattern: each render node wraps its work in `time_span("<span>")`;
`RenderDiagnosticsPlugin` surfaces it at `render/<span>/elapsed_gpu` (and `.../elapsed_cpu`
fallback); `hud.rs` has a `const`-checked path pair per node and calls `write_timing`
(`hud.rs:19-58,126-176`).

A-2 adds **one** timing line for the new TAA node:

- `render/graph.rs`: `pub const TAA_REPROJECT_SPAN: &str = "naadf_taa_reproject";`, and
  `naadf_taa_reproject_node` wraps its compute pass in `diagnostics.time_span(encoder,
  TAA_REPROJECT_SPAN)` exactly as `naadf_first_hit_node` does (`graph.rs:63-77`).
- `hud.rs`: add `TAA_REPROJECT_GPU_PATH = "render/naadf_taa_reproject/elapsed_gpu"` +
  `TAA_REPROJECT_CPU_PATH = "render/naadf_taa_reproject/elapsed_cpu"`; add the
  `const _: () = assert!(matches_span(TAA_REPROJECT_GPU_PATH, TAA_REPROJECT_SPAN));` line
  (`hud.rs:30-33`); add a `write_timing(s, &diagnostics, "taa-reproject",
  TAA_REPROJECT_GPU_PATH, TAA_REPROJECT_CPU_PATH)` call in the `"NAADF passes:"` block
  (`hud.rs:131-145`) — between `first-hit` and `final-blit` (matching the render order).
- Optionally update the renderer-mode string (`hud.rs:104-107`) to mention "+ 16-frame TAA" —
  cosmetic, low priority.

`write_timing` itself (`hud.rs:156-176`) is **unchanged** — it is generic.

---

## 12. Numbered Phase-A-2 implementation sequence (brief item 8)

Each step ends at a **compiling** state; runnable states are marked **▶**. Mirrors
`03-design.md` §8's shape. The `impl` group executes these in order.

1. **GPU types + WGSL sample format.** `gpu_types.rs`: add `GpuTaaParams` (§4.2),
   `GpuCameraHistorySlot` (§4.3) + size asserts. `assets/shaders/taa_common.wgsl` (NEW): port
   `commonTaa.fxh` — `taa_compress_sample` / `taa_decompress_sample` (§3), `taa_hash_from_data`,
   `taa_neighbor_offsets[9]`, `TAA_SAMPLE_RING_DEPTH = 16u`. **Compiles** (WGSL is asset data;
   validated at pipeline-compile time later). Unit-test the Rust struct sizes (the asserts do
   this at compile time already).

2. **`CameraHistory` + frame counter + jitter + the shared camera helpers.** `render/taa.rs`
   (NEW): the `CameraHistory` resource (§2.3, with `frame_count` + `taa_index` +
   `current_jitter`), `CAMERA_HISTORY_DEPTH`/`TAA_SAMPLE_RING_DEPTH` consts, `taa_index_of()`,
   `halton_jitter()` (§9.3 — port `WorldRender.cs` Halton), `rotation_only_view_proj()` (the
   shared helper extracted from `extract.rs:117-118`). `update_camera_history` system (§9.3).
   `main.rs`: seed/insert `CameraHistory`, add `update_camera_history` to `Update` after
   `sync_position_split`. `extract.rs`: refactor `extract_camera` to use
   `rotation_only_view_proj()`. **▶ Compiles & runs** — Phase A still renders (TAA not wired
   yet); `update_camera_history` populates the ring (verify with a debug log of `frame_count` /
   `taa_index`).

3. **Extract the camera history + the non-inverted view-proj.** `extract.rs`: add `view_proj` to
   `ExtractedCameraData` (store `clip_from_view_rot` before the `.inverse()`); add
   `ExtractedCameraHistory` resource + `extract_camera_history` system (copies the rings +
   `frame_count` + `taa_index` + `current_jitter`). `render/mod.rs`: init
   `ExtractedCameraHistory`, add `extract_camera_history` to `ExtractSchedule`. **Compiles** —
   extract populates the render-world mirror (verify via a debug log or render-doc).

4. **Fix `frame_count` / `rand_counter` / `taa_index` / `taa_jitter` in `prepare_frame_gpu`.**
   `prepare.rs`: drop `Res<Time>`; set `GpuRenderParams.frame_count` / `rand_counter` /
   `taa_index` from `ExtractedCameraHistory`; set `taa_jitter` from the extracted
   `current_jitter` (still zero unless `AppArgs.taa` — but the plumbing is in). This is the
   carried `05-review.md` §4 fix landing on its own. **▶ Compiles & runs** — Phase A renders
   identically (jitter still zero, TAA node not added), but the counters are now real (verify
   the HUD or a debug log shows a monotonic `frame_count`).

5. **`TaaGpu` + `prepare_taa` + buffer creation.** `render/taa.rs`: `TaaGpu` resource (§9.4),
   `prepare_taa` system (§9.2 — create `taa_samples` 16-ring + `taa_sample_accum` +
   `camera_history` + `taa_params`, zero-clear, resize handling, upload `camera_history` +
   `taa_params` each frame, build `taa_first_hit_bind_group`). `render/mod.rs`: add `prepare_taa`
   to `PrepareResources`. **Move `taa_sample_accum` ownership:** delete `FrameGpu.shaded_color`,
   have `prepare_frame_gpu` take `Res<TaaGpu>` and bind `taa_gpu.taa_sample_accum` where it bound
   `shaded_color` (frame `bind_group` slot 3, `blit_bind_group` slot 1); rename the bindings.
   `pipelines.rs`: rename the `frame_layout` / `blit_layout` binding comments
   `shaded_color`→`taa_sample_accum` (the layout *shape* is unchanged). `naadf_first_hit.wgsl` +
   `naadf_final.wgsl`: rename the `shaded_color` binding to `taa_sample_accum` (no logic change).
   **▶ Compiles & runs — the blit-source swap landed:** the app renders exactly as Phase A did,
   but now reading the real `taa_sample_accum` buffer (which the first-hit pass writes and the
   blit reads — TAA node still absent, so it behaves identically to Phase A's `shaded_color`).
   This is the designed-in drop-in swap proven before any TAA logic is added.

6. **First-hit `taaSamples` write.** `pipelines.rs`: add `taa_layout` (`@group(2)`), extend
   `first_hit_pipeline` layout to 3 groups. `naadf_first_hit.wgsl`: add the `@group(2)`
   `taa_samples` binding, import `taa_compress_sample` + `TAA_SAMPLE_RING_DEPTH`, add the
   `if ((flags & FLAG_IS_TAA) != 0)` ring write (§6.1). `graph.rs`: `naadf_first_hit_node` binds
   `@group(2)` = `taa_gpu.taa_first_hit_bind_group`. **▶ Compiles & runs** — with `AppArgs.taa`
   still `false` the write is skipped; flip it on temporarily and verify (render-doc / readback)
   that `taa_samples` slot `taa_index % 16` fills. Renders identically (no reproject node yet —
   the ring is written but unused).

7. **The TAA reproject WGSL.** `assets/shaders/taa.wgsl` (NEW): port
   `renderTaaSampleReverse.fx` `reprojectOldSamples` → `reproject_old_samples` (§7):
   `get_hit_data_from_planes_a2` (§7.3), `get_screen_pos_projection` /
   `get_screen_index_projection` (§7.4), the 3×3 precompute (§7.2), the reprojection loop (§7.4),
   the accumulation (§7.5); the `GpuTaaParams` + `GpuCameraHistorySlot` struct decls (§4.4).
   Entity blocks omitted, rough-specular branch left as a dead-`if` comment (§7.4), `% 16` not
   `% 32`. Does not run yet — compiles as a shader asset. **Compiles** (validated at pipeline
   compile in step 8).

8. **The TAA node + pipeline + graph wiring.** `pipelines.rs`: add `taa_reproject_layout` (§5.3),
   `taa_reproject_pipeline` (entry `reproject_old_samples`), `TAA_REPROJECT_SHADER` const.
   `prepare.rs`: build `taa_reproject_bind_group` in `prepare_frame_gpu` (mixes `TaaGpu` +
   `FrameGpu.first_hit_data` — §5.5). `graph.rs`: `naadf_taa_reproject_node` (§8.1) +
   `TAA_REPROJECT_SPAN`; gate its dispatch on `AppArgs.taa` (extract the flag — §8.2).
   `render/mod.rs`: insert `naadf_taa_reproject_node` into the `Core3d` `.chain()` between
   first-hit and final-blit (§8.2). **▶ Compiles & runs — THE PHASE-A-2 DELIVERABLE:** with
   `AppArgs.taa` on (step 9 makes it the default), the full three-node graph runs — first-hit
   writes the ring + accum, TAA reproject accumulates 16 frames of history into `taa_sample_accum`,
   final blit shows it. With `AppArgs.taa` off, the TAA node early-returns and the result is
   bit-identical to Phase A. Smoke-run: no WGSL / pipeline-validation errors, no panics.

9. **Default-on + HUD + polish.** `main.rs`: flip `AppArgs.taa` default to `true` (§9.5).
   `hud.rs`: add the `naadf_taa_reproject` timing line + its `const`-checked path pair (§11);
   optionally update the renderer-mode string. `README.md`: mark Phase A-2 complete in the
   roadmap. **▶ Compiles & runs** — HUD shows three NAADF pass timings; TAA is on by default.
   `cargo test --bin bevy-naadf` green (no test regressions; the step-1 size asserts are
   compile-time).

**Phase-A-2 review gate:** after step 9, Phase A-2 is reviewed — build + the existing test suite
green, and the **user interactive re-test confirms temporal stability** (jitter-AA'd, no
per-frame shimmer — the A-2 done-bar, `01-context.md` §2c). The per-pixel sample-count signal in
`taa_sample_accum.x` is verified intact and exposed (a readback or debug-view check) so Phase B's
adaptive 0.25-spp sampler has its input.

---

## 13. Open items the orchestrator should surface before A-2 implementation begins (brief deliverable)

These are not blockers — the design above resolves each — but the orchestrator should confirm
them with the user, as they are small scope/behaviour calls:

1. **`color_compression.wgsl` is NOT an A-2 file.** The brief's file list anticipated a
   `color_compression.wgsl` from `commonColorCompression.fxh`. Verified against source: the
   TAA sample's exponential colour compression lives in `commonTaa.fxh` (`compressSample` —
   8-bit/channel), which A-2 ports into `taa_common.wgsl`. `commonColorCompression.fxh` is the
   *5-bit/channel ReSTIR-GI* compression, tagged **Phase B** by `02-research.md` §5.1. A-2 does
   not create `color_compression.wgsl`. (§10.1)

2. **`taa_jitter` is gated on `AppArgs.taa`, not a separate `isTAAJitter` knob.** NAADF has a
   distinct ImGui `isTAAJitter` toggle (`WorldRenderAlbedo.cs:21,73`). A-2 simplifies: TAA-on
   implies jitter-on (one knob — `AppArgs.taa`). If the user wants the jitter independently
   toggleable, that is a trivial extra `AppArgs` field — flag it. (§9.3)

3. **`rand_counter` is the monotonic frame counter, not a `randValues[32]` table index.** NAADF
   refills a 32-entry random table per frame and indexes it (`WorldRender.cs:82-86`). A-2 uses
   the frame counter directly as the RNG salt — load-bearing property (per-frame-varying salt)
   preserved, the table is gold-plating omitted. (§4.1)

4. **`sample_age` is a fixed `16` in A-2** (NAADF exposes it as a 1–32 ImGui slider). A-2 has no
   GUI; the design fixes it at the full ring depth. If a runtime knob is wanted later it is an
   `AppArgs` field — flag it. (§7.1)

5. **The rough-specular reweight branch in `renderTaaSampleReverse.fx:138-148` is ported as a
   dead-`if` comment, not a working branch.** `extraData` is provably `0` in the albedo path, so
   the branch never executes; porting its body would force a premature Phase-B WGSL port of
   `pdf_vndf_isotropic`. A-2 leaves a structural comment in its place. (§7.4)

6. **`update_camera_history` computes `taa_index` exactly once per frame and stores it on
   `CameraHistory`** (rather than re-deriving it render-side), to eliminate an off-by-one trap
   around the `frame_count` increment / `ExtractSchedule` boundary. This is the one subtle
   ordering call in the design — the implementer must follow §9.3 exactly. (§9.3)
