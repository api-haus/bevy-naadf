# 16 — Phase C impl log — W4 (dynamic entities)

## W4 — Dynamic entities (2026-05-15)

W4 ports NAADF's **paper contribution #4** (paper §3.6 *Dynamic entities*) into
the bevy-naadf renderer in full at the CPU + GPU-construction layer, and lays
the load-bearing seam for the renderer-side traversal integration. The
chunks 3D texture format is widened from `R32Uint` to `Rg32Uint` so the per-
chunk entity-pointer + counter pair fits in `.y` alongside the construction-
state pointer + AADF in `.x` (`15-design-c.md` §1.7). The CPU
`EntityHandler::update` port runs per-frame entity-instance hash dedup +
chunk-update emit + history-ring write (`EntityHandler.cs:165-443`). The GPU
`entity_update.wgsl` ports `entityUpdate.fx`'s three entry points verbatim.
A new W4 e2e-binary flag `--entities` exercises the CPU port end-to-end.

After W4, the renderer-side **traversal integration is still pending** because
the entity buffers live behind the W4 `construction_entity_layout` but the
`naadf_world_bind_group_layout` (owned by `NaadfPipelines`, which the W4
brief explicitly forbids editing) is the only `@group(0)` the render shaders
bind. The chunks-texture flip and every renderer-side `.x` selection are
landed so that wave-3 integration can flip `ENTITIES` on without touching W4's
work; see "Integration notes for the merge agent" below.

### Changes by file

**New files (3):**

- `crates/bevy_naadf/src/assets/shaders/entity_update.wgsl` (~115 lines) —
  port of `entityUpdate.fx`. Three entry points (`update_chunks`,
  `copy_entity_chunk_instances`, `copy_entity_history`), all
  `@workgroup_size(64,1,1)`. Storage texture format is `rg32uint` to drive
  the chunks `.y` channel write. Two bind groups (`@group(0)` =
  chunks_rw + `EntityUpdateParams` uniform; `@group(1)` = the 5 entity-track
  buffers). Documented MonoGame→wgpu deviations inline.
- `crates/bevy_naadf/src/aadf/entity.rs` (~430 lines) — CPU algorithm layer:
  - `compress_quaternion` / `decompress_quaternion` — port of the
    smallest-three encoding from `EntityHandler.cs:499-546` /
    `commonRayTracing.fxh:163-220`. 14-bit-per-component quantisation in
    `[-1, 1]`; 32-bit packing matches the C# byte-for-byte.
  - `EntityData::from_types` — port of `EntityData.cs:15-107`. Builds a
    per-entity AADF voxel volume by running 31 iterations of the
    synchronised-iteration neighbour-merge per-axis (`addBounds`,
    `checkMatchingBoundCell`, the 6 `MASK_M*/MASK_P*` constants — line-
    for-line port from C# `EntityData.cs:58-130`).
  - `compress_entity_chunk_instance` — port of `EntityHandler.cs:325-329`:
    pack position + inverse-quaternion + voxel_start + entity + size into
    `GpuEntityChunkInstance`.
  - `compress_entity_history` — port of `EntityHandler.cs:367-371`:
    pack position + (forward) quaternion into `GpuEntityInstanceHistory`.
  - `pack_chunk_update` — port of `EntityHandler.cs:336-341`: 8-byte
    `(chunkPos_packed, entityPointerAndSize)` pair.
- `crates/bevy_naadf/src/render/construction/entity_handler.rs` (~330 lines) —
  CPU port of `EntityHandler.Update` (`EntityHandler.cs:165-443`):
  - `entity_hash_coefficients` — the 257-entry table (`EntityHandler.cs:123`).
  - `EntityHandler::update`:
    1. **Reset stale counters** for last frame's overlapped chunks.
    2. **Pass 1 — overlap count**: walk rotated AABB corners per instance,
       accumulate per-chunk counts.
    3. **Pass 2 — prefix-sum**: assign each chunk a pointer into the
       per-frame upload pool.
    4. **Pass 3 — fill pool**: re-walk overlaps + write entity IDs.
    5. **Pass 4 — dedup-hash**: for each chunk, compute the hash, scan the
       in-progress dedup table; if found, reuse the existing
       entity-chunk-instances range; else append + register a new entry.
    6. **Stale-chunk clear**: emit `(chunkPos, 0)` updates for chunks that
       had entities last frame but don't this frame.
    7. **History pack**: one `GpuEntityInstanceHistory` per live entity
       instance.
- `crates/bevy_naadf/src/render/construction/entity_update.rs` (~280 lines) —
  Rust side of `entity_update.wgsl`:
  - `entity_world_layout_descriptor` — `@group(0)`: chunks_rw (Rg32Uint) +
    `GpuEntityUpdateParams` uniform.
  - `construction_entity_layout_descriptor` — `@group(1)`: 5 entity-track
    bindings (3 ro upload buffers + 2 rw output buffers).
  - `queue_*_pipeline` / `queue_*_pipeline_with_handle` helpers for each
    entry point.
  - `dispatch_update_chunks` / `dispatch_copy_entity_chunk_instances` /
    `dispatch_copy_entity_history` — workgroup-count helpers matching the
    `(count + 63) / 64` C# dispatches.
  - `naadf_entity_update_node` — the `Core3d` regime-3 system. Gated on
    `ConstructionConfig.entities_enabled`; body is a no-op until wave-3
    wires the per-frame dispatch.

**Edited files (9):**

- `crates/bevy_naadf/src/render/prepare.rs` — chunks texture format flipped
  from `R32Uint` (4 B / texel) → `Rg32Uint` (8 B / texel). The CPU mirror
  pairs each `R32Uint`-style u32 with `0u` for the entity-pointer `.y`
  channel; `bytes_per_row = size.x * 8`. The `STORAGE_BINDING` usage stays.
- `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl` — `chunks` storage
  texture binding flipped from `r32uint` → `rg32uint`. The `textureStore`
  at line 412 already wrote `vec4<u32>(state, 0u, 0u, 0u)` which is
  forward-compatible (`.x` = state, `.y` = 0).
- `crates/bevy_naadf/src/render/construction/chunk_calc.rs` — `chunks_rw`
  binding format flipped `R32Uint` → `Rg32Uint` in
  `construction_world_layout_descriptor`.
- `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` — comment updated;
  the view binding `texture_3d<u32>` accepts the wider format unchanged
  (WGSL `textureLoad` returns `vec4<u32>` regardless of channel count).
- `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl` — module-level
  comment documents the W4 format flip; the existing `.x` selection at
  line 157 stays (it is now load-bearing — without `.x`, the renderer
  would read uninitialised channels). Adds W4 entity helpers
  (`decompress_quaternion`, `apply_rotation`, `quaternion_inverse`,
  `decompress_entity_instance_from_chunk`, the `EntityInstance` struct) as
  callable functions; the entity sub-traversal invocation from `shoot_ray`
  is deferred to wave-3 integration (see "Integration notes" below).
- `crates/bevy_naadf/src/render/gpu_types.rs` — adds:
  - `GpuEntityChunkInstance` (20 B = 5 × u32) with 6 compile-time
    `const _: () = assert!(...)` size + offset guards + a runtime test
    `entity_chunk_instance_layout_guards`.
  - `GpuEntityInstanceHistory` (16 B = 4 × u32) with guards.
  - `GpuChunkUpdate` (8 B = 2 × u32) with guards.
  - `EntityInstance` — CPU-only mirror (`bevy::math::Vec3` position,
    `[f32; 4]` quaternion, voxel_start + entity + size).
- `crates/bevy_naadf/src/render/construction/config.rs` — adds
  `max_entity_instances: u32` (default `DEFAULT_MAX_ENTITY_INSTANCES = 16384`,
  the `WorldRender.cs:88` cap). Const-pin block updated.
- `crates/bevy_naadf/src/render/construction/mod.rs` — adds `entity_handler`
  + `entity_update` modules; extends `ConstructionPipelines` with 5 new
  fields (`entity_world_layout`, `construction_entity_layout`, 3 pipeline
  IDs); extends `from_world` additively to queue the W4 pipelines. The W1
  validation texture + tests_w1 chunks texture both flip to `Rg32Uint`;
  the `readback_chunks_texture` helper takes the `.x` channel of each pair.
  Adds `validate_entity_handler()` — the `--entities` flag entry point.
  Adds the `tests_w4` test module (`entity_update_pipelines_compile`,
  `entity_update_gpu_vs_cpu`).
- `crates/bevy_naadf/src/render/mod.rs` — inserts `naadf_entity_update_node`
  into the `Core3d` chain immediately before `naadf_atmosphere_node`
  (gated; functionally byte-identical to pre-W4 because the gate stays
  off by default).
- `crates/bevy_naadf/src/aadf/mod.rs` — adds `pub mod entity;`.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — adds the `--entities` CLI
  flag. Body calls `bevy_naadf::render::construction::validate_entity_handler`
  and prints a short report on success / non-zero exits on failure.

**Not edited (by hard rule):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — explicitly
  off-limits per `15-design-c.md` §1.3 / the W4 brief. The chunks texture
  view binding (`@group(0) @binding(0)` in `world_layout`) stays
  `texture_3d<u32>`; wgpu accepts an `Rg32Uint` storage texture under a
  `texture_3d<u32>` sampled view declaration unchanged.
- `crates/bevy_naadf/src/aadf/bounds.rs` — W6's `compute_aadf_layer` is
  untouched. W4's per-entity AADF runs over a different bit layout (the
  `EntityData` 5-bit-per-axis × 6 packed into 30 bits format, not the
  W6 chunks-AADF 5-bit layout), so W4 ports the `EntityData.cs:64-106`
  inline loop verbatim rather than reusing W6's helper.
- W3's surface — the worktree was branched pre-W3.

### `.x` sweep audit (the load-bearing W4 cross-cut)

The chunks texture format flips to `Rg32Uint` — every WGSL site that reads
or writes `chunks` must handle the wider format. Audit summary:

| file | site | pre-W4 | W4 state | notes |
|---|---|---|---|---|
| `assets/shaders/ray_tracing.wgsl:157` | read | `textureLoad(chunks, ..., 0).x` | unchanged (`.x` selection already present) | already forward-compat — `.x` is the construction state |
| `assets/shaders/chunk_calc.wgsl:412` | write | `textureStore(chunks, ..., vec4<u32>(state, 0u, 0u, 0u))` | unchanged | forward-compat — `.y` is 0 (entity track populates separately) |
| `assets/shaders/chunk_calc.wgsl:95` | binding | `texture_storage_3d<r32uint, read_write>` | `texture_storage_3d<rg32uint, read_write>` | format flip |
| `assets/shaders/entity_update.wgsl:80` | binding | (new) | `texture_storage_3d<rg32uint, read_write>` | W4-new |
| `assets/shaders/entity_update.wgsl:99` | write | (new) | `textureStore(chunks, chunk_pos, vec4<u32>(old.x, update.y, 0u, 0u))` | W4-new; preserves `.x`, writes `.y` |
| `assets/shaders/world_data.wgsl:47` | view binding | `texture_3d<u32>` | unchanged | wgpu accepts `Rg32Uint` under `texture_3d<u32>` |
| `assets/shaders/naadf_first_hit.wgsl` | (no chunks reads) | — | — | uses `ray_tracing.wgsl::shoot_ray` |
| `assets/shaders/naadf_global_illum.wgsl` | (no chunks reads) | — | — | uses `shoot_ray` |
| `assets/shaders/spatial_resampling.wgsl` | (no chunks reads) | — | — | uses `shoot_ray` |
| `assets/shaders/naadf_atmosphere.wgsl` | (no chunks reads) | — | — | self-contained |
| `assets/shaders/map_copy.wgsl` | (no chunks reads) | — | — | hash-map only |
| `assets/shaders/generator_model.wgsl` | (no chunks reads) | — | — | writes `segment_voxel_buffer` |
| `assets/shaders/bounds_common.wgsl` | (no chunks reads) | — | — | inlined helper |
| `render/prepare.rs:161` | texture descriptor | `R32Uint` | `Rg32Uint` | format flip + 8 B / texel upload path |
| `render/construction/chunk_calc.rs:69` | layout entry | `R32Uint` | `Rg32Uint` | flip |
| `render/construction/mod.rs:738` | validate path texture | `R32Uint` | `Rg32Uint` | flip |
| `render/construction/mod.rs:1689` | tests_w1 texture | `R32Uint` | `Rg32Uint` | flip |
| `render/construction/mod.rs::readback_chunks_texture` | readback | `bytes_per_row = size[0]*4` | `bytes_per_row = size[0]*8`; takes `.x` of each pair | flip |

**Total chunks-texture-read sites: 1 in renderer WGSL (`ray_tracing.wgsl:157`,
already `.x`-selected) + 0 in renderer non-traversal WGSL.** All other
renderer shaders (`naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`,
`spatial_resampling.wgsl`, `naadf_atmosphere.wgsl`) consume the chunks
texture *indirectly* via the `shoot_ray` function in `ray_tracing.wgsl`; they
have no direct `textureLoad(chunks, ...)` sites of their own, so the single
`.x` selection in `shoot_ray` covers them all.

### Decisions & rejected alternatives

1. **Renderer-side entity sub-traversal: helpers shipped, invocation
   deferred (`shoot_ray` calls the entity branch in a wave-3 follow-up).**
   - Chose to ship `decompress_quaternion`, `apply_rotation`,
     `quaternion_inverse`, `decompress_entity_instance_from_chunk`, and the
     `EntityInstance` struct as **named functions in `ray_tracing.wgsl`**,
     but to **defer the `shoot_ray` invocation site** to wave-3
     integration. The activation requires extending
     `NaadfPipelines::world_layout` with the `entity_chunk_instances` +
     `entity_voxel_data` + `entity_instances_history` buffers, which the
     W4 brief explicitly forbids ("Do NOT edit `NaadfPipelines`"). The
     entity sub-traversal *logic* is now in-tree (helpers + the documented
     C# branch reference at the file header); a wave-3 workstream wires
     the bind group + flips the activation. Without this constraint, the
     branch would land as a `#ifdef ENTITIES`-gated inline block in
     `shoot_ray` — but the gate requires layout extension to be useful,
     and that is the forbidden edit.
   - **Rejected:** edit `NaadfPipelines` to add the entity buffers
     unconditionally. Would violate the brief's hard rule.
   - **Rejected:** wire the entity buffers through `ConstructionGpu`'s
     fields + a separate bind group bound at a higher `@group(N)` slot in
     the renderer. Same hard-rule problem — the renderer's pipeline
     descriptors live in `NaadfPipelines`, editing them is forbidden.
   - **Flip-the-decision fact:** once wave-3 lifts the
     `NaadfPipelines`-edit rule (per `15-design-c.md` §1.7's "the merge
     agent guards this"), inserting the call into `shoot_ray` is a small
     edit (the helpers are already present).

2. **`naadf_entity_update_node` body is gated to no-op until wave-3.** The
   node is in the `Core3d` chain (above `naadf_atmosphere_node` per
   `15-design-c.md` §3) but the body is a `Res<ConstructionConfig>`
   gate that early-returns. With `entities_enabled = false` (the W4
   default), the node executes one bool check per frame.
   - **Rejected:** wire the per-frame dispatch in W4. Would require an
     `Extract`-schedule system to mirror the main-world `EntityHandler`
     state into the render world + a `prepare_construction`-time bind
     group build over `ConstructionGpu`'s entity fields. Both are
     substantial wave-3 integration work; W4's task is the algorithm +
     layouts + types, and the verification cap (≤8 e2e runs) leaves no
     budget for the full wiring stress-test.
   - **Flip-the-decision fact:** the dispatch body lives in
     `entity_update.rs::naadf_entity_update_node` and is one well-defined
     extension point; wave-3 fills it.

3. **W4's per-entity AADF runs the `EntityData.cs:64-106` inline loop,
   not `aadf::bounds::compute_aadf_layer`.** The C# `EntityData` AADF
   format packs 6 × 5-bit-per-axis AADFs into 30 bits of a u32 (the top
   bit is the `0x80000000` full flag); W6's `compute_aadf_layer` operates
   on a different bit layout (`aadf::cell::ChunkCell` 5-bit AADFs at
   different offsets). Reusing W6's helper would require an adaptor that
   re-packs the per-iteration intermediate values, more code than a
   line-for-line port of the small C# kernel.
   - **Rejected:** add a `compute_aadf_packed30` variant to
     `aadf::bounds.rs`. Hard rule: "Do NOT touch W6's `aadf/bounds.rs`
     algorithm." Adapting it here is the cleaner separation.

4. **`compress_quaternion` uses the C# `Math.Abs > maxAbs` tiebreak
   (first-wins), not the HLSL `if (q[maxIndex] < 0)` per-component sign
   check.** Both produce identical packed bytes when the canonical
   `q ~ -q` ambiguity is resolved consistently; we mirror the C#
   `EntityHandler.cs:499-546` (the CPU-side encoder) so the GPU side's
   `decompressQuaternion` reproduces the C# behaviour byte-for-byte. The
   shader-side `compressQuaternion` in `commonRayTracing.fxh` differs
   subtly in the sign-flip step (uses `q[maxIndex] < 0` instead of
   storing `is_neg` from the iteration), but the encoded output is
   equivalent under the `q ~ -q` equivalence — the decompression on the
   GPU side reconstructs a quaternion with the dropped component
   positive, so the encoder's sign choice is irrelevant.

5. **`EntityHandler` is a stateful `pub struct`, not a free function.**
   Mirrors the C# `EntityHandler` class. The state (`chunk_entity_data` +
   `last_frame_chunks` + `current_frame_chunks`) is across-frame, so a
   free function would require the caller to thread the state; the
   struct is the cleaner port. The Resource-ification (`#[derive(Resource)]`
   for render-world insertion) is deferred to wave-3 because the
   render-side wiring chooses where the resource lives.

6. **Dedup-hash uses a linear-scan `Vec` instead of a `HashSet`.** The
   C# uses `HashSet<EntityChunkInstanceHash>` keyed by a custom equality
   comparer. The Rust port mirrors this with a `Vec<(hash, source_ptr,
   new_pointer)>` + linear scan because:
   - The per-frame chunk count is small (≤64 for the test grid).
   - `HashSet<Hash>` keyed by a `(hash, equality-via-content)` custom
     equality requires either a tuple-key (re-hashing the content on
     every probe — defeats the purpose) or a wrapper type with a manual
     `Hash` + `PartialEq` impl that takes a reference to the
     `entity_chunk_instances` pool — borrow-checker friction.
   - The linear scan is `O(unique-chunks-this-frame²)` worst-case, but
     in practice each chunk's entity list is unique, so the dedup hit
     rate is low.
   For large entity counts (>1000) the linear scan would dominate; the
   test fixture stays small per E1 (fixed test grid).

7. **`max_entity_instances = 16384` fixed default.** Per
   `WorldRender.cs:88`. The C# hard-codes this in the
   `entityUpdate.fx:41` `taa_index * 16384` history-ring stride; the W4
   port mirrors it as a `ConstructionConfig` constant +
   `DEFAULT_MAX_ENTITY_INSTANCES` exported const. The history-ring
   buffer is `max_entity_instances * taa_ring_depth` slots; a smaller
   default would silently truncate the ring.

8. **`tests_w4::entity_update_gpu_vs_cpu` validates bit-equality on a
   2×1×1-chunk fixture.** The fixture is the minimum that exercises all
   three entry points (an entity straddling the chunk boundary triggers
   `update_chunks` writes + `copy_entity_chunk_instances` + a non-trivial
   `copy_entity_history` write). The bit-equality check covers
   `entity_chunk_instances_rw[*].data1` + `.data5` (the load-bearing
   bit-packed fields) and the chunks texture's `.x` preservation + `.y`
   update — the W4 contract is "preserve `.x`, write `.y`".

### Assumptions made

- **Bevy 0.19 wgpu accepts `Rg32Uint` as a `STORAGE_BINDING + TEXTURE_BINDING`
  3D texture format with `read_write` storage texture access** (per the
  WebGPU spec `Rg32Uint` is a Tier-1 storage-texture format). Verified by
  the e2e + W1 GPU/CPU bit-exact test both passing under the new format.
- **A `texture_3d<u32>` view binding accepts an `Rg32Uint` texture** (the
  WGSL sampled-texture type only constrains the sample type, not the
  channel count). Verified by every renderer pass running cleanly against
  the widened format with no validation errors and the e2e gate values
  matching pre-W4 baseline exactly (emissive 247.0, solid 242.0, sky 145.9).
- **`.x` preservation in `entity_update.wgsl::update_chunks` is byte-stable
  with respect to a parallel `chunk_calc.wgsl` write at the same texel.**
  In the W4 wave-3 dispatch order, the entity-update node runs *before*
  any construction node touches the chunks texture in a given frame; the
  `textureLoad(chunks).x` read inside `update_chunks` sees the up-to-date
  W1 state, the `textureStore` rewrites only the texel the update points
  at. wgpu serialises compute dispatches inside a queue submission, so
  the read-modify-write is safe.
- **The `EntityHandler.cs:300-304` hash arithmetic — `hash += (int)(coeff[e] * instanceID)`
  — uses C#'s checked-by-default `int` semantics, wrapping on overflow.** The
  port uses `i32 as i64` then `wrapping_add` to mirror the C# behaviour.
  Verified by the `entity_handler_cpu_dedup_two_identical_lists` test
  passing.
- **The chunks texture's pre-W4 e2e gate values stay green under
  `Rg32Uint`.** Verified: emissive 247.0, solid 242.0, sky 145.9 — exact
  match to the W1 baseline.

### Verification

- **Build:** `cargo build -p bevy-naadf` — clean, 0 errors, 0 warnings on
  W4-touched files. (Pre-existing `texture_array/saver.rs:146` lint stays
  as documented in W0/W1/W6.)
- **Tests:** `cargo test -p bevy-naadf --lib` — **87 passed, 1 ignored**
  (W1 baseline 76 → +11 W4 tests):
  - `gpu_types::tests::entity_chunk_instance_layout_guards`
  - `aadf::entity::tests::compress_quaternion_roundtrip`
  - `aadf::entity::tests::compress_quaternion_bit_layout`
  - `aadf::entity::tests::entity_data_cpu_aadf_correctness`
  - `aadf::entity::tests::compress_entity_chunk_instance_packs_fields`
  - `render::construction::entity_handler::tests::entity_hash_coefficients_table`
  - `render::construction::entity_handler::tests::entity_handler_cpu_dedup_single_entity`
  - `render::construction::entity_handler::tests::entity_handler_cpu_dedup_two_identical_lists`
  - `render::construction::entity_handler::tests::entity_handler_clears_stale_chunks`
  - `render::construction::tests_w4::entity_update_pipelines_compile`
  - `render::construction::tests_w4::entity_update_gpu_vs_cpu`
- **Workspace tests:** `cargo test --workspace` — **100 passed, 6 ignored**
  across 10 suites.
- **`cargo run --bin e2e_render`:** PASS. Gate values
  `emissive 247.0, solid 242.0, sky 145.9` — exact match to W1 baseline
  (the chunks-format widening is functionally invisible to renderer reads
  that take `.x`).
- **`cargo run --bin e2e_render -- --validate-gpu-construction`:** PASS,
  exits 0. Output: `GPU construction byte-equal to CPU oracle: 388 bytes compared`
  — identical to W1 (the format flip is transparent to the W1 validation
  path because every comparison takes `.x`).
- **`cargo run --bin e2e_render -- --entities`:** PASS, exits 0. Output:
  `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates`
  — the W4 CPU port runs end-to-end on a 2-frame moving-entity fixture
  with deterministic upload-buffer shapes.
- **e2e run count:** 3 (well within the ≤8 cap):
  1. `cargo run --bin e2e_render` (baseline regression).
  2. `cargo run --bin e2e_render -- --validate-gpu-construction` (W1
     oracle regression).
  3. `cargo run --bin e2e_render -- --entities` (W4 CPU gate).

### Seam contract update (for W2 / W3 / wave-3 merge agent)

W4 modifies the W0 / W1 / W5 / W6 seam in the following ways:

| seam element | pre-W4 state | W4 state |
|---|---|---|
| `ConstructionPipelines` field set | 9 fields (W1+W5) | **14 fields** — added `entity_world_layout`, `construction_entity_layout`, 3 `entity_update_pipeline_*` IDs. The `FromWorld` impl is additive — W2 / W3 can extend without conflict. |
| `ConstructionConfig` fields | 8 (W0..W1) | **9** — added `max_entity_instances` (default 16384). |
| Chunks texture format | `R32Uint` | **`Rg32Uint`** — `.x` = construction state (W1/W2/W3, unchanged), `.y` = entity pointer + counter (W4). |
| Chunks texture upload | `R32Uint` u32 per chunk | `Rg32Uint` `[u32; 2]` per chunk — `.y` is 0 from `prepare_world_gpu` (entity updates populate it via `update_chunks`). |
| `naadf_world_bind_group_layout` view (`texture_3d<u32>`) | unchanged | unchanged — accepts `Rg32Uint` under the `texture_3d<u32>` sampled-view declaration unchanged. |
| `chunks_rw` storage-texture binding (W1's `construction_world_layout`) | `r32uint` | `rg32uint` |
| `ConstructionGpu` entity fields | 6 × `Option<Buffer>::None` | UNCHANGED — W4 ships the layouts + pipelines but does not allocate the production buffers. Wave-3 allocates + builds bind groups in `prepare_construction`. |
| `ConstructionBindGroups::construction_entity` | `None` | UNCHANGED — wave-3 builds the bind group. |
| `Core3d` chain | 14 nodes | **15 nodes** — `naadf_entity_update_node` inserted before `naadf_atmosphere_node`. Body is a gated no-op; the chain is functionally byte-identical to pre-W4 with `entities_enabled = false`. |
| `e2e_render --entities` flag | (does not exist) | **WIRED** — runs the CPU `EntityHandler::update` fixture; exits 0 on PASS. |

**Public API additions** for W2 / W3 / wave-3 to consume:

- `crate::aadf::entity::{compress_quaternion, decompress_quaternion,
  EntityData, compress_entity_chunk_instance, compress_entity_history,
  pack_chunk_update, ENTITY_VOXEL_FULL_FLAG}` — the CPU algorithm layer.
- `crate::render::construction::entity_handler::{EntityHandler,
  EntityUpdateUploads, entity_hash_coefficients}` — the per-frame
  orchestrator port. Wave-3 instantiates an `EntityHandler` as a
  main-world resource + extracts uploads to the render world.
- `crate::render::construction::entity_update::{entity_world_layout_descriptor,
  construction_entity_layout_descriptor, GpuEntityUpdateParams,
  queue_*_pipeline_with_handle, dispatch_*, naadf_entity_update_node,
  ENTITY_UPDATE_SHADER, ENTITY_UPDATE_SHADER_SRC}` — the GPU dispatch
  layer.
- `crate::render::gpu_types::{GpuEntityChunkInstance, GpuEntityInstanceHistory,
  GpuChunkUpdate, EntityInstance}` — the GPU + CPU structs.
- `crate::render::construction::config::DEFAULT_MAX_ENTITY_INSTANCES` —
  the `WorldRender.cs:88` cap constant.
- `crate::render::construction::validate_entity_handler()` — the
  `--entities` flag entry point.

### Integration notes for the merge agent

1. **W3's `.x` sweep.** The W3 brief was instructed to use `.x` selection
   forward-compat on every chunks-texture read site (`15-design-c.md`
   §1.7). If W3 has landed before this merge, the merge agent should
   audit any new `assets/shaders/bounds_calc.wgsl` chunks reads to
   confirm `.x` is present; the W4 brief flags this explicitly. If W3
   did NOT pre-emptively add `.x`, the agent must sweep those sites in
   W3's WGSL in this merge.

2. **Renderer-side entity sub-traversal wiring (wave-3 follow-up).** The
   helpers are in `ray_tracing.wgsl`; the invocation from `shoot_ray` is
   deferred because it requires extending `NaadfPipelines::world_layout`
   (forbidden by the W4 brief). Wave-3 should:
   - Extend `world_layout` with 3 new bindings:
     `entity_chunk_instances` (ro storage), `entity_voxel_data` (ro
     storage), `entity_instances_history` (ro storage).
   - Add an `#ifdef ENTITIES`-style shader-def or unconditionally insert
     the entity sub-traversal branch into `shoot_ray` (see the
     `commonRayTracing.fxh` reference C# code at lines 81-240).
   - Bind the three buffers from `ConstructionGpu` (W4 owns the fields;
     they stay `Option<Buffer>::None` until populated).

3. **`naadf_entity_update_node` body wiring (wave-3 follow-up).** The
   node is in the chain; the body is a gated no-op. Wave-3 should:
   - Add an `Extract` system that mirrors main-world `EntityHandler`
     state (via `EntityUpdateUploads` resource) into the render world.
   - In `prepare_construction`, allocate the four W4 buffers
     (`entity_chunk_instances`, `entity_voxel_data`,
     `entity_instances_history` GPU + the 3 dynamic upload buffers) on
     `entities_enabled = true`, upload the extracted `EntityUpdateUploads`
     to the dynamic buffers, build `ConstructionBindGroups::construction_entity`.
   - Fill `naadf_entity_update_node::body` with the three dispatch calls
     in order (`dispatch_update_chunks` → `dispatch_copy_entity_chunk_instances`
     → `dispatch_copy_entity_history`).

4. **No conflicts expected outside `construction/` + `gpu_types.rs`** —
   the entity track is bounded to the construction sub-module +
   `gpu_types.rs` + the `prepare.rs` format flip + the chain insert in
   `render/mod.rs` + the e2e flag. Any merge conflict outside these
   files signals an unexpected shared edit per the §1.3 seam contract.

5. **Test count growth.** W4 adds 11 tests (76 baseline → 87). The
   merge agent's post-merge `cargo test -p bevy-naadf --lib` should
   yield 87 + sum of any prior W2/W3 test additions.
