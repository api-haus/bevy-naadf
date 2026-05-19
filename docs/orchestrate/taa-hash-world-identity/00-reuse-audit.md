# Reuse Audit — TAA Hash World-Data Identity (`data_id_lo13`)

## Summary

No existing shader helper produces a stable per-voxel-cell discriminator ready
to plug in as `data_id_lo13`. The closest candidate — `RayResult.voxel_pos`
(an `i32` world-cell coordinate) — is populated by `shoot_ray` but is **not
propagated** into the `first_hit_data` G-buffer or the `FirstHitResult` struct;
it is consumed only inside `naadf_first_hit.wgsl` and then discarded. The
derivation the handoff proposes (`floor(first_hit_result.pos)` quantised to 13
bits) is a greenfield expression, though it is built entirely from existing
accessible data.

## Candidates

| Path:Line | Symbol | What it does | Fits as `data_id_lo13` source? | Notes |
|---|---|---|---|---|
| `assets/shaders/ray_tracing.wgsl:148` | `RayResult.voxel_pos: vec3<i32>` | The integer world-cell coordinate of the DDA hit voxel, set at lines 383/529. | **Partial** | Exact world-space voxel identity — the ideal discriminator — but is NOT forwarded into the G-buffer (`compress_first_hit_data`) or `FirstHitResult`. Would require adding a new G-buffer channel or a second binding to carry it to the TAA pass. |
| `assets/shaders/render_pipeline_common.wgsl:366` | `get_hit_data_from_planes` → `FirstHitResult.pos: vec3<f32>` | Reconstructs the camera-int-relative floating-point world position of the first hit by marching along the ray using the packed normal-tang planes. Available at `taa.wgsl:228-230` (`cur_first_hit_result.pos`) and `:422-424`. | **Yes** | `pos` is already computed for every neighbour pixel in the 9-iteration precompute loop and at the `calc_new_taa_sample` site. `floor(pos)` gives a camera-int-relative integer voxel cell; the bit-packing the handoff proposes is a two-liner. No new bindings needed. |
| `assets/shaders/ray_tracing_common.wgsl:19` | `pcg_hash(input: u32) -> u32` | The PCG hash used to seed the RNG from a 3-component key. Pure arithmetic, no state. | **Partial** | The avalanche quality is sufficient for a discriminator hash; however, `taa_hash_from_data` already has its own mixing chain, so `pcg_hash` is a redundant additional mixer — the value produced by `floor(pos)` bit-packing fed directly into the existing `taa_hash_from_data` chain is simpler. Not needed separately. |
| `assets/shaders/world_data.wgsl:166` | `streaming_chunk_index` → `slot` (line 191) | The `window_indirection` lookup that translates a chunk-local coordinate to its streaming slot index (0–511). The slot index IS the world-data segment identity. | **No** | The slot is accessible in `shoot_ray` (via `streaming_chunk_load`), but it is also not forwarded into the G-buffer. Extracting it would require a new G-buffer write in `naadf_first_hit.wgsl` and a new TAA bind-group entry. Over-engineered for the purpose — the handoff explicitly forbids new bind-group entries. |
| `assets/shaders/chunk_calc.wgsl:404` | Block-content hash loop (polynomial over voxel types) | Computes a rolling polynomial hash of all voxel type values in a 4×4×4 block, used for the dedup hash-map during construction. | **No** | CPU-side construction pipeline only; result is never exposed to the render/TAA shaders. Not accessible at runtime in the TAA path. |
| `assets/shaders/taa_common.wgsl:49` | `taa_hash_from_data(is_diffuse, specular_normals, entity) -> u32` | The existing TAA surface-classification hash function. Mixes three inputs; bits 2–14 of the pre-mix `u32` are currently zero (unused). | **Extend** | This is Site 1 of the fix. Extending the signature with `data_id_lo13: u32` and OR-ing `(data_id_lo13 & 0x1FFF) << 2u` into the pre-mix word is the intended change. Both call sites (`taa_compress_sample` at line 107; `taa.wgsl:262-264`) need to pass the new argument. |
| `assets/shaders/taa_common.wgsl:80` | `taa_compress_sample(...)` | Calls `taa_hash_from_data` at line 107, packs the masked 16-bit hash into `sample_comp.x >> 16` (line 110). The SOLE writer of the TAA sample hash field. | **Extend** | Site 3. Needs one new argument forwarded from its call site in `taa.wgsl:457`. No structural change — the hash storage slot (bits 16–31 of `sample_comp.x`) already exists. |

## Borderline calls

- **`RayResult.voxel_pos`** — borderline between Partial and Not-Applicable. It
  is the most *semantically* correct world-data identity (exact DDA hit cell),
  and its reuse would make the fix origin-shift-exact rather than
  float-reconstruction-approximate. It flips from Partial to "use it" only if
  the implementer adds a new channel to the G-buffer (or a small separate
  per-pixel buffer) to carry it from `naadf_first_hit.wgsl` through to the TAA
  pass — which the handoff explicitly forbids ("No new auxiliary buffers, no
  new bind-group entries"). Under the given constraints it is Not-Applicable.

- **`pcg_hash`** — borderline between Partial and Not-Applicable. It is a
  hash-quality consideration: using `pcg_hash(pos_id)` instead of the raw bit-
  packed position before feeding into `taa_hash_from_data` would improve
  avalanche from position inputs. However, `taa_hash_from_data` already
  applies two multiply-xor mix steps, giving adequate avalanche on the 13-bit
  input. Using `pcg_hash` as a pre-processor would be a quality improvement,
  not a requirement. Verdict stays Not-Applicable for the core audit; the
  implementer can optionally add it as a quality micro-improvement.

## `first_hit_result` struct layout

**Struct definition:** `render_pipeline_common.wgsl:69–76`

```wgsl
struct FirstHitResult {
    pos: vec3<f32>,              // camera-int-relative world-space hit position (float)
    normal: vec3<f32>,           // surface normal at hit
    normal_mirror_fac: vec3<f32>,// accumulated mirror reflectance
    dist: f32,                   // total ray distance to hit (accumulated through bounces)
    normal_tang: u32,            // deepest plane normal-tang code
    ray_dir: vec3<f32>,          // ray direction at hit (after reflections)
}
```

**Frame of reference for `pos`:** camera-int-relative, NOT world-absolute.
`get_hit_data_from_planes` initialises `r.pos = cam_pos_frac`
(`render_pipeline_common.wgsl:375`) and accumulates ray-segment offsets via
`r.pos += r.ray_dir * dist_fac` through each specular bounce. The int camera
position (`cam_pos_int`) is added back implicitly only inside the plane-
distance calculation (`f32(r.normal_tang >> 3u) - dot(vec3<f32>(cam_pos_int), abs(r.normal))`
at lines 391/407). The final `pos` is **not** adding `cam_pos_int` back as a
whole-number offset — it remains fractional-camera-relative.

**Consequence for `data_id_lo13` derivation:** To get an absolute-world voxel
coordinate, the implementer must add `vec3<f32>(cam_pos_int)` to `pos` before
calling `floor`. At the 9-iteration precompute site in `taa.wgsl`, `cam_pos_int`
is available as the local `cam_pos_int = params.cam_pos_int.xyz` (line 182). At
the `calc_new_taa_sample` site it is `cam_pos_int = cnts_params.cam_pos_int.xyz`
(line 407). Both are `vec3<i32>`; cast to `vec3<f32>` before adding.

The expression from the handoff with this correction applied:
```wgsl
let voxel_pos = vec3<i32>(floor(cur_first_hit_result.pos + vec3<f32>(cam_pos_int)));
let pos_id =
      (u32(voxel_pos.x & 0xF))
    | (u32(voxel_pos.y & 0xF) << 4u)
    | (u32(voxel_pos.z & 0xF) << 8u)
    // ... coarse-grid mixing for bits 12..
```

**Alternative cheaper source — `first_hit.z & 0x7FFFu` (the `voxel_type_raw`):**
The 15-bit `voxel_type_raw` packed into the G-buffer `.z` is the voxel-TYPE id
(a material class), not a world position. Two distinct cells with the same
material type produce the same `voxel_type_raw`. This is NOT a world-data
identity discriminator; it cannot distinguish origin shifts on homogeneous terrain.

## Recommendation

The only path that satisfies all constraints (no new bindings, no new buffers,
shader-only fix) is to derive `data_id_lo13` from `first_hit_result.pos` — a
value already computed at both load-bearing TAA sites. The derivation is a
greenfield two-liner (add `cam_pos_int`, floor, bit-pack 13 bits from x/y/z
components of the integer result), feeding into an EXTEND of
`taa_hash_from_data` (Site 1) and `taa_compress_sample` (Site 3). No
existing shader helper produces this identity pre-computed; the value emerges
from an arithmetic expression over already-available variables. The `pcg_hash`
function could optionally improve the position-input mixing quality, but the
existing `taa_hash_from_data` avalanche chain makes it non-mandatory. Greenfield
expression, extend of two existing functions.
