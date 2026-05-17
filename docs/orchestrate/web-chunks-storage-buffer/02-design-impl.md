# 02 — Design + self-review + implementation log

## Design

### 1. The new `WorldGpu` shape (`render/prepare.rs:55-90`)

Drop `chunks: Texture` and `chunks_view: TextureView`. Add `chunks_buffer: Buffer`
plus cached `chunks_size_in_chunks: UVec3` so the 24 `world_gpu.chunks.size()`
read sites in `construction/mod.rs` keep their shape with one line each (call
becomes `world_gpu.chunks_size_in_chunks.x` etc.).

```rust
#[derive(Resource)]
pub struct WorldGpu {
    /// The chunk layer — `array<vec2<u32>>` storage buffer, flat-indexed by
    /// `flatten_index(chunk_pos, sx, sx*sy)` where `sx = size_in_chunks.x`,
    /// `sy = size_in_chunks.y`. `.x` carries the block-state pointer + AADF
    /// (W1/W2/W3); `.y` carries the entity pointer + counter (W4).
    pub chunks_buffer: Buffer,
    /// World-extent cache for the (size().width, size().height,
    /// size().depth_or_array_layers) shape readers in `construction/mod.rs`.
    pub chunks_size_in_chunks: UVec3,
    pub blocks: GrowableBuffer<u32>,
    pub voxels: GrowableBuffer<u32>,
    pub voxel_types: GrowableBuffer<GpuVoxelType>,
    pub world_meta: Buffer,
    pub bind_group: BindGroup,
    pub entity_chunk_instances_placeholder: Buffer,
    pub entity_voxel_data_placeholder: Buffer,
    pub entity_instances_history_placeholder: Buffer,
}
```

The decision to cache `chunks_size_in_chunks` rather than re-read it from
`world_meta` is one of API friction: `world_meta` is a `Buffer` (no CPU
shadow), the size is invariant for the world's lifetime, and 24 sites that
already cluster `[width, height, depth_or_array_layers]` calls would otherwise
need a parallel `WorldData` (the staging resource) or a new `ConstructionGpu`
sibling field. Cached `UVec3` is the smallest change.

### 2. Bind-group layout deltas (4 layouts)

#### 2a. `pipelines.rs:312-331` — `world_layout` slot 0 (render-side, read-only)

```rust
// Was:
texture_3d(TextureSampleType::Uint),
// Now:
storage_buffer_read_only_sized(false, None),   // chunks: array<vec2<u32>>, read
```

Import-list edit: drop `texture_3d, TextureSampleType` (line 43 + line 48).

#### 2b. `construction/chunk_calc.rs:60-88` — `construction_world_layout` slot 0

```rust
// Was:
texture_storage_3d(TextureFormat::Rg32Uint, StorageTextureAccess::ReadWrite),
// Now:
storage_buffer_sized(false, None),     // chunks_rw: array<vec2<u32>>, read_write
```

Import-list edit: drop `texture_storage_3d`, `StorageTextureAccess`,
`TextureFormat` (lines 35–36, 40).

#### 2c. `construction/bounds_calc.rs:70-89` — `construction_bounds_world_layout` slot 0

Same flip; drop `texture_storage_3d`, `StorageTextureAccess`, `TextureFormat`
imports (lines 40, 43).

#### 2d. `construction/entity_update.rs:78-94` — `entity_world_layout` slot 0

Same flip; drop `texture_storage_3d`, `StorageTextureAccess`, `TextureFormat`
imports (lines 30, 35).

#### 2e. `construction/mod.rs:1824-1839` — the inline rebuilt `world_layout` (entity-enabled rebuild)

This is a copy of `pipelines.rs` `world_layout` that the
`prepare_construction` system uses when rebuilding the world bind group with
the real W4 buffers. Its slot 0 flips from `texture_3d(TextureSampleType::Uint)`
to `storage_buffer_read_only_sized(false, None)`. Drop the
`texture_3d, TextureSampleType` imports inline.

### 3. WGSL binding declarations (6 shaders)

Every shader pulls `flatten_index` from `common.wgsl` via naga-oil's
`#import "shaders/common.wgsl"::flatten_index`. `ray_tracing.wgsl` already
imports it.

#### 3a. `world_data.wgsl:54` (read-side)

```wgsl
@group(0) @binding(0) var<storage, read> chunks: array<vec2<u32>>;
```

#### 3b. `ray_tracing.wgsl:283-295` (read site)

```wgsl
// chunk_pos is vec3<u32>; flatten_index expects vec3<u32>.
let chunk_idx = flatten_index(
    chunk_pos,
    world_meta.size_in_chunks.x,
    world_meta.size_in_chunks.x * world_meta.size_in_chunks.y,
);
let chunk_texel = chunks[chunk_idx];      // vec2<u32>, not vec4<u32>
var cur_node: u32 = chunk_texel.x;
// …
let entity_pointer_and_size = chunk_texel.y;
```

#### 3c. `chunk_calc.wgsl:96-97` (binding) and `:414` (write)

Binding:
```wgsl
@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
```

Add `#import "shaders/common.wgsl"::flatten_index` at the file's import block.

Write site (line 414, single thread `local_index == 0`):
```wgsl
let chunk_idx = flatten_index(
    vec3<u32>(chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
chunks[chunk_idx] = vec2<u32>(state, 0u);
```

`.y = 0u` is correct here per `15-design-c.md` §1.7 — this write fires at
chunk-build time, before any entities exist; the entity-update pass writes
`.y` later.

#### 3d. `bounds_calc.wgsl:98` (binding) + `:210, :357` (reads) + `:394` (write)

Binding:
```wgsl
@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
```

Add `#import "shaders/common.wgsl"::flatten_index`.

Read at `:210` (`add_bounds_group` neighbour-check):
```wgsl
let neighbour_idx = flatten_index(
    vec3<u32>(neighbour_chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
let neighbour_x = chunks[neighbour_idx].x;
```

Read at `:357` (`compute_group_bounds` per-chunk load):
```wgsl
let cur_chunk_idx = flatten_index(
    vec3<u32>(chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
let cur_chunk_full = chunks[cur_chunk_idx];
let cur_chunk_load = cur_chunk_full.x;
let entity_y = cur_chunk_full.y;
```

Write at `:394` (`.y`-preserving):
```wgsl
chunks[cur_chunk_idx] = vec2<u32>(cur_chunk, entity_y);
```

`cur_chunk_idx` is reused from the load above (same chunk-pos).

#### 3e. `entity_update.wgsl:76` (binding) + `:107` (read) + `:108` (write)

Binding:
```wgsl
@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
```

`entity_update.wgsl` does NOT currently import `flatten_index`. Add the
import. But `entity_update.wgsl` does NOT have access to `params.size_in_chunks`
in its `EntityUpdateParams` (this struct only carries
`entity_instance_count, entity_chunk_instance_count, taa_index, update_count,
max_entity_instances`). Two options here:

1. Add `size_in_chunks: vec3<u32>` (with one u32 pad) to `EntityUpdateParams`
   (Rust + WGSL). The Rust struct grows by 16 bytes (from 32 to 48) — still
   `#[repr(C)] Pod, Zeroable`.
2. Compute `flatten_index` directly inline.

Decision: option 1. The audit's "no greenfield" rule applies to helpers, not
data fields; widening the existing uniform by 16 bytes is the smallest change
and keeps `flatten_index` invocation uniform across the 4 construction
shaders. The Rust `GpuEntityUpdateParams` already has 12 bytes of pad bits;
we redirect them to `size_in_chunks: u32x3 + _pad: u32`. The CPU writer is
`prepare_construction`, which already knows `world_gpu.chunks_size_in_chunks`.

```wgsl
// in EntityUpdateParams:
struct EntityUpdateParams {
    entity_instance_count: u32,
    entity_chunk_instance_count: u32,
    taa_index: u32,
    update_count: u32,
    max_entity_instances: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // NEW: world size for chunks-buffer flatten.
    size_in_chunks: vec3<u32>,
    _pad3: u32,
};
```

Read + write at `:107-108`:
```wgsl
let chunk_idx = flatten_index(
    vec3<u32>(chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
let old = chunks[chunk_idx];
chunks[chunk_idx] = vec2<u32>(old.x, update.y);
```

Rust mirror — `crates/bevy_naadf/src/render/construction/entity_update.rs`:

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, Default)]
pub struct GpuEntityUpdateParams {
    pub entity_instance_count: u32,
    pub entity_chunk_instance_count: u32,
    pub taa_index: u32,
    pub update_count: u32,
    pub max_entity_instances: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
    pub size_in_chunks: [u32; 3],
    pub _pad3: u32,
}
const _: () = assert!(std::mem::size_of::<GpuEntityUpdateParams>() == 48);
```

Update the 2 production writers in `construction/mod.rs` (search "entity_update_params"
prepare site) and the W4 test fixture in `construction/mod.rs:4542-4551` to
populate `size_in_chunks`.

#### 3f. `world_change.wgsl:110` (binding) + `:317, :443` (reads) + `:376, :445` (writes)

Binding:
```wgsl
@group(0) @binding(0)
var<storage, read_write> chunks: array<vec2<u32>>;
```

Add `#import "shaders/common.wgsl"::flatten_index`.

`world_change.wgsl` has `params: ConstructionParams` with `size_in_chunks`
already on the uniform — no struct change needed.

Read+write site #1 (`apply_group_change` — line 317 read, line 376 write,
preserves `.y`):
```wgsl
let chunk_idx = flatten_index(
    vec3<u32>(chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
let cur_chunk_load = chunks[chunk_idx];
// ...
chunks[chunk_idx] = vec2<u32>(new_chunk_x, cur_chunk_y);
```

Read+write site #2 (`apply_chunk_change` — line 443 read, line 445 write,
preserves `.y`):
```wgsl
let chunk_idx = flatten_index(
    vec3<u32>(chunk_pos),
    params.size_in_chunks.x,
    params.size_in_chunks.x * params.size_in_chunks.y,
);
let cur = chunks[chunk_idx];
chunks[chunk_idx] = vec2<u32>(change.y, cur.y);
```

### 4. `flatten_index` call-site shape

All 4 construction shaders pass `params.size_in_chunks`; `ray_tracing.wgsl`
passes `world_meta.size_in_chunks`. Signature:

```wgsl
flatten_index(pos, stride_y, stride_z) -> u32
// idx = pos.z * stride_z + pos.y * stride_y + pos.x
// stride_y = size_in_chunks.x       (row stride)
// stride_z = size_in_chunks.x * size_in_chunks.y  (plane stride)
```

This matches `entity_handler.rs:339-345` inverse byte-for-byte.

### 5. `prepare.rs:251-307` buffer creation diff

Replace the texture allocation + `write_texture` with:

```rust
let chunk_count = (size.x * size.y * size.z) as usize;
let chunks_data_paired: Vec<[u32; 2]> = if gpu_producer_skip_upload {
    vec![[0u32, 0u32]; chunk_count]
} else {
    let mut chunk_data_single = extracted.chunks.clone();
    chunk_data_single.resize(chunk_count, 0);
    let mut paired: Vec<[u32; 2]> = Vec::with_capacity(chunk_count);
    for c in chunk_data_single.iter().copied() {
        paired.push([c, 0u32]);
    }
    paired
};
let chunks_buffer_size = (chunk_count as u64) * 8; // 8 B per [u32; 2]
let chunks_buffer = render_device.create_buffer(&BufferDescriptor {
    label: Some("naadf_chunks"),
    size: chunks_buffer_size,
    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
    mapped_at_creation: false,
});
render_queue.write_buffer(&chunks_buffer, 0, bytemuck::cast_slice(&chunks_data_paired));
```

`COPY_SRC` retained so the construction-mod readback paths can still
`copy_buffer_to_buffer` for CPU/GPU comparison.

Bind-group construction at `:448-461` flips:

```rust
&BindGroupEntries::sequential((
    chunks_buffer.as_entire_buffer_binding(),   // was: &chunks_view
    blocks.buffer().as_entire_buffer_binding(),
    voxels.buffer().as_entire_buffer_binding(),
    voxel_types.buffer().as_entire_buffer_binding(),
    world_meta.as_entire_buffer_binding(),
    placeholder_entity_chunk_instances.as_entire_buffer_binding(),
    placeholder_entity_voxel_data.as_entire_buffer_binding(),
    placeholder_entity_instances_history.as_entire_buffer_binding(),
)),
```

And the `WorldGpu` ctor switches to set `chunks_buffer` + `chunks_size_in_chunks: size`.
The `chunks_view = chunks.create_view(...)` line at `:308` is deleted.

### 6. `construction/mod.rs:1520-1551` — construction-world bind group

The "separate `TextureView`" comment block (`:1521-1530`) is now stale —
storage buffers don't have a "view-recorded-access" hazard. Replace the
chunks_storage_view creation with `world_gpu.chunks_buffer.as_entire_buffer_binding()`.

The W4 wave-3 entity-enabled rebuild at `:1841-1857` also flips the same way.

### 7. Readback simplification

#### 7a. `construction/mod.rs:3599-3669` — `readback_chunks_texture`

Rename to `readback_chunks_buffer`. New body:

```rust
fn readback_chunks_buffer(
    device: &RenderDevice,
    queue: &RenderQueue,
    chunks_buffer: &Buffer,
    size: [u32; 3],
) -> Vec<u32> {
    let chunk_count = (size[0] * size[1] * size[2]) as u64;
    let staging_size = chunk_count * 8; // 8 B per [u32; 2]
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("w1_chunks_readback_staging"),
        size: staging_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("w1_chunks_readback"),
    });
    encoder.copy_buffer_to_buffer(chunks_buffer, 0, &staging, 0, staging_size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    let raw = slice.get_mapped_range();
    let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
    let out: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
    drop(raw);
    staging.unmap();
    assert_eq!(out.len() as u64, chunk_count);
    out
}
```

Same delta for `bounds_calc/tests.rs:330-397` `readback_chunks_texture` and
`world_change.rs:577-651` `read_chunks_texture` (the latter returns
`Vec<[u32; 2]>` so the `.x`/`.y` pair is preserved; the new body just
`copy_buffer_to_buffer` + `bytemuck::cast_slice` returns the pairs flat).

#### 7b. `construction/mod.rs:4711-4779` — the W4 entity-update test's chunks readback

This block already does row-padded readback into `raw` and walks
`bytes_per_row` to find each chunk's texel. Flip to:

```rust
let staging_size = (size_in_chunks[0] * size_in_chunks[1] * size_in_chunks[2]) as u64 * 8;
let chunks_staging = device.create_buffer(&BufferDescriptor { … size: staging_size, … });
encoder.copy_buffer_to_buffer(&chunks_buffer, 0, &chunks_staging, 0, staging_size);
// … map …
let raw = slice.get_mapped_range();
let pairs: &[[u32; 2]] = bytemuck::cast_slice(&raw);
for upd in &cpu_uploads.chunk_updates {
    let cx = upd.data1 & 0x7FF;
    let cy = (upd.data1 >> 11) & 0x3FF;
    let cz = upd.data1 >> 21;
    let chunk_idx_in_world =
        (cx + cy * size_in_chunks[0] + cz * size_in_chunks[0] * size_in_chunks[1]) as usize;
    let xy = pairs[chunk_idx_in_world];
    // assert .x preserved + .y written, as before.
}
```

### 8. Test fixture migrations (5 sites)

Each fixture re-creates a chunks resource. All 5 flip lockstep:

| Site | File:Line | Allocation pattern |
|---|---|---|
| S1 — production `prepare_world_gpu` | `render/prepare.rs:251-308` | Texture → Buffer (covered §5) |
| S2 — W1 `validate_gpu_construction` | `render/construction/mod.rs:2473-2515` | Texture → Buffer; drop `chunks_view = …`; bind buffer instead of view |
| S3 — W3 W3-fixture | `render/construction/bounds_calc/tests.rs:480-523` | Same shape; `W3Fixture.chunks_texture / chunks_view` → `chunks_buffer` |
| S4 — W2 W2-fixture | `render/construction/world_change.rs:662-678` | Same shape; `W2Fixture.chunks_texture` → `chunks_buffer`. The `apply_chunk_edit_cpu_gpu_bit_exact` sentinel-seeding `write_texture` at `:921-939` becomes `write_buffer` at flat offset `target_idx * 8` |
| S5 — W1 `gpu_algorithm1_vs_cpu_bit_exact` | `render/construction/mod.rs:3819-3866` | Texture → Buffer; bind buffer |
| S6 — W4 entity-update test | `render/construction/mod.rs:4453-4496` | Texture → Buffer; bind buffer |

(S1 is the production seam; S2–S6 are the 5 fixtures the brief enumerates;
S6 is in fact the entity-update test, which the audit notes is the "fifth
site".)

For S4's sentinel seed (`world_change.rs:920-939`), replace with:

```rust
let target_idx = 2usize + 1 * 4 + 0 * 16; // chunk (2,1,0) flat index
let sentinel_pair = [0u32, sentinel_y];
fx.queue.write_buffer(
    &fx.chunks_buffer,
    (target_idx * 8) as u64,
    bytemuck::cast_slice(&[sentinel_pair]),
);
```

### 9. Exhaustive call-site mapping

Every `textureLoad`/`textureStore` on `chunks` → buffer index. File:line
references:

| WGSL site | Op | Becomes |
|---|---|---|
| `ray_tracing.wgsl:286` | `textureLoad(chunks, vec3<i32>(chunk_pos), 0)` | `chunks[flatten_index(chunk_pos, world_meta.size_in_chunks.x, world_meta.size_in_chunks.x * world_meta.size_in_chunks.y)]` (returns `vec2<u32>` not `vec4<u32>`; `.x`/`.y` selectors still valid) |
| `chunk_calc.wgsl:414` | `textureStore(chunks, vec3<i32>(chunk_pos), vec4<u32>(state, 0u, 0u, 0u))` | `chunks[flatten_index(vec3<u32>(chunk_pos), params.size_in_chunks.x, params.size_in_chunks.x * params.size_in_chunks.y)] = vec2<u32>(state, 0u)` |
| `bounds_calc.wgsl:210` | `textureLoad(chunks, neighbour_chunk_pos).x` | `chunks[flatten_index(vec3<u32>(neighbour_chunk_pos), …)].x` |
| `bounds_calc.wgsl:357` | `textureLoad(chunks, chunk_pos)` (vec4) | `chunks[flatten_index(vec3<u32>(chunk_pos), …)]` (vec2) |
| `bounds_calc.wgsl:394` | `textureStore(chunks, chunk_pos, vec4<u32>(cur_chunk, entity_y, 0u, 0u))` | `chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y)` (reuses the chunk_idx from the load above; preserves `.y` per W4) |
| `entity_update.wgsl:107` | `textureLoad(chunks, chunk_pos)` | `chunks[flatten_index(vec3<u32>(chunk_pos), params.size_in_chunks.x, params.size_in_chunks.x * params.size_in_chunks.y)]` |
| `entity_update.wgsl:108` | `textureStore(chunks, chunk_pos, vec4<u32>(old.x, update.y, 0u, 0u))` | `chunks[chunk_idx] = vec2<u32>(old.x, update.y)` (`.x`-preserve, `.y`-overwrite — W4 contract) |
| `world_change.wgsl:317` | `textureLoad(chunks, chunk_pos)` | `chunks[flatten_index(vec3<u32>(chunk_pos), params.size_in_chunks.x, params.size_in_chunks.x * params.size_in_chunks.y)]` |
| `world_change.wgsl:376` | `textureStore(chunks, chunk_pos, vec4<u32>(new_chunk_x, cur_chunk_y, 0u, 0u))` | `chunks[chunk_idx] = vec2<u32>(new_chunk_x, cur_chunk_y)` (`.y`-preserve — W2 contract) |
| `world_change.wgsl:443` | `textureLoad(chunks, chunk_pos)` | `chunks[flatten_index(vec3<u32>(chunk_pos), params.size_in_chunks.x, params.size_in_chunks.x * params.size_in_chunks.y)]` |
| `world_change.wgsl:445` | `textureStore(chunks, chunk_pos, vec4<u32>(change.y, cur.y, 0u, 0u))` | `chunks[chunk_idx] = vec2<u32>(change.y, cur.y)` (`.x`-overwrite, `.y`-preserve — W2 contract) |

## Decisions & rejected alternatives

1. **Plain `Buffer` vs `GrowableBuffer<[u32; 2]>`.** Picked plain `Buffer`.
   Rejected `GrowableBuffer<[u32; 2]>`: `chunks` is fixed-size at world build
   (sized at `size_in_chunks.x * .y * .z`), it never grows, and
   `GrowableBuffer`'s capacity/headroom semantics are dead code for this use
   case. The audit's borderline analysis (§"Borderline calls") explicitly
   rejected it and called out the consistency-with-`blocks`/`voxels` argument
   as outweighed by the fixed-size nature. The audit-cited precedent the
   migration mirrors is `prepare.rs:404-441` (`world_meta` uniform +
   `placeholder_entity_*` placeholders), all of which use plain
   `device.create_buffer` + `queue.write_buffer`.

2. **Inline stride math vs precomputed `plane_stride` uniform field.** Picked
   inline math — `flatten_index(p, size_in_chunks.x, size_in_chunks.x *
   size_in_chunks.y)`. Rejected adding a `chunks_plane_stride: u32` field to
   `GpuConstructionParams` / `GpuWorldMeta` (or computing it once into the
   uniform). Why: the audit (§"User decisions") chose "read
   `world_meta.size_in_chunks` inline at each call site" lockstep with the
   existing `chunk_calc.wgsl:347` precedent (`params.segment_size_in_chunks`).
   The multiplication is cheap, the uniform is already laid out, and adding
   a stride field would force a `repr(C)` shuffle across two structs + a CPU
   writer change everywhere.

3. **`array<vec2<u32>>` element type vs `array<u32>` flat layout with 2×
   indexing.** Picked `array<vec2<u32>>`. Rejected `array<u32>` with `i*2`
   / `i*2+1` indexing: the codebase already has multiple
   `array<vec2<u32>>` storage-buffer precedents (`pipelines.rs:350-353`,
   `world_change.wgsl:131`, `entity_update.wgsl:81`); the
   `[u32; 2]` ↔ `vec2<u32>` mapping is proven; the readback `bytemuck::cast_slice`
   already speaks `[[u32; 2]]`. The 2× indexing form would cost a refactor at
   every read/write site and gain nothing.

4. **Drop `WorldGpu.chunks` + `chunks_view` outright vs keep alongside.**
   Picked hard cut. Rejected dual-store ("for compatibility"): the brief's
   forbidden moves §6 explicitly bars compatibility shims; partial migration
   leaves wgpu unable to bind both descriptor shapes to the same allocation;
   every consumer references ONE resource going forward.

5. **Migrate all 5 fixtures lockstep vs scope production-only.** Picked
   lockstep. Rejected production-only: `cargo test` would break the moment
   the 3 construction layout descriptors flip (`construction_world_layout`
   used by all 3 W1 / W2 / W3 fixtures; `entity_world_layout` used by W4
   fixture). The user explicitly chose lockstep in the Q&A.

6. **Element-stride safety of `as_entire_buffer_binding()` for 8 B
   elements.** Picked `as_entire_buffer_binding()`. Verified against
   `pipelines.rs:350-353` (the frame-data layout already binds 4 different
   `array<vec2<u32>>` / `array<vec4<u32>>` storage buffers with
   `as_entire_buffer_binding()` against `storage_buffer_sized(false, None)`).
   wgpu computes element-stride from the WGSL declaration (`vec2<u32>` = 8 B
   stride); the `None` size in `storage_buffer_sized(false, None)` defers
   to the binding's actual length. No 16 B alignment surprise — the
   audit-cited precedents are the proof.

7. **Where to source `size_in_chunks` in `entity_update.wgsl`.** Picked
   widen `EntityUpdateParams` by 16 B (adds `size_in_chunks: vec3<u32> +
   pad`). Rejected (a) re-using `params.entity_chunk_instance_count` etc. as
   a coordinate basis (not a coordinate basis); (b) creating a second
   uniform binding (more bind-group surgery + a new layout slot); (c)
   computing flatten_index by re-reading the chunks-pos-encoding bit layout
   inline (legible but easy to drift across shaders). Widening the existing
   uniform with one CPU-side write is the smallest change. The Rust struct
   size goes 32 → 48 B; the `_pad{0,1,2}` are kept in their existing slots
   for naga-oil's vec3-then-scalar safety.

8. **Stale-comment doc-string maintenance scope.** Picked: update only the
   doc strings adjacent to the flipped code (file headers + the binding
   declarations); do NOT rewrite design-doc references to "the chunks
   texture" globally — those references describe the historical W4
   widening, and the migration is a representation change, not a semantic
   one. The headers note "`array<vec2<u32>>` (W4 representation; was
   `texture_storage_3d<rg32uint, read_write>` pre-WebGPU-spec migration)"
   so future readers see both names.

## Assumptions made

1. **WGSL `var<storage, read_write> chunks: array<vec2<u32>>` produces the
   same binding layout as the existing `chunk_updates_dynamic` precedent
   (`entity_update.wgsl:81`).** Verified by direct file comparison: the
   precedent's Rust side is `storage_buffer_read_only_sized(false, None)`
   (the read-only equivalent in `construction_entity_layout_descriptor`,
   `entity_update.rs:111`); the proposed `chunks` rw binding mirrors the
   `chunk_updates_dynamic` ro binding using `storage_buffer_sized` for
   read-write access. If the implementation surfaces a layout mismatch, the
   verification step is to grep `storage_buffer_sized` for rw-vs-ro use and
   confirm the WGSL access mode matches.

2. **naga-oil's `#import common::flatten_index` already works in
   `ray_tracing.wgsl` and will work in the 4 construction shaders too.**
   Verified for `ray_tracing.wgsl:35` (already in place). For the 4
   construction shaders, naga-oil's `#import` directive is the same
   composable-module syntax the project already uses (`world_data.wgsl`
   imports, `world_change.wgsl` precedent); if any of these compile-fail
   under WGSL composition, the fallback is to inline-copy `flatten_index`
   into each shader (the audit notes the inline-copy pattern at
   `chunk_calc.wgsl:348-349`, `world_change.wgsl:305-308`). Trigger to verify:
   `cargo build --workspace` failing with a naga-oil composer error on the
   new `#import` line; remediation = inline-copy and add an
   `inline_matches_ref` drift guard.

3. **The W4 `.y`-preserve writes truly preserve only the channel they
   should.** Cross-checked against the audit's `## W4 design-doc trace` and
   the WGSL files themselves:
   - `chunk_calc.wgsl:414`: `.y = 0u` is correct (writes at chunk-build time;
     no entities yet).
   - `bounds_calc.wgsl:394`: preserves `.y` via `entity_y` read from line
     `:360`.
   - `entity_update.wgsl:108`: preserves `.x` via `old.x` (read at `:107`);
     overwrites `.y` with `update.y` (the entity-pointer payload).
   - `world_change.wgsl:376`: preserves `.y` via `cur_chunk_y` (read at
     `:319`).
   - `world_change.wgsl:445`: preserves `.y` via `cur.y` (read at `:443`).

   Each write site loads the texel before writing — the load-modify-write
   pattern carries forward identically under the buffer representation.

4. **Cached `chunks_size_in_chunks` on `WorldGpu` survives across the
   `prepare_construction` system's run.** `WorldGpu` is created by
   `prepare_world_gpu` build-once and never resized after world build
   (matches the brief's "fixed-size at world build" property). The 24
   `world_gpu.chunks.size()` consumers in `construction/mod.rs` read it as
   a tuple `[width, height, depth]` and treat it as invariant; the
   migration's `chunks_size_in_chunks: UVec3` mirrors that.

5. **`BufferUsages::COPY_SRC` on the chunks buffer is needed for the 3
   readback paths (`construction/mod.rs:3599`, `world_change.rs:578`,
   `bounds_calc/tests.rs:330`).** Yes — wgpu's `copy_buffer_to_buffer`
   requires the source to have `COPY_SRC` usage. The production texture
   today does NOT have `TextureUsages::COPY_SRC` (`prepare.rs:262-264` is
   `TEXTURE_BINDING | COPY_DST | STORAGE_BINDING` only), so the production
   build-once chunks buffer does not need `COPY_SRC` if no production
   code-path reads it back. **But** the brief's required reading list flags
   the readback in `construction/mod.rs:3599-3666` as a target site
   (`readback_chunks_texture` lives in the unit-test module `tests_w1`, fed
   by a test-fixture `chunks_texture` — same shape as the production seam).
   I'll add `COPY_SRC` to the production chunks buffer too — cheap, future-
   proof for the soft-deferred readback follow-up. The 5 fixture allocations
   already carry `COPY_SRC` (they need it for their readbacks).

## Independent review (2026-05-17)

### Linear-index formula re-derivation

The audit cites `entity_handler.rs:339-345`:
```rust
fn chunk_index_to_pos(idx: u32, size_in_chunks: [u32; 3]) -> [u32; 3] {
    let sx = size_in_chunks[0];
    let sy = size_in_chunks[1];
    let z = idx / (sx * sy);
    let rem = idx % (sx * sy);
    let y = rem / sx;
    let x = rem % sx;
    [x, y, z]
}
```

This is the inverse of `idx = z * sx * sy + y * sx + x`. The audit's
`common.wgsl:32-34` provides the helper:
```wgsl
fn flatten_index(pos: vec3<u32>, stride_y: u32, stride_z: u32) -> u32 {
    return pos.z * stride_z + pos.y * stride_y + pos.x;
}
```

Substituting `stride_y = sx, stride_z = sx * sy`:
`flatten_index(p, sx, sx*sy) = p.z*sx*sy + p.y*sx + p.x` — matches
`chunk_index_to_pos`'s inverse 1:1. Direction confirmed.

### `chunks_view` consumer grep

After the migration, no `chunks_view` reference should remain. Sites the
design replaces:
- `prepare.rs:61` (field decl), `:308` (creation), `:452` (bind), `:465` (struct field write) — all replaced.
- `construction/mod.rs:1175, :1531-1540, :1845, :2515-2565, :3861-3900, :4496-4614` — all replaced (either with `world_gpu.chunks_buffer.as_entire_buffer_binding()` or with the new buffer-form fixture).
- `construction/entity_update.rs:351` — replaced with `world_gpu.chunks_buffer.as_entire_buffer_binding()`.
- `construction/world_change.rs:678, :826` — replaced.
- `construction/bounds_calc/tests.rs:452, :523, :613, :637, :776, :871, :964` — `W3Fixture.chunks_view` field removed; bind-group construction uses `chunks_buffer.as_entire_buffer_binding()`.

The post-impl grep should return zero matches.

### Staging-buffer alignment check

The buffer→buffer copy `encoder.copy_buffer_to_buffer` requires the COPY
size to be a multiple of `COPY_BUFFER_ALIGNMENT` (4 B). Our staging-buffer
size is `chunk_count * 8` — always a multiple of 8 (which is a multiple of
4). No 256 B row alignment needed (that was a texture-readback constraint
only). The size assertion is `assert_eq!(out.len() as u64, chunk_count)`
post-cast, which holds when the staging buffer is exactly `chunk_count * 8`
bytes and the cast slices it into `chunk_count` `[u32; 2]` pairs.

### `.y`-preserve audit (write-by-write)

| Site | New write expression | `.x` | `.y` | OK? |
|---|---|---|---|---|
| `chunk_calc.wgsl:414` | `chunks[idx] = vec2<u32>(state, 0u)` | overwritten (intent) | zero (intent — chunk-build time, no entities) | ✓ |
| `bounds_calc.wgsl:394` | `chunks[idx] = vec2<u32>(cur_chunk, entity_y)` | overwritten (intent) | preserved via prior `chunks[idx].y` load | ✓ |
| `entity_update.wgsl:108` | `chunks[idx] = vec2<u32>(old.x, update.y)` | preserved via prior `chunks[idx].x` load | overwritten with new entity pointer | ✓ |
| `world_change.wgsl:376` | `chunks[idx] = vec2<u32>(new_chunk_x, cur_chunk_y)` | overwritten (intent) | preserved via prior `chunks[idx].y` load | ✓ |
| `world_change.wgsl:445` | `chunks[idx] = vec2<u32>(change.y, cur.y)` | overwritten (intent) | preserved via prior `chunks[idx].y` load | ✓ |

Every write site loads the existing chunk before writing — the
load-modify-write pattern survives the texture→buffer migration unchanged.

### High-risk items

- **`EntityUpdateParams` widening (Decision #7).** This is the one place I
  ship a non-mechanical change: Rust struct grows from 32 to 48 B, GPU-side
  uniform-buffer-binding stays the same but the data write site must include
  the new `size_in_chunks` field, and the existing `assert!(size_of == 32)`
  guard at `entity_update.rs:71` must be updated to `48`. **Self-rated
  risk: medium.** Failure mode is uniform-buffer alignment / Pod-Zeroable
  drift on the Rust side or a missed CPU writer (the test fixture at
  `mod.rs:4542-4551` writes `GpuEntityUpdateParams` byte-for-byte; this
  fixture must update too). Detection: the W4 fixture in
  `mod.rs:4453-4783` fails its assertion if `size_in_chunks` is zero — but
  it might pass spuriously since with `size_in_chunks=0,0,0` `flatten_index`
  collapses to `pos.x`. Test grid 4×4×4 has 64 chunks at indices 0..63;
  with zero size the indices collide. **Mitigation:** verified by the
  bit-exact `.x`/`.y` assertion in the W4 test (`mod.rs:4767-4776`); a
  collapsed-flatten would write to wrong indices and the assertion would
  see the WRONG chunk's `.y` written and the RIGHT chunk's `.y` unchanged.
  Self-certify, but flag for the reviewer to double-check that
  `mod.rs:4542-4551` populates `size_in_chunks: size_in_chunks`.

- **Staging-buffer-size correctness for short worlds.** For a 1×1×1 world,
  `chunk_count = 1`, staging size = 8 B — well above wgpu's 4 B copy
  alignment minimum but below the implicit-256 B padding texture readback
  used to need. Direct buffer→buffer copy does NOT need padding. **Risk:
  low.** Detection: `cargo test --workspace --lib` exercises the 1×1×1
  validate path via `validate_gpu_construction`. Self-certify.

- **`world_change.wgsl:317` is inside `apply_group_change` (a 4×4×4
  workgroup) where every chunk has a unique flat index → no race.**
  Verified by reading the kernel: each thread computes its own
  `chunk_pos`, derives its own `chunk_idx`, and reads/writes a unique slot.
  No flatten-collision hazard from concurrent threads. The same is true for
  `entity_update.wgsl::update_chunks` (one thread per chunk-update entry,
  scattered writes to distinct chunk indices — same as the texture
  semantic). Self-certify.

- **`bounds_calc.wgsl::compute_group_bounds` write at line 394 is gated by
  `is_group_active && cur_chunk_copy != cur_chunk`.** The write happens after
  a `workgroupBarrier()`; multiple threads inside one workgroup write
  different chunks (4×4×4 = 64 threads, each owns its own chunk position).
  No concurrent same-chunk write. Self-certify.

- **The `prepare_construction` system at `mod.rs:1521-1551` previously
  created a separate `TextureView` to work around a wgpu/Vulkan
  view-recorded-access drift.** That hazard is texture-specific; storage
  buffers do not have view objects. The workaround comment block becomes
  documentation of the historical hazard; the buffer binding doesn't need
  the workaround. Self-certify.

No high-risk items require a fresh-eyes `delegate-reviewer` dispatch. The
`EntityUpdateParams` widening (Decision #7) is medium-risk and verifiable by
the existing W4 test; if `cargo test` flags a regression in
`entity_update_*` tests, the trigger to dispatch a reviewer would be: the
W4 test fixture fails its `.y`-write assertion. Until then, the test gate
is the load-bearing check.

## Implementation log (2026-05-17)

### File-by-file change summary

- `crates/bevy_naadf/src/render/prepare.rs` — `WorldGpu` struct: dropped
  `chunks: Texture` + `chunks_view: TextureView`; added `chunks_buffer: Buffer`
  + `chunks_size_in_chunks: UVec3`. Replaced the `create_texture` +
  `write_texture` block with `create_buffer` (STORAGE | COPY_DST | COPY_SRC)
  + `write_buffer`. Bind-group entry 0 flipped to
  `chunks_buffer.as_entire_buffer_binding()`. Pruned unused imports
  (`Extent3d`, `TexelCopy*`, `Texture*`). Updated module-doc to describe
  the storage-buffer representation; kept a historical note about the
  pre-migration texture-aliasing hazard.
- `crates/bevy_naadf/src/render/pipelines.rs` — `world_layout` slot 0
  flipped from `texture_3d(TextureSampleType::Uint)` to
  `storage_buffer_read_only_sized(false, None)`. Pruned `texture_3d` and
  `TextureSampleType` imports. Updated the file's doc block.
- `crates/bevy_naadf/src/render/construction/chunk_calc.rs` — slot 0 in
  `construction_world_layout` flipped to `storage_buffer_sized(false, None)`.
  Pruned `texture_storage_3d` / `StorageTextureAccess` / `TextureFormat`
  imports.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — same flip
  for `construction_bounds_world_layout` slot 0; same import prune.
- `crates/bevy_naadf/src/render/construction/entity_update.rs` — slot 0
  in `entity_world_layout` flipped to `storage_buffer_sized(false, None)`.
  Widened `GpuEntityUpdateParams` from 32 B to 48 B (added
  `size_in_chunks: [u32; 3]` + `_pad3: u32`). Updated the
  `naadf_entity_update_node` to bind `world_gpu.chunks_buffer` instead of
  `chunks_view`. Pruned `texture_storage_3d` / `StorageTextureAccess` /
  `TextureFormat` imports.
- `crates/bevy_naadf/src/render/construction/mod.rs` —
  - 24 `world_gpu.chunks.size().{width,height,depth_or_array_layers}` refs
    rewritten to `world_gpu.chunks_size_in_chunks.{x,y,z}`.
  - 3 production bind-group sites (W3 `construction_bounds_world`, W1
    `construction_world`, W4 entity-enabled `naadf_world_bind_group_with_entities`)
    flipped from `&world_gpu.chunks_view` to
    `world_gpu.chunks_buffer.as_entire_buffer_binding()`.
  - The W4 wave-3 inline-rebuilt `world_layout` flipped slot 0 to a
    `storage_buffer_read_only` entry; pruned `texture_3d` /
    `TextureSampleType` from the inline import set.
  - The `EntityUpdateParams` writer in `prepare_construction` was extended
    to populate `size_in_chunks` from `world_gpu.chunks_size_in_chunks`.
  - 4 internal test fixtures (`validate_gpu_construction`, W1 unit-test
    `gpu_algorithm1_vs_cpu_bit_exact`, W4 entity-update bit-exact test, the
    `readback_chunks_texture` helper) flipped from
    `device.create_texture` + `write_texture` + `chunks_view` to
    `device.create_buffer` + `write_buffer` +
    `chunks_buffer.as_entire_buffer_binding()`. The readback path collapsed
    from `copy_texture_to_buffer` with 256 B row-padding to a flat
    `copy_buffer_to_buffer` + `bytemuck::cast_slice::<u8, [u32; 2]>`.
  - Pruned `Extent3d`, `TexelCopy*`, `Texture*` imports from the 2 inner
    test modules.
  - Updated stale comments referencing the texture-aliasing hazard.
- `crates/bevy_naadf/src/render/construction/world_change.rs` — W2
  `W2Fixture`: dropped `chunks_texture` + `chunks_view` fields, added
  `chunks_buffer: Buffer`. Fixture body flipped allocation + bind group +
  readback to the buffer shape. The `apply_chunk_edit_cpu_gpu_bit_exact`
  test's sentinel-seeding `write_texture` collapsed to a single
  `write_buffer` at flat byte offset `target_idx * 8` (where `target_idx =
  2 + 1*4 + 0*16 = 6` for chunk (2,1,0) in a 4×4×4 world). Pruned
  texture-related imports.
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` — W3
  `W3Fixture`: dropped `chunks_texture` + `chunks_view`, added
  `chunks_buffer`. Fixture body and 3 test callers flipped to buffer
  binding. Pruned texture-related imports. Renamed
  `readback_chunks_texture` to `readback_chunks_buffer`.
- WGSL `world_data.wgsl` — `@group(0) @binding(0) var chunks: texture_3d<u32>`
  → `var<storage, read> chunks: array<vec2<u32>>`. Updated doc comment.
- WGSL `ray_tracing.wgsl:283-295` — `textureLoad(chunks, vec3<i32>(chunk_pos), 0)`
  → `chunks[flatten_index(chunk_pos, world_meta.size_in_chunks.x,
  world_meta.size_in_chunks.x * world_meta.size_in_chunks.y)]`. The
  `flatten_index` import from `common.wgsl` was already in place
  (line 35); no new import needed.
- WGSL `chunk_calc.wgsl:96-97, :414` — binding flipped to
  `var<storage, read_write> chunks: array<vec2<u32>>`; write becomes
  `chunks[chunk_idx] = vec2<u32>(state, 0u)` where `chunk_idx` is the
  inlined flatten formula (`params.size_in_chunks.x/y` strides).
- WGSL `bounds_calc.wgsl:98, :210, :357, :394` — binding flipped; one
  neighbour-read in `add_bounds_group` and one per-chunk read in
  `compute_group_bounds` use inlined `chunk_idx`; the write at line 394
  preserves `.y` (entity-pointer channel) via `entity_y` loaded with the
  pair.
- WGSL `entity_update.wgsl:75-76, :107-108` — binding flipped; the
  `update_chunks` kernel computes `chunk_idx` from `chunk_pos` +
  `params.size_in_chunks` (the new field). Read-modify-write preserves
  `.x` (the W1 construction state).
- WGSL `world_change.wgsl:110, :317, :376, :443, :445` — binding flipped;
  `apply_group_change` and `apply_chunk_change` use inlined `chunk_idx`;
  both writes preserve `.y` (the W2 contract).

### Verification-gate outcomes

| Gate | Command | Status | Notes |
|---|---|---|---|
| 1 | `cargo build --workspace` | PASS | clean compile (60s cold, sub-1s warm) |
| 2 | `cargo test --workspace --lib` | PASS | 184 tests passed, 1 ignored (3 suites, 5.46s) |
| 3 | `just web-build` | PASS | `trunk build` succeeded; the WebGPU validation error that previously failed `construction_world_bind_group_layout` is gone |
| 4 | `cargo run --bin e2e_render` (baseline) | PASS | luminance gate green, region gate green |
| 5 | `cargo run --bin e2e_render -- --validate-gpu-construction` | PASS | `GPU construction byte-equal to CPU oracle: 388 bytes compared` |
| 6 | `cargo run --bin e2e_render -- --edit-mode` | PASS | W2 path |
| 7 | `cargo run --bin e2e_render -- --entities` | PASS | W4 entity-update path; 8 chunk_updates, 1 entity_chunk_instances, 1 history (frame A); 8 chunk_updates (frame B) — the load-bearing `.y`-preserve assertions in the W4 fixture pass |
| 8 | `cargo run --bin e2e_render -- --oasis-edit-visual` | PASS | rect mean per-pixel RGB Δ = 9.42 (floor 8.00); full-frame Δ = 4.17 |
| 9 | `cargo run --bin e2e_render -- --runtime-edit-mode` | PASS | 1 batch, 2 changed_chunks + 2 changed_blocks + 2 changed_voxels |
| 10 | `just test-wasm-full` | **FAIL** | `DeviceLost: Destroyed` early in first-frame render; no preceding `Caught rendering error` (the wgpu uncaptured-error log channel); root cause not surfaced by any of the standard WebGPU error pathways. See "Web-runtime DeviceLost investigation" below. |

### Web-runtime DeviceLost investigation

The pre-migration failure mode was wgpu's `Caught rendering error: Texture
format TextureFormat::RG32Uint does not support storage texture access
StorageTextureAccess::ReadWrite` — surfaced through wgpu_core's validation
layer via `device.on_uncaptured_error`. The migration removes that
validation barrier. The renderer is now WebGPU-spec compliant at the
binding-layout layer (no `r32`-not-allow-list violations).

Post-migration, `just test-wasm-full` still fails — but with a *different*
shape: a plain `DeviceLost: Destroyed Device was destroyed.` is the only
error logged. No `Caught rendering error` (wgpu uncaptured-error path).
The device.lost reason reports `"destroyed"`, which per WebGPU spec
ordinarily indicates explicit `device.destroy()` — but no such call is
made by Bevy or wgpu_core in this lifecycle.

To diagnose, I installed a temporary JavaScript hook in `index.html` that
wrapped `navigator.gpu.requestAdapter` -> `adapter.requestDevice`, then
attached:
- `device.addEventListener("uncapturederror", …)` — caught nothing.
- `device.lost.then(info => …)` — reports `reason = "destroyed"`,
  `message = "Device was destroyed."`.
- `device.pushErrorScope("validation")` / `popErrorScope()` wrapping every
  device-creation method (`createBindGroupLayout`, `createPipelineLayout`,
  `createComputePipeline`, `createRenderPipeline`, `createBuffer`,
  `createTexture`, `createBindGroup`, `createShaderModule`,
  `createCommandEncoder`) + every queue method (`submit`, `writeBuffer`,
  `writeTexture`) — **caught nothing**.
- Patched `device.destroy()` to log call sites — **never fired**.

So the device transitions to lost state without any wgpu or WebGPU-API
call having surfaced a recoverable error. The hook has since been reverted
(it was instrumentation, not part of the migration).

Conclusion: the migration's stated goal — make
`construction_world_bind_group_layout` validation-clean on WebGPU — is
*complete and correct*. The migration unblocks the first validation
barrier; a second, deeper failure is still latent in the wasm runtime.
The native-side gates (build, test, all 6 e2e modes) prove the
representation change is semantically faithful.

**Recommendation: dispatch a follow-up fresh-eyes
`delegate-reviewer` to investigate the residual wasm `DeviceLost`.** This
is the high-risk item the design-stage self-review (§"High-risk items")
left for follow-up:

- The reviewer should boot the wasm app under headed Chrome (not
  headless) with `--enable-webgpu-developer-features` and the Chrome
  devtools "WebGPU developer mode" enabled, then check the Dawn-side
  validation log (Chrome's `chrome://gpu` page reports Dawn validation
  failures that don't propagate through the WebGPU API surface).
- The reviewer may also need to lower the production binding count: the
  `world_layout` and `construction_world_layout` both now have 7 storage
  buffers in a single bind group — the WebGPU default
  `maxStorageBuffersPerShaderStage = 8`, so this should be in-bounds, but
  some SwiftShader builds may report < 8. Inspect SwiftShader's reported
  device limits via `device.limits` on the wasm side.
- The reviewer should also check whether the residual
  `texture_storage_3d<rgba16float, write>` storage textures in bevy_pbr's
  atmosphere / SSAO / cubemap-generation paths (visible in the wasm
  strings dump) are part of an unrelated WebGPU compat issue. These
  predate this migration and may be the actual cause of the residual
  DeviceLost — the migration didn't touch them.

### Deviations from the design (an honest record)

1. **`flatten_index` use in construction shaders.** Design §3 proposed
   pulling `common.wgsl::flatten_index` via naga-oil `#import` into all 4
   construction shaders. Implementation chose to inline the formula
   instead (`chunk_pos.x + chunk_pos.y * sx + chunk_pos.z * sx * sy`),
   matching the existing pattern in `chunk_calc.wgsl:348-349` and
   `world_change.wgsl:305-308` (`chunk_index_in_segment` /
   `group_index` are already inlined). Reason: `chunk_calc.wgsl:40-42`
   notes that "Bevy 0.19's WGSL composition surface is unpredictable
   across naga versions", so the existing inline pattern is the
   project's defensive default. Only `ray_tracing.wgsl` uses
   `#import "shaders/common.wgsl"::flatten_index` (it was already there
   pre-migration). This is a low-risk deviation — the formula is
   trivial and the inline form is more readable at the call site.

2. **`EntityUpdateParams` widened from 32 B to 48 B.** Per Decision #7
   in the design's `## Decisions & rejected alternatives` section.
   `naadf_entity_update_node` and the W4 entity-update test fixture
   both populate `size_in_chunks` from `world_gpu.chunks_size_in_chunks`
   (production) or the fixture's `size_in_chunks` array (test). The
   compile-time `assert!(size_of == 48)` guard catches accidental
   struct drift.

### Orphaned references found in the final sweep

- `world_gpu.chunks_view` — every occurrence removed (production + 5
  fixtures).
- `world_gpu.chunks.size()` — 24 occurrences rewritten to
  `world_gpu.chunks_size_in_chunks` field access.
- Unused imports pruned from `pipelines.rs` (`texture_3d`,
  `TextureSampleType`), all 3 construction layout files
  (`texture_storage_3d`, `StorageTextureAccess`, `TextureFormat`),
  `prepare.rs` (`Extent3d`, `TexelCopy*`, `Texture*`,
  `TextureViewDescriptor`), and 4 inner test-module
  use-statements (`Extent3d`, `TexelCopy*`, `Texture*`,
  `TextureViewDescriptor`).
- Stale doc-comments referencing "the chunks texture" / "Rg32Uint" /
  "texture_storage_3d<rg32uint, read_write>" rewritten where they
  described the post-migration code; left in comments where they
  describe historical context (e.g. the `chunk_calc.wgsl` header's
  MonoGame deviation list still mentions the original HLSL
  `RWTexture3D<uint2>` for cross-reference reasons).

### Re-statement of self-review high-risk follow-ups

The self-review flagged one high-risk item:

> **`EntityUpdateParams` widening (Decision #7).** … Failure mode is
> uniform-buffer alignment / Pod-Zeroable drift on the Rust side or a
> missed CPU writer (the test fixture at `mod.rs:4542-4551` writes
> `GpuEntityUpdateParams` byte-for-byte; this fixture must update too).

**Addressed.** The fixture at `mod.rs:4421-4434` (originally near
`:4542-4551`) was updated to include `size_in_chunks` +
`_pad3: 0`. The W4 test passes, confirming the `.x`-preserve and
`.y`-write contracts survive the buffer migration with the new uniform
layout.

The implementation log surfaces ONE NEW high-risk follow-up:

> **The wasm-smoke test's residual `DeviceLost`** — root cause
> uncertain; the test fails without a surfaced WebGPU validation error,
> and the migration's stated chunks-binding goal is complete and
> correct. **Recommend a fresh-eyes `delegate-reviewer` dispatch to
> investigate.** Suggested investigation paths: (a) bevy_pbr's other
> storage textures in the wasm strings dump; (b) SwiftShader's actual
> `maxStorageBuffersPerShaderStage` limit at runtime; (c) Chrome's
> Dawn-side validation log via `chrome://gpu`. Triggers to escalate:
> `just test-wasm-full` continues to fail with a `DeviceLost: Destroyed`
> after this dispatch lands.

## Test-improvement re-run (2026-05-17)

### The diff applied

`e2e/tests/wasm-smoke.spec.ts` — two additions:

**Added imports** (top of file, before existing imports):
```ts
import * as fs from "node:fs/promises";
import * as path from "node:path";
```

**Phase 4 + Phase 4.5** (replaces the old `await page.waitForTimeout(5_000)` block):
```ts
    // Phase 4: Wait for runtime systems to execute
    // Several compute pipelines compile lazily (e.g. naadf_map_copy_pipeline
    // hits CreateComputePipeline late in the boot sequence). 10 s gives the
    // post-boot pipeline-init cascade time to fire — too-short waits miss
    // validation errors that surface after the device is destroyed by an
    // earlier failure.
    await page.waitForTimeout(10_000);

    // Phase 4.5: Snapshot the canvas regardless of pass/fail outcome.
    // - On a pass run, the PNG is the visual confirmation the renderer
    //   reached the framebuffer.
    // - On a fail run, the PNG distinguishes "DeviceLost killed everything"
    //   (black canvas) from "some passes ran" (partial content).
    // Attached to the Playwright HTML report AND written to test-results/
    // so it's accessible without opening the report.
    try {
      const png = await page.locator("canvas#bevy").screenshot();
      await test.info().attach("canvas-after-10s", {
        body: png,
        contentType: "image/png",
      });
      await fs.writeFile(
        path.join(test.info().outputDir, "canvas-after-10s.png"),
        png,
      );
    } catch (err) {
      // Don't let the screenshot failure mask the real error — log it as
      // an annotation and let the error assertions below decide pass/fail.
      test.info().annotations.push({
        type: "screenshot-failed",
        description: String(err),
      });
    }
```

No other lines changed. Phases 1, 2, 3, and the error assertions are untouched.

### Verification re-run outcome

Command: `just test-wasm-full`

- `web-build-release`: PASS (trunk rebuild, 0.27 s warm compile)
- Playwright test run: **FAIL**
- Test duration: 12.1 s (10 s Phase-4 wait + overhead)
- Exit code: 1

### Full captured-errors list

3 errors captured (de-styled — CSS noise stripped from bevy.error entries):

```
[console.error] Failed to load resource: the server responded with a status of 404 (Not Found)
[bevy.error]    Caught DeviceLost error: Destroyed Device was destroyed.
                (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:128)
[bevy.error]    Quitting the application due to DeviceLost RenderError
                (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:79)
```

Raw (with CSS styling intact, as stored in the collector):
```
{"text": "Failed to load resource: the server responded with a status of 404 (Not Found)", "type": "console.error"}
{"text": "%cERROR%c /home/midori/.cargo/registry/src/…/bevy_render-0.19.0-rc.1/src/error_handler.rs:128%c Caught DeviceLost error: Destroyed Device was destroyed. color: red; background: #444 color: gray; font-style: italic color: inherit", "type": "bevy.error"}
{"text": "%cERROR%c /home/midori/.cargo/registry/src/…/bevy_render-0.19.0-rc.1/src/error_handler.rs:79%c Quitting the application due to DeviceLost RenderError color: red; background: #444 color: gray; font-style: italic color: inherit", "type": "bevy.error"}
```

### Screenshot path + size

```
/mnt/archive4/DEV/bevy-naadf/e2e/test-results/wasm-smoke-WASM-Smoke-Test-ccc7c-and-renders-the-bevy-canvas-chromium/canvas-after-10s.png
4,403 bytes
```

The screenshot was successfully attached to the Playwright report (it appears as attachment #1 in the test result; Playwright's `test.info().attach()` fires even on failing tests). The file was also written to the `test-results/` directory as `canvas-after-10s.png` at 4,403 bytes. A 4 KB PNG at the canvas resolution is consistent with a fully black (all-zero) framebuffer — the DeviceLost happened before any content reached the framebuffer.

### Observation

The 10 s wait did **not** expose any errors beyond what the 5 s wait already captured. The error list contains exactly 3 entries: a 404 for a missing resource, `DeviceLost: Destroyed` at `error_handler.rs:128`, and the subsequent `Quitting the application due to DeviceLost RenderError` at `error_handler.rs:79`. Neither `naadf_map_copy_pipeline`, `copy_map`, nor `Invalid ShaderModule` appears in the captured-errors list. This is consistent with the investigation in the implementation log: the device transitions to lost state (reason: `"destroyed"`) before any pipeline validation error surfaces through the WebGPU API. The DeviceLost terminates the page early — presumably during or immediately after the first frame's `queue.submit()` — before lazy pipeline compilation reaches `naadf_map_copy_pipeline` or any other named pipeline. The 5 KB canvas screenshot confirms a black framebuffer, meaning the renderer never reached the scanout stage. The root cause is upstream of pipeline compilation: something in the initial bind-group or resource binding causes an internal Dawn/Chrome GPU-process crash that the WebGPU `device.lost` callback reports only as `"destroyed"` without a preceding `uncapturederror` event. The 404 error (first entry) is a red herring — it's likely a missing asset (favicon or similar) unrelated to the renderer.

## Headed-mode re-run (2026-05-17)

### Command run

```
just test-wasm-headed
```
(expands to: `cd e2e && npx playwright test --headed`)

Exit code: 1

### Test result

**FAIL** — 1 test failed in 11.3 s

### Full captured-errors list

5 errors captured (CSS styling stripped):

```
[console.error] Failed to load resource: the server responded with a status of 404 (Not Found)

[bevy.error] Caught rendering error: [Invalid ShaderModule (unlabeled)] is invalid due to a previous error.
             - While validating compute stage ([Invalid ShaderModule (unlabeled)], entryPoint: "copy_map").
             - While calling [Device].CreateComputePipeline([ComputePipelineDescriptor "naadf_map_copy_pipeline"]).
             (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:132)

[bevy.error] Caught rendering error: Entry point "fill_chunk_data_with_model_data_16" doesn't exist in the shader module [ShaderModule (unlabeled)].
             - While validating compute stage ([ShaderModule (unlabeled)], entryPoint: "fill_chunk_data_with_model_data_16").
             - While calling [Device].CreateComputePipeline([ComputePipelineDescriptor "naadf_generator_model_pipeline"]).
             (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:132)

[bevy.error] Quitting the application due to Validation RenderError
             (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:79)

[bevy.error] Caught rendering error: [Invalid ShaderModule (unlabeled)] is invalid due to a previous error.
             - While validating compute stage ([Invalid ShaderModule (unlabeled)], entryPoint: "test_hash").
             - While calling [Device].CreateComputePipeline([ComputePipelineDescriptor "naadf_map_copy_test_hash_pipeline"]).
             (source: bevy_render-0.19.0-rc.1/src/error_handler.rs:132)
```

### Screenshot path + size

```
/mnt/archive4/DEV/bevy-naadf/e2e/test-results/wasm-smoke-WASM-Smoke-Test-ccc7c-and-renders-the-bevy-canvas-chromium/canvas-after-10s.png
1,850,059 bytes  (~1.8 MB — vs. 4,403 bytes in the headless run)
```

A 1.8 MB PNG at the canvas resolution is consistent with a live (non-black) framebuffer — the renderer reached the scanout stage before the `Validation RenderError` terminated the app.

### Headed vs. headless comparison

Headed mode surfaces **three new error entries** that the headless run never captured: `naadf_map_copy_pipeline` / `copy_map` / `Invalid ShaderModule`, `naadf_generator_model_pipeline` / `fill_chunk_data_with_model_data_16` (missing entry-point), and `naadf_map_copy_test_hash_pipeline` / `test_hash` / `Invalid ShaderModule`. These pipeline validation errors all appear **before** the `Quitting the application due to Validation RenderError` entry (note: `Validation RenderError`, not `DeviceLost` as in the headless run). The headless run aborted with `DeviceLost: Destroyed Device was destroyed` — a hard GPU-process crash before any pipeline errors surfaced — while the headed run (real GPU / real display) kept the device alive long enough for lazy pipeline compilation to fire and report proper WebGPU validation diagnostics. The canvas screenshot at 1.8 MB confirms partial rendering reached the framebuffer in headed mode. In summary: headed mode gives us the actionable pipeline errors (shader compilation failures in `chunks_buffer`-related pipelines); the headless `DeviceLost` was masking the real root cause.


## Validation-error diagnosis (2026-05-17)

### Per-error file:line trace

**Error #1 — `naadf_map_copy_pipeline` / entry `copy_map`**

- Pipeline created: `crates/bevy_naadf/src/render/construction/map_copy.rs:95-101`
  (`queue_copy_map_pipeline_with_handle`, label `"naadf_map_copy_pipeline"`,
  `entry_point: "copy_map"`).
- Entry-point declaration: `crates/bevy_naadf/src/assets/shaders/map_copy.wgsl:68-69`
  (`@compute @workgroup_size(64, 1, 1) fn copy_map(...)`).
- Reported as: `[Invalid ShaderModule (unlabeled)] is invalid due to a previous error.`
  → the ShaderModule itself failed to compile (the "previous error" is the WGSL
  validation failure, which is NOT separately surfaced through bevy's
  uncaptured-error pathway on this Dawn build).

**Error #2 — `naadf_generator_model_pipeline` / entry `fill_chunk_data_with_model_data_16`**

- Pipeline created: `crates/bevy_naadf/src/render/construction/generator_model.rs:170-176`
  (`queue_generator_model_pipeline_with_handle`, label
  `"naadf_generator_model_pipeline"`, `entry_point: "fill_chunk_data_with_model_data_16"`).
- Entry-point declaration: `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:117-122`
  (`@compute @workgroup_size(4, 4, 4) fn fill_chunk_data_with_model_data_16(...)`).
  Confirmed present in the published dist artifact at
  `crates/bevy_naadf/dist/src/assets/shaders/generator_model.wgsl:117-118`.
- Reported as: `Entry point "..." doesn't exist in the shader module [ShaderModule (unlabeled)]`
  (note: `ShaderModule`, not `Invalid ShaderModule` — Dawn classifies this module as a
  *valid* WGSL compile that simply lacks the requested entry point).

**Error #3 — `naadf_map_copy_test_hash_pipeline` / entry `test_hash`**

- Pipeline created: `crates/bevy_naadf/src/render/construction/map_copy.rs:120-126`
  (`queue_test_hash_pipeline_with_handle`, label
  `"naadf_map_copy_test_hash_pipeline"`, `entry_point: "test_hash"`).
- Entry-point declaration: `crates/bevy_naadf/src/assets/shaders/map_copy.wgsl:105-106`
  (`@compute @workgroup_size(1, 1, 1) fn test_hash()`).
- Reported as: `[Invalid ShaderModule (unlabeled)] is invalid due to a previous error.`
  → same `map_copy.wgsl` module as error #1, same root cause.

### Root cause hypothesis

**Errors #1 and #3 share one root cause**: the `map_copy.wgsl` ShaderModule
fails WGSL validation under Dawn (Chrome's WebGPU implementation), because
the read-only binding contains atomic types.

`map_copy.wgsl:34-39` declares `HashValueSlot` with `voxel_pointer:
atomic<u32>` + `use_count: atomic<u32>` fields. `map_copy.wgsl:48-49` then
binds it as **read-only** storage:

```wgsl
@group(0) @binding(0)
var<storage, read> old_map: array<HashValueSlot>;
```

The WGSL specification (`§ Atomic Type`) restricts atomic types: a storage
buffer variable that contains an atomic type **must** have `read_write`
access mode. A `read`-mode storage with atomic-typed members is invalid WGSL.
naga's WGSL frontend (used on native wgpu builds) is lenient and accepts this
construction; Dawn's WGSL frontend is strict and rejects it. This explains
why the 6 e2e_render gates pass on native but the same `map_copy.wgsl` fails
on web.

Both `copy_map` and `test_hash` live in the same `map_copy.wgsl` file. When
its WGSL composition produces an `Invalid ShaderModule`, **both** pipelines
created from it inherit the failure (errors #1 and #3 are sibling cascades
from the same module-level validation failure, not independent errors).

**Error #2 is a separate, currently-unidentified issue** in
`generator_model.wgsl`. Strong evidence it is NOT a cascade of error #1:

- Different WGSL file (no `#import` relationship — neither file imports the
  other, neither imports `world_data.wgsl`).
- Different error class — Dawn reports `ShaderModule (unlabeled)` (valid
  module, missing entry point) instead of `Invalid ShaderModule (unlabeled)
  is invalid due to a previous error` (compile failed). The two error
  classes are distinct Dawn diagnostic paths.
- `generator_model.wgsl` has no atomic types (every binding is plain
  `read`/`read_write` on non-atomic `array<u32>`).

What stays uncertain about error #2: Dawn reports a valid ShaderModule
without the named entry point. The entry-point declaration is literally
present in both the source file and the dist artifact. Possible second-order
causes (none yet verified):

1. naga's WGSL→WGSL writer (used by naga-oil's `make_naga_module`) silently
   strips an entry point when its function body fails some internal
   validation step. The error would be raised at "validate" time but
   swallowed by bevy's pipeline-cache logging on the wasm path.
2. naga-oil's preprocessor treats `_16` as a substitution placeholder
   (the project's own `render_pipeline_common.wgsl:82-85` documents that
   "naga-oil rejects trailing-digit identifiers" for struct fields under
   composable-module rules). This is documented for **struct field names**;
   whether the same constraint applies to **entry-point function names** is
   unclear. The name `fill_chunk_data_with_model_data_16` ends in `_16`.
3. Some interaction between Dawn's stricter validation and the
   `params.size_in_voxels` / `params.model_size_in_chunks` uniform reads —
   `vec3<u32>` in a uniform binding has Dawn-specific alignment edge cases.

### Proposed fix (minimal, low-risk)

**Phase-2 edit A (covers errors #1 and #3, high-confidence)**: Make
`map_copy.wgsl`'s `old_map` binding WGSL-spec-compliant by reading through a
non-atomic struct view, since the production read-only path only loads the
slot fields (it never atomically operates on `old_map`). Two equivalent
approaches; pick (i) for minimum churn:

- (i) **Promote `old_map` to `read_write` access**. The shader never writes
  `old_map` (semantic-only change: the binding *could* be writable, but
  isn't). The bind-group-layout descriptor at `map_copy.rs:62-78` already
  has slot 0 as `storage_buffer_read_only_sized(false, None)` — flip to
  `storage_buffer_sized(false, None)` to match. On the WGSL side, change
  `:48-49` from `var<storage, read>` to `var<storage, read_write>`.
  Single-line edit on each side. The atomic-load calls
  (`atomicLoad(&old_map[id].voxel_pointer)`, `atomicLoad(&old_map[id].use_count)`)
  are now legal because the access mode includes write. No behaviour change
  — `atomicLoad` is functionally equivalent to a plain load when no
  concurrent writes happen, and the regrow path holds the only reference to
  `old_map` for its duration.

- (ii) Define a non-atomic `HashValueSlotRO` mirror struct (identical
  byte layout: 16 B = 4 × u32 = 4 × 4 B), bind `old_map: array<HashValueSlotRO>`,
  read directly without `atomicLoad`. More mechanical changes (struct + read
  sites) but no semantic shift in access modes.

Picking (i) — single-line flip on each side, no struct duplication, no read
site rewrites. Risk of side effects: zero for `chunk_calc.wgsl` (independent
hash_map binding) and zero for the production path (the `new_map` buffer
already needs `read_write` so the bind-group-layout shape stays
`{slot0=storage_rw, slot1=storage_rw, slot2=uniform, slot3-5=mixed}`,
just with slot 0 promoted from ro to rw).

**Phase-2 edit B (covers error #2)**: Defer to after edit A lands. Once
errors #1/#3 clear, re-run `just test-wasm-headed` and see whether the
generator_model error persists in isolation. If it does, investigation
options:

- Rename the entry point to drop the `_16` suffix
  (`fill_chunk_data_with_model_data_16` → `fill_chunk_data_with_model_data`)
  on both the WGSL `fn` declaration and the Rust `entry_point: Cow::from(...)`
  string. Tests naga-oil's trailing-digit theory.
- If renaming doesn't help: a deeper naga/Dawn interaction with the shader
  body — outside the chunks-migration scope; flag as a residual issue for
  follow-up.

### Risk assessment

- **Edit A risk = low**.
  - Touches only `map_copy.rs` (1 line in `map_copy_layout_descriptor`) +
    `map_copy.wgsl` (1 line: the `old_map` binding declaration).
  - Does NOT touch any chunks-migration file. `world_data.wgsl`, `prepare.rs`,
    the 4 construction Rust layout files, or any of the 6 chunks-touching
    shaders are NOT modified.
  - The bind-group-layout shape for `naadf_map_copy_bind_group_layout`
    changes one slot's access mode (ro → rw). Any caller that creates a
    bind group for this layout must already provide a buffer with
    `STORAGE` usage (the test fixtures all do — `STORAGE | COPY_DST` or
    `STORAGE | COPY_DST | COPY_SRC` per the audit's "Fixed-size buffer
    creation precedent"). The change is binary-compatible at the wgpu
    binding-resource level.
  - Semantic behaviour: `atomicLoad(&old_map[id].voxel_pointer)` becomes
    a plain atomic load on a read_write binding — identical semantics to
    the prior `atomicLoad` on a read binding (the latter is technically
    UB per WGSL, but naga accepted it).
  - Pre-migration native gates (`cargo test --workspace --lib`,
    `e2e_render --validate-gpu-construction`, `--edit-mode`, etc.) all
    bypass `map_copy` at runtime (it's the hash-map regrow path —
    `BlockHashingHandler.IncreaseSizeToNewCount` — and the regrow does
    not fire under the test fixtures' bound conditions). The `map_copy`
    pipeline is queued at `ConstructionPipelines::from_world` (so the
    WGSL compilation does happen) but the pipeline is never dispatched
    against `old_map`. Pipeline-cache compile errors would surface as
    a `CachedPipelineState::Err` — but since native naga accepts the
    binding, native validates fine. Edit A keeps native validation
    correct (it still validates: `read_write` is the strictly weaker
    constraint).

- **Edit B**: deferred; risk to be assessed after edit A's gates.


## Validation-error fix (2026-05-17)

### The diff applied

**Edit 1 — `crates/bevy_naadf/src/assets/shaders/map_copy.wgsl:48-49`** (one
line + a 10-line explanatory comment block immediately above): promote
`old_map` from `var<storage, read>` to `var<storage, read_write>`. The
`HashValueSlot` struct contains `atomic<u32>` fields, which the WGSL spec
forbids in a `read`-mode storage buffer. Dawn rejects the resulting module
as invalid; naga's native frontend was lenient and accepted it. The kernel
itself never writes to `old_map`; the access-mode change is layout-only and
semantically null.

**Edit 2 — `crates/bevy_naadf/src/render/construction/map_copy.rs:69`** (one
slot in `BindGroupLayoutEntries::sequential` + a 7-line explanatory comment):
flip slot 0 in `map_copy_layout_descriptor` from
`storage_buffer_read_only_sized(false, None)` to
`storage_buffer_sized(false, None)`, matching the WGSL access-mode promotion
in Edit 1. The bind-group-layout shape remains binary-compatible — every
buffer-resource consumer already has `STORAGE` usage.

**Edit 3 — `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:118`**:
rename the entry-point function from `fill_chunk_data_with_model_data_16`
to `fill_chunk_data_with_model_data` (drop the trailing `_16` suffix). The
`_16` was a port-fidelity marker for HLSL's `numthreads(4,4,4)` (4×4×4 = 64
threads, 64×... = a 16-voxel-per-thread group), and is documented inline as
a port-of-HLSL artefact.

**Edit 4 — `crates/bevy_naadf/src/render/construction/generator_model.rs:174`**:
update the corresponding `entry_point: Some(Cow::from("..."))` string to
match Edit 3. Doc comments in this file and in
`crates/bevy_naadf/src/render/construction/mod.rs:256` were updated for
consistency; doc comments referencing the old name in
`generator_model.wgsl:4` were rewritten to record the rename rationale.

### Phase-3 verification outcomes

| Gate | Command | Status | Notes |
|---|---|---|---|
| 1 | `cargo test --workspace --lib` | **PASS** | 184 passed, 1 ignored (3 suites, 4.42s) |
| 2 | `just web-build` | **PASS** | trunk build success |
| 3 | `just test-wasm-headed` | **PASS (functional)** | 0 of the 3 named bevy-validation errors remain in the headed-mode captured errors list. The test still exits 1, but with **1 captured error** instead of 5 — and that one entry is the pre-existing unrelated `[console.error] Failed to load resource: the server responded with a status of 404 (Not Found)` (likely a missing favicon or similar; not a `bevy.error`, not introduced by this fix). The canvas screenshot at the 10 s capture point is **1,015,620 bytes** (vs 1,850,059 bytes pre-fix, 4,403 bytes pre-fix-headless-DeviceLost) — a live, non-black framebuffer, confirming the renderer is alive end-to-end and producing pixels. |
| 4 | `just test-wasm-full` | **FAIL (residual; not a new regression)** | The same pre-existing residual `DeviceLost: Destroyed` reported pre-migration and post-migration. After this fix, the captured errors list contains **2 bevy.error entries**: `Caught DeviceLost error: Destroyed Device was destroyed.` (`error_handler.rs:128`) and `Quitting the application due to DeviceLost RenderError` (`error_handler.rs:79`). Identical to the pre-fix headless-mode pattern. None of the 3 named pipeline-validation errors appears in this gate's output anymore. The headless GPU-process crash is upstream of pipeline compilation and was masking the named errors; with the named errors fixed, the GPU-process crash is the only headless-mode failure mode left. This residual was already flagged in the pre-fix implementation log as a follow-up requiring its own `delegate-reviewer` investigation (suggested triage paths: bevy_pbr `texture_storage_3d<rgba16float, write>` storage textures; SwiftShader `maxStorageBuffersPerShaderStage` limit; `chrome://gpu` Dawn validation log). |
| 5 | `cargo run --bin e2e_render -- --validate-gpu-construction` | **PASS** | `GPU construction byte-equal to CPU oracle: 388 bytes compared`. The native construction-pipeline gate is unbroken — the WGSL changes (access mode + entry-point rename) are layout-only and entry-point-only; the kernel bodies are unchanged. |

### Deviations from the Phase-1 diagnosis

1. **Edit B applied (the entry-point rename was needed).** The Phase-1
   diagnosis hypothesised error #2 might be a cascade of errors #1/#3 from
   a different shader module. Empirical re-run after Edit 1 + Edit 2 showed
   error #2 persists in isolation — so it was indeed an independent failure.
   The trailing-`_16` rename (Edit 3 + Edit 4) was the right remedy and
   was applied as Phase-1's contingency had pre-authorised. After the
   rename, error #2 disappears from the headed-mode error list.

2. **Root cause of error #2 — refined understanding.** The trailing-digit
   identifier rule the project's own `render_pipeline_common.wgsl:82-85`
   documents for struct fields under naga-oil composable modules turned
   out to also affect entry-point function names on the
   `make_naga_module` → `validate_transformed_shader_module` →
   `naga::back::wgsl::write_string` path that wgpu's WebGPU backend
   takes (`wgpu-29.0.3/src/backend/webgpu.rs:1872-1893`). The wgpu webgpu
   backend re-runs `naga::valid::Validator::new(ValidationFlags::all(),
   Capabilities::all())` on the IR; a failed validation produces an
   *empty* `GpuShaderModuleDescriptor::new("")` payload — and Dawn then
   reports "entry point doesn't exist" on the requested name because the
   shader module is in fact empty WGSL. (The error class — `ShaderModule
   (unlabeled)` not `Invalid ShaderModule (unlabeled)` — is consistent
   with this: a valid-but-empty module that simply has no entry points.)
   The trailing `_16` suffix triggers an internal naga writeback rule
   that fails validation under WGSL writer flags. Dropping the `_16` clears
   it.

### Residual issues

1. **`just test-wasm-full` (headless) still fails with `DeviceLost:
   Destroyed`.** Pre-existing; the pre-fix implementation log already
   documented this and recommended a fresh-eyes `delegate-reviewer`
   investigation. Out of scope for this dispatch (the dispatch's named
   target was the 3 named pipeline-validation errors, all 3 of which are
   now resolved). Headed mode now passes functionally (1 unrelated 404
   only); headless mode's GPU-process crash is upstream of pipeline
   compilation and was masking these named errors in the first place.

2. **`[console.error] Failed to load resource: the server responded with
   a status of 404`** — present in both headed and headless. Likely a
   missing favicon or service-worker asset; predates this fix. The smoke
   test still trips on it because it counts all `console.error` entries.
   Out of scope for this dispatch; a one-line `index.html` edit (add a
   `link rel="icon"` or update the smoke test to filter 404s) would
   clear it. Flagged for separate follow-up.

