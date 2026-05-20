# D4 — render-pipeline · 02-exploration

**Author:** refactor-explorer (D4 — render-pipeline).
**Date:** 2026-05-20.
**Scope:** the Phase-A/A-2/B render graph + GI/TAA/atmosphere/first-hit subsystems under `crates/bevy_naadf/src/render/` (excluding `render/construction/**`, which is D5's domain). Shared seam files (`gpu_types.rs`, `prepare.rs`, `pipelines.rs::NaadfPipelines`) read-only-to-D5; D4's refactor lands AFTER D5's split.

Every file:line reference below was verified with Read/Grep against the current `main`. No invented citations.

---

## Findings

### Summary table

| # | severity | location | category | one-line description |
|---|----------|----------|----------|----------------------|
| 1 | high     | `crates/bevy_naadf/src/render/prepare.rs:189-717` + `:733-1207` | god-file | `prepare_world_gpu` (528 LOC build-once) and `prepare_frame_gpu` (474 LOC per-frame) live in one 1 207-LOC file with no shared types; the file also owns the W4 wave-3 placeholder buffers + the WorldGpu↔ConstructionGpu bind-group ownership tangle |
| 2 | high     | `crates/bevy_naadf/src/render/mod.rs:300-330` | bevy-idiom-misfit | 17-element `add_systems(Core3d, (…).chain())` tuple — every new node forces editing one tuple at one site; defeats per-workstream-PR seam (BEV-1) |
| 3 | high     | `crates/bevy_naadf/src/render/graph_b.rs:242-446` | DUP-3 | 5 `naadf_sample_refine_*_node` systems, each ~40 LOC of identical prologue (lookup pipeline, lookup bind group, dispatch). NAADF C# does the same with 5 `dispatch()` calls in one function (`WorldRenderBase.cs:352-362`) |
| 4 | medium   | `crates/bevy_naadf/src/render/gpu_types.rs:35-720` (~30 `#[repr(C)]` structs) | bevy-idiom-misfit | Every uniform / storage struct hand-pads `_pad0`, `_pad0b` … `_pad10` to std140 (~70 explicit pad fields, ~140 LOC, plus ~40 `const _: () = assert!(offset_of! …)` guards). Bevy 0.19's `ShaderType` derive auto-pads (BEV-2) |
| 5 | medium   | `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:655` + `:668` | SSoT-4 / UA-2 | `(cur_bucket_x >> 18u) * 8u` and `array<u32, 32>` shadow `INVALID_SAMPLE_STORAGE_COUNT = 8` / `BUCKET_STORAGE_COUNT = 32` (`gi.rs:54,57`). The bucket-storage value is documented *in a comment* (`:667`) — pure footgun |
| 6 | medium   | `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:217,320-360` (and ~25 sibling files) | SSoT-3 | `CELL_DIM=4` / `CELL_CHILDREN=64` hardcoded as `4u`/`16u`/`64u` literals throughout WGSL with no `const NAADF_CELL_DIM = 4u` decl or naga-oil shader-def injection |
| 7 | medium   | `crates/bevy_naadf/src/render/prepare.rs:650-699` + `:272-291` | concern-leak | `prepare_world_gpu` allocates W4 entity placeholder buffers and builds the `world_layout` bind group with them — then D5's `prepare_construction` may *rebuild the same bind group* with real buffers. Two systems write `WorldGpu.bind_group`; the cross-domain ownership is invisible at the type level |
| 8 | low      | `crates/bevy_naadf/src/render/graph.rs:309` + `graph_b.rs:574` (split across two files) | scaffold-residual | `graph.rs` vs `graph_b.rs`: `graph.rs` holds 4 nodes (first-hit + 2 TAA + final blit), `graph_b.rs` holds 10 GI/atmosphere nodes. The split was Batch-1 readability ("keep A-2 readable"); now Phase B is the production path the comment at `graph_b.rs:3-5` still treats as "new" |
| 9 | low      | `crates/bevy_naadf/src/render/extract.rs:452-483` | DUP / micro | `extract_taa_config` (7 LOC) + `extract_gi_config` (8 LOC) are mechanically-identical `args → ResMut` mirror systems; no shared helper |
| 10| low      | `crates/bevy_naadf/src/render/pipelines.rs:264-862` | god-impl | `impl FromWorld for NaadfPipelines` is a single ~600-LOC method that declares 15 `BindGroupLayoutDescriptor`s + 14 `CachedComputePipelineId`s with no internal structure; field count is 30+ on the struct (line 97-262), every layout inlined |

---

### Finding 1 — `render/prepare.rs` is two unrelated systems + a cross-domain bind-group tangle (severity: high)

**Location:** `crates/bevy_naadf/src/render/prepare.rs:189-717` + `:733-1207` (1 207 LOC total; verified by `wc -l`).

**Current state:**

The file contains:
- `prepare_world_gpu` (lines 189-717, ~528 LOC): a `PrepareResources`-set build-once system that allocates the chunks storage buffer + `blocks`/`voxels`/`voxel_types` `GrowableBuffer`s + `world_meta` uniform from a `WorldGpuStaging` hand-off, then builds `WorldGpu.bind_group`. Has a focused-refresh side-branch (`:218-317`) that re-uploads the palette + rebuilds the bind group on a `VoxelTypesRefresh` event.
- `prepare_frame_gpu` (lines 733-1207, ~474 LOC): a `PrepareBindGroups`-set per-frame system that writes the `GpuCamera` + `GpuRenderParams` uniforms, (re)creates `first_hit_data`/`first_hit_absorption`/`final_color` on a viewport resize, and builds five bind groups (`bind_group`, `first_hit_atmosphere_bind_group`, `blit_bind_group`, `taa_reproject_bind_group`, `calc_new_taa_sample_bind_group`).

These two systems share no types and run in different sets. The module docblock (`:1-34`) itself explicitly enumerates them as separate concerns. The file's "shared" infrastructure is just three `use` statements and the `W2_BUFFER_HEADROOM_MUL` constant (`:173`).

Additionally, lines 650-699 of `prepare_world_gpu` allocate three W4 entity placeholder buffers and bind them into `WorldGpu.bind_group` at slots 5/6/7 — buffers that D5's `prepare_construction` may then rebuild past (see Finding 7).

**Why it's a problem:**

- `01-context.md` Q3 / `15-design-c.md` §1.4 establishes the D4 ↔ D5 seam: D5 treats this file as read-only, but D5 actually does cross-write `WorldGpu.bind_group` (see Finding 7). The 1.2k-LOC file hides this — readers expect "one file = one cohesive concern", but `prepare.rs` is two systems + a cross-domain bind-group ownership concern.
- Recompile blast radius: editing the per-frame side touches all `prepare_world_gpu` consumers' build cache. The file's 1207 LOC plus the 8 separate bind-group construction sites in `prepare_frame_gpu` make scrolling around it an extended exercise — `:880`, `:976`, and `:1100` all build different bind groups with the same idiom.

**Suggested direction (NOT a design):**

Split into `prepare/world.rs` + `prepare/frame.rs` + `prepare/mod.rs` (the two `pub fn` re-exports + the shared `WorldGpu` / `FrameGpu` structs). Keep the seam contract intact: D5 reads the new module the same way it reads the current file. Architect should also evaluate factoring `prepare_frame_gpu`'s 5 bind-group construction calls into per-bind-group builders (one per `*_bind_group` field).

**Out-of-scope ripple:** D5's `prepare_construction` reads `WorldGpu` + `NaadfPipelines.world_layout` from this file — those exports must keep their paths or D5 sees the split.

---

### Finding 2 — 17-element `.chain()` tuple at `render/mod.rs:300-330` (severity: high)

**Location:** `crates/bevy_naadf/src/render/mod.rs:300-330` (verified — 17 nodes listed in the `add_systems(Core3d, (…).chain())` tuple).

**Current state:**

```
.add_systems(
    Core3d,
    (
        naadf_gpu_producer_node,
        naadf_bounds_compute_node,
        naadf_world_change_node,
        naadf_entity_update_node,
        naadf_atmosphere_node,
        naadf_first_hit_node,
        naadf_taa_reproject_node,
        naadf_sample_refine_clear_node,
        naadf_ray_queue_node,
        naadf_global_illum_node,
        naadf_sample_refine_valid_history_node,
        naadf_sample_refine_count_valid_node,
        naadf_sample_refine_count_invalid_node,
        naadf_sample_refine_buckets_node,
        naadf_spatial_resampling_node,
        naadf_denoise_node,
        naadf_calc_new_taa_sample_node,
        naadf_final_blit_node,
    ).chain().in_set(Core3dSystems::PostProcess).before(tonemapping),
)
```

The plugin imports the 13 D4-owned nodes from `graph::` + `graph_b::` (lines 56-66) and 4 construction-owned nodes from `construction::` (lines 70-93), then concatenates them into one tuple. Comments at `:282-299` document the structural intent (construction-nodes-first, atmosphere-then-render).

**Why it's a problem:**

- 4 of the 17 systems are owned by D5 (`naadf_gpu_producer_node`, `naadf_bounds_compute_node`, `naadf_world_change_node`, `naadf_entity_update_node`). Every D5 workstream that adds a node has to edit this D4-owned file — exactly the "do not cross-edit shared files" pattern `01-context.md` forbids (§"Forbidden moves" #7).
- The W0 seam contract (`15-design-c.md` §1.1) explicitly designed construction to be **a single sub-module under `render/`** so each Phase-C workstream could merge in its own PR without touching the parent. The current 17-element tuple **defeats that design** — it is the one parent edit every W needs.
- Bevy ships `RenderLabel` + `add_render_graph_edges` + `IntoSystemConfigs::after()` / `before()` for this case. Each subsystem could declare its own label + ordering relative to siblings, then a Plugin per subsystem adds them — no central registry.

**Suggested direction (NOT a design):**

Hoist each `naadf_*_node` into a `Plugin` sub-trait that declares: (a) the system function, (b) a `RenderLabel`, (c) one `.before(...)` / `.after(...)` edge. The construction nodes' plugins live in `render/construction/` (D5's domain); the render plugins live in `render/` (D4). `NaadfRenderPlugin` becomes a `.add_plugins((AtmospherePlugin, FirstHitPlugin, TaaPlugin, SampleRefinePlugin, RayQueuePlugin, GiPlugin, SpatialResamplingPlugin, DenoisePlugin, BlitPlugin))` shim. The 17-element tuple disappears.

**Out-of-scope ripple:** D5's `construction` sub-module would gain `.add_plugins(...)` calls for its 4 nodes instead of `pub use`-ing them through to D4's plugin — a self-contained D5 internal change but synchronised with D4's plugin restructure.

---

### Finding 3 — DUP-3: 5 `sample_refine_*_node` systems with identical prologue (severity: high)

**Location:** `crates/bevy_naadf/src/render/graph_b.rs:242, 286, 329, 369, 413` (verified line numbers).

**Current state:**

Five nearly-identical compute-node systems:

| node | line | role | dispatch shape |
|---|---|---|---|
| `naadf_sample_refine_clear_node` | 242-275 | clear buckets, calc mask | `ceil(bucket_count/64) workgroups` |
| `naadf_sample_refine_valid_history_node` | 286-320 | walk 128-frame ring | `(1,1,1)` |
| `naadf_sample_refine_count_valid_node` | 329-360 | reproject lit samples | indirect off `valid_dispatch` |
| `naadf_sample_refine_count_invalid_node` | 369-400 | reproject unlit samples | indirect off `invalid_dispatch` |
| `naadf_sample_refine_buckets_node` | 413-446 | brightness-level survivors | `ceil(bucket_count/64) workgroups` |

Each node opens a `Some(gi_gpu), Some(gi_bind_groups)` else-return, fetches `pipelines.sample_refine_*_pipeline`, opens `begin_compute_pass`, calls `set_bind_group(0, &gi_bind_groups.sample_refine_bind_group, &[])`, and either dispatches `workgroups` or `dispatch_workgroups_indirect`. The `valid_history` variant additionally binds `@group(1)` (the `sample_refine_dispatch_bind_group`). All five share `SAMPLE_REFINE_SPAN` (line 42 — one HUD timing span across all five).

NAADF C# does this with **5 `dispatch(...)` calls in a single function** (`WorldRenderBase.cs:272-362`). The Rust port split them into 5 separate `Core3d` systems because they interleave with `ray_queue` / `global_illum` in NAADF's dispatch order — but that interleaving is across nodes #4 (`clear`) → #5 (`ray_queue`) → #6 (`global_illum`) → #7-10 (`valid_history`, `count_valid`, `count_invalid`, `buckets`). So `clear` is one detached call (correctly), and the other 4 are a contiguous group that could be one node.

**Why it's a problem:**

- `~40 LOC × 4 redundant copies = ~160 LOC` is mechanically duplicated to satisfy a graph-edge requirement that no longer holds for the 4 contiguous nodes.
- The `RenderLabel`/edges refactor (Finding 2) makes the split into 5 separate systems even more friction — that's 5 labels, 5 `before` declarations, 5 system registrations, when the actual sequencing for the contiguous 4 is "all run together after `global_illum`".
- The HUD already aggregates these as one span (`SAMPLE_REFINE_SPAN`, `graph_b.rs:42`) — confirming the per-node split is purely structural, not observational.

**Suggested direction (NOT a design):**

Audit the order graph in `render/mod.rs:300-330`. If the contiguous 4 (`valid_history` → `count_valid` → `count_invalid` → `buckets`) can be one system that opens one `ComputePass` and runs 4 `pass.set_pipeline(...) + pass.dispatch_*` calls in sequence (wgpu's automatic buffer barriers serialise the inter-dispatch buffer access — same pattern `naadf_ray_queue_node` uses at `:151-158` to fold `RayQueue` + `RayQueueStore` into one node), collapse them. `clear` stays a separate node (different position in the chain).

**Out-of-scope ripple:** None — internal to D4.

---

### Finding 4 — Hand-padded `Pod` over `ShaderType` (severity: medium)

**Location:** `crates/bevy_naadf/src/render/gpu_types.rs` (1 055 LOC); ~30 `#[repr(C)]` structs; explicit pad-field count counted by grep `_pad` = ~70.

**Current state:**

Every GPU-side struct is hand-padded:

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCamera {
    pub inv_view_proj: Mat4,
    pub cam_pos_int: IVec3,
    pub _pad0: u32,           // std140 padding
    pub cam_pos_frac: Vec3,
    pub _pad1: u32,           // std140 padding
}
```

And every struct is also guarded by `const _: () = assert!(std::mem::size_of::<S>() == N)` / `assert!(offset_of!(S, field) % 16 == 0)` (e.g. `gpu_types.rs:844-902` — ~60 compile-time asserts). `GpuGiParams` (336 B, lines 412-546) holds **11** explicit pad fields (`_pad0`…`_pad10`) and 8 offset guards.

The justification spans the file docblock (`:1-22`) and the per-struct comments — the project chose `bytemuck::Pod + #[repr(C)]` over Bevy 0.19's `bevy::render::render_resource::ShaderType` (an `encase` re-export that auto-handles std140/std430).

**Why it's a problem:**

- `ShaderType` derive removes the manual padding *entirely* — the layout is computed from the field types. The 70 `_padN` fields and the 60 offset asserts both go away (the assert macro stays as belt-and-braces if desired, but the *need* for it dissolves because `encase` enforces the layout at serialisation time).
- The 3× recurrence of the `vec3`-then-scalar hazard (`12-alignment-gap.md` D-A, cited at `gpu_types.rs:838-843`) is a direct symptom of hand-padding: the project added compile-time guards because the hazard kept biting. `encase` makes that hazard impossible by construction.
- Estimated LOC reduction: ~70 pad fields + ~60 offset asserts + the surrounding comments = roughly 300 LOC across `gpu_types.rs` alone.

**Whether the choice is load-bearing — investigated:**

- The structs are uploaded via `RenderQueue::write_buffer(&buf, 0, bytemuck::bytes_of(&data))` (e.g. `prepare.rs:929-930`, `taa.rs:419,442`, `gi.rs:404-440`). With `ShaderType` this becomes `encase::UniformBuffer::new(...).write(&data)` (or the `ShaderType` write helper). One mechanical sweep; no API change for the call sites — just a different serialisation function.
- Web/WASM target compat: `encase` is core to Bevy 0.19's `render_resource` module — used by `bevy_pbr` + Bevy's built-in PBR uniforms, including the web target. No web blocker.
- The `Pod` derive may still be wanted on the `GpuVoxelType` / `GpuSampleValid` / `GpuBucketInfo` / W1 `GpuHashValueSlot` / W4 entity structs because they're packed-`[u32;N]` payloads with no `vec3` hazard and the byte representation matches WGSL's storage-buffer-array element layout 1:1. Those can keep `Pod`; the *uniform* structs (`GpuCamera`, `GpuRenderParams`, `GpuTaaParams`, `GpuGiParams`, `GpuAtmosphereParams`, `GpuWorldMeta`, `GpuConstructionParams`) are where the padding lives.

**Suggested direction (NOT a design):**

Convert the **uniform** structs (the seven listed above) to `#[derive(ShaderType)]`; keep the packed-array structs as-is. Drop the `_padN` fields and the per-struct offset asserts. Keep a size-equality assert if the architect wants belt-and-braces. Estimated drop: ~300 LOC in `gpu_types.rs`.

**Out-of-scope ripple:**

- D5 owns `GpuConstructionParams` (`gpu_types.rs:583-631`) read-only — the layout decision must include it. D5's W1 WGSL declares the WGSL counterpart; if D4 converts the Rust side to `ShaderType`, the WGSL declarations stay the same (WGSL is layout-stable by construction), so the change is one-sided in `gpu_types.rs`. **Sequencing note:** D5 lands first; D4 then performs the `ShaderType` flip on `GpuConstructionParams` along with the others. D5's bind-group construction doesn't change — D4 still binds via `as_entire_buffer_binding()`.

---

### Finding 5 — SSoT-4 / UA-2: WGSL `* 8u` + `array<u32, 32>` shadow Rust storage-count constants (severity: medium)

**Location:** `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:655` + `:668` (verified).

**Current state:**

```wgsl
let cur_bucket_x = atomicLoad(&bucket_info[bucket_index].x);
let valid_count = (cur_bucket_x >> 6u) & 0xFFFu;
let invalid_count = (cur_bucket_x >> 18u) * 8u;       // <-- the `* 8u` IS INVALID_SAMPLE_STORAGE_COUNT
...
var comp_color_max_storage: array<u32, 32>;          // <-- the `32` IS BUCKET_STORAGE_COUNT
// The HLSL function-scope `static uint compColorMaxStorage[32]` — per-thread
// scratch, bounded by `effective_valid_count ≤ bucket_storage_count = 32`.   ← comment SAYS so
```

The Rust SSoT is `gi.rs:51-60`:

```rust
pub const VALID_SAMPLE_STORAGE_COUNT: u32 = 2;
pub const INVALID_SAMPLE_STORAGE_COUNT: u32 = 8;
pub const BUCKET_STORAGE_COUNT: u32 = 32;
pub const REFINED_BUCKET_STORAGE_COUNT: u32 = 8;
```

Most WGSL sites correctly read `gi_params.{valid_sample,invalid_sample,bucket,refined_bucket}_storage_count` (e.g. `sample_refine.wgsl:259,260,489,523,608,658,673,711,741`, `spatial_resampling.wgsl:340`, `naadf_global_illum.wgsl:510,528`). The two outliers at `:655` and `:668` use bare literals.

`:668` is especially insidious: WGSL `array<T, N>` requires a const expression for `N`, so it can't directly read `gi_params.bucket_storage_count`. Whoever changes the Rust SSoT to anything other than 32 will silently get a `static array bound mismatch` (or worse, undefined behaviour if the bound is loosely enforced).

**Why it's a problem:**

- These two are the last live shadows of the SSoT-4 family — 11 of 13 WGSL sites already do this right; these two are the holdouts.
- `:655` is a derivable expression (`invalid_count = (cur_bucket_x >> 18u) * gi_params.invalid_sample_storage_count`) — one mechanical edit.
- `:668` has a real WGSL constraint (`array<T, N>` needs a compile-time `N`). The fix is either a top-of-file `const NAADF_BUCKET_STORAGE_COUNT: u32 = 32u;` (with a Rust-side `naga-oil` shader-def at minimum: `pipelines.rs::NaadfPipelines::from_world` can inject `ShaderDefVal::UInt("BUCKET_STORAGE_COUNT".into(), BUCKET_STORAGE_COUNT)` similar to how `TAA_SAMPLE_RING_DEPTH` already works at `:269-279`) or a documented "must equal Rust constant" assert in the Rust prepare path.

**Suggested direction (NOT a design):**

For `:655`: replace `* 8u` with `* gi_params.invalid_sample_storage_count`. For `:668`: add a naga-oil `#{BUCKET_STORAGE_COUNT}` shader-def to `NaadfPipelines::sample_refine_*_pipeline` (mirror the `TAA_SAMPLE_RING_DEPTH` injection in `pipelines.rs:269-279`) and reference it as `array<u32, #{BUCKET_STORAGE_COUNT}>`. The `naga_oil` const-injection is already the project's idiom for this exact case.

**Out-of-scope ripple:** None — D4-owned WGSL + D4-owned shader-def in `pipelines.rs`.

---

### Finding 6 — SSoT-3: `CELL_DIM=4` / `CELL_CHILDREN=64` hardcoded across WGSL (severity: medium)

**Location:** `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:54,116,217,320,322,324,332,360,479` (9 explicit `4u`/`16u`/`64u` literals in one file alone) + ~25 sibling WGSL files. Confirmed by `grep -n "CELL_DIM\|CELL_CHILDREN" *.wgsl` returning **zero hits**.

**Current state:**

`ray_tracing.wgsl:320-332` walks the AADF tree using bare literals:

```wgsl
let block_pos_in_chunk = voxel_pos_in_chunk / 4u;                       // CELL_DIM
(cur_node & 0x3FFFFFFFu) + flatten_index(block_pos_in_chunk, 4u, 16u);  // (CELL_DIM, CELL_DIM*CELL_DIM)
let voxel_pos_in_block = vec3<u32>(cur_cell) % 4u;                       // CELL_DIM
...
let voxel_index_in_block = flatten_index(voxel_pos_in_block, 4u, 16u);   // 16u == CELL_CHILDREN
```

The Rust SSoT lives in `voxel/mod.rs:63-65` per `01-context.md` audit row SSoT-3. The WGSL doesn't reference it. Changing `CELL_DIM` would require editing every `4u`/`16u`/`64u` literal across the ~25 WGSL files by hand.

**Why it's a problem:**

- `01-context.md`'s audit explicitly flagged this; D4 owns the renderer side (D5 owns the construction side; the constants are paper-canonical so the literal won't change in practice — but the *documentation* concern remains).
- The literal `4u` and `16u` are also overloaded with other meanings (e.g. `ray_tracing.wgsl:217` `select(6u, 4u, …)` is unrelated to `CELL_DIM` — but a reader can't tell without tracing). Named consts would disambiguate at the reading point.

**Suggested direction (NOT a design):**

Add a `naga-oil` shader-def `#{NAADF_CELL_DIM}` + `#{NAADF_CELL_CHILDREN}` to every pipeline's shader-def list in `NaadfPipelines::from_world` (`pipelines.rs:269-279` already does the `TAA_SAMPLE_RING_DEPTH` pattern; mirror it). Sweep the WGSL sites by hand. Architect should also decide whether to keep the literals adjacent for the **deliberately-low-level** sites (the `flatten_index(block_pos_in_chunk, 4u, 16u)` pattern reads like a packed pointer-extract — possibly clearer with explicit literals) or pursue uniform replacement.

**Out-of-scope ripple:** D5 owns ~half the affected WGSL files (`chunk_calc.wgsl`, `bounds_calc.wgsl`, `world_change.wgsl`, `entity_update.wgsl`, `generator_model.wgsl`, `map_copy.wgsl`). The shader-def injection is a Rust-side change in `NaadfPipelines` + `ConstructionPipelines`; the WGSL sweep is half D4, half D5 — **flagged as a cross-domain coordination point**.

---

### Finding 7 — `WorldGpu.bind_group` is written by two domains (severity: medium)

**Location:** `crates/bevy_naadf/src/render/prepare.rs:650-699` (D4 writes initial); `crates/bevy_naadf/src/render/prepare.rs:272-291` (D4 writes refresh); and per `15-design-c.md` §1.7 / `prepare.rs:683-686` comments, D5's `prepare_construction` rebuilds the same bind group with real entity buffers when `entities_enabled=true`.

**Current state:**

`prepare_world_gpu` (D4) allocates three "placeholder" buffers at `prepare.rs:661-679` (20-byte / 4-byte / 16-byte stub storage buffers) and binds them into slots 5/6/7 of `WorldGpu.bind_group`. The docblock at `:650-660` says:

> ` `prepare_construction` rebuilds this bind group with the real W4 buffers (and the production `WorldGpu` chunks view) once `ConstructionGpu` has them allocated AND `entities_enabled = true`. `

So `WorldGpu.bind_group` has two writers: `prepare_world_gpu` (build-once + on palette refresh) and D5's `prepare_construction` (on `entities_enabled` toggle). The `WorldGpu` struct itself (`prepare.rs:63-97`) carries 3 fields named `entity_*_placeholder` that exist *only* because the layout requires slots 5/6/7 — they are pure layout-padding buffers.

**Why it's a problem:**

- A `Resource`'s `bind_group: BindGroup` field with two writers in two different domains is a maintainability footgun — neither domain "owns" the canonical bind group. If D5's rebuild path has a bug (wrong buffer at slot 5), the symptom shows up in renderer-side WGSL reads; the cause is in D5; the type system says they're unrelated.
- The three placeholder buffer fields on `WorldGpu` are real fields users of the resource see; their semantic value is "ignore me if entities are on". That's primitive obsession at the struct level (no `enum` of "with-entities" vs "without-entities" state).

**Suggested direction (NOT a design):**

Two architect-level options to consider:

1. Push the entity-buffer ownership entirely into D5's `ConstructionGpu` + give `world_layout` two flavours (with entities / without entities) with separate bind groups — `WorldGpu.bind_group` is the no-entities case, D5 provides `WorldGpu.bind_group_with_entities` via a separate Resource. D4 (the renderer) selects between them at dispatch time based on `ExtractedGiConfig` or similar.
2. Keep the current single-bind-group design but make D5 the only writer (D4 hands D5 a builder closure; D5 picks placeholder vs real). The refactor moves `entity_*_placeholder` ownership across the seam to D5.

This is genuinely a *D4-vs-D5 design decision* — `01-context.md` Q3 doesn't disambiguate. Architect must choose; user-decision-required if architect prefers neither.

**Out-of-scope ripple:** D5's `prepare_construction` already rebuilds this bind group; whichever option lands changes the boundary contract — flagged for architect Q&A.

---

### Finding 8 — `graph.rs` vs `graph_b.rs` split is residual scaffolding (severity: low)

**Location:** `crates/bevy_naadf/src/render/graph.rs:1-309` (4 nodes) + `graph_b.rs:1-574` (10 nodes).

**Current state:**

`graph.rs` (309 LOC, 4 nodes): `naadf_first_hit_node`, `naadf_taa_reproject_node`, `naadf_calc_new_taa_sample_node`, `naadf_final_blit_node`.

`graph_b.rs` (574 LOC, 10 nodes): the atmosphere/GI/sample-refine/spatial/denoise cluster.

The split was rationalised in `graph_b.rs:3-6`:

> "Phase B expands the Phase-A-2 three-node graph (`naadf_first_hit → naadf_taa_reproject → naadf_final_blit`) into NAADF's full deferred GI pipeline. The new nodes land here (rather than in `graph.rs`) to keep the A-2 graph readable — `09-design-b.md` §2.1."

Phase B is now the production path. The graph.rs/graph_b.rs split no longer serves "keep A-2 readable" — A-2 is a historical phase, not a current path.

**Why it's a problem:**

- Mild — readers have to look in two files for "all render-graph nodes". The current `mod.rs` already imports them as a flat list and chains them in one tuple (Finding 2). The two-file split is purely historical.
- After Finding 2 (plugin-per-subsystem), the per-file location of each node body becomes academic — they could live alongside the plugin that owns them (e.g. `taa::naadf_taa_reproject_node` instead of `graph::naadf_taa_reproject_node`).

**Suggested direction (NOT a design):**

The natural target state if Finding 2 lands: each node moves into its *subsystem* module (`atmosphere.rs`, `gi.rs`, `taa.rs`, …) co-located with its plugin and bind-group resource. `graph.rs` + `graph_b.rs` disappear as the central node directory. Architect should consider this as a natural consequence of Finding 2, not a standalone refactor.

**Out-of-scope ripple:** None.

---

### Finding 9 — `extract_taa_config` + `extract_gi_config` are identical 7-LOC mirrors (severity: low)

**Location:** `crates/bevy_naadf/src/render/extract.rs:452-459` + `:476-483`.

**Current state:**

```rust
pub fn extract_taa_config(
    mut extracted: ResMut<ExtractedTaaConfig>,
    args: Extract<Option<Res<crate::AppArgs>>>,
) {
    if let Some(args) = &*args {
        extracted.enabled = args.taa;
    }
}
pub fn extract_gi_config(
    mut extracted: ResMut<ExtractedGiConfig>,
    args: Extract<Option<Res<crate::AppArgs>>>,
) {
    if let Some(args) = &*args {
        extracted.settings = args.gi;
    }
}
```

Two mechanically-identical mirror systems, differing only in which `AppArgs` sub-field they shovel and which extracted resource they update.

**Why it's a problem:**

- Mild. Every new `AppArgs` mirror requires a new ~7-LOC system + a `register_resource` + a `ExtractSchedule` add — boilerplate.
- A generic `extract_field<F, S, T>(get: F)` helper or a `#[derive(ExtractMirror)]` attribute over `ExtractedTaaConfig` / `ExtractedGiConfig` would absorb the boilerplate. Bevy itself doesn't ship this derive; the project could either add it or accept the small dup.

**Suggested direction (NOT a design):**

Probably leave as-is — the boilerplate per system is small, the cost of an extra abstraction layer is comparable to the LOC saved. Architect's call. If the architect ends up adding a `ExtractedConfigPlugin<A, E, Fn(A) -> E>` helper for unrelated reasons (e.g. consolidating extract systems across the seam), fold this in.

**Out-of-scope ripple:** None.

---

### Finding 10 — `impl FromWorld for NaadfPipelines` is one ~600-LOC method (severity: low)

**Location:** `crates/bevy_naadf/src/render/pipelines.rs:264-862` (verified — single `impl` block, one `fn from_world` body).

**Current state:**

The `NaadfPipelines` struct (`:97-262`) has **30+ fields**: 15 `BindGroupLayoutDescriptor`s + 14 `CachedComputePipelineId`s + a `HashMap<TextureFormat, CachedRenderPipelineId>` for blit pipelines + 3 odds-and-ends (`empty_bind_group`, `blit_vertex`, `blit_shader`).

`from_world` (`:265-862`) creates all of them inline:

- 9 layouts declared as `BindGroupLayoutDescriptor::new(...)` literals (each 15-30 LOC).
- 14 compute pipelines queued as `queue_compute_pipeline(ComputePipelineDescriptor { ... })` (each ~12-20 LOC).
- Each pipeline carries its own `shader_defs` vec (mostly empty, some with `TAA_SAMPLE_RING_DEPTH`).

**Why it's a problem:**

- `01-context.md` row D4 + audit BEV-2 already flagged the size. The single `fn from_world` is the longest method in the D4 source tree.
- Every new subsystem adds fields here. The "Plugin-per-subsystem" target state (Finding 2) implies each plugin owns its own layouts + pipelines (declared in its own module's `FromWorld`), with `NaadfPipelines` becoming a thin aggregate (or going away entirely).
- A subsystem that owns its layouts + pipelines could also own its WGSL shader path constants (currently a flat list at `pipelines.rs:34-88`) — collocating the asset declaration with the pipeline that consumes it.

**Suggested direction (NOT a design):**

If Finding 2's plugin-per-subsystem lands, this folds out naturally: each subsystem's plugin declares its own `Resource` holding its layouts + pipeline ids (the way `GiBindGroups` / `AtmosphereGpu` / `TaaGpu` already do for *bind groups* + buffers — extend the same pattern to layouts + pipelines). `NaadfPipelines` shrinks to the shared `world_layout` + `frame_layout` + `blit_*` core.

**Out-of-scope ripple:** D5's `ConstructionPipelines` (`render/construction/mod.rs:482`) is an "empty sibling" already (per audit OA-2); the same pattern proliferating into ~10 subsystem-pipeline resources is healthy if each is sized appropriately, dangerous if they accidentally become parallel registries. Architect should design the per-subsystem pipeline-resource pattern explicitly.

---

## Confirmed / refuted audit suspicions

| suspicion (from brief) | verdict | notes |
|---|---|---|
| 1. `prepare.rs` (1 207 LOC) is two unrelated systems | **CONFIRMED** | See Finding 1. Split is the right move. Has additional concern (Finding 7) — shared bind-group ownership tangle that affects the W0 seam. |
| 2. `gpu_types.rs` + `pipelines.rs` use hand-padded `Pod` instead of `ShaderType` | **CONFIRMED** | See Finding 4. Estimated ~300 LOC reduction in `gpu_types.rs`. `ShaderType` is Bevy 0.19's canonical idiom; no web blocker. |
| 3. `render/mod.rs` 17-element `.chain()` undermines per-workstream-PR seam | **CONFIRMED** | See Finding 2. Has cross-domain impact (4 of 17 are D5-owned). The architect should design the `Plugin`-per-subsystem split. |
| 4. `graph.rs` vs `graph_b.rs` is residual scaffolding (Phase A-2 vs Phase B split) | **PARTIALLY CONFIRMED** | See Finding 8. The split is residual *but* the per-file split would collapse naturally as a consequence of Finding 2 (subsystem-local node placement). Standalone-collapsing it without Finding 2 is low value. |

| crosscutting item (from brief) | verdict |
|---|---|
| **SSoT-1** (max_ray_steps_*) | **NEARLY COMPLETE.** `ray_tracing.wgsl:122-136` are DOCUMENTATION-ONLY (verified — the comment at `:123-131` explicitly says so); live SSoT is `GpuRenderParams.max_ray_steps_primary` + `GpuGiParams.max_ray_steps_*`. The remaining issue is the **defaults table duplication** (D7's domain — `lib.rs:223-228` defaults must equal the WGSL consts at `:132-136` bit-for-bit), not D4's. **Flag for architect:** the `ray_tracing.wgsl` consts could be deleted entirely now that they are unreferenced — naga DCEs them but they read as live code. Possibly worth a one-line `// see GpuRenderParams.max_ray_steps_primary for the live value` followed by deletion. |
| **SSoT-3** (CELL_DIM=4 / CELL_CHILDREN=64 in ~25 WGSL files) | **CONFIRMED.** See Finding 6. Cross-domain (D4 + D5). |
| **SSoT-4** (storage counts in `gi.rs:51-60` vs WGSL literals) | **MOSTLY MITIGATED.** 11 of 13 sites read `gi_params.{valid_sample,invalid_sample,bucket,refined_bucket}_storage_count` (uniform-uploaded). Two outliers (Finding 5) remain — both in `sample_refine.wgsl`. The audit hint at `:655` was correct. |
| **SSoT-5** (TAA ring depth) | **CONFIRMED audit-complete.** `pipelines.rs:269-279` injects `#{TAA_SAMPLE_RING_DEPTH}` shader-def from `TaaRingConfig`; `taa.rs:306` uses the same value for buffer sizing. The Rust side is the SSoT; WGSL reads it via shader-def. The audit comment "mostly OK" is accurate. |
| **BEV-1** (17-element `.chain()`) | **CONFIRMED.** See Finding 2. |
| **BEV-2** (hand-padded `Pod`) | **CONFIRMED.** See Finding 4. |
| **DUP-3** (5 `sample_refine_*_node` systems) | **CONFIRMED.** See Finding 3. Architect to decide whether to collapse the contiguous 4 into one node or keep the 5-node structure for HUD-per-pass observability (currently they share one span — see `SAMPLE_REFINE_SPAN`, `graph_b.rs:42`). |
| **UA-2** (WGSL bare literals shadowing storage counts) | **CONFIRMED.** See Finding 5 — same 2 sites as SSoT-4. |

---

## D4 ↔ D5 shared-file notes

D4 ↔ D5 share three files. Per `01-context.md` Q3, **D5 lands first** treating these as read-only; **D4 lands second** and refactors them.

### `render/gpu_types.rs` (1 055 LOC)

**D4-owned:** `GpuCamera`, `GpuRenderParams`, `GpuWorldMeta`, `GpuVoxelType`, `GpuTaaParams`, `GpuCameraHistorySlot`, `GpuAtmosphereParams`, `GpuSampleValid`, `GpuBucketInfo`, `GpuGiParams`, `f16_bits`, the `FLAG_*` / `GI_FLAG_*` constants.

**D5-owned (read-only for D4):** `GpuConstructionParams` (`:583-631`), `GpuHashValueSlot` (`:656-661`), `GpuBoundQueueInfo` (`:682-685`), `GpuEntityChunkInstance` (`:714-720`), `GpuEntityInstanceHistory` (`:743-748`), `GpuChunkUpdate` (`:763-766`), `EntityInstance` (CPU-only mirror, `:779-791`).

**D4's refactor needs in this file (Finding 4):**
- Convert the seven uniform structs (`GpuCamera`, `GpuRenderParams`, `GpuTaaParams`, `GpuGiParams`, `GpuAtmosphereParams`, `GpuWorldMeta`, `GpuConstructionParams`) from `#[derive(Pod, Zeroable)]` to `#[derive(ShaderType)]`.
- `GpuConstructionParams` is D5-owned semantically but the `ShaderType` conversion is layout-mechanical — D4's refactor sweeps all uniform structs in one mechanical pass. **D5 should not preemptively touch its struct's layout**; D4's pass converts it in-place.
- The W1/W3/W4 packed-`[u32; N]` structs (`GpuHashValueSlot`, `GpuBoundQueueInfo`, `GpuEntityChunkInstance`, `GpuEntityInstanceHistory`, `GpuChunkUpdate`) stay `Pod` — no `vec3` hazard, no need to convert.

### `render/prepare.rs` (1 207 LOC)

**D4-owned:** `WorldGpu` struct, `FrameGpu` struct, `prepare_world_gpu`, `prepare_frame_gpu`.

**Shared-with-D5 surface:**
- `WorldGpu.bind_group` is written by `prepare_world_gpu` AND rebuilt by D5's `prepare_construction` (see Finding 7). The cross-write is intentional per `15-design-c.md` §1.7 but architect-fragile.
- `WorldGpu.entity_chunk_instances_placeholder` / `_entity_voxel_data_placeholder` / `_entity_instances_history_placeholder` are D4-allocated, D5-consumed fields.
- `WorldGpu.chunks_buffer` is D4-allocated but written into by D5's `naadf_gpu_producer_node` (the runtime GPU producer chain).

**D4's refactor needs in this file (Finding 1):** split into `prepare/world.rs` + `prepare/frame.rs`. Architect must decide Finding 7's outcome before this split lands (whether D5 owns the entity-placeholder fields, or D4 owns them with a clean handoff API).

**D5's responsibility, NOT D4's:** any changes to `prepare_construction` — D5 owns. Any new fields on `WorldGpu` driven by D5 work — D5 may *propose* but D4's refactor lands them (file-write authority).

### `render/pipelines.rs` (909 LOC)

**D4-owned:** `NaadfPipelines` struct + its `FromWorld` impl + `prepare_blit_pipeline`. All 15 bind-group layouts (`world_layout`, `frame_layout`, `blit_layout`, `taa_layout`, `atmosphere_*_layout`, `taa_reproject_layout`, `calc_new_taa_sample_layout`, `empty_layout`, `ray_queue_layout`, `global_illum_layout`, `sample_refine_*_layout`, `spatial_resampling_layout`, `denoise_layout`). All 14 compute-pipeline ids.

**D5-owned (in a separate file):** `ConstructionPipelines` is at `render/construction/mod.rs:482`, NOT in `pipelines.rs` — the W0 seam contract per audit OA-2. D5's pipelines do **not** appear in `pipelines.rs`.

**D4's refactor needs in this file (Finding 10):** if the Plugin-per-subsystem refactor (Finding 2) lands, the 15-layout / 14-pipeline aggregate splits across subsystem modules. `NaadfPipelines` shrinks to the shared core (`world_layout`, `frame_layout`, `blit_*`). `ConstructionPipelines` retains its place; it just isn't the only sibling resource any more — `GiPipelines`, `TaaPipelines`, `AtmospherePipelines`, etc. join it.

**D5 should not preemptively split anything here.** D5 lands first with the current file intact; D4 refactors after.

### W4 follow-up status (orthogonal — flagged for awareness)

Per `render/mod.rs:289-299` + `entity_update.rs`, W4's `naadf_entity_update_node` is a **gated no-op** at HEAD: the system body fires only when `ConstructionConfig.entities_enabled = true`, which is the W4 default-off state. Net behaviour-byte-identical to pre-W4. So the W4 placeholder bind-group plumbing (Finding 7) is fully active even though no entities flow through it. **The cleanest path is for the architect's design to choose Finding 7's resolution before any other D4↔D5 shared-file work lands.**

---

## Open questions for the architect

1. **Finding 2 — Plugin-per-subsystem boundary.** The 17-element `.chain()` collapses if each subsystem owns its node + label + edges. Question: do D4-subsystem plugins live in `render/<subsystem>/mod.rs` (per-subsystem dir) or `render/<subsystem>.rs` (flat siblings)? The latter is the current layout; the former is cleaner if `gi.rs` (618 LOC) gets split into `gi/buffers.rs` + `gi/uniform.rs` + `gi/plugin.rs`.

2. **Finding 3 — Collapsing 4 of 5 sample-refine nodes.** Architect must decide between (a) keep 5 nodes for per-pass HUD timing (one span line per pass) and accept the ~160 LOC dup, or (b) collapse the contiguous 4 into one node, lose per-pass HUD lines but gain a single shared bind group + dispatch loop. **Current state** already shares one span (`SAMPLE_REFINE_SPAN`) across all 5, so (b) doesn't degrade observability — but a future architect adding granular telemetry might want (a).

3. **Finding 4 — `ShaderType` cutover scope.** Convert all 7 uniform structs at once, or stage (Phase-A uniforms first, then Phase-B GI, then Phase-C construction)? `GpuConstructionParams` is D5-owned read-only-to-D4; the architect must confirm D5's impl lands the *current* layout (D5 ships with `Pod`), then D4's refactor swaps all 7 to `ShaderType` in one mechanical pass.

4. **Finding 5 — naga-oil shader-def for storage counts.** WGSL `array<T, N>` requires compile-time `N`. Option A: inject `#{BUCKET_STORAGE_COUNT}` shader-def from Rust. Option B: hardcode the WGSL `const` and add a Rust-side runtime assert that `BUCKET_STORAGE_COUNT` Rust = N WGSL. Architect's call — the existing `TAA_SAMPLE_RING_DEPTH` (option A, at `pipelines.rs:269-279`) is the project's idiom.

5. **Finding 6 — `CELL_DIM`/`CELL_CHILDREN` injection.** Cross-domain — half the WGSL files are D5's. **Coordination point with D5's architect.** Should the shader-def injection live in `NaadfPipelines`'s `from_world` (D4) and re-used by `ConstructionPipelines`'s `from_world` (D5), or should each side declare its own injection independently? Suggest: shared helper in `pipelines.rs` (D4-owned), called from both.

6. **Finding 7 — `WorldGpu.bind_group` cross-domain writers.** **Architect-fragile decision.** Option A: split `world_layout` into "with entities" / "without entities" variants — D4 owns both, D4 selects. Option B: D5 owns the entity-placeholder allocations + the bind-group rebuild, D4 hands D5 a builder closure. Option C: leave as-is — accept the cross-domain write, document it loudly. Each has a different blast radius — needs architect Q&A with the user before D4's `prepare.rs` split (Finding 1) lands.

7. **Finding 10 — `NaadfPipelines` decomposition.** If Finding 2 + Finding 10 both land, the result is ~10 `*Pipelines` resources (one per subsystem) instead of one. Architect should design the **naming convention** + the **single common parent** (if any). Compare to D5's `ConstructionPipelines` — does the architect want to *also* split `ConstructionPipelines` along workstream lines (W1Pipelines, W2Pipelines, etc.), or keep it monolithic? Cross-domain consistency call.

---

## Side notes / observations / complaints

1. **The 17-element `.chain()` is the load-bearing smell.** Of every D4 finding, this is the one that has the broadest impact (defeats the W0 per-workstream-PR design, ties D4 + D5 together at a central registry, forces every architect to recall a long ordering invariant). The architect should pursue Finding 2 first; many other findings (3, 8, 10) collapse out naturally if it lands.

2. **`ShaderType` is the highest LOC-yield finding.** ~300 LOC drop in `gpu_types.rs` alone, plus removal of the `vec3`-then-scalar guard discipline (which the project has admitted bit them 3× — `gpu_types.rs:838-843`). The cost is one mechanical sweep of `bytemuck::bytes_of` → `encase`-write at the upload sites. The benefit is that future structs *cannot* recreate the hazard. **High leverage.**

3. **The "audit suspicion 4" verdict is `partially-confirmed`** because the `graph_b.rs` split *is* residual but it doesn't add value to fix independently. Architect should not bother with a standalone `graph_b.rs → graph.rs` merge unless Finding 2 is already moving the nodes into their subsystem modules — in which case both files vanish naturally.

4. **The `ray_tracing.wgsl:122-136` "documentation-only consts" pattern is good.** The comment explicitly tells the reader "this is the canonical value, but the live SSoT is `GpuRenderParams`". This is a healthy convention worth duplicating elsewhere — particularly in `gi.rs:51-60` which sets `VALID_SAMPLE_STORAGE_COUNT` etc. as live `pub const` but the WGSL side reads from `gi_params.*`. A docblock at `gi.rs:51-60` clarifying "these constants are also uploaded to `GpuGiParams`; WGSL reads via uniform" would prevent the next Finding-5-class shadow.

5. **`prepare_world_gpu`'s palette-refresh path (`prepare.rs:218-317`) deserves a separate function name.** Currently it's a 100-LOC `if let Some(refresh) = voxel_types_refresh.as_deref()` inside the existing system body. The two responsibilities (build-once and palette-refresh) have nothing to do with each other except sharing the GrowableBuffer they touch. Architect should consider extracting it as `apply_voxel_types_refresh` and calling it from a small dispatch shell. This is a sub-concern of Finding 1, fold it in.

6. **The `_pad0a` / `_pad0b` history at `gpu_types.rs:84-91` is technical-debt comment-ware.** Comments reference "the former `exposure`/`tone_mapping_fac` half of the `18-taa-fidelity.md` fix #2 repurpose; kept as a pad to preserve the 112-byte layout". `ShaderType` makes this comment moot — the layout is computed, not preserved. **Side win of Finding 4.**

7. **The `prepare_frame_gpu` doc comments are unusually dense** (~50% of the file's LOC is doc comments). This is the project's "verbose docs ethos" per `01-context.md`. The architect should *not* prune docs as a LOC-reduction strategy — that violates `01-context.md` Q1's idiom-fit-first directive. Doc volume stays.

8. **The "D5 reads it as read-only" contract for `gpu_types.rs` is partially fictional.** D5's W4 added structs (`GpuEntityChunkInstance` + 3 siblings) to `gpu_types.rs` — they're in the file at `:712-766`. So D5's "read-only" really means "no edits to D4's structs", not "no edits to the file". The shared-files-notes section above clarifies this. **Architect must take this as the actual contract** when designing D4's `ShaderType` cutover.

9. **Equal-footing complaint.** The brief contains 4 "audit suspicions" and 7 "crosscutting items"; I read every cited file:line. The biggest single judgment I had to make solo was Finding 7 (the W4 placeholder bind-group cross-write) — it's only obvious if you read both `prepare.rs` and `15-design-c.md` §1.7. Architect, if you bypass Finding 7 ("just split the file, the bind-group write is fine"), you'll either (a) ship two systems writing the same field and discover the contention only in the merge of D4+D5 impl PRs, or (b) D5's impl agent will hit a refactor wall and escalate. Worth the Q&A round.

10. **Equal-footing — what's NOT in this exploration.** I deliberately did not enumerate every WGSL "magic number" in the render shaders beyond Findings 5 & 6 — there are dozens (`0x3FFFFFFFu`, `0xFFFu`, the various `>> Nu` shifts), most of which are bit-layout encoding helpers documented in the surrounding comment. A pure "magic-number sweep" of the WGSL files would surface ~40 candidates; ~35 would correctly read as load-bearing bit masks. I included only the 2 that genuinely shadow live Rust constants (Finding 5). Architect can re-scope if a broader WGSL constants pass is wanted.
