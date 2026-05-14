# 08 — Phase A-2 Review

## review findings — Phase A-2 (2026-05-14)

Static verification of the committed Phase-A-2 Batch 2 code (commit `8abd2ec`
"feat: implement Phase A-2 TAA reproject node and first-hit ring write") against
`06-design-a2.md`, the NAADF HLSL reference, and the `05-review.md` perspective
fix. The Batch 2 implementing agent was interrupted before writing its log; this
review reconstructs and verifies what it did. **Verification is by reading the
code** — the only runtime check is the single post-`hud.rs`-edit smoke-run logged
in `07-impl-a2.md`.

Files verified: `src/assets/shaders/taa.wgsl`, `naadf_first_hit.wgsl`,
`taa_common.wgsl`; `src/render/{taa,graph,prepare,pipelines,extract,gpu_types,
mod}.rs`; `src/main.rs`; `src/hud.rs`. NAADF HLSL read for the comparison:
`Content/shaders/render/versions/albedo/{renderTaaSampleReverse,renderFirstHit}.fx`,
`common/taa/commonTaa.fxh`, `common/commonRenderPipeline.fxh`.

---

### Verdict up front

**Phase A-2 has ONE blocking issue: leftover TEMP STEP-8 instrumentation is
still committed and active** (it was never reverted before commit `8abd2ec`).
It writes garbage into one pixel of `taa_sample_accum` every frame, spams
per-frame `info!` logs, and adds a dead readback node + a `COPY_SRC` usage flag.
**The TAA logic itself — the part the user actually asked about — is correct and
faithfully ported.** The blocking issue is incomplete-cleanup, not a design bug;
it must be reverted before Phase A-2 closes, but it is mechanical (delete the
clearly-fenced blocks) and is a separate scoped task, not part of this review's
implementation surface.

- **0.25-spp readiness: READY.** The per-pixel accumulated sample-count signal
  is genuinely maintained, incremented correctly over frames, and exposed in
  `taa_sample_accum.x` in exactly the form a Phase-B GI sampler reads. (§1)
- **Faithful port: YES** (modulo the instrumentation). `taa.wgsl` faithfully
  ports `renderTaaSampleReverse.fx`; the first-hit ring write faithfully ports
  the `if(isTAA)` path; the ring is 16-deep everywhere (`% 16`, not `% 32`). (§2)
- **Matrix convention: CORRECT.** Every matrix multiply in `taa.wgsl` uses the
  glam column-vector `M * v` convention with the perspective `w`-divide — the
  `05-review.md` perspective-fix lineage. No verbatim `v * M`. (§3)
- **Blocking issue: leftover instrumentation** — see §5.

---

### 1. 0.25-spp readiness — THE KEY CHECK — verdict: **READY**

The question: does the committed Batch 2 TAA genuinely maintain a per-pixel
accumulated **sample-count** (the "weight" / age), increment it correctly over
frames, and store it in `taa_sample_accum` so a future Phase-B GI sampler can
read it? **Yes — verified end-to-end against `06-design-a2.md` §7.5 and
`renderTaaSampleReverse.fx:163-171`.**

**Where the count is written each frame — `taa.wgsl:389-408` (Phase 3,
accumulation):**

```wgsl
let taa_color_comp = taa_sample_accum[pixel_index];
let weight_rg = unpack2x16float(taa_color_comp.x); // .x = f16(weight), .y = f16(R)
...
let sample_weight = weight_rg.x;
...
new_color_comp.x = pack2x16float(vec2<f32>(sample_weight + color_sum.a, taa_color.r));
...
taa_sample_accum[pixel_index] = new_color_comp;
```

This is a line-faithful port of `renderTaaSampleReverse.fx:163-171`:
`sampleWeight = f16tof32(taaColorComp.x & 0xFFFF)` → `unpack2x16float(...).x`;
`newColorComp.x = f32tof16(sampleWeight + colorSum.a) | ...` →
`pack2x16float(vec2(sample_weight + color_sum.a, ...))`. The accumulated count is
`sample_weight + color_sum.a`, stored as an f16 in the low 16 bits of
`taa_sample_accum[px].x`.

**Where the `+1` per-frame increment comes from:**

- The current frame's contribution — `sample_weight` — is the `1.0` weight the
  first-hit pass writes: `naadf_first_hit.wgsl:196`,
  `new_color.x = pack2x16float(vec2<f32>(1.0, light.r))`. Faithful to
  `renderFirstHit.fx:120` `f32tof16(1.0f) | ...`.
- The history contribution — `color_sum.a` — is the count of *accepted*
  reprojected history samples. Each accepted sample contributes `s.color`
  (`taa.wgsl:386`), and `s.color.a` is **always `1.0`**: `taa_common.wgsl:126`
  decompresses the colour as `vec4<f32>(100.0 * pow(...), 1.0)` — the `.a = 1.0`
  is the per-sample "this sample counts as 1" weight, exactly
  `commonTaa.fxh:49`'s `color = float4(..., 1)`. So `color_sum.a` after the
  reproject loop is literally the integer count of accepted past frames.
- Result: `sample_weight + color_sum.a` = 1 (this frame) + N (accepted history)
  = the per-pixel count of frames currently contributing to that pixel. Over
  successive frames the ring fills and the count climbs toward `sample_age`
  (= 16) — the adaptive signal Phase B's `rayQueueCalc` reads.

**Where it would be read (the Phase-B consumer):** `naadf_final.wgsl` already
reads this same `weight` to average the colour (`renderFinal.fx:36-39` —
divides RGB by `max(1, weight)`); Phase B's `rayQueueCalc.fx` reads
`taaSampleAccum[px].x & 0xFFFF` (as f16) to decide which pixels need GI rays
(`02-research.md` §1.2.3). The signal is in the documented location and format.

**Faithfulness of the `.a` chain — not simplified, not dropped:**
`06-design-a2.md` §3.2 / §7.5 explicitly flag the `.a = 1.0` as "load-bearing —
do not drop it." Verified intact at all three points: `taa_common.wgsl:126`
(decompress sets `.a = 1.0`), `taa.wgsl:386` (`color_sum = color_sum + s.color`
sums it), `taa.wgsl:406` (`sample_weight + color_sum.a` stores it). The
accumulation is the exact `06-design-a2.md` §7.5 / `renderTaaSampleReverse.fx`
algorithm, not a simplification.

**Caveat (not a blocker, but record it):** the leftover instrumentation block
at `taa.wgsl:410-422` *overwrites* `taa_sample_accum` for the single debug pixel
`screen_width*screen_height/2 + 7` with raw integer counters instead of the
real packed accum value. For that one pixel the 0.25-spp signal is corrupt. This
is a consequence of the §5 blocking issue — once the instrumentation is
reverted, the signal is intact for **every** pixel. The *mechanism* (the §1
accumulation) is correct and ready; one pixel is collateral damage of
un-reverted scaffolding.

**Verdict: 0.25-spp readiness — READY.** The per-pixel accumulated sample-count
is genuinely maintained, incremented `+1` per frame plus accepted history,
stored in `taa_sample_accum.x` as f16 in the Phase-B-readable format. The only
asterisk is the one debug pixel corrupted by the §5 leftover instrumentation,
which the §5 revert fixes.

---

### 2. Faithful port — verdict: **YES** (modulo the §5 instrumentation)

#### 2.1 `taa.wgsl` vs `renderTaaSampleReverse.fx`

Read side-by-side. The port is faithful:

- **3×3 neighbourhood precompute** (`taa.wgsl:227-294` vs HLSL `:32-75`): the
  9-iteration loop over `taa_neighbor_offsets`, `get_hit_data_from_planes_a2`
  per neighbour, `cur_first_hit_entity = cur_first_hit.x & 0x3FFFu`,
  `cur_first_hit_is_diffuse = cur_first_hit.y & 0x1u`,
  `cur_dist = unpack2x16float(cur_first_hit.w & 0x7FFFu).x` with the
  `if ((cur_first_hit.z & 0x7FFFu) == 0u) cur_dist = 65520.0` miss override,
  the `if (cur_dist < first_hit_dist)` closest-neighbour tracking, the
  `dist_min_max` min/max, the `valid_hash_center` / `valid_hashes_comp[(i-1)/2]
  |= hash << (16*((i-1)%2))` packing — all match HLSL `:44-73` exactly. The
  `validNormalsSpec` accumulation (HLSL `:65-67`) is correctly folded to a
  no-op (A-2's `cur_first_hit_specular_normals` is always 0) and the `ENTITIES`
  block (HLSL `:76-84`) is correctly omitted — both per `06-design-a2.md` §7.2.
- **`get_hit_data_from_planes_a2`** (`taa.wgsl:111-137`): verified against the
  HLSL `getHitDataFromPlanes` tail (`commonRenderPipeline.fxh:205-211`) with the
  `firstHitResult` initialisation at `:157-162`. With planes 1-3 = `HIT_UNDEFINED`
  the HLSL specular loop (`:164-181`) runs zero iterations, so the function
  reduces to: `normalTang = firstHit.x >> 15`; `pos = camPosFrac`; `dist = 0`;
  then `normal = NORMAL[normalTang & 0x7]`,
  `rayDirCompForNormal = abs(dot(rayDir, normal))`,
  `distToTang = abs(dot(pos, abs(normal)) - (float)((normalTang>>3) -
  dot(camPosInt, abs(normal))))`, `distFac = distToTang/rayDirCompForNormal`,
  `dist += distFac`, `pos += rayDir * distFac`. The WGSL reproduces this
  line-for-line (`taa.wgsl:119-135`), with `r.pos = cam_pos_frac + ray_dir *
  dist_fac` (= `camPosFrac + rayDir*distFac` since `pos` started as `camPosFrac`)
  and `r.dist = dist_fac` (= `0 + distFac`). `normal_mirror_fac = (1,1,1)`
  identity — correct, no specular bounces. Faithful single-plane reduction.
- **`get_screen_pos_projection` / `get_screen_index_projection`**
  (`taa.wgsl:154-203`) vs HLSL `commonRenderPipeline.fxh:133-152`: the NDC
  reject (`ndc.x/y ∈ [-1,1]`, `ndc.z ∈ [0,1]`), the `ndc.y *= -1`,
  `ndc01 = (ndc.xy + 1) * 0.5`, `screen_pos = ndc01 * (w,h)`, the
  `clamp(screen_pos + pixel_offset, 0, (w-1,h-1))`, `screen_index = x + y*w`,
  `return valid` — all match. WGSL returns small structs in place of HLSL `out`
  params (no `out` in WGSL) — a correct mechanical adaptation. The HLSL computes
  `screenIndex` even when `valid` is false and gates on `valid` at the call
  site; the WGSL does the same (`taa.wgsl:189-202`).
- **Reprojection loop** (`taa.wgsl:296-387` vs HLSL `:86-161`):
  `pos_virtual = ray_dir * first_hit_dist`; `for i in 1..sample_age`;
  `cur_history_index = (taa_index + i) % 128`;
  `cur_taa_index = (taa_index + i) % TAA_SAMPLE_RING_DEPTH`;
  `reproject_pos = cur_pos_virtual - slot.cam_pos_from_cur_int`;
  `get_screen_index_projection(..., slot.view_proj, -cur_taa_jitter)`;
  `cur_samp = taa_samples[screen_index + cur_taa_index * w * h]`;
  `taa_decompress_sample`; the distance reject
  (`dist_cur < dist_min_max.x * 1022/1024 || dist_cur > dist_min_max.y *
  1026/1024 || s.dist > dist_min_max.y * 2`); the 1-pixel screen reject; the
  hash reject (`s.hash != valid_hash_center` then the 8-neighbour-hash loop);
  `color_sum += s.color` — every line matches HLSL `:88-160`. The `entityPosChange`
  term is correctly absent (A-2 is entity-free; `entityPosChange` is `(0,0,0)`
  without entities) and the rough-specular `if (extraData != 0)` branch
  (HLSL `:138-148`) is correctly left as a structural dead-code comment
  (`taa.wgsl:366-370`) — `extra_data` is provably `0` in the albedo path, so the
  branch never executes; porting its body would pull in `pdf_vndf_isotropic`, a
  Phase-B function. Both omissions are per `06-design-a2.md` §7.4 and are sound.
- **Accumulation** (`taa.wgsl:389-408` vs HLSL `:163-171`): verified faithful
  in §1 above.
- **Edge-pixel clamp** (`taa.wgsl:248-252`): the 3×3 neighbour reads clamp
  `pixel_pos + offset` to `[0, (w-1,h-1)]` before indexing — WGSL storage
  out-of-bounds reads are undefined; DX11 SRVs return 0. This is a sound
  port-correctness deviation flagged in the `taa.wgsl` header comment, exactly
  the kind `06-design-a2.md` §7.2 prescribes.

#### 2.2 First-hit ring write vs `renderFirstHit.fx:109-117`

`naadf_first_hit.wgsl:170-187` ports the `if (isTAA)` block faithfully:
gated on `(params.flags & FLAG_IS_TAA) != 0u`; `specular_normals = 0u`
(hardcoded — `getSpecularNormals` is always 0 for A-2's plane-0-only world, per
`06-design-a2.md` §6.1); `sample_dist = select(distance_ray, 65520.0,
voxel_type_raw == 0u)` (WGSL `select(f, t, cond)` = `cond ? t : f`, so this is
`voxel_type_raw==0 ? 65520 : distance_ray` — matches HLSL `voxelTypeRaw == 0 ?
65520 : distanceRay`); `taa_compress_sample(sample_dist, light, norm_tangs.x &
0x7u, 1u, specular_normals, 0u, entity)` matches `compressSample(f32tof16(...),
light, firstHitNormalTang & 0x7, true, specularNormals, 0, entity)`. The
`first_hit_data` write is kept **unconditional** (Phase A's logged deviation)
— correct, the reproject pass needs plane 0 always populated. The
`taa_sample_accum` write (`naadf_first_hit.wgsl:195-202`) is unchanged from
Phase A and matches `renderFirstHit.fx:119-124`.

#### 2.3 The ring is 16-deep everywhere — verified

The §6 VRAM lever requires *both* `% 32` sites in the HLSL to become `% 16`:
- `renderFirstHit.fx:116` `(taaIndex % 32)` → `naadf_first_hit.wgsl:184`
  `params.taa_index % TAA_SAMPLE_RING_DEPTH` (= 16u). ✓
- `renderTaaSampleReverse.fx:91` `(taaIndex + i) % 32` → `taa.wgsl:309`
  `(params.taa_index + i) % TAA_SAMPLE_RING_DEPTH`. ✓ (the "do not miss the
  second one" site — not missed.)
- `renderTaaSampleReverse.fx:90` `(taaIndex + i) % 128` → `taa.wgsl:307`
  `(params.taa_index + i) % 128u` — correctly **stays 128** (the camera-history
  ring is not the §6 lever). ✓

`TAA_SAMPLE_RING_DEPTH = 16u` is declared once in `taa_common.wgsl:20` and once
as `TAA_SAMPLE_RING_DEPTH: u32 = 16` in `taa.rs:36`; the buffer is sized
`pixel_count * 16 * 8` bytes (`taa.rs:395`). Consistent — single source of truth
on each side.

#### 2.4 `taa_common.wgsl` vs `commonTaa.fxh`

`taa_compress_sample` / `taa_decompress_sample` / `taa_hash_from_data` /
`taa_neighbor_offsets` are line-faithful ports of `commonTaa.fxh:6-53`. The
exponential colour compression, the `[0,100]` clamp, the
`u32(...)` + `min(255u, ...)` explicit truncation (WGSL has no implicit
float→uint truncation — `06-design-a2.md` §3.2 implementer note), and the
`.a = 1.0` on decompress are all correct. The one logged deviation —
`taa_compress_sample` takes `dist: f32` and folds the `f32tof16` in via
`pack2x16float(vec2(dist, 0.0)) & 0xFFFFu` rather than taking pre-converted f16
bits — is behaviour-identical and matches `06-design-a2.md` §6.1's snippet.

**Verdict: faithful port — YES.** The TAA logic, the first-hit ring write, and
the 16-deep ring are all faithfully ported. The only thing standing between this
and a clean Phase-A-2 is the §5 instrumentation residue.

---

### 3. Matrix-convention check — verdict: **CORRECT**

The `05-review.md` perspective fix established that glam-built (column-major)
matrices use the column-vector convention `M * v` with a perspective `w`-divide,
*not* the verbatim HLSL `mul(v, M)` row-vector form. The Phase-A perspective
regression was exactly a `v * M` against a glam matrix. Every matrix multiply in
`taa.wgsl` was checked:

1. **`get_ray_dir(params.inv_view_proj, ...)`** (`taa.wgsl:223-225`) — delegates
   to the shared `render_pipeline_common.wgsl::get_ray_dir`, which is the
   perspective-fixed version: `inv_view_proj * vec4<f32>(ndc, 1.0, 1.0)` then
   `normalize(unprojected.xyz / unprojected.w)` (verified at
   `render_pipeline_common.wgsl:155-156`). `M * v` + `w`-divide. ✓
2. **`get_screen_pos_projection`** (`taa.wgsl:161-162`):
   `let screen_projection = transformation * vec4<f32>(pos, 1.0);` then
   `let ndc = screen_projection.xyz / screen_projection.w;`. `M * v` +
   `w`-divide. Ports HLSL `mul(float4(pos,1), transformation)`
   (`commonRenderPipeline.fxh:135`) with the convention flipped. ✓ Called with
   `slot.view_proj` (a past frame's rotation-only view-proj).
3. **The 1-pixel screen-position reject** (`taa.wgsl:355-356`):
   `let screen_projection_new = params.view_proj * vec4<f32>(old_virtual_pos,
   1.0);` then `var ndc_new = screen_projection_new.xyz /
   screen_projection_new.w;`. `M * v` + `w`-divide. Ports HLSL
   `mul(float4(oldVirtualPos, 1), camMatrix)` (`renderTaaSampleReverse.fx:127`)
   with the convention flipped. ✓ `params.view_proj` is
   `ExtractedCameraData.view_proj` — the *non-inverted* rotation-only
   `clip_from_view_rot` (the C# `camMatrix`), built by the shared
   `rotation_only_view_proj` helper (`extract.rs:125,135`, `taa.rs:132-136`).

There is **no `v * M` anywhere** in `taa.wgsl`. The `taa.wgsl` header comment
(`:14-19`) and the inline comments at `:145-147` and `:352-354` explicitly cite
the `05-review.md` perspective-fix reasoning and warn against "fixing" it back
to `v * M` — good defensive documentation. The matrices fed in are the correct
rotation-only ones (`inv_view_proj` for ray dirs, `view_proj` / past-frame
`slot.view_proj` for projection) — same lineage as the Phase-A fix, reused not
re-derived.

**Verdict: matrix convention — CORRECT.** This is the same bug class as the
Phase-A perspective regression and it was *not* reintroduced.

---

### 4. Other correctness observations (non-blocking)

- **Bind-group layout / WGSL agreement.** `taa_reproject_layout`
  (`pipelines.rs:200-212`) declares binding 0 = uniform, 1-3 = read-only
  storage, 4 = read-write storage; `taa.wgsl:86-90` declares
  `params` uniform, `camera_history`/`first_hit_data`/`taa_samples` read storage,
  `taa_sample_accum` read_write storage — order and access match.
  `prepare.rs:435-445` builds the bind group in the same 0-4 order. The
  first-hit `@group(2)` (`taa_layout`, one rw storage binding) matches
  `naadf_first_hit.wgsl:52` and `taa.rs:367-374`. `frame_layout` /
  `blit_layout` are pure renames (`shaded_color` → `taa_sample_accum`), shape
  unchanged. Consistent.
- **`GpuTaaParams` / `GpuCameraHistorySlot` layout.** Rust `#[repr(C)]`
  (`gpu_types.rs:160-223`) is 192 / 96 bytes (compile-time asserted at
  `:309-310`); the `taa.wgsl:60-81` struct decls use the no-explicit-`_pad`
  convention — WGSL's `vec3`→16-byte / `vec2`→8-byte slotting reproduces the
  padded Rust layout. The Rust trailing scalars
  (`screen_width/height/frame_count/taa_index` then `sample_age` + 3 pads) and
  the WGSL trailing scalars pack to the same 192. Matches.
- **`taa_reproject_bind_group` rebuild coherence.** It is rebuilt in
  `prepare_frame_gpu` only on `needs_new_storage || existing.is_none()`
  (`prepare.rs:414-415`). `taa_params` / `camera_history` are fixed-size buffers
  `prepare_taa` creates once and never recreates, so caching the bind group
  across frames is safe; `taa_samples` / `taa_sample_accum` / `first_hit_data`
  all resize on the *same* `pixel_count` trigger, so the rebuild covers them
  together. Coherent — no stale-buffer hazard.
- **`sample_age` clamp.** `taa.rs:223` `TAA_SAMPLE_AGE = TAA_SAMPLE_RING_DEPTH`
  (16), uploaded `clamp(1, TAA_SAMPLE_RING_DEPTH)` (`taa.rs:358`); the
  `taa.wgsl:306` loop is `for i in 1..params.sample_age` — walks 15 past frames
  (i = 1..15), correct for a 16-deep ring (i=0 is the current frame, written by
  first-hit). Per `06-design-a2.md` §7.1.
- **Frame counter / `taa_index` / jitter** (the carried `05-review.md` §4 fix):
  `prepare.rs:306,313,314` set `frame_count` / `rand_counter` / `taa_index` from
  the extracted `CameraHistory` (real monotonic counter, stored `taa_index` —
  not re-derived); `FLAG_IS_TAA` is set when `extracted_taa.enabled`
  (`prepare.rs:321-325`). `main.rs:51` has `taa: true` — the §9.5 default flip
  is done. All correct.
- **TAA-off path.** `naadf_taa_reproject_node` early-returns when
  `!taa_config.enabled` (`graph.rs:236-238`), leaving `taa_sample_accum`
  untouched — bit-identical to Phase A's `shaded_color` path, as designed
  (§8.2). Verified.

The Batch-2 code's own comments accurately flag every deviation (the entity
omissions, the dead rough-specular branch, the single-plane reduction, the
edge-pixel clamp, the `dist: f32` compress-helper signature) and each is sound
per `06-design-a2.md`.

---

### 5. BLOCKING ISSUE — leftover TEMP STEP-8 instrumentation was never reverted

The Batch-2 commit message (`8abd2ec`) itself says "temporary step-8
staging-buffer instrumentation logs per-frame TAA debug counters" — and that
instrumentation **is still in the committed code**. The implementing agent's
global instruction is "instrumentation reverted before commit"; this was not
done (the agent was interrupted). It is a real defect: it corrupts one pixel of
the production output buffer, spams logs, and leaves dead code. **Confidence:
very high — it is self-fenced with `TEMP` / `TEMPORARY STEP-8` comments and
confirmed active in the smoke-run (`07-impl-a2.md`).**

**This review does NOT fix it** — the brief scopes this group's implementation
work to the `hud.rs` line only and forbids touching the Batch-2 TAA code. It is
reported here as a finding with exact file:line evidence and a recommended
revert; the revert is a separate, mechanical, scoped task.

#### 5.1 Evidence — every instrumentation site

| file | lines | what it is |
|---|---|---|
| `src/assets/shaders/taa.wgsl` | `301-304` | `var dbg_valid / dbg_dist_pass / dbg_screen_pass` debug counters, incremented through the reproject loop (`:328, :350, :364`). |
| `src/assets/shaders/taa.wgsl` | `410-422` | the `if (pixel_index == screen_width*screen_height/2 + 7u)` block — **overwrites `taa_sample_accum[pixel_index]` with raw integer counters** instead of the real packed accum value. Corrupts the 0.25-spp signal for that one pixel (see §1 caveat). |
| `src/render/graph.rs` | `37-46` | the `TEMPORARY STEP-8 INSTRUMENTATION` `use` block + the `TaaDebugReadback` resource. |
| `src/render/graph.rs` | `48-117` | `taa_debug_copy_node` (a `Core3d` node copying a pixel into a staging buffer) + `taa_debug_readback_system` (maps + logs it). |
| `src/render/graph.rs` | `119-144` | the `half_to_f32` helper (used only by the readback). |
| `src/render/graph.rs` | `240-265` | inside `naadf_taa_reproject_node`: `info!("TAA_DEBUG ...")` calls — the missing-resource log, the per-pipeline-state `match` logging, and `info!("TAA_DEBUG reproject_node: DISPATCHING")`. The `else` `match` arm exists *only* to log. |
| `src/render/mod.rs` | `36-37` | `// TEMPORARY STEP-8 INSTRUMENTATION` + `use graph::{taa_debug_copy_node, taa_debug_readback_system};`. |
| `src/render/mod.rs` | `96-97` | `taa_debug_copy_node` inserted into the `Core3d` `.chain()` between the reproject node and the final blit. |
| `src/render/mod.rs` | `104-105` | `taa_debug_readback_system` added to `RenderSystems::Cleanup`. |
| `src/render/taa.rs` | `400-405` | `taa_sample_accum` created with `\| BufferUsages::COPY_SRC` "TEMPORARY STEP-8" so the readback node can copy from it; the comment says "reverted before return" — it was not. |

#### 5.2 Impact

- **Functional:** `taa.wgsl:414-421` writes garbage (raw integer counters, no
  f16 packing) into `taa_sample_accum` for pixel
  `screen_width*screen_height/2 + 7` every frame. The final blit reads that
  buffer, so that one pixel renders wrong; the 0.25-spp signal for that pixel is
  corrupt. (The user's "kinda looks ok" eyeball would not notice a single
  pixel.) Every other pixel is correct.
- **Performance / noise:** `taa_debug_readback_system` does a *blocking*
  `render_device.poll(PollType::wait_indefinitely())` every frame on the render
  thread — a GPU sync stall on the hot path. The per-frame `info!` logs spam the
  console (confirmed in the smoke-run — dozens of lines/second).
- **Code health:** `taa_debug_copy_node` is a live `Core3d` node and
  `taa_debug_readback_system` a live `Cleanup` system — dead-weight scaffolding
  in the committed graph.

#### 5.3 Recommended fix (the revert — a separate scoped task)

Delete every site in the §5.1 table:
1. `taa.wgsl`: remove `:301-304` (the `dbg_*` decls), the three `dbg_* = dbg_* +
   1.0` increments at `:328, :350, :364`, and the whole `:410-422` debug-pixel
   `if` block.
2. `graph.rs`: remove the `:37-46` fenced `use` + `TaaDebugReadback`, the
   `:48-117` two debug systems, the `:119-144` `half_to_f32`, and inside
   `naadf_taa_reproject_node` the `info!("TAA_DEBUG ...")` calls at `:240` and
   `:251-265` (collapse the pipeline-not-ready `else` back to a plain
   `return;`) and `:265` (`info!(... DISPATCHING)`).
3. `mod.rs`: remove the `:36-37` `use`, drop `taa_debug_copy_node` from the
   `Core3d` `.chain()` (`:96-97`) so it is back to
   `(naadf_first_hit_node, naadf_taa_reproject_node, naadf_final_blit_node)`,
   and remove the `:104-105` `taa_debug_readback_system` registration.
4. `taa.rs`: remove `| BufferUsages::COPY_SRC` from the `taa_sample_accum`
   `BufferDescriptor` (`:405`) and the `:400-402` "TEMPORARY STEP-8" comment.

After the revert: rebuild, `cargo test --bin bevy-naadf` (39 must still pass),
one smoke-run to confirm clean launch + no `TAA_DEBUG` log spam + clean exit.
Nothing else changes — the revert removes only the fenced scaffolding; the TAA
logic verified in §1-§4 is untouched.

---

### 6. Overall verdict

**Phase A-2 has a blocking issue: the leftover TEMP STEP-8 instrumentation
(§5) must be reverted before Phase A-2 closes.**

But the substance the user asked about is sound:
- **0.25-spp readiness: READY** — the per-pixel sample-count signal is genuinely
  maintained, incremented correctly, and exposed in the Phase-B-readable format
  (§1).
- **Faithful port: YES** — `taa.wgsl`, the first-hit ring write, and the 16-deep
  ring all faithfully port the NAADF HLSL (§2).
- **Matrix convention: CORRECT** — every multiply is `M * v` + `w`-divide; the
  Phase-A perspective bug class was not reintroduced (§3).
- **Other correctness: clean** — bind groups, struct layouts, the carried
  `05-review.md` §4 frame-counter fix, the TAA-off path all verified (§4).

The blocking issue is incomplete-cleanup, not a design or algorithm bug — it is
mechanical to revert (§5.3) and is a separate scoped task, not part of this
review group's implementation surface (which was the `hud.rs` timing line only,
done — see `07-impl-a2.md`). Once §5 is reverted, Phase A-2 is verified: the
TAA logic is correct, faithful, and Phase-B-ready.
