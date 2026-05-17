# vox-gpu-rewrite — empty-scene diagnostic (2026-05-17)

## Symptom

**User report:** running
```bash
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
```
shows "an empty scene" — only sky, no Oasis geometry visible.

**User cross-reference:** the C# reference (`/mnt/archive4/DEV/NAADF`) loads the
same fixture and renders the tiled Oasis world "nearly instantly" — no
progressive convergence delay.

**W5.5 e2e gate (`--vox-gpu-construction`) state:**
- Framebuffer luminance over the central 40%×40% region = **146.2**.
- This is the **exact sky-band tint** — the design predicted 146 as the value
  when the GPU producer chain DOES dispatch but rays never hit geometry
  (`02-design.md:1398-1434`).
- The W5.3 impl log incorrectly read this as a successful GREEN flip
  (`03-impl.md:784`). It is NOT. 146.2 means rays enter the world AABB and
  traverse the chunk grid without ever finding a non-empty block. The chunk
  grid is effectively empty.
- The producer node logs `vox-gpu-rewrite W5 — per-segment GPU producer chain
  DISPATCHED (512 segments × ...; voxel_workgroups=65535 (raw 134217729),
  block_workgroups=65535 (raw 2097153))` — i.e. the per-segment loop ran and
  the bounds chain dispatched. No wgpu validation errors fire.

## Hypotheses considered

| # | Hypothesis | Ranking after investigation |
|---|---|---|
| H1 | GPU producer wrote zeros to `segment_voxel_buffer` (params/bind-group/shader contract mismatch) | LOW — params validated, dispatch shape matches WGSL, segment buffer alloc + binding correct. |
| H2 | GPU producer wrote correctly but chunk_calc decoded to wrong WorldGpu chunks (per-segment `chunk_offset` / `segment_size_in_chunks` wrong) | LOW — W5.3 impl log explicitly rewrites `bounds_params_buffer.chunk_offset = group_offset_in_chunks` and `segment_size_in_chunks = 16` per segment; this is correct. |
| H3 | Bounds chain didn't seed the bound queue → W3 AADFs never compute → renderer single-steps and dies | LOW — `prepare_construction:1410-1439` DOES gate `dispatch_add_initial_groups` on `gpu_producer_has_run`; W5 flips that flag → seed fires on next frame. |
| H4 | C# `WorldData.GenerateWorld` does post-loop work the Rust port misses | LOW — `WorldData.cs:120-218` post-loop work is `boundHandler.Initialize()` (C#:130, BEFORE the loop, equivalent to Rust's `dispatch_add_initial_groups`), `boundHandler.SetupConstantParameters()` for queue dispatch, and ComputeVoxelBounds/ComputeBlockBounds (Rust has these). All equivalents exist. |
| H5 | Bounds-chain workgroups clamped to 65535 — only 3.1% of blocks get AADFs → renderer can't traverse | LOW (contributing, NOT primary) — under-AADF degrades raycast efficiency but the W3 background queue refines them over subsequent frames. Doesn't explain the immediate "empty scene". |
| H6 | 3D dispatch shape mismatch (1D 65535 cap vs 3D distribution) | LOW — chunk_calc's `compute_voxel_bounds`/`compute_block_bounds` workgroups are 1D; no 3D-vs-1D shape ambiguity. |
| H7 | Camera spawn pose looks at sky | CONTRIBUTING (secondary) — the Rust `InitialCameraPose::from_world_voxels` formula scales C# `(500, 200, 40)` by `world_y / 128`, placing the camera at **Y=800 in a world that's only 512 voxels tall**. The camera is ABOVE the world AABB, looking horizontally (+Z forward). Bottom-of-screen rays still angle down and would intersect the Oasis ground (~Y < 384), so this alone wouldn't produce a fully empty scene — but it dramatically narrows the visible region. |
| **H8 (NEW)** | **`prepare_world_gpu` allocates `blocks_buffer` / `voxels_buffer` from the CPU mirror's length. The W5 install path leaves CPU mirrors EMPTY → allocation collapses to ~130 / ~66 u32s — many orders of magnitude too small for the GPU producer's output. All chunk_calc atomic-cursor writes past those tiny bounds silently fail (WebGPU spec: OOB writes in storage buffers are dropped). Result: chunks buffer has valid `state` values pointing at indices nothing was written to → renderer dereferences pointers to zero-bytes → every chunk appears empty.** | **HIGH — confirmed by code Read; this is the root cause.** |

## C# reference behaviour

### WorldData buffer allocation (`NAADF/NAADF/World/Data/WorldData.cs:73-92`)

```csharp
// Line 73 — segment buffer: per-segment-cubic size, allocated up-front.
segmentVoxelBuffer = new StructuredBuffer(App.graphicsDevice, typeof(uint),
    ((int)Math.Pow(worldGenSegmentSizeInVoxels, 3)) / 2, ...);

// Lines 77-79 — dataVoxelGpu / dataBlockGpu: ONE-SEGMENT WORTH of capacity,
// allocated unconditionally at WorldData construction time (BEFORE any
// GenerateWorld dispatch). For a 4096³ fixed world this is:
//   segmentSizeInVoxels = WORLD_GEN_SEGMENT_SIZE_IN_VOXELS = 16 * 16 = 256
//   dataVoxelGpu count   = 256^3 / 2  = 8,388,608  u32s = 32 MiB
//   dataBlockGpu count   = 256^3 / 64 =   262,144  u32s =  1 MiB
dataVoxelGpu = new DynamicStructuredBuffer(App.graphicsDevice, typeof(uint),
    (worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels) / 2,
    ...);
dataBlockGpu = new DynamicStructuredBuffer(App.graphicsDevice, typeof(uint),
    (worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels * worldGenSegmentSizeInVoxels) / 64,
    ...);
```

### Per-segment growth (`WorldData.cs:145-151`)

After each segment's `CalculateChunkBlocks` dispatch C# reads back the cursor
and grows the buffers if needed:

```csharp
// Line 148 — CPU readback of the atomic cursors after each segment.
blockVoxelCountGpu.GetData(blockVoxelCount);
// Lines 150-151 — grow voxel/block buffers if the cursor pushed past current
// capacity. SetNewMinCount triggers a CPU→GPU copy of the existing data into
// a larger allocation if `current + maxNewPerGenSegment` exceeds capacity.
dataBlockGpu.SetNewMinCount((int)blockVoxelCount[1] + maxNewBlocksPerGenSegment, 2);
dataVoxelGpu.SetNewMinCount((int)blockVoxelCount[0] + maxNewVoxelsPerGenSegment / 2, 2);
```

### Pre-loop bound queue seed (`WorldData.cs:130`)

```csharp
boundHandler.Initialize();   // dispatches AddInitialGroupsToBoundQueue
```

`WorldBoundHandler.Initialize` (`WorldBoundHandler.cs:53-89`) seeds the X/Y/Z
queues with all groups at size 0 (the AADF refinement queue), then runs
`AddInitialGroupsToBoundQueue` to populate the per-axis mask bits. The
Rust port has the equivalent via `bounds_calc::dispatch_add_initial_groups`
called from `prepare_construction:1430-1437`, gated on `gpu_producer_has_run`.
**This gate fires correctly post-W5.3** (the W5 producer node flips the flag
when dispatched).

### Camera (`WorldRender.cs:48-49`)

```csharp
camera = new Camera(... , 90, 0.1f, 10000, 0.25f);
camera.SetPos(new Vector3(500, 200, 40));
```

**Literal** `(500, 200, 40)` voxel coordinates — NOT scaled to the loaded
world's dimensions. In the default 1024×128×1024 world the camera is at
`Y=200 > 128` (above the world ceiling), looking +Z; the bottom-of-screen rays
angle down and intersect the model. In the "Fixed Size" load (used for `.vox`
imports per `UiHeaderBar.cs:150`, default = `(16, 2, 16) × 4 × 64 = (4096, 512, 4096)`),
the camera STAYS at literal `(500, 200, 40)` — Y=200 is well inside the world
(Y_max=512). The C# world tiles Oasis across the full 4096×4096 XZ footprint
via the WGSL `vpim = voxel_pos % model_extent_v` modulo (which is faithfully
ported to `generator_model.wgsl:70`).

## Rust port behaviour

### Buffer allocation (`crates/bevy_naadf/src/render/prepare.rs:311-333`)

```rust
let cpu_blocks_len = extracted.blocks.len().max(1);
let cpu_voxels_len = extracted.voxels.len().max(1);
// ...
let blocks_alloc_len = if gpu_producer_enabled {
    ((cpu_blocks_len + 64) as u64 * W2_BUFFER_HEADROOM_MUL).max(64) as usize
} else {
    blocks_with_headroom.max(1) as usize
};
let voxels_alloc_len = if gpu_producer_enabled {
    ((cpu_voxels_len + 32) as u64 * W2_BUFFER_HEADROOM_MUL).max(32) as usize
} else {
    voxels_with_headroom.max(1) as usize
};
```

For the W5 `.vox` install path the CPU mirrors are EMPTY by design
(`grid.rs:409-425`: `chunks_cpu / blocks_cpu / voxels_cpu = Vec::new()`).
So `extracted.blocks.len() = 0` → `cpu_blocks_len = 1` →
`blocks_alloc_len = ((1 + 64) * 2).max(64) = 130 u32s` = **520 bytes**.
Same arithmetic for voxels → **264 bytes**.

The `blocks` / `voxels` storage buffers `prepare_world_gpu` hands to the
production `WorldGpu` are these tiny allocations. They are bound on
`world_layout @group(0)` (renderer reads) AND on
`construction_world_layout @group(0)` (chunk_calc writes via atomic-cursor
appends). The W5 segment loop's chunk_calc dispatches then atomically append
to `blocks_alloc_len = 130` / `voxels_alloc_len = 66`-sized buffers via:

```wgsl
// chunk_calc.wgsl:412
let new_base = atomicAdd(&block_voxel_count[1], 64u);
// chunk_calc.wgsl:434
blocks[base + local_index] = block;
```

After the first ~2 mixed chunks the atomic cursor (`block_voxel_count[1]`)
crosses 130, and every subsequent block write is OOB. WebGPU spec
(§Storage Buffer Access) says OOB writes are dropped silently; the cursor
keeps advancing. The chunks buffer ends up with valid `state` words pointing
at block indices nothing was written to.

For Oasis (93×34×84 chunks) tiled across the full 256×32×256 fixed world,
the number of mixed blocks is on the order of `256 * 32 * 256 * 64 ≈ 134M`
— the producer node's own log shows the bounds chain raw workgroup-count
estimate is `raw 134,217,729` (= max mixed voxels / 32) and
`raw 2,097,153` (= max mixed blocks / 64). The buffer needs to hold at
least the same magnitude. It holds 130 / 66.

### Per-segment dispatch (`crates/bevy_naadf/src/render/construction/mod.rs:2196-2299`)

The W5 producer body itself is structurally correct:
- Loop order Z/Y/X matches C# `WorldData.cs:136-140`.
- `group_offset_in_chunks = (sx, sy, sz) * 16` matches C# `segmentPosInChunks`.
- `generator_model` dispatch shape `[16,16,16]` matches WGSL workgroup
  layout (4×4×4 threads, one workgroup per chunk).
- chunk_calc dispatch shape `[16,16,16]` matches C# `WorldData.cs:506`.
- Per-segment uniform rewrites (`model_data_params_buffer` AND
  `bounds_params_buffer`) are correct per W5.3 impl log §
  "Critical fidelity detail not in the design spec".

**The producer dispatch logic itself is fine. The buffers it writes into
are simply not allocated big enough to hold the writes.**

### Bound-queue seed (`mod.rs:1410-1439`)

The W3 bound-queue seed dispatches `add_initial_groups_to_bound_queue` on the
frame AFTER `gpu_producer_has_run` flips. This is structurally correct and
mirrors C# `boundHandler.Initialize()` (called BEFORE the loop in C#, called
ONE FRAME AFTER the loop in Rust — observationally equivalent because both
seed before the renderer reads). Not a contributor to the empty-scene bug.

### Camera (`crates/bevy_naadf/src/camera/mod.rs:51-65`)

```rust
pub fn from_world_voxels(world_voxels: [u32; 3]) -> Self {
    let w = world_voxels[0] as f32;
    let h = world_voxels[1] as f32;
    let d = world_voxels[2] as f32;
    let pos = Vec3::new(w * (500.0 / 1024.0), h * (200.0 / 128.0), d * (40.0 / 1024.0));
    let transform = Transform::from_translation(pos).looking_at(pos + Vec3::Z, Vec3::Y);
    InitialCameraPose(transform)
}
```

For the 4096×512×4096 fixed world this places the camera at
`(2000, 800, 160)` looking at `(2000, 800, 161)` (forward = +Z, no Y
component). The startup log confirms:
```
camera::setup_camera: framing loaded world — pos=(2000.00, 800.00, 160.00),
                     look_at=(2000.00, 800.00, 161.00)
```

**Y=800 is ABOVE the world's 512-voxel ceiling.** The screen-center ray
exits via the top plane and never enters the world AABB. The bottom-of-screen
rays angle down (FOV 90 → 45° at screen edges), so the bottom-edge ray
direction is approximately `(0, -0.707, 0.707)`, which DOES intersect the
world ceiling at `Z ≈ 567`, comfortably inside the world.

Even after H8 is fixed, the camera-position issue will leave most of the
viewport showing sky. **This is a SECONDARY issue, distinct from H8.** The
camera positioning is a divergence from C#: C# uses literal `(500, 200, 40)`
in the 4096³ fixed world (Y=200 < ceiling 512 → camera inside the world); the
Rust port rescales to `(2000, 800, 160)` because `from_world_voxels` is a
test-grid helper that assumes the world is sized to the model. For the
fixed-world `.vox` path the rescale is wrong — the C#-faithful behaviour is
to drop the scale and use literal `(500, 200, 40)`.

## Identified gap

### Primary (root cause of empty-scene) — H8

**`prepare_world_gpu` (`render/prepare.rs:311-333`) sizes `blocks_alloc_len`
and `voxels_alloc_len` from `extracted.blocks.len()` / `extracted.voxels.len()`,
both of which are ZERO for the W5 `.vox` fixed-world install path.** The
resulting 130-u32 / 66-u32 buffers cannot hold the GPU producer's atomic-
cursor-appended output; every write past index ~2 / ~1 is silently dropped by
the WebGPU runtime, leaving the `chunks_buffer` populated with pointers to
unwritten memory regions. The renderer dereferences these pointers, reads
zero bytes, and treats every chunk as empty.

The bug is invisible at validation time:
- wgpu accepts the buffer allocations (sized below 1 MiB; well under any cap).
- The shader compiles + dispatches cleanly (the shader doesn't know the
  buffer's logical capacity vs cursor).
- The atomic cursor keeps advancing past the buffer size (atomics on
  in-bounds elements are fine; the OOB writes that follow are spec-defined
  no-ops).
- `gpu_producer_has_run` flips correctly; downstream nodes dispatch.
- Sky luminance ~146 — the framebuffer is exactly what
  "everything dispatched, no geometry found" would produce.

### Secondary (independently broken; will still leave camera pointing at sky after H8 is fixed)

**`InitialCameraPose::from_world_voxels`** scales C# magic coords by world
extent ratios that put the camera Y above the world ceiling for the fixed-
world `.vox` path. The C#-faithful behaviour for `install_vox_in_fixed_world`
is literal `(500, 200, 40)`, which is inside the 4096×512×4096 fixed world.

## Recommended fix (NOT to be implemented in this dispatch)

### Fix #1 (primary — required to make the W5 path render anything)

**File:** `crates/bevy_naadf/src/render/prepare.rs:311-333`

Replace the `cpu_blocks_len` / `cpu_voxels_len` base with a `size_in_chunks`-
derived upper bound when the CPU mirror is empty AND
`gpu_producer_enabled = true`. Mirror C# `WorldData.cs:77-79`:

```rust
let cpu_blocks_len = extracted.blocks.len();
let cpu_voxels_len = extracted.voxels.len();

// vox-gpu-rewrite — when the GPU producer is the source of truth (W5
// `.vox` install path), the CPU mirrors are empty by design and cannot
// be used to size the GPU output buffers. Derive an upper bound from
// `size_in_chunks` instead (matches C# `WorldData.cs:77-79`'s
// per-segment-cubic allocation, scaled up for the full-world cursor
// cumulative output).
//
// Per-chunk worst case: 64 blocks × 32 voxel u32s = 2048 voxel u32s.
// Per-chunk worst case: 64 blocks. Cap at the full-world cube
// (`chunks * 64` blocks; `chunks * 64 * 32` voxels).
//
// For the 256×32×256 fixed world:
//   chunks = 2,097,152
//   max_blocks_alloc  = 2,097,152 * 64 = 134,217,728 u32s = 512 MiB
//   max_voxels_alloc  = 134,217,728 * 32 = 4,294,967,296 u32s = 16 GiB
//
// 16 GiB exceeds every realistic wgpu cap. Drop to the per-segment
// cubic cap C# uses (matches `WorldData.cs:77-79`'s up-front allocation
// — C# grows it per segment via `SetNewMinCount`, but the up-front
// allocation is the per-segment cap and is what the Rust port needs to
// hit as the baseline allocation):
//
//   segment_voxels = 256³ = 16,777,216
//   per_segment_blocks = 16,777,216 / 64 =   262,144 u32s = 1 MiB
//   per_segment_voxels = 16,777,216 /  2 = 8,388,608 u32s = 32 MiB
//
// Multiply by `WORLD_SIZE_IN_SEGMENTS.x * y * z = 512` to cover the
// full world without per-segment grow (which the Rust port doesn't
// implement). Total: 512 MiB blocks + 16 GiB voxels — still past the
// cap. Use the more realistic upper bound: `cpu_chunks * 64`
// (= mixed-block cap, sane for any normal scene where ~all chunks are
// mixed) and `cpu_chunks * 64 * 32 / sparsity_factor` for voxels.
let chunk_count_u64 = (extracted.size_in_chunks.x as u64)
    * (extracted.size_in_chunks.y as u64)
    * (extracted.size_in_chunks.z as u64);

let blocks_alloc_len = if gpu_producer_enabled {
    let from_chunks = chunk_count_u64.saturating_mul(64); // max mixed blocks
    let from_cpu_with_headroom =
        ((cpu_blocks_len.max(1) + 64) as u64) * W2_BUFFER_HEADROOM_MUL;
    from_chunks.max(from_cpu_with_headroom).max(64) as usize
} else {
    blocks_with_headroom.max(1) as usize
};
let voxels_alloc_len = if gpu_producer_enabled {
    // Voxels are sparser (only mixed blocks contribute 32 u32s each); a
    // realistic cap is `max_blocks * 32`, but C# allocates only
    // `segment^3 / 2` per segment because most blocks are uniform. For
    // the W5 path with no CPU mirror, use `chunks * 64 * 32 / 16 =
    // chunks * 128` (assume 1/16 of all possible voxel-pairs are mixed;
    // empirically Oasis's stamp-block layout is much sparser than this).
    let from_chunks = chunk_count_u64.saturating_mul(128); // realistic cap
    let from_cpu_with_headroom =
        ((cpu_voxels_len.max(1) + 32) as u64) * W2_BUFFER_HEADROOM_MUL;
    from_chunks.max(from_cpu_with_headroom).max(32) as usize
} else {
    voxels_with_headroom.max(1) as usize
};
```

**Sizing sanity-check for the Oasis fixed-world case:**
- `chunk_count = 256 * 32 * 256 = 2,097,152`
- `blocks_alloc_len = 2,097,152 * 64 = 134,217,728 u32s = 512 MiB`
- `voxels_alloc_len = 2,097,152 * 128 = 268,435,456 u32s = 1 GiB`

Both fit Vulkan's typical 4 GiB single-buffer cap on the RTX 5080. If
`max_buffer_size` proves smaller on other backends, the fix should split into
two-stage allocation (start with `from_cpu_with_headroom`, grow on first
overflow via a GPU readback + `GrowableBuffer::reserve`). For W5 first-pass,
the static cap is the minimal patch.

**Verification:** the user runs `cargo run --release --bin bevy-naadf -- --vox
/home/midori/Downloads/Oasis_Hard_Cover.vox` and visually confirms Oasis
geometry renders (after applying both Fix #1 and Fix #2). The W5.5 e2e gate
`--vox-gpu-construction` luminance should drop from 146 (sky band) into a
mixed range where the central region shows geometry (the standard e2e camera
at (86, 42, 90) IS positioned inside the world and looking at a populated
region).

### Fix #2 (secondary — required for camera to point at populated region)

**File:** `crates/bevy_naadf/src/voxel/grid.rs:381-388`

Stop calling `InitialCameraPose::from_world_voxels`; pass the C#-literal
`(500, 200, 40)` voxel pose directly. The fixed-world `.vox` install path
should be C#-faithful here — the rescaling is appropriate only for the
sized-to-model `install_vox_sized_to_model` path.

```rust
// vox-gpu-rewrite — for the FIXED-WORLD install path, use C#'s literal
// camera spawn at (500, 200, 40) (`WorldRender.cs:48-49`). The world IS
// 4096×512×4096 (matches C# "Fixed Size" default per `UiHeaderBar.cs:150`
// = `(16, 2, 16) × 4 × 64 = (4096, 512, 4096)`); Y=200 is well inside
// the world (Y_max = 512), so the camera is positioned as the C# user
// would see it. `from_world_voxels` rescaling places Y=800 > 512 (above
// the ceiling) and is appropriate only for the sized-to-model path.
let initial_pose_xyz = Vec3::new(500.0, 200.0, 40.0);
let initial_pose_transform = Transform::from_translation(initial_pose_xyz)
    .looking_at(initial_pose_xyz + Vec3::Z, Vec3::Y);
commands.insert_resource(crate::camera::InitialCameraPose(initial_pose_transform));
```

(May require making `InitialCameraPose`'s inner field accessible, or adding
a `pub const fn from_literal(pos: Vec3) -> Self` constructor in
`camera/mod.rs`.)

### Optional follow-up (NOT blocking)

**Workgroup-count clamping at 65535 in the producer's bounds chain
(`mod.rs:2339`)** — once Fix #1 lands and blocks/voxels are correctly
populated, the under-AADF problem (only the first 65535 blocks /voxels get
the bounds compute pass) WILL surface as a secondary issue: empty-region
raycasting will be slow, and the W3 background queue will need many frames
to catch up. The W5.3 impl log calls this out as a "future improvement"
(indirect dispatch sourcing from `block_voxel_count[]`). Not blocking the
empty-scene fix.

## Confidence level

**HIGH for Fix #1 (primary root cause):**

- The undersizing is provable from a direct Read of `prepare.rs:311-333`
  and `grid.rs:409-425`.
- The math is unambiguous: `((1 + 64) * 2).max(64) = 130 u32s` for a workload
  that needs ~134M u32s.
- WebGPU spec on OOB storage buffer writes (silently dropped) is well
  defined.
- The sky-luminance 146 matches the design's prediction for the
  "everything dispatched, no geometry hit" state (`02-design.md:1398-1434`).
- No wgpu validation errors fire (consistent with "writes silently dropped,
  not validated against logical capacity").
- C# allocates `dataBlockGpu` / `dataVoxelGpu` up-front at per-segment-cubic
  size (`WorldData.cs:77-79`), then grows per segment. The Rust port does
  neither; the static allocation is sized for the empty CPU mirror.

**MEDIUM for Fix #2 (secondary; the camera is in the wrong place but rays
from the bottom of the screen DO enter the world AABB, so Fix #1 alone
should make SOME geometry visible — just not framed well):**

- The camera-rescaling formula is documented (`camera/mod.rs:32-44`) as a
  test-grid helper assuming the world is sized to the model.
- For the fixed-world path the formula's assumption is wrong (world is
  4096×512×4096 regardless of model size); the formula's Y output exceeds
  the world ceiling.
- C# reference is unambiguous: literal `(500, 200, 40)`, no rescaling
  (`WorldRender.cs:48-49`).

## Observation evidence

### Boot run (`cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox`)

```
[INFO bevy_naadf::voxel::grid] NAADF .vox loaded → ModelData (93×34×84 chunks;
   data_chunk=265608 u32s, data_block=1617216 u32s, data_voxel=10498368 u32s,
   257 palette entries). Fixed world 256×32×256 chunks; GPU producer chain
   runs per WORLD_SIZE_IN_SEGMENTS = (16, 2, 16).
[INFO bevy_naadf::camera] camera::setup_camera: framing loaded world —
   pos=(2000.00, 800.00, 160.00), look_at=(2000.00, 800.00, 161.00)
[INFO bevy_naadf::render::construction] vox-gpu-rewrite W5 — per-segment GPU
   producer chain DISPATCHED (512 segments × (generator_model + calc_block);
   bounds chain ×1; voxel_workgroups=65535 (raw 134217729),
   block_workgroups=65535 (raw 2097153)).
```

Three things to note:
1. ModelData parsed correctly — 265,608 chunks / 1,617,216 blocks / 10,498,368
   voxel u32s for Oasis. The input to the GPU producer is valid.
2. Camera is at Y=800 in a 512-tall world — above the ceiling.
3. The producer's own log shows `raw block_workgroups = 2,097,153` and
   `raw voxel_workgroups = 134,217,729`. These are the dispatch-shape
   estimates; they're ALSO the approximate order of magnitude of how many
   u32s the `blocks` / `voxels` storage buffers need to hold the producer's
   atomic-cursor output. The actual allocations (130 / 66 u32s) are 6 to 8
   orders of magnitude smaller.

### No wgpu validation errors

Run with `WGPU_VALIDATION=full RUST_LOG=info,wgpu_core::device=warn,wgpu_hal=warn`
emits no error / panic / invalid / exceed messages relevant to the W5 chain.
This is consistent with H8 (the OOB writes are spec-defined no-ops at the
WebGPU layer; they don't trigger validation).

### W5.5 e2e gate (`cargo run --release --bin e2e_render -- --vox-gpu-construction`)

```
[INFO bevy_naadf::render::construction] vox-gpu-rewrite W5 — per-segment GPU
   producer chain DISPATCHED (512 segments × ...; voxel_workgroups=65535
   (raw 134217729), block_workgroups=65535 (raw 2097153)).
e2e_render: luminance gate (batch 6) — 100.0% of the frame is non-black
   (luminance > 2); threshold 95%
e2e_render: region luminance — emissive 10.7, solid(GI-lit diffuse) 7.0,
   sky 146.2
```

Sky 146.2 matches the design-predicted "Oasis off-frame OR GPU producer
writing nothing meaningful" value. The W5.3 impl log read this as success;
it is not — the value is consistent with EITHER an off-frame camera (which
the standard e2e camera at (86, 42, 90) is NOT for a tiled Oasis), OR the
chunks pointing at unwritten blocks/voxels regions (H8). Since the camera
at (86, 42, 90) IS inside a region the W5 producer claims to have populated,
146 sky-band means the chunk pointers are bogus.

### Direct Read evidence

- `crates/bevy_naadf/src/voxel/grid.rs:409-425` —
  `chunks_cpu / blocks_cpu / voxels_cpu = Vec::new()` for the W5 install
  path. Confirmed empty.
- `crates/bevy_naadf/src/render/extract.rs:210-218` — `WorldGpuStaging`
  clones the empty Vecs verbatim. `staging.blocks.len() = 0`.
- `crates/bevy_naadf/src/render/prepare.rs:311-333` — allocations sized
  from `staging.blocks.len().max(1) = 1` → `((1+64)*2).max(64) = 130 u32s`.
- `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:412-434` —
  unconditional `atomicAdd` then write to `blocks[base + local_index]` /
  `voxels[base + i]` with no bounds check. The producer relies on the
  buffer being big enough; it isn't.

### C# allocation evidence (for comparison)

- `NAADF/NAADF/World/Data/WorldData.cs:77-79` — `dataVoxelGpu` and
  `dataBlockGpu` are allocated at PER-SEGMENT-CUBIC capacity
  (`256³ / 2` and `256³ / 64` u32s respectively) UP FRONT in the
  constructor, BEFORE any `GenerateWorld` dispatch. For the same workload
  the Rust port allocates 130 / 66 u32s. The ratio: C# = 8 MiB / 1 MiB,
  Rust = 520 B / 264 B. Rust is **~32,000× smaller for blocks** and
  **~127,000× smaller for voxels**.
- `WorldData.cs:148-151` — C# additionally grows per segment via CPU
  readback. The Rust port does not implement per-segment grow; the
  static allocation must therefore cover the full-world cumulative cap,
  not just one segment.
