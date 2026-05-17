# Reuse audit — `chunks` 3D texture → flat storage buffer migration

Audit of existing code in `bevy-naadf` that already covers, partially covers, or could be extended for the migration of the `chunks` 3D `Rg32Uint` storage texture (currently bound as `texture_storage_3d<rg32uint, read_write>` in 4 construction shaders and `texture_3d<u32>` in 2 render shaders) to a flat WebGPU-compliant storage buffer `array<vec2<u32>>` indexed by `idx = z * size_in_chunks.x * size_in_chunks.y + y * size_in_chunks.x + x`.

## Candidates table

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `GrowableBuffer<T>` | `crates/bevy_naadf/src/world/buffer.rs:45-222` | A growable typed wgpu storage buffer (`STORAGE \| COPY_SRC \| COPY_DST`) with `reserve` (grow+copy), `reserve_discard` (grow+throw), `upload_all` (the build-once one-shot) and `.buffer().as_entire_buffer_binding()` | **not applicable as `GrowableBuffer`, but `upload_all` is the upload precedent** | `chunks` is sized once at world build to `size_in_chunks.x * .y * .z` and never grows — `GrowableBuffer`'s growth/copy/headroom complexity buys nothing; the bare `device.create_buffer` + `queue.write_buffer` pattern (already used for `world_meta`, `placeholder_*` in the same `prepare.rs` system) is the right shape. |
| Existing `array<vec2<u32>>` storage-buffer binding precedents (frame-data slots 3-5; `changed_chunks_dynamic`; `chunk_updates_dynamic`) | `crates/bevy_naadf/src/render/pipelines.rs:350-353` + `crates/bevy_naadf/src/assets/shaders/world_change.wgsl:131` + `crates/bevy_naadf/src/assets/shaders/entity_update.wgsl:81` | Several bindings already declare `array<vec2<u32>>` against `storage_buffer_sized(false, None)` / `storage_buffer_read_only_sized(false, None)` on the Rust side and `var<storage, read_write> name: array<vec2<u32>>` / `var<storage, read> name: array<vec2<u32>>` on the WGSL side. Identical element type to the proposed `chunks` buffer. | **reuse the pattern verbatim** | The `[u32; 2]` ↔ `vec2<u32>` mapping (8 B element, `repr(C)` Pod) is already proven in the codebase — the new chunks binding is a textbook copy of these existing declarations. |
| `flatten_index(pos, stride_y, stride_z)` helper in `common.wgsl` | `crates/bevy_naadf/src/assets/shaders/common.wgsl:32-34` | Generic flatten of `vec3<u32>` to `u32` with `pos.z*stride_z + pos.y*stride_y + pos.x` (x-fastest). Exported via naga-oil `#import`. | **reuse — pass `(size_in_chunks.x, size_in_chunks.x * size_in_chunks.y)`** | The exact x-fastest convention the migration requires (`z * sx*sy + y * sx + x`); the helper is already imported into `ray_tracing.wgsl` at line 35 for block/voxel-in-chunk addressing, so importing it for chunk-in-world is one new `flatten_index` call site, no helper authorship. Note: stride_y must be passed as `size_in_chunks.x` (the *row stride*), and stride_z as `size_in_chunks.x * size_in_chunks.y` — the function signature already takes the row/plane strides, not the dims. |
| In-shader `chunk_index_in_segment` formulas in construction shaders | `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:348-349`, `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:126-128`, `crates/bevy_naadf/src/assets/shaders/world_change.wgsl:305-308` (`apply_group_change`), `crates/bevy_naadf/src/render/construction/entity_handler.rs:339-345` (`chunk_index_to_pos`) | Hand-rolled `gx + gy*sx + gz*sx*sy` flatten copies for chunk-index-in-segment / group-index-in-world. The CPU side has the same formula in `entity_handler.rs::chunk_index_to_pos`. | **mirror as precedent** | The exact `idx = z*sx*sy + y*sx + x` math the brief asks for is already in 4 shader sites + the CPU side. The migration should call `flatten_index` rather than inline the math a 5th time — but the existing inline copies establish that x-fastest is the codebase convention. |
| `chunks_packed: &[[u32; 2]]` CPU-side helper for the GPU readback | `crates/bevy_naadf/src/aadf/edit.rs:21-59`, `:582` | Treats the GPU `Rg32Uint` chunks texture readback as a flat `[[u32; 2]]` slice keyed by linear chunk index, already the same data layout the new buffer would carry. | **reuse — the CPU mirror layout already matches** | The CPU-side oracle/edit comparison surface already speaks "`[u32; 2]` per chunk indexed by flatten". Migrating the GPU side from texture to buffer makes the comparison `bytemuck::cast_slice::<u8, [u32; 2]>(&staging_bytes)` instead of the row-padded `bytes_per_row` walk in `readback_chunks_texture` — a strict simplification of the existing readback. |
| Bevy 0.18-downgrade design doc §5 wgpu storage-texture format audit | `docs/orchestrate/naadf-bevy-port/23-design-bevy-018-downgrade.md:368-394` | Explicitly documents that `Rg32Uint` `STORAGE_READ_WRITE` is NOT in the wgpu guaranteed feature set — only `STORAGE_READ_ONLY \| STORAGE_WRITE_ONLY` is — and that the WebGPU-spec-compliant path requires either format change (the `r32{uint,sint,float}` allow-list) or feature-gated access. | **W4's design-doc trace anticipates the validation gap; the proposed migration is the WebGPU-correct fix.** | Quoted in §"W4 design-doc trace" below. The audit-anticipated risk has materialised; the buffer migration sidesteps the storage-texture format restriction entirely. |
| `WorldGpuStaging` + `WorldGpu` `prepare.rs` build-once system | `crates/bevy_naadf/src/render/prepare.rs:165-478` | The single render-world system that owns the build-once upload of the chunks texture + blocks/voxels/voxel_types + world_meta uniform + placeholder entity buffers, then builds the `@group(0)` bind group binding all of them, then drops `WorldGpuStaging`. | **extend — the chunks texture allocation + upload + view becomes a `chunks_buffer: Buffer` allocation + `queue.write_buffer` + `.as_entire_buffer_binding()`** | This is the one production seam that owns the chunks resource. Every other reference (construction, render passes, test fixtures) consumes `WorldGpu.chunks` / `WorldGpu.chunks_view`; replacing those fields with `WorldGpu.chunks_buffer: Buffer` is the load-bearing edit. The 1-element placeholder `Buffer` allocations at `prepare.rs:423-441` (`STORAGE \| COPY_DST`, `mapped_at_creation: false`) are the exact pattern the new chunks buffer mirrors at full size. |
| Test-fixture chunks-texture allocations (W2/W3 fixtures, validate harness) | `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:480-516`, `crates/bevy_naadf/src/render/construction/world_change.rs:662-698`, `crates/bevy_naadf/src/render/construction/mod.rs:2473-2515`, `:3819-3866`, `:4453-…` | Each test fixture re-creates a standalone `Rg32Uint` 3D texture with the same `TEXTURE_BINDING \| COPY_DST \| COPY_SRC \| STORAGE_BINDING` usages, then `queue.write_texture` with `bytes_per_row = size_in_chunks[0] * 8`, `rows_per_image = size_in_chunks[1]`, plus the row-padded `bytes_per_row.next_multiple_of(256)` readback. | **all 5 sites need migration — same Buffer-create + write_buffer pattern as the production seam** | Migration scope is wider than just the 6 WGSL shaders + `prepare.rs`: the 5 test-fixture sites each re-roll the same allocation. They will fail at bind-group construction the instant `construction_world_layout_descriptor` flips from `texture_storage_3d` to `storage_buffer_sized`. |
| Construction bind-group layout descriptors (`construction_world_layout`, `construction_bounds_world_layout`, `entity_world_layout`) | `crates/bevy_naadf/src/render/construction/chunk_calc.rs:60-88`, `:69`; `crates/bevy_naadf/src/render/construction/bounds_calc.rs:70-89`, `:83`; `crates/bevy_naadf/src/render/construction/entity_update.rs:78-94`, `:86-89` | The three construction-side layouts each carry `chunks_rw` as binding 0 of `@group(0)` via `texture_storage_3d(TextureFormat::Rg32Uint, StorageTextureAccess::ReadWrite)`. | **extend — flip each entry to `storage_buffer_sized(false, None)`** | Three independent layout descriptors, all flipping binding 0 from `texture_storage_3d` to `storage_buffer_sized`. The other 7 / 1 / 1 bindings stay identical. |
| Renderer `world_layout` `@group(0)` slot 0 (`texture_3d<u32>` sampled view) | `crates/bevy_naadf/src/render/pipelines.rs:312-331`, slot 0 `texture_3d(TextureSampleType::Uint)` consumed by `ray_tracing.wgsl::shoot_ray` line 286 (`textureLoad(chunks, vec3<i32>(chunk_pos), 0)`) and `world_data.wgsl:54` (`var chunks: texture_3d<u32>`) | The renderer's read-only chunks view binding; lives in `NaadfPipelines::world_layout` with 7 other read-only storage / uniform bindings. | **extend — flip slot 0 to `storage_buffer_read_only_sized(false, None)` and replace `textureLoad(chunks, ..., 0).xy` with `chunks[flatten_index(chunk_pos, sx, sx*sy)]`** | Mechanical 1-call-site shader edit (the `shoot_ray` traversal); WGSL `textureLoad` on a `texture_3d<u32>` is the only render-side consumer (`naadf_first_hit.wgsl`, `naadf_global_illum.wgsl`, `spatial_resampling.wgsl` all consume chunks indirectly via `shoot_ray`, verified by `16-impl-c-W4.md` §`.x` sweep audit). No mipmaps, no sampling, no view-format quirks — the binding is `textureLoad` with mip 0 only. `world_meta.size_in_chunks` is already available in the bind group at slot 4 of the same `@group(0)` so the stride is in-scope. |

## Top reuse recommendation

**Mirror the existing `array<vec2<u32>>` storage-buffer precedents** (`pipelines.rs:350-353` frame-data slots, `world_change.wgsl:131` `changed_chunks_dynamic`, `entity_update.wgsl:81` `chunk_updates_dynamic`) for the new chunks binding, **import the existing `flatten_index` helper** from `common.wgsl:32` (call as `flatten_index(chunk_pos, size_in_chunks.x, size_in_chunks.x * size_in_chunks.y)`), and **extend `WorldGpu` / `WorldGpuStaging` / `prepare_world_gpu`** by replacing the texture allocation+upload+view with a `device.create_buffer(STORAGE | COPY_DST | COPY_SRC) + queue.write_buffer + .as_entire_buffer_binding()` sequence — modelled on the same file's `world_meta` and `placeholder_entity_*` buffer paths. `GrowableBuffer<T>` is **not** the right tool (chunks is fixed-size at world build, never grows). No greenfield helper authoring is required — every primitive the migration needs already exists, including the linear-index math the brief specified verbatim (`flatten_index` matches `z*sx*sy + y*sx + x` directly when `stride_y = sx`, `stride_z = sx*sy`).

## Borderline calls

- **`GrowableBuffer<T>` reuse** — borderline because `WorldGpu`'s siblings `blocks`/`voxels`/`voxel_types` ARE `GrowableBuffer<u32>` / `GrowableBuffer<GpuVoxelType>`, so there's a consistency argument for typing chunks the same way (`GrowableBuffer<[u32; 2]>`). What flipped me to "not applicable": `chunks` count = `size_in_chunks.x * .y * .z` is **fixed at world build** (the W4 design doc and prepare.rs:251-266 both treat it as a sized-once resource), and `GrowableBuffer` carries `len`-vs-`capacity` semantics, `reserve` growth, and a 2× growth factor that all dead-code for the chunks use case. **What would flip it**: a future workstream that wanted dynamic world resizing — but that's not in any current design doc and would also break `world_meta.size_in_chunks` and every linear-index call site that bakes the stride into a uniform.
- **`construction_bounds_world_layout` vs `construction_world_layout` independence** — borderline whether to migrate them lockstep or stagger. The three construction layouts each independently declare binding 0 as `chunks_rw`. They're already not unified (W3 deliberately split `construction_bounds_world_layout` from W1's 8-binding `construction_world_layout` per `15-design-c.md` §4.2 / `bounds_calc.rs:67-89`). **What flipped me to "extend (all three lockstep)"**: every consumer of every layout writes to the same underlying chunks resource — a partial migration leaves wgpu unable to alias storage-texture-binding vs storage-buffer-binding on the same allocation, so they must all flip in one PR.
- **W4's `Rg32Uint` widening intent vs the buffer migration** — borderline whether the buffer migration honours W4 ("chunks is widened to two u32s per chunk: `.x` = block-state pointer + AADF, `.y` = entity pointer + counter") or contradicts it. The buffer element type `vec2<u32>` is byte-identical to `Rg32Uint` and the `.x`/`.y` field semantics carry forward without change. **What flipped me to "honours W4"**: every existing read site already uses `.x` / `.y` field selectors on the loaded `vec4<u32>` (`bounds_calc.wgsl:357-360`, `world_change.wgsl:317-319`, `ray_tracing.wgsl:286-295`, `entity_update.wgsl:107-108`) — these are byte-stable when the source is `array<vec2<u32>>` indexed by linear chunk-pos. The migration is a **representation change**, not a semantic change; W4's `.y`-preservation contract is preserved verbatim.

## Quoted precedents

### `flatten_index` helper (`crates/bevy_naadf/src/assets/shaders/common.wgsl:23-34`)

```wgsl
// Flatten a 3D position into a 1D index, x-fastest then y then z.
//
// HLSL `common.fxh`:
//   #define FLATTEN_INDEX(pos, sy, sz) mad(pos.z, sz, mad(pos.y, sy, pos.x))
//
// NAADF calls this with `(blockPosInChunk, 4, 16)` and
// `(voxelPosInBlock, 4, 16)` — note the *second* stride argument is the
// y-stride (4) and the *third* is the z-stride (16), i.e. for a 4×4×4 cell
// `flatten_index(p, 4u, 16u)`.
fn flatten_index(pos: vec3<u32>, stride_y: u32, stride_z: u32) -> u32 {
    return pos.z * stride_z + pos.y * stride_y + pos.x;
}
```

### Existing `array<vec2<u32>>` storage-buffer bindings — Rust side (`crates/bevy_naadf/src/render/pipelines.rs:343-356`)

```rust
let frame_layout = BindGroupLayoutDescriptor::new(
    "naadf_frame_bind_group_layout",
    &BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,
        (
            uniform_buffer_sized(false, Some(camera_size)),
            uniform_buffer_sized(false, Some(params_size)),
            storage_buffer_sized(false, None), // first_hit_data: array<vec4<u32>>, rw
            storage_buffer_sized(false, None), // taa_sample_accum: array<vec2<u32>>, rw
            storage_buffer_sized(false, None), // first_hit_absorption: array<vec2<u32>>, rw
            storage_buffer_sized(false, None), // final_color: array<vec2<u32>>, rw
        ),
    ),
);
```

### Existing `array<vec2<u32>>` ro storage binding — WGSL side (`crates/bevy_naadf/src/assets/shaders/world_change.wgsl:128-131`)

```wgsl
// `@group(1)` = `construction_change_layout` (W2-owned, 4 bindings) — the 4
// CPU-staged upload buffers consumed by the 4 apply passes.
@group(1) @binding(0)
var<storage, read> changed_groups_dynamic: array<vec2<u32>>;
@group(1) @binding(1)
var<storage, read> changed_chunks_dynamic: array<vec2<u32>>;
```

### Existing fixed-size `Buffer` creation + `queue.write_buffer` upload (the production precedent the chunks buffer should mirror) — `crates/bevy_naadf/src/render/prepare.rs:404-441`

```rust
let world_meta = render_device.create_buffer(&BufferDescriptor {
    label: Some("naadf_world_meta"),
    size: std::mem::size_of::<GpuWorldMeta>() as u64,
    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
    mapped_at_creation: false,
});
render_queue.write_buffer(&world_meta, 0, bytemuck::bytes_of(&world_meta_data));

// --- W4 wave-3 — placeholder entity-track buffers ...
let placeholder_entity_chunk_instances = render_device.create_buffer(&BufferDescriptor {
    label: Some("naadf_world_entity_chunk_instances_placeholder"),
    size: 20,
    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
    mapped_at_creation: false,
});
```

### Current chunks texture upload — what the migration replaces (`crates/bevy_naadf/src/render/prepare.rs:251-307`)

```rust
let chunks = render_device.create_texture(&TextureDescriptor {
    label: Some("naadf_chunks"),
    size: Extent3d {
        width: size.x, height: size.y, depth_or_array_layers: size.z,
    },
    mip_level_count: 1, sample_count: 1,
    dimension: TextureDimension::D3,
    format: TextureFormat::Rg32Uint,
    usage: TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::STORAGE_BINDING,
    view_formats: &[],
});
// ... chunk_data_paired: Vec<[u32; 2]> ... = paired (x, 0) per CPU chunk ...
render_queue.write_texture(
    TexelCopyTextureInfo { texture: &chunks, mip_level: 0, .. },
    bytemuck::cast_slice(&chunk_data_paired),
    TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(size.x * 8),  // Rg32Uint = 8 bytes per texel
        rows_per_image: Some(size.y),
    },
    Extent3d { width: size.x, height: size.y, depth_or_array_layers: size.z },
);
```

### Renderer-side chunks read site — the one call site `world_layout` slot 0 covers (`crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-295`)

```wgsl
// --- chunk lookup ---------------------------------------------------
let chunk_pos = vec3<u32>(cur_cell) / 16u;
let voxel_pos_in_chunk = vec3<u32>(cur_cell) % 16u;
let chunk_texel = textureLoad(chunks, vec3<i32>(chunk_pos), 0);
var cur_node: u32 = chunk_texel.x;

// W4 entity-track — collect this chunk's entity-pointer (`.y`) if
// non-zero ...
let entity_pointer_and_size = chunk_texel.y;
```

### Renderer-side `world_data.wgsl` view binding declaration (`:43-54`)

```wgsl
// The chunk layer: encoded chunk pair per chunk, indexed by chunk position
// (HLSL `Texture3D<CHUNKTYPE> chunks`; CHUNKTYPE = `uint2` under `ENTITIES`).
//
// **W4 (`15-design-c.md` §1.7) — texture format widened to `Rg32Uint`** so the
// chunks texture carries the per-chunk entity pointer in `.y`. The renderer's
// view binding stays `texture_3d<u32>` (a `textureLoad` returns `vec4<u32>`
// regardless of channel count); every render-side read takes `.x` explicitly
// ...
@group(0) @binding(0) var chunks: texture_3d<u32>;
```

### Existing `GpuWorldMeta` uniform carrying `size_in_chunks` (the stride the buffer indexing needs)

WGSL — `crates/bevy_naadf/src/assets/shaders/world_data.wgsl:30-40`:

```wgsl
struct GpuWorldMeta {
    // World size in chunks.
    size_in_chunks: vec3<u32>,
    // Geometry AABB minimum, in voxels — NAADF's `boundingBoxMin` ...
    bounding_box_min: vec3<f32>,
    bounding_box_max: vec3<f32>,
}
```

Rust — `crates/bevy_naadf/src/render/gpu_types.rs:155-172`:

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuWorldMeta {
    pub size_in_chunks: UVec3,
    pub _pad0: u32,
    pub bounding_box_min: Vec3,
    pub _pad1: u32,
    pub bounding_box_max: Vec3,
    pub _pad2: u32,
}
```

### Construction-side `chunks_rw` layout entry (the descriptor side that flips) (`crates/bevy_naadf/src/render/construction/chunk_calc.rs:60-88`)

```rust
pub fn construction_world_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<GpuConstructionParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_construction_world_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // chunks_rw — `texture_storage_3d<rg32uint, read_write>` (W4 §1.7).
                texture_storage_3d(TextureFormat::Rg32Uint, StorageTextureAccess::ReadWrite),
                storage_buffer_sized(false, None),     // blocks_rw
                storage_buffer_sized(false, None),     // voxels_rw
                storage_buffer_sized(false, None),     // block_voxel_count
                storage_buffer_read_only_sized(false, None), // segment_voxel_buffer
                storage_buffer_sized(false, None),     // hash_map_rw
                uniform_buffer_sized(false, Some(params_size)),  // params
                storage_buffer_read_only_sized(false, None),     // hash_coefficients
            ),
        ),
    )
}
```

### Existing CPU-side flatten of chunk_pos (matches the brief's target formula bit-for-bit) (`crates/bevy_naadf/src/render/construction/entity_handler.rs:339-345`)

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

(Inverse of `idx = z * sx * sy + y * sx + x` — the exact formula the buffer indexing needs.)

### Test-fixture chunks-texture allocation (the wider migration scope) — `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:480-516`

```rust
let chunks_texture = device.create_texture(&TextureDescriptor {
    label: Some("w3_chunks"),
    size: Extent3d {
        width: size_in_chunks[0], height: size_in_chunks[1],
        depth_or_array_layers: size_in_chunks[2],
    },
    mip_level_count: 1, sample_count: 1, dimension: TextureDimension::D3,
    format: TextureFormat::Rg32Uint,
    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC | TextureUsages::STORAGE_BINDING,
    view_formats: &[],
});
let paired_chunks: Vec<[u32; 2]> =
    initial_chunks.iter().map(|&x| [x, 0u32]).collect();
queue.write_texture(
    TexelCopyTextureInfo { texture: &chunks_texture, .. },
    bytemuck::cast_slice(&paired_chunks),
    TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(size_in_chunks[0] * 8),  // Rg32Uint = 8 B/texel
        rows_per_image: Some(size_in_chunks[1]),
    },
    Extent3d { .. },
);
```

This same pattern appears in 5 sites total (1× `prepare.rs`, 4× construction test/validate fixtures); all must flip in the migration PR.

## W4 design-doc trace

### `15-design-c.md` §1.7 — original chunks-widening intent (`docs/orchestrate/naadf-bevy-port/15-design-c.md:368-399`)

> ### 1.7 Entity-track impact on the seam — pre-emptive chunk widening (decision: NO)
>
> NAADF's entity track widens the chunk 3D texture format from `R32Uint` to
> `Rg64Uint` (NAADF: `Rg32Uint`, `settings.fxh:14-18` — `#define CHUNKTYPE uint2`).
> This is *the* breaking change to the chunk format; every existing render pass
> that reads the chunk texture would have to be updated.
>
> **Decision: the entity track (W4) owns the widening.** Not pre-emptive.
>
> Reasoning:
>
> - Pre-widening adds a `.y = 0` ignore-this-channel discipline to every
>   existing read of the chunk texture. WGSL's `textureLoad(chunks, pos, 0)`
>   returns a `vec4<u32>` regardless of format (`r32uint` → `.x` only,
>   `rg32uint` → `.x` + `.y`), and the existing render shaders read
>   `chunks[chunkPos]` (or `textureLoad(chunks, chunkPos).x`) as a scalar.

Note: the rationale here ("textureLoad returns vec4<u32> regardless of format") is **format-stable** but not **binding-type-stable** — i.e. the buffer migration preserves the `.x` / `.y` field discipline byte-for-byte but changes the binding type from `var chunks: texture_3d<u32>` to `var<storage, read> chunks: array<vec2<u32>>`. The W4 design's read-site discipline survives intact.

### `15-design-c.md` §6 assumption #6 — Rg32Uint read-write storage texture viability (`:1233-1237`)

> 6. **Wgpu's `texture_storage_3d` macro with `Rg32Uint` is supported as a
>    `read_write` storage texture format.** Per the wgpu spec, `rg32uint`
>    should be supported but it requires `Features::TEXTURE_FORMAT_NV12` —
>    actually no, `rg32uint` is a base WebGPU format. Verified in the wgpu
>    spec; W4's impl agent verifies with the first build.

The author's first-take uncertainty here is the exact assumption the buffer migration invalidates: `Rg32Uint` is supported as a *base format* but **not** at `read_write` access tier per the WebGPU spec, only at `read_only` / `write_only`. The native wgpu vendor extension lets it pass on Vulkan/Metal/DX12 (`23-design-bevy-018-downgrade.md` §5 confirms `STORAGE_READ_WRITE` requires `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`); the bare WebGPU target rejects it.

### `23-design-bevy-018-downgrade.md` §5 — the explicit wgpu format-features audit (`:368-394`)

> The wgpu `Rg32Uint` `guaranteed_format_features` row is identical between
> wgpu 27 (`wgpu-types-27.0.1/src/lib.rs:3182`) and wgpu 29
> (`wgpu-types-29.0.3/src/texture/format.rs:982`):
>   ```text
>   Rg32Uint =>  (s_ro_wo, all_flags),
>   ```
>   i.e. **`STORAGE_READ_ONLY | STORAGE_WRITE_ONLY` is in the *guaranteed* feature
>   set; `STORAGE_READ_WRITE` is NOT** — both versions require the adapter to
>   enable `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` (or equivalent)
>   for `read_write` to be valid. Since this already works at 0.19, the adapter
>   must already expose it ...

This is the load-bearing precedent: the codebase already knew `Rg32Uint` `read_write` was non-WebGPU-spec but worked on native adapters via the format-features extension. The buffer migration removes the dependency on the extension entirely — `array<vec2<u32>>` is plain `storage_buffer_sized`, WebGPU-spec-compliant without any feature gate.

### `16-impl-c-W4.md` — the `.x` sweep audit verifying the field-selector discipline carries forward (`:151-183`, abridged)

> The chunks texture format flips to `Rg32Uint` — every WGSL site that reads
> or writes `chunks` must handle the wider format. Audit summary:
>
> | file | site | pre-W4 | W4 state | notes |
> |---|---|---|---|---|
> | `assets/shaders/ray_tracing.wgsl:157` | read | `textureLoad(chunks, ..., 0).x` | unchanged | already forward-compat |
> | `assets/shaders/chunk_calc.wgsl:412` | write | `textureStore(chunks, ..., vec4<u32>(state, 0u, 0u, 0u))` | unchanged | forward-compat |
> | `assets/shaders/chunk_calc.wgsl:95` | binding | `texture_storage_3d<r32uint, read_write>` | `texture_storage_3d<rg32uint, read_write>` | format flip |
> ...
>
> **Total chunks-texture-read sites: 1 in renderer WGSL (`ray_tracing.wgsl:157`,
> already `.x`-selected) + 0 in renderer non-traversal WGSL.**

The audit cap of "1 renderer read site (`shoot_ray`) + 4 construction write/read sites" carries over to the buffer migration unchanged — all 5 sites still need touching, just to flip from `textureLoad/textureStore` to `chunks[flatten_index(...)]`. The `12-alignment-gap.md:219` B-7 "wgpu/Vulkan storage-texture barrier hazard" is **the** related-but-distinct issue the migration may incidentally fix (barriers on `texture_storage_3d<read_write>` ↔ `texture_3d<u32>` cross-binding hazards become moot when both bindings are plain `array<vec2<u32>>`).
