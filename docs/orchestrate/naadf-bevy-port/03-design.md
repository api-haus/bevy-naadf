# 03 — Architecture Design

## delegate-architect findings (2026-05-14)

This is the Bevy architecture for porting NAADF (Ulschmid et al., CGF 2026) to
`/mnt/archive4/DEV/bevy-naadf`. **Phase A is designed in implementable detail**; **Phase B is a
structural sketch only** (Phase B detailed design waits for Phase A review per 01-context.md §2
phasing + the forbidden move in §5).

All decisions sit inside the binding constraints Q1–Q4 (01-context.md §2) and D1–D4 (§2b). They
are cited, not relitigated. Every C# path / shader / line below is verified against the source
or against `02-research.md`'s verified citations — nothing here is invented.

Source-of-truth reminder (Q3): the AADF data structure is re-derived from the paper; the C# is
the correctness cross-check. The bit layouts below are the paper's three-state model encoded in
the *exact* re-encoding the C# traversal shader uses (`02-research.md` divergence #3), because
the WGSL traversal port must bit-match the algorithm.

> **Orchestrator addendum (2026-05-14, post-dates the design agent):** the phasing was
> restructured *after* this design was written. It is now **four gated phases**, not two:
> **Phase A** (this document — unchanged and accurate), **Phase A-2** (long-term-memory TAA —
> its own phase, between A and B), **Phase B** (the GI pipeline *minus* TAA — ReSTIR GI +
> sparse bilateral denoiser), **Phase C** (GPU world construction + background AADF queue +
> flood-fill edit invalidation — a speed-up/editability track, not a rendering foundation).
> See `01-context.md` §2b (D4, D5) and §2 "Phasing decision" for the canonical definitions.
> **The Phase-A content of this document (§1–§8) is unaffected and remains the implementation
> spec.** Only §9 "Phase B structural sketch" is superseded — its TAA content moves to Phase
> A-2 and its construction-shader notes move to Phase C; §9 will be redesigned per-phase when
> those phases' `design` steps run.

---

## 0. Scope of this document

| section | covers | brief item |
|---|---|---|
| §1 | `src/` module layout + scaffold disposition + Solari strip | brief 1 |
| §2 | AADF data structure — CPU types + GPU layouts + bind groups | brief 2 |
| §3 | `GrowableBuffer` — the `DynamicStructuredBuffer` equivalent | brief 3 |
| §4 | ECS decomposition — components / resources / systems, `*Handler` → ECS map | brief 4 |
| §5 | Phase-A render-graph plan — nodes, passes, bind groups, G-buffer, blit | brief 5 |
| §6 | AADF construction — where hashing + cuboid expansion run in Phase A | brief 6 |
| §7 | Resolution of all 7 research open questions | brief 7 |
| §8 | Numbered Phase-A implementation sequence | brief 8 |
| §9 | Phase B structural sketch | brief 9 |

---

## 1. `src/` module layout

Single binary crate, modules under `src/` (Q4 — **no workspace**). The scaffold's `main.rs` app
wiring, `camera.rs` (minus Solari), `hud.rs`, and the Cargo/toolchain config are reused per the
reuse audit; `scene.rs` is replaced.

```
src/
  main.rs              KEEP+EDIT  app root: DefaultPlugins + plugin list, AppArgs, schedule
  camera/
    mod.rs             NEW(from camera.rs)  free-fly spawn, PositionSplit component, toggles
    position_split.rs  NEW        PositionSplit type (IVec3 + Vec3), normalise, +/-, to/from world
  hud.rs               KEEP+EDIT  diagnostics overlay; swap Solari timing paths for NAADF nodes
  voxel/
    mod.rs             NEW        VoxelType, Material, cell-state consts, bit-pack/unpack helpers
    grid.rs            NEW        the hard-coded Phase-A test-grid builder (D2)
  aadf/
    mod.rs             NEW        AADF re-export surface
    cell.rs            NEW        Chunk/Block/Voxel cell encode/decode (paper §3.1, bit layouts)
    construct.rs       NEW        CPU-side: dense-voxel -> chunk/block/voxel buffers + hash dedup
    bounds.rs          NEW        CPU-side AADF cuboid-expansion (paper §3.3) for the test grid
  world/
    mod.rs             NEW        WorldPlugin: wires the resources + extract + render nodes
    data.rs            NEW        WorldData resource (the three buffers + sizes + CPU mirrors)
    buffer.rs          NEW        GrowableBuffer<T> abstraction (§3)
  render/
    mod.rs             NEW        NaadfRenderPlugin: registers pipelines, bind layouts, graph nodes
    extract.rs         NEW        ExtractSchedule: WorldData/camera -> render-world mirror
    prepare.rs         NEW        Prepare: upload buffers, build bind groups, camera uniforms
    graph.rs           NEW        render-graph node definitions + edges (Phase-A node set)
    pipelines.rs       NEW        ComputePipeline / RenderPipeline descriptors for the WGSL passes
    gpu_types.rs       NEW        #[repr(C)] bytemuck structs mirroring every WGSL struct/uniform
  assets/
    shaders/           NEW        WGSL files (see §5.5 for the file list + HLSL provenance)
```

`scene.rs` is **deleted** — the reuse audit marks it throwaway placeholder; `voxel/grid.rs` is
the slot that replaces it (D2).

### 1.1 Scaffold file disposition (keep / extend / replace / delete)

| file | verdict | what changes |
|---|---|---|
| `src/main.rs` | **KEEP + EDIT** | Remove `solari::{...}` import (`main.rs:20`). Remove `SolariPlugins` and the `if args.pathtracer { PathtracingPlugin }` block (`main.rs:50,58-60`). Keep `DefaultPlugins`, `FreeCameraPlugin`, `FrameTimeDiagnosticsPlugin`, `RenderDiagnosticsPlugin`, the `DlssProjectId` cfg-block (DLSS stays available for Phase B; harmless dormant). Add `WorldPlugin` + `NaadfRenderPlugin` to the `add_plugins` tuple. Replace `scene::setup_scene` in the Startup tuple with `voxel::grid::setup_test_grid`. Keep `camera::setup_camera`, `hud::setup_hud`. `AppArgs` **extends** (see §4.1). |
| `src/camera.rs` → `src/camera/mod.rs` | **EXTEND, strip Solari** | Per **D3**: delete `solari::{pathtracer::Pathtracer, prelude::SolariLighting}` import (`camera.rs:12`); delete the `Pathtracer`/`SolariLighting` insert branch (`camera.rs:66-70`); delete `CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)` (`camera.rs:62`) and the `render::render_resource::TextureUsages` import — those existed *only* for Solari. `Msaa::Off` (`camera.rs:63`) **stays** (the NAADF render path is compute + fullscreen blit, not MSAA-rasterised — keep it off). Keep `Camera3d`, `Camera{clear_color}`, `FreeCamera`, `Transform`. Keep the DLSS-RR toggle (`camera.rs:30-39,83-105`) verbatim — generic, Phase-B-useful, no Solari coupling. **Add**: a `PositionSplit` component (§4.2) inserted on the camera entity; a startup/Update system that derives `PositionSplit` from the `FreeCamera`-driven `Transform` each frame. |
| `src/camera/position_split.rs` | **NEW** | The `PositionSplit` value type (§4.2, D1). |
| `src/hud.rs` | **KEEP + EDIT** | Keep the overlay scaffold (`hud.rs:19-37`) and FPS line. Replace the renderer-mode string (`hud.rs:61-69`) and the Solari/DLSS GPU-timing `write_timing` paths (`hud.rs:90-94`) with the NAADF render-node names from `render/graph.rs` (e.g. `render/naadf_first_hit/elapsed_gpu`). Keep `write_timing` helper unchanged. The DLSS status block stays (dormant until Phase B). |
| `src/scene.rs` | **DELETE** | Throwaway Solari Cornell-box. Replaced wholesale by `voxel/grid.rs` (D2). |
| `Cargo.toml` | **KEEP + EDIT** | Per **D3**: remove `"bevy_solari"` and `"bluenoise_texture"` from the `bevy` feature list (`Cargo.toml:14-15`) — `bluenoise_texture` only ships the Solari blue-noise samples. Keep `"free_camera"`. Keep the `dlss` / `force_disable_dlss` feature plumbing (`Cargo.toml:19-27`) and the `[profile.dev*]` tuning. **Add** deps: `bytemuck` (with `derive`) for `#[repr(C)]` GPU structs (Bevy re-exports a compatible version — pin to Bevy's), and `glam` is already transitively available via `bevy`. No other new crates for Phase A — the render path is hand-built render-graph nodes, not a third-party renderer. |
| `.cargo/config.toml`, `rust-toolchain.toml` | **KEEP** | Unchanged — `mold` linker + stable toolchain are port-agnostic. |
| `README.md` | **KEEP + EDIT** (low priority) | Update the milestone roadmap to the Phase A / Phase B split; not load-bearing for `impl`. |

**Solari strip is complete after the three edits above** (Cargo feature, `main.rs` plugins,
`camera.rs` components). No `bevy_solari` symbol remains. This is D3 "strip entirely, not
dormant" — confirmed: no reference renderer kept.

---

## 2. Data structure design — the AADF three-layer cell hierarchy

Re-derived from paper §3.1–3.3 (`02-research.md` §1.1), cross-checked against the C# bit
layouts verified in `02-research.md` §1.1.2 and the `rayTracing.fxh:73` `shootRay` source
(read directly: chunks indexed `curCell/16`, voxelPosInChunk `curCell%16`, confirming
**chunk = 16³ voxels = 4³ blocks, block = 4³ voxels, voxel = 1**).

### 2.1 Layer geometry (Q3, paper §3.1)

```
chunk  = 4x4x4 blocks  = 16x16x16 voxels
block  = 4x4x4 voxels
voxel  = 1
```

Three separate buffers, one per layer — no interleaving, no tree pointer-chasing
(`02-research.md` §1.1.1).

### 2.2 Cell states & bit layout (paper §3.1; C# re-encoding per `02-research.md` divergence #3)

The paper's conceptual states are `empty` / `uniformly-full` / `mixed`. The C# **traversal
shader** re-encodes these — and the WGSL port must bit-match it because traversal reads these
bits directly (verified in `rayTracing.fxh` `shootRay`):

**Chunk / Block payload word (`u32`):**
- bit 31 — **has-children** flag (`curNode.x >> 31`). Set ⇒ this cell is *mixed*, low 30 bits =
  child pointer.
- bit 30 — **uniform-full** flag (`curNode.x & 0x40000000`). Set ⇒ *uniformly full*, low bits =
  15-bit voxel type.
- bits 31 & 30 both clear ⇒ **empty**, bits 0–29 = the AADF.
- AADF field packing:
  - **chunk**: 6 × 5-bit fields at shifts `0,5,10,15,20,25` for `-x,+x,-y,+y,-z,+z` (max
    distance 31).
  - **block**: 6 × 2-bit fields at shifts `0,2,4,6,8,10` (max distance 3).

**Voxel half-word (`u16`):**
- bit 15 — full/empty flag.
- bits 0–14 — 15-bit voxel type (if full) **or** AADF: 6 × 2-bit fields at shifts
  `0,2,4,6,8,10` (if empty).
- Voxels are **packed two per `u32`** in the voxel buffer: `voxel0 | voxel1 << 16`. A voxel
  index `i` addresses `voxels[i/2]`, masked `>> (16 * (i & 1))` (`02-research.md` divergence #4
  — flag this for `impl`, easy to get wrong).

### 2.3 CPU-side Rust types (`src/aadf/cell.rs`, `src/voxel/mod.rs`)

Idiomatic Rust, **not** a C# transliteration (Q3, forbidden move §5). The cells are *encoded as
`u32`/`u16` in the buffers*; the Rust types are encode/decode helpers + a typed view, not a
struct-of-fields stored layout.

```rust
// src/aadf/cell.rs
/// 6 axis-aligned empty-distance values, order: -x,+x,-y,+y,-z,+z.
#[derive(Clone, Copy, Default)]
pub struct Aadf6 { pub d: [u8; 6] }

pub enum ChunkCell { Empty(Aadf6), UniformFull(VoxelTypeId), Mixed(BlockPtr) }
pub enum BlockCell { Empty(Aadf6), UniformFull(VoxelTypeId), Mixed(VoxelPtr) }
pub enum VoxelCell { Empty(Aadf6), Full(VoxelTypeId) }

impl ChunkCell {
    pub fn encode(&self) -> u32 { /* bit 31/30 + 5-bit AADF packing */ }
    pub fn decode(raw: u32) -> ChunkCell { /* mirror of shootRay's bit tests */ }
}
// BlockCell::encode/decode use 2-bit AADF fields; VoxelCell encodes a u16.
```

`VoxelTypeId` = newtype `u16` (15-bit valid range). `BlockPtr`/`VoxelPtr` = newtype `u32`
(offsets into the block / voxel `u32` buffers; `VoxelPtr` is a *u32-element* offset per
divergence #4).

### 2.4 The material buffer (`src/voxel/mod.rs` — `VoxelType` system)

Follow the **C# 128-bit `Uint4` entry**, not the paper's 16-bit summary (`02-research.md`
divergence #1, §4.6 — explicitly directed). CPU type:

```rust
#[derive(Clone, Copy)]
pub struct VoxelType {
    pub material_base:  MaterialBase,   // Diffuse=0 Emissive=1 MetallicRough=2 MetallicMirror=3
    pub material_layer: MaterialLayer,  // None=0 MetallicRough=2 MetallicMirror=3
    pub roughness: f32,
    pub color_base:    Vec3,            // RGB
    pub color_layered: Vec3,            // RGB (emissive intensity for Emissive; tint for layered)
}
```

GPU form — `GpuVoxelType` `#[repr(C)]`, 16 bytes (`UVec4`), mirroring
`VoxelType.compressForRender()`:
- `data[0]` = `base | layer<<2 | f16(roughness)<<16`
- `data[1..4]` = the 6 half-floats of `color_base` + `color_layered`, packed two per `u32`.

Stored in a `GrowableBuffer<GpuVoxelType>`; element 0 reserved as the empty placeholder
(C# convention). Voxel 15-bit type ids index into it.

**Phase A uses only the geometry-relevant fields** — `color_base` for albedo, `material_base`
to know "emissive vs diffuse". The metal/mirror BRDF and emissive *contribution* are Phase B
(`02-research.md` §4.6). The 128-bit layout is built fully in Phase A so Phase B needs no
data-format change.

### 2.5 GPU buffer / texture layouts (Phase A)

| resource | wgpu type | element | role | grows? |
|---|---|---|---|---|
| `chunks` | **`Texture3D`, `R32Uint`** | `u32` per chunk | the chunk layer, indexed `[chunkPos]` | no (world fixed-size in Phase A) |
| `blocks` | `Buffer` (STORAGE) via `GrowableBuffer<u32>` | `u32` per block | 64 consecutive blocks per mixed chunk | yes |
| `voxels` | `Buffer` (STORAGE) via `GrowableBuffer<u32>` | `u32` = 2 packed voxels | 32 `u32` per mixed block | yes |
| `voxel_types` | `Buffer` (STORAGE) via `GrowableBuffer<UVec4>` | 16-byte material | the material buffer (§2.4) | yes |
| `first_hit_data` | `Buffer` (STORAGE, RW) | `UVec4` per pixel | Phase-A G-buffer (§5.3) | resized on window resize |
| camera uniform | `Buffer` (UNIFORM) | `GpuCamera` (§5.2) | int+frac camera + matrices | no |
| render-params uniform | `Buffer` (UNIFORM) | `GpuRenderParams` | screen size, frame count, sun dir/color, jitter, flags | no |

**Chunk = 3D texture** (open question #3, resolved §7.3): keep the C# choice. `R32Uint` for
Phase A (entities deferred ⇒ no `Rg32Uint` widening yet). The 3D texture gives free
3D-coordinate indexing in WGSL (`textureLoad(chunks, chunkPos, 0)`) and matches `chunkCalc.fx`
/ `rayTracing.fxh` which declare `RWTexture3D<CHUNKTYPE> chunks`. Note: wgpu storage textures
do not support `R32Uint` *read-write* in one bind group on all backends — Phase A only ever
**reads** `chunks` in the render pass and **writes** it during construction; use two bind-group
variants (write-only `texture_storage_3d<r32uint, write>` for construction, sampled/`textureLoad`
for traversal) or, simplest for Phase A, build the chunk texture **CPU-side and upload** (see
§6) so the render pass only ever reads it.

### 2.6 Bind group plan (Phase A)

Two stable bind-group layouts, shared across the Phase-A passes:

- **`@group(0)` — world data** (read-only in render passes):
  `chunks` (texture_3d<u32>), `blocks` (storage<array<u32>, read>),
  `voxels` (storage<array<u32>, read>), `voxel_types` (storage<array<vec4<u32>>, read>),
  plus a small `WorldMeta` uniform (`bounding_box_min/max` in voxels, `size_in_chunks`).
- **`@group(1)` — frame data**: `camera` uniform, `render_params` uniform,
  `first_hit_data` (storage<array<vec4<u32>>, read_write>).

`renderFirstHit` binds `@group(0)` + `@group(1)`. `renderFinal` binds only
`first_hit_data` + `render_params` (its own small layout — it is a fullscreen pass, see §5.4).
This mirrors the C# `WorldData.setEffect` "bind all voxel buffers" call collapsing into one
stable layout (`02-research.md` §4.2).

---

## 3. The growable GPU buffer abstraction — `GrowableBuffer<T>` (`src/world/buffer.rs`)

The wgpu/Bevy equivalent of `Common/DynamicStructuredBuffer.cs` (read directly; research open
question #2). The C# `SetNewMinCount(count, factor)` reallocs to `count * factor`, caps at
`0xFFFF0000` bytes, and `Resize` copies old→new via `CopyData` (or the chunked
`CopyIntoStructuredBufferLarge` for 4-byte elements).

### 3.1 Design

```rust
pub struct GrowableBuffer<T: Pod> {
    buffer: Buffer,            // STORAGE | COPY_SRC | COPY_DST
    capacity: u64,             // element count
    len: u64,                  // logical element count in use
    label: &'static str,
    _t: PhantomData<T>,
}
```

- **`reserve(min_capacity, device, queue, encoder)`**: if `min_capacity > capacity`, compute
  `new_cap = max(min_capacity, capacity * GROWTH_FACTOR)` with **`GROWTH_FACTOR = 2`** (paper
  §3.2 "resize by 100%" / `02-research.md` open question #2 spec "growth factor 2×"); allocate a
  new `Buffer`; `encoder.copy_buffer_to_buffer(old, 0, new, 0, capacity * size_of::<T>())`;
  swap. Old buffer dropped after the encoder submits (Bevy's frame boundary handles lifetime; or
  stash in a one-frame "retired buffers" `Vec` to be safe).
- **`write(offset, data, queue)`**: `queue.write_buffer`.
- **`as_entire_binding()` / `binding(len)`**: for bind-group construction.

### 3.2 wgpu buffer-size limit — chunked copies?

**Resolved (open question #2):** for Phase A, **no chunked copies needed.** Reasoning:
- The C# `Helper`/`dataCopy.fx` chunked-copy path exists purely to work around **DX11's**
  ~2 GB structured-buffer copy limit (`02-research.md` §3 `Helper` row). wgpu/Vulkan does not
  have that specific limit.
- `copy_buffer_to_buffer` in wgpu has no special chunking requirement; the only constraint is
  `Limits::max_buffer_size` (default 256 MiB, raisable). The Phase-A **hard-coded test grid**
  (D2) is small — a few hundred chunks at most — so `blocks`/`voxels` stay well under any limit.
- **Action for Phase A:** `GrowableBuffer` does a single `copy_buffer_to_buffer`. Add a
  `debug_assert!` that `new_cap * size_of::<T>() <= device.limits().max_buffer_size` so the
  ceiling is *visible*. If Phase B's larger worlds ever approach it, a chunked-copy loop is a
  localised addition to `reserve` — note it as a Phase-B extension point, do not build it now
  (no gold-plating).

### 3.3 Who uses it in Phase A

`blocks`, `voxels`, `voxel_types` (§2.5). In Phase A all three are sized **once** at test-grid
build time (build-once, §6) — so `reserve` is exercised but growth is rare. The
`changedChunks/Blocks/Voxels` dynamic buffers and the hash-map buffer (which *do* grow
repeatedly) belong to the **editing / GPU-construction** path, deferred — see §6 / §7.4.

---

## 4. ECS decomposition (Phase A)

NAADF's C# `*Handler` orchestration is **not ported verbatim** (forbidden move §5). The map:

| C# construct | Bevy equivalent |
|---|---|
| `WorldHandler` (top orchestrator) | `WorldPlugin` + `NaadfRenderPlugin` (plugins, not a god-object) |
| `WorldData` (owns the 3 buffers + CPU mirror + sub-handlers) | `WorldData` **main-world `Resource`** (the three buffers + sizes + CPU mirrors) + `WorldGpu` **render-world `Resource`** (the GPU handles after extract/prepare) |
| `BlockHashingHandler` (block dedup hash map) | **deferred** — Phase A builds the test grid CPU-side with an in-memory `HashMap`-based dedup (§6); no GPU hash-map resource in Phase A |
| `ChangeHandler` (flood-fill AADF invalidation) | **deferred** — Phase A is build-once, no edits (§6, §7.4) |
| `WorldBoundHandler` (background chunk-AADF queue) | **deferred for chunks** — Phase A computes *all* AADFs CPU-side at build time (§6); the background-queue render node is Phase-B-or-later |
| `VoxelTypeHandler` | `VoxelTypes` `Resource` (a `Vec<VoxelType>` + the `GrowableBuffer<UVec4>` handle) |
| `EditingHandler` / `EditingTools` | **deferred** (editor concern, §5 forbidden move + brief) |
| `EntityHandler` / `EntityData` | **deferred Phase-A sub-feature**, feature-flagged (§7.5) |
| `WorldGenerator` / `WorldGeneratorModel` | **deferred** (D2 — replaced by `voxel/grid.rs`) |
| `WorldRender` + `Versions/WorldRenderAlbedo` | the Phase-A render-graph nodes (§5) |
| MonoGame `Effect` + `setEffect` bind calls | Bevy `ComputePipeline`/`RenderPipeline` + `BindGroupLayout` (§2.6) |

### 4.1 `AppArgs` extension (`src/main.rs`)

Reuse-audit says "extend". Phase A adds:
```rust
pub struct AppArgs {
    pub grid_preset: GridPreset,   // which hard-coded test grid to build (D2)
    pub taa: bool,                 // false in Phase A (D4) — wired but defaults off
    // `pathtracer` field DELETED — Solari is gone (D3)
}
```
Parsed once at startup as today (`main.rs:34-36` pattern kept).

### 4.2 `PositionSplit` — camera (D1, `src/camera/position_split.rs`)

Faithful port of `Common/Camera.cs`'s `PositionSplit` (read directly): `Point3 integer` +
`Vector3 frac`, with `updateInternals` folding the floor of `frac` into `integer` so
`frac ∈ [0,1)³`.

```rust
#[derive(Clone, Copy, Default)]
pub struct PositionSplit { pub pos_int: IVec3, pub pos_frac: Vec3 }

impl PositionSplit {
    pub fn from_world(p: Vec3) -> Self;          // floor split
    pub fn to_world(self) -> Vec3;               // pos_int as f32 + pos_frac
    pub fn normalise(&mut self);                 // fold floor(frac) into pos_int
    // operator + / - mirror the C# (add components, then normalise)
}
```

**As a Bevy component** it is attached to the camera entity alongside `Camera3d` +
`FreeCamera` + `Transform`. A `camera::sync_position_split` system (in the `Update` schedule,
after `FreeCameraPlugin`'s movement system) reads the `FreeCamera`-driven `Transform.translation`
and writes `PositionSplit::from_world(...)`. So Phase A still uses Bevy's free-fly camera for
*input* (reuse-audit "reuse"), but the *render-side position* is the int+frac split (D1).

The `extract.rs` step copies `PositionSplit` + the camera's view/projection into `GpuCamera`
(§5.2). Threading detail in §7.1.

### 4.3 Components (Phase A)

Phase A is **light on components** — the voxel world is a small set of big GPU resources, not
per-voxel entities. Components:
- `PositionSplit` — on the camera entity (§4.2).
- (existing) `Camera3d`, `Camera`, `FreeCamera`, `Transform`, `Msaa::Off` — on the camera.
- (existing) `HudText` — on the HUD node.

No per-voxel / per-chunk entities in Phase A. (NAADF's chunks aren't ECS entities either.)

### 4.4 Resources (Phase A)

**Main world:**
- `AppArgs` (extended, §4.1).
- `WorldData` — `{ chunks_cpu: Vec<u32>, blocks_cpu: Vec<u32>, voxels_cpu: Vec<u32>,
  size_in_chunks: UVec3, bounding_box: IAabb3, dirty: bool }`. The CPU mirror + geometry. In
  Phase A this is built once by `voxel/grid.rs` and never edited; `dirty` triggers the
  one-time GPU upload.
- `VoxelTypes` — `{ types: Vec<VoxelType>, dirty: bool }`.

**Render world** (populated by `ExtractSchedule` + `Prepare`):
- `WorldGpu` — `{ chunks: Texture + TextureView, blocks: GrowableBuffer<u32>,
  voxels: GrowableBuffer<u32>, voxel_types: GrowableBuffer<UVec4>, world_meta: Buffer }`.
- `FrameGpu` — `{ camera: Buffer, render_params: Buffer, first_hit: Buffer,
  bind_group_world: BindGroup, bind_group_frame: BindGroup }`.
- `NaadfPipelines` — the cached `ComputePipeline` / `RenderPipeline` ids + `BindGroupLayout`s
  (a `FromWorld` resource, the standard Bevy specialised-pipeline pattern).

### 4.5 Systems (Phase A)

**Startup (`Startup` schedule, main world):**
1. `voxel::grid::setup_test_grid` — builds `WorldData` + `VoxelTypes` (D2, §6).
2. `camera::setup_camera` — spawns the camera (scaffold system, Solari-stripped, + `PositionSplit`).
3. `hud::setup_hud` — scaffold system unchanged.

**Update (`Update` schedule, main world):**
- `camera::sync_position_split` — `Transform` → `PositionSplit` (§4.2).
- `camera::toggle_dlss` — scaffold system kept verbatim (dormant in Phase A).
- `hud::update_hud` — scaffold system, timing paths re-pointed (§1.1).

**ExtractSchedule (render world) — `render/extract.rs`:**
- `extract_world` — on `WorldData.dirty`, clone the CPU mirrors into a render-world staging
  resource (or mark a flag); extract `VoxelTypes` likewise. After the first frame this is a
  cheap no-op (build-once).
- `extract_camera` — copy `PositionSplit` + camera matrices into a render-world `ExtractedCamera`-
  style resource.

**Render `Prepare` set — `render/prepare.rs`:**
- `prepare_world_gpu` — first frame: create `chunks` texture + upload, create
  `blocks`/`voxels`/`voxel_types` `GrowableBuffer`s + upload, build `bind_group_world`. Later
  frames: no-op (build-once).
- `prepare_frame_gpu` — every frame: `write_buffer` the `GpuCamera` + `GpuRenderParams`
  uniforms; (re)create `first_hit` buffer on resize; rebuild `bind_group_frame`.

**Render graph — `render/graph.rs`:** see §5.

---

## 5. Phase-A render-graph plan

Faithful WGSL port of NAADF's albedo render path (Q2). The C# `WorldRenderAlbedo` runs three
effects: `albedo/renderFirstHit` → (optional) `albedo/renderTaaSampleReverse` →
`albedo/renderFinal` (`02-research.md` §4.11, §5.3). **Per D4, Phase A ships TAA off** — so the
Phase-A graph is **two nodes**: first-hit, then final blit. The TAA node is a Phase-B addition
(its slot is named below so Phase B knows where it goes).

### 5.1 Render-graph nodes & edges

Custom Bevy render-graph nodes inserted into the `Core3d` sub-graph (or a dedicated
`NaadfRender` sub-graph driven from `Core3d`). Phase-A node set:

```
[ NaadfFirstHitNode ]  --(first_hit_data ready)-->  [ NaadfFinalBlitNode ]  -->  view target
        |                                                   |
   compute pass                                       fullscreen render pass
   binds @group(0) world + @group(1) frame            binds first_hit + render_params
```

- **`NaadfFirstHitNode`** — a `Node` running one compute pass: dispatch
  `ceil(screen_w*screen_h / 64)` workgroups of the `naadf_first_hit` compute shader
  (`[numthreads(64,1,1)]` in the HLSL — verified in the albedo `renderFirstHit.fx` read:
  `[numthreads(64, 1, 1)] void calcFirstHit(...)`). Writes `first_hit_data`.
- **`NaadfFinalBlitNode`** — a fullscreen render pass over the view's color target running the
  `naadf_final` fragment shader; reads `first_hit_data`, tonemaps, writes the swapchain.

Graph edges: `NaadfFirstHitNode → NaadfFinalBlitNode`, and `NaadfFinalBlitNode` ordered before
the UI pass (so the HUD draws on top). Both nodes register GPU timing spans so `hud.rs`'s
`write_timing` can show them.

### 5.2 The int+frac camera uniforms (D1)

`GpuCamera` `#[repr(C)]` mirrors the C# albedo `renderFirstHit.fx` uniform set (verified by
direct read — `matrix invCamMatrix; int camPosIntX,Y,Z; float3 camPosFrac;`):

```rust
#[repr(C)]
struct GpuCamera {
    inv_view_proj: Mat4,   // invCamMatrix — for getRayDir
    cam_pos_int:   IVec3,  // camPosIntX/Y/Z
    _pad0: u32,
    cam_pos_frac:  Vec3,   // camPosFrac
    _pad1: u32,
}
```

WGSL `getRayDir` takes `inv_view_proj` + pixel coords + jitter (the albedo
`renderFirstHit.fx` calls `getRayDir(invCamMatrix, pixelPos, screenWidth, screenHeight,
taaJitter)`). The DDA (`shootRay`) takes `cam_pos_int` + `cam_pos_frac` separately as
`rayOriginInt` / `rayOriginFrac` — confirmed in the read: `shootRay(int3 rayOriginInt,
float3 rayOriginFrac, ...)`. All ray-origin and bounding-box math stays in int+frac space;
no f32 world-position is ever formed in the shader (D1).

`GpuRenderParams` `#[repr(C)]` mirrors the rest of the albedo `renderFirstHit.fx` uniforms:
`screen_width`, `screen_height`, `frame_count`, `rand_counter`, `taa_index`, `sky_sun_dir`
(vec3), `sun_color` (vec3), `taa_jitter` (vec2), and the `bool` flags
(`show_ray_step`, `check_sun`, `is_taa`) packed as a `u32` bitfield. Phase A sets
`is_taa = 0` (D4).

### 5.3 The Phase-A G-buffer layout (`first_hit_data`)

Faithful to the C# albedo path (verified by direct read of `compressFirstHitData` in the
albedo `renderFirstHit.fx`): one **`UVec4` per pixel**, in a `STORAGE` buffer
`first_hit_data : array<vec4<u32>>` indexed `pixel.x + pixel.y * screen_width`.

Per the verified `compressFirstHitData`:
- `.x = entity | (normTang0 << 15)`
- `.y = 1 | (normTang1 << 15)`  (the `1` = "is hit" flag)
- `.z = voxelTypeRaw | (normTang2 << 15)`
- `.w = (f16(dist) & 0x7FFF) | (normTang3 << 15)`

where each `normTang` is the packed `(3-bit normal index, distance-along-normal)` plane code
(`02-research.md` §1.2.1). Phase A's first-hit only fills **plane 0** meaningfully (no specular
bounces — that is `base/renderFirstHit.fx`, Phase B); planes 1–3 are written `HIT_UNDEFINED`.
This is exactly what the albedo shader does (`normTangs = uint4(HIT_NOTHING, HIT_UNDEFINED,
HIT_UNDEFINED, HIT_UNDEFINED)` initial, only `normTangs[0]` set on hit — verified in the read).

**Phase A does not pre-design for TAA's 64-bit sample layout** (D4) — `taaSamples` /
`taaSampleAccum` buffers are *not created* in Phase A. The albedo `renderFirstHit.fx` writes a
TAA sample, but with `is_taa = 0` the Phase-A WGSL port simply **skips that write** and the
final blit reads colour directly from a per-pixel colour the first-hit pass also writes.

> **Phase-A blit-source decision:** the C# `albedo/renderFinal.fx` reads `taaSampleAccum`
> (verified by direct read — `StructuredBuffer<uint2> taaSampleAccum`, weight in `.x&0xFFFF`,
> RGB as f16 in `.x>>16,.y&0xFFFF,.y>>16`). With TAA off there is no accum buffer. **Resolution:**
> Phase A adds a fourth `UVec4` channel is not enough — instead Phase A's `naadf_first_hit`
> writes a **second small buffer** `shaded_color : array<vec2<u32>>` (one `vec2<u32>` per pixel,
> RGB packed as 3×f16 + a `1.0` weight — exactly the `taaSampleAccum` element format) so
> `naadf_final` is a **near-verbatim port of `albedo/renderFinal.fx`** reading that buffer. This
> keeps `renderFinal` faithful (Q2) and gives Phase B a drop-in seam: Phase B replaces
> `shaded_color` with the real `taaSampleAccum` and inserts the TAA node between first-hit and
> final. (`shaded_color` lives in `@group(1)` alongside `first_hit_data`.)

### 5.4 The final blit (`NaadfFinalBlitNode`)

C# `albedo/renderFinal.fx` is a **VS+PS pair** drawn over a unit cube to run a fullscreen PS
(`02-research.md` divergence #9; verified by direct read — `technique SpriteDrawing { pass P0 }`
with `MainVS`/`MainPS`). The Bevy port replaces the cube trick with a **standard fullscreen
triangle pass** (`render::render_resource` fullscreen-shader vertex helper). The fragment
shader `naadf_final.wgsl` is a near-verbatim port of `MainPS`: read `shaded_color[pixelIndex]`,
divide RGB by `max(1, weight)`, apply the verified tonemap
(`lerp(curColor/(exposure+luminance), tv, tv)` with `tv = curColor/(1+curColor)`), output.
`exposure` comes from `GpuRenderParams`. The `HDR` `#ifdef` branches become a const-or-uniform
toggle (Phase A: HDR off).

### 5.5 WGSL file list & HLSL provenance

`src/assets/shaders/` — each WGSL file names the HLSL `.fx`/`.fxh` it derives from
(`02-research.md` §5). Phase-A set:

| WGSL file | derives from (HLSL) | contents |
|---|---|---|
| `naadf_first_hit.wgsl` | `render/versions/albedo/renderFirstHit.fx` (`calcFirstHit`) | the compute entry: per-pixel ray setup, `rayAABB` volume test, `shootRay`, simple sun+ambient, write `first_hit_data` + `shaded_color` |
| `naadf_final.wgsl` | `render/versions/albedo/renderFinal.fx` (`MainPS`) | fullscreen tonemap fragment |
| `ray_tracing.wgsl` | `render/rayTracing.fxh` | **`shootRay` — the AADF DDA** (chunk→block→voxel descent + AADF empty-cuboid skip), `rayAABB`, `RayResult`, `MAX_RAY_STEPS_*`. The Phase-A core. Entity `#ifdef ENTITIES` branch **omitted** (§7.5). |
| `render_pipeline_common.wgsl` | `render/common/commonRenderPipeline.fxh` | `HIT_*`/`SURFACE_*` consts, `VoxelType` + `decompressVoxelType`, `FirstHitResult`, `NORMAL[8]` LUT, `getRayDir`, `compressFirstHitData`. **Phase-A subset** — specular-path `getHitDataFromPlanes` reconstruction is Phase B (`02-research.md` §5.1 "splits"). |
| `ray_tracing_common.wgsl` | `render/common/commonRayTracing.fxh` | PCG/xoroshiro RNG (`initRand`), octahedral normal encode/decode, `FLATTEN_INDEX`. **Phase-A subset** — VNDF/GGX sampling is Phase B. |
| `common.wgsl` | `render/common/common.fxh` + `commonConstants.fxh` + `settings.fxh` | `PI`, `FLATTEN_INDEX` macro→fn, the `CHUNKTYPE` choice (Phase A: `u32`, no `ENTITIES`). |
| `world_data.wgsl` | the `RWStructuredBuffer`/`RWTexture3D` declarations in `rayTracing.fxh` + `chunkCalc.fx` | the `@group(0)` bind declarations: `chunks`, `blocks`, `voxels`, `voxel_types`, `world_meta`. |

WGSL has no `#include`; Bevy's shader system supports `#import` (naga oil). Each file above is a
Bevy shader-`#import` module; `naadf_first_hit.wgsl` imports the rest. **Note for `impl`:** the
HLSL `.fxh` headers split A/B (`02-research.md` §5.5) — port only the Phase-A functions now,
expect to *add* B-only functions to the same WGSL modules when Phase B starts.

> **Construction shaders are NOT in the Phase-A render graph.** `chunkCalc.fx` (Algorithm 1),
> `boundsCalc.fx`/`boundsCommon.fxh` (AADF expansion), `worldChange.fx`, `mapCopy.fx`,
> `generatorModel.fx`, `typeMapping.fx` are all *world-construction* shaders. Per §6 + §7.4
> Phase A builds the world **CPU-side**, so **none of these are ported in Phase A.** They are
> the GPU-construction phase (post-Phase-A). This is a deliberate scope cut — see §6.

---

## 6. AADF construction in the Phase-A pipeline

**Phase-A decision: build-once, CPU-side.** The hard-coded test grid (D2) is small and static —
there is no editing, no flood-fill, no streaming in Phase A. So Phase A does **not** port the
GPU hashing construction (`chunkCalc.fx` Algorithm 1) or the GPU background AADF queue
(`boundsCalc.fx`). It implements their *result* on the CPU at startup. This is explicitly
called out as the Phase-A approach (brief item 6 permits "build-once for the hard-coded grid").

### 6.1 Phase-A construction path (`src/voxel/grid.rs` + `src/aadf/`)

1. **`voxel/grid.rs` — author a dense voxel volume.** `setup_test_grid` builds a dense
   `Vec<VoxelTypeId>` over a small fixed extent (e.g. 4×2×4 chunks = 64×32×64 voxels) from
   simple primitives: a ground slab, a few axis-aligned boxes, a sphere, one emissive box.
   Also builds the `VoxelTypes` palette (a handful of `VoxelType`s — ground, walls, emissive).
   `GridPreset` (in `AppArgs`) selects between a couple of these.
2. **`aadf/construct.rs` — dense → three-layer buffers + dedup.** A faithful CPU re-derivation
   of paper Algorithm 1 (`02-research.md` §1.1.3), *not* a transliteration of `chunkCalc.fx`:
   - For each block (4³ voxels): if all 64 equal → `BlockCell::UniformFull`; else dedup against
     an in-memory `HashMap<[VoxelTypeId;64], VoxelPtr>` (the CPU stand-in for
     `BlockHashingHandler`'s GPU hash map — exact hash function not needed CPU-side, a Rust
     `HashMap` keyed on the 64-voxel array is correct and simpler), appending packed voxels
     (2-per-`u32`, §2.2) to `voxels_cpu` on a miss.
   - For each chunk (4³ blocks): all-equal → `ChunkCell::UniformFull`; all-empty →
     `ChunkCell::Empty`; else reserve 64 consecutive slots in `blocks_cpu`, write the child
     pointer, write the 64 blocks.
3. **`aadf/bounds.rs` — AADF cuboid expansion.** A faithful CPU re-derivation of paper §3.3
   (`02-research.md` §1.1.4): for every *empty* cell, the alternating-axis cuboid-expansion
   producing the 6 per-direction distances, bounded by the max field size (block/voxel → 3,
   chunk → 31) and by the containing upper-layer cell. The paper's O(3·d·n)
   "merge with neighbour's already-computed cuboid" optimisation is *optional* for Phase A's
   tiny static grid — a straightforward per-cell expansion is acceptable and simpler; note the
   linear-merge optimisation as an `impl` choice, not a requirement. Block/voxel AADFs (`d≤3`,
   3 iterations) and chunk AADFs (`d≤31`) use the same routine with different caps. Results are
   packed into the cell `u32`/`u16` words (§2.2).
4. **Upload.** `WorldData.dirty = true` ⇒ `prepare_world_gpu` (§4.5) creates the `chunks` 3D
   texture (CPU-built, write-uploaded — §2.5 resolves the storage-texture read/write concern),
   the `blocks`/`voxels` `GrowableBuffer`s, and `voxel_types`, builds `bind_group_world`. Done
   once.

### 6.2 What this defers (and why it is safe)

- **GPU Algorithm 1 (`chunkCalc.fx`)** — deferred. It is a *performance* construction path for
  large GPU-generated worlds; the Phase-A static grid does not need it. The CPU path produces a
  bit-identical buffer layout, so the GPU path is a drop-in replacement later.
- **GPU background chunk-AADF queue (`boundsCalc.fx` / `WorldBoundHandler`)** — deferred.
  Justified: that machinery exists to amortise AADF recompute *across frames after edits*. Phase
  A has no edits, so all AADFs can be computed up front in one CPU pass.
- **Flood-fill invalidation (`ChangeHandler` / `worldChange.fx`)** — deferred. No edits in
  Phase A ⇒ nothing to invalidate. (Brief explicitly: "no editing/flood-fill needed yet".)

The seam is clean: Phase A produces the same `chunks`/`blocks`/`voxels` buffer contents the GPU
path would; a later GPU-construction phase swaps the *producer* without touching the *consumer*
(the traversal shader).

---

## 7. Resolution of the 7 research open questions (`02-research.md` §7)

### 7.1 `PositionSplit` int+frac camera — **decided by D1: port it.** How it threads:

- CPU: `PositionSplit` value type in `src/camera/position_split.rs` (faithful port of
  `Common/Camera.cs`, §4.2); attached as a component on the camera entity; a `sync_position_split`
  Update system derives it from the `FreeCamera` `Transform`.
- Extract: `render/extract.rs` copies `PositionSplit` + the camera's `inv_view_proj` matrix
  into the render world.
- Prepare: `render/prepare.rs` writes `GpuCamera { inv_view_proj, cam_pos_int, cam_pos_frac }`
  (§5.2) into the camera uniform buffer every frame.
- Shaders: every Phase-A WGSL pass that needs the camera binds `GpuCamera`. `getRayDir` uses
  `inv_view_proj`; `shootRay` takes `cam_pos_int`/`cam_pos_frac` as `rayOriginInt`/
  `rayOriginFrac`; `rayAABB` and all bounding-box math stay in int+frac space. **No f32 world
  position is reconstructed in any shader** — this is the faithful NAADF camera-relative
  rendering D1 asks for. The G-buffer distance (`first_hit_data.w`) is a distance-along-ray, not
  a world position, so it is already split-agnostic.

### 7.2 `DynamicStructuredBuffer` → wgpu wrapper — **see §3.** `GrowableBuffer<T>`:
realloc + single `copy_buffer_to_buffer` on growth, `GROWTH_FACTOR = 2`. **No chunked copies in
Phase A** — the DX11 size limit the C# `Helper`/`dataCopy.fx` works around does not apply to
wgpu/Vulkan, and the static test grid is small. A `debug_assert` keeps `max_buffer_size`
visible; chunked-copy is a noted Phase-B extension point, not built now.

### 7.3 Chunk layer — 3D texture vs. buffer — **decided: 3D texture (`Texture3D`, `R32Uint`).**
Justification: (a) the C# uses `Texture3D<uint>` and the traversal shader `rayTracing.fxh`
declares `RWTexture3D<CHUNKTYPE> chunks` indexed directly by `chunkPos` — keeping the texture
gives a near-verbatim WGSL traversal port (Q2 faithfulness); (b) 3D textures give free
3D-coordinate addressing (`textureLoad(chunks, chunkPos, 0)`) with no manual flatten; (c) the
`ENTITIES` widening to `Rg32Uint` (the C# `Rg64Uint` analogue — wgpu has no `Rg64Uint`, see
§7.5) is a clean later format change. The only wgpu wrinkle — storage textures don't universally
support read-write `R32Uint` in one bind group — is sidestepped in Phase A because the chunk
texture is **CPU-built and upload-only**, then **read-only** in the render pass (§2.5, §6.1).
Block/voxel layers stay plain storage **buffers** (matching the C# and the `GrowableBuffer`
abstraction).

### 7.4 Phase-A content path — **decided by D2: hard-coded test grid.** Designed concretely in
§6.1: `voxel/grid.rs` authors a dense small voxel volume from primitives + a small `VoxelType`
palette; `aadf/construct.rs` does CPU Algorithm-1 construction with `HashMap`-based block dedup;
`aadf/bounds.rs` does CPU AADF cuboid expansion; `prepare_world_gpu` uploads once. **No `.vox`
reader, no `WorldGenerator` port** — both deferred (D2). `scene.rs` is deleted; `voxel/grid.rs`
is its replacement slot.

### 7.5 Entities — **decided: explicitly-deferred Phase-A sub-feature, feature-flagged.** Phase A
is designed **entity-free** (matching `02-research.md` §1.1.7's recommendation and §7.5):
- A Cargo feature `entities` (the analogue of the C# `BuildFlags.Entities` / `settings.fxh`
  `#define ENTITIES`), **off by default**.
- Phase A: `chunks` is `R32Uint` (not the widened format); `ray_tracing.wgsl` omits the
  `#ifdef ENTITIES` sub-traversal branch; no `EntityHandler`/`EntityData` resources, no
  `entityUpdate.fx` port.
- Note for whenever entities are picked up: wgpu has **no `Rg64Uint`** texture format (the C#
  uses it for the widened chunk texture). The entity port will either use `Rg32Uint` (two
  `u32`s — sufficient: the C# `.y` channel is a 24-bit pointer + 8-bit counter, fits in one
  `u32`) or move the chunk layer to a storage buffer of `vec2<u32>`. Flag this as an entity-port
  decision, not a Phase-A one.

### 7.6 `taaSampleMaxAge` / long-term TAA — **decided by D4: TAA is Phase B, Phase A ships TAA
off.** Phase A's render graph is two nodes (first-hit → final blit), no TAA node. `is_taa = 0`
in `GpuRenderParams`; `naadf_first_hit.wgsl` skips the TAA-sample write and instead writes the
`shaded_color` buffer that `naadf_final.wgsl` reads (§5.3). The 64-bit TAA sample layout is
**not** pre-designed (D4). Phase B inserts the TAA node and swaps `shaded_color` for the real
`taaSamples`/`taaSampleAccum` ring buffers — the §5.3 design names this seam explicitly.

### 7.7 Solari strip-vs-dormant — **decided by D3: strip entirely.** Executed in §1.1: remove
the `bevy_solari` + `bluenoise_texture` Cargo features, delete `SolariPlugins` +
`PathtracingPlugin` from `main.rs`, delete the `SolariLighting`/`Pathtracer` components +
`CameraMainTextureUsages` STORAGE_BINDING from `camera.rs`. No reference renderer kept. (DLSS
plumbing stays — it is independent of Solari and is Phase-B-relevant; `Msaa::Off` stays — the
NAADF path is compute-based.)

---

## 8. Numbered Phase-A implementation sequence

Each step ends at a compiling state; runnable states are marked **▶**. The `impl` group
executes these in order.

1. **Strip Solari (D3).** Edit `Cargo.toml` (remove `bevy_solari`, `bluenoise_texture` features),
   `src/main.rs` (remove Solari imports/plugins/pathtracer block), `src/camera.rs` (remove
   Solari components + import). Delete `src/scene.rs`. Replace the `scene::setup_scene` schedule
   entry with a temporary empty system. **▶ Compiles & runs** — empty black window + HUD (HUD
   timing lines just won't populate yet).
2. **Module skeleton + `AppArgs` extension.** Create the empty module tree (§1): `camera/`,
   `voxel/`, `aadf/`, `world/`, `render/`, `assets/shaders/`. Extend `AppArgs` (§4.1). Add
   `bytemuck` to `Cargo.toml`. **Compiles.**
3. **`PositionSplit` + camera (D1).** `src/camera/position_split.rs` (the value type, §4.2);
   add the `PositionSplit` component to `setup_camera`; add `sync_position_split` to `Update`.
   **▶ Compiles & runs** — camera flies, `PositionSplit` updates (verify with a debug log).
4. **CPU data structure — `voxel/mod.rs` + `aadf/cell.rs`.** `VoxelType`, `Material*` enums,
   cell-state consts, `Aadf6`, `ChunkCell`/`BlockCell`/`VoxelCell` with `encode`/`decode`
   (§2.2–2.4). Unit-test `encode`∘`decode` round-trips against the bit layout. **Compiles + tests.**
5. **CPU construction — `aadf/construct.rs` + `aadf/bounds.rs`.** Dense→three-layer with
   `HashMap` dedup (§6.1 step 2); AADF cuboid expansion (§6.1 step 3). Unit-test on a tiny
   hand-checked volume. **Compiles + tests.**
6. **Hard-coded test grid — `voxel/grid.rs` (D2).** `setup_test_grid` builds the dense volume +
   `VoxelTypes` palette, runs construction, fills `WorldData`. Wire it into `Startup`. **▶
   Compiles & runs** — still black window, but `WorldData` is populated (verify with a debug log
   of chunk/block/voxel counts).
7. **`GrowableBuffer` — `world/buffer.rs` (§3).** The wrapper + `reserve`/`write`. Unit-test the
   grow-and-copy path with a small buffer. **Compiles + tests.**
8. **Render-world resources + extract/prepare — `render/{extract,prepare,gpu_types}.rs`,
   `world/data.rs`.** `WorldGpu`, `FrameGpu`, `GpuCamera`/`GpuRenderParams`/`GpuVoxelType`
   `#[repr(C)]` structs; `extract_world`/`extract_camera`; `prepare_world_gpu` (create + upload
   `chunks` texture + `blocks`/`voxels`/`voxel_types` buffers, build `bind_group_world`);
   `prepare_frame_gpu` (camera/params uniforms, `first_hit` + `shaded_color` buffers, build
   `bind_group_frame`). No render node yet. **Compiles** — buffers upload (verify via render-doc
   or a readback debug path).
9. **WGSL — world data + traversal.** Port `common.wgsl`, `world_data.wgsl`,
   `ray_tracing_common.wgsl` (RNG/oct subset), `render_pipeline_common.wgsl` (Phase-A subset),
   and `ray_tracing.wgsl` (`shootRay` AADF DDA — the core; entity branch omitted). These don't
   run yet — they compile as `#import` modules. Verify with `naga` validation. **Compiles.**
10. **WGSL — first-hit + pipeline.** `naadf_first_hit.wgsl` (compute, ports albedo
    `renderFirstHit.fx` `calcFirstHit`); `render/pipelines.rs` (`ComputePipeline` +
    `BindGroupLayout`s); `NaadfFirstHitNode` + register it in the render graph. The node
    dispatches and writes `first_hit_data` + `shaded_color`. **Compiles** — node runs (verify
    `first_hit_data` contents via readback).
11. **WGSL — final blit.** `naadf_final.wgsl` (fullscreen fragment, ports albedo
    `renderFinal.fx` `MainPS`); `NaadfFinalBlitNode` (fullscreen render pass); graph edge
    `NaadfFirstHitNode → NaadfFinalBlitNode`, ordered before UI. **▶ Compiles & runs — THE PHASE
    A DELIVERABLE:** a flat-lit voxel scene the user flies through with correct AADF-accelerated
    geometry.
12. **HUD re-point + polish.** Update `hud.rs` renderer-mode string + GPU-timing paths to the
    NAADF node names (§1.1). Register timing spans in the two render nodes. Update `README.md`
    roadmap. **▶ Compiles & runs** — HUD shows FPS + NAADF pass timings.

**Phase-A review gate:** after step 12, Phase A is reviewed and confirmed runnable before any
Phase-B design begins (01-context.md §2, §5 forbidden move).

---

## 9. Phase B structural sketch (structural only — not designed in detail)

Phase B ports the §4 GI pipeline (`02-research.md` §1.2) — long-term-memory TAA, compressed
ReSTIR GI, sparse bilateral denoiser. **It is not designed here** (D4 + forbidden move §5);
this is only where it slots into the Phase-A architecture so the seams are visible.

**Module shape Phase B adds (no Phase-A module is restructured):**
- `src/render/` gains modules for the GI passes: a `taa.rs`, `gi.rs`, `resample.rs`,
  `denoise.rs`, `atmosphere.rs` (node + pipeline definitions). `gpu_types.rs` gains the 64-bit
  TAA sample struct, the lit/unlit/refined ReSTIR sample structs, the 8×8 bucket-info struct.
- `src/assets/shaders/` gains the `base/` shader tree ports: `renderFirstHit` (Phase-B
  4-plane-bounce variant), `rayQueueCalc`, `renderGlobalIllum`, `renderSampleRefine`,
  `renderSpatialResampling`, `renderDenoiseSplit`, `base/renderTaaSampleReverse`,
  `renderAtmosphere` — and the B-only functions get *added* to the existing shared WGSL
  modules (`ray_tracing_common.wgsl` gains VNDF-GGX; `render_pipeline_common.wgsl` gains the
  specular-path `getHitDataFromPlanes`; a new `color_compression.wgsl` from
  `commonColorCompression.fxh`; a new `taa_common.wgsl` from `commonTaa.fxh`).

**Render-graph shape Phase B adds** — the Phase-A two-node graph
(`first_hit → final`) expands to NAADF's deferred pipeline (`02-research.md` §1.2.1):
```
[atmosphere precompute] [first_hit (4-plane G-buffer)] -> [TAA reproject] -> [rayQueueCalc]
   -> [globalIllum] -> [sampleRefine] -> [spatialResampling] -> [denoiseSplit] -> [final blit]
```
The Phase-A `NaadfFirstHitNode` is *replaced* by the Phase-B 4-plane-bounce first-hit; the
Phase-A `NaadfFinalBlitNode` stays (it already reads a `taaSampleAccum`-format buffer — §5.3 —
so Phase B just feeds it the real accum buffer instead of `shaded_color`). The **TAA node slots
between first-hit and the GI passes** (NAADF's "TAA placed unusually early") — the named seam
in §5.3.

**Where Phase B touches Phase-A data:** the `first_hit_data` `UVec4` layout (§5.3) gains
meaningful planes 1–3 (specular bounces); the §2 voxel/material data structure and the
`ray_tracing.wgsl` `shootRay` traversal are **reused unchanged** as the GI secondary-ray
tracer. The `GrowableBuffer` abstraction (§3) gets heavy Phase-B use for the ReSTIR sample list
buffers. **Construction** (the deferred `chunkCalc.fx` / `boundsCalc.fx` / `worldChange.fx` GPU
path, §6.2) is orthogonal to Phase B and can be picked up independently whenever GPU-generated
or editable worlds are needed.

**Deferred-but-not-Phase-B items** (neither Phase A nor the GI pipeline — picked up only when
explicitly scoped): GPU world construction (Algorithm 1 on GPU), the background chunk-AADF
queue, flood-fill edit invalidation, editing tools, dynamic entities, the `WorldGenerator`,
`.vox`/`.cvox`/`obj2voxel` importers, the `Gui/` editor. All out of scope per Q1 / D2 / the
forbidden moves.
