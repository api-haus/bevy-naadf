# 02c — Design: `WindowedSlotMap` (Phase 2.6, supersedes `03e` § "What's left")

Author: delegate-architect (Phase 2.6).
Status: design only. Implementation lives in Phase 2.6 impl dispatch.

This file SUPERSEDES the "Option A / Option B" sketch at the end of
`03e-impl-residency-fix.md`. The `WindowedSlotMap` primitive consolidates the
three concerns (slot pool, world↔slot bidirectional mapping, GPU-uploaded
window indirection) the residency layer needs into a single closed-API data
structure with enforced invariants.

The mismatch this fixes — `03e` § "Surprises during implementation" #2 — is
that today the residency-driver's Pass 3 assigns world segments to free slot
indices in arbitrary order (`empty_slots.pop()`), so slot `N` does NOT
geometrically correspond to window-local position `local_of(N)`. The renderer
ASSUMES that geometric mapping (since the camera is pre-translated to
window-local frame in `pin_streaming_window_camera`), so reads at world camera
position `(8, 1, 8)` look up `chunks_buffer[slot_index_of(8,1,8) = 280]`,
which holds content for whatever world segment happened to land at slot 280 —
not the one the camera is actually inside.

`WindowedSlotMap` fixes this by introducing an EXPLICIT renderer-side
indirection table (`array<u32, 512>`) that maps `pack(local_xyz) →
SlotIndex`. Slot allocation is now pool-driven (pick any free slot for an
admission) and the renderer reads `chunks_buffer[indirection[pack(local_xyz)]
* CHUNKS_PER_SEGMENT + chunk_offset_within_segment]`. This decouples slot
position in the GPU buffer from window-local geometry, which means
`set_origin` shifts can keep existing slots in place (just rebuild the
indirection table to point to new local positions) — no GPU memcpy.

## Design

### A. Type spec — full impl-ready definition

```rust
// crates/bevy_naadf/src/streaming/windowed_slot_map.rs (new file)
//
// Author: Phase 2.6.

use std::collections::HashMap;

use bevy::math::{IVec3, UVec3};

use super::residency::{SlotIndex, WorldSegmentPos};

/// Sentinel meaning "no slot occupies this local position".
/// `u32::MAX` is safe — capacity is 512 (16×2×16), far below MAX.
pub const EMPTY_SLOT: u32 = u32::MAX;

/// A fixed-capacity association between resident world segments and GPU slot
/// indices, plus the window-local indirection table the renderer consumes.
///
/// Three concerns, ONE invariant: the indirection table is always consistent
/// with the bindings, enforced by making every mutation go through
/// [`bind`] / [`unbind`] / [`set_origin`].
///
/// See `docs/orchestrate/streaming-world/02c-design-windowed-slot-map.md`
/// for the full rationale.
#[derive(Clone, Debug)]
pub struct WindowedSlotMap {
    // -------- Pool (LIFO over [0, capacity)) ------------------------------
    /// Free-list of slot indices, popped from the back. Seeded in
    /// reverse order at `new()` so `allocate()` returns slot 0 first
    /// (deterministic — every test relies on this).
    free_list: Vec<SlotIndex>,

    // -------- Mapping (bidirectional, host-side) --------------------------
    world_to_slot: HashMap<WorldSegmentPos, SlotIndex>,
    /// Dense reverse lookup. `slot_to_world[slot.0 as usize]` is `Some(w)`
    /// when slot is bound to world-segment `w`, `None` when free.
    slot_to_world: Vec<Option<WorldSegmentPos>>,

    // -------- Window (derived view, GPU-uploaded) -------------------------
    origin: IVec3,
    window_size: UVec3,
    /// Flat row-major-X-fastest table of size `window_size.x * y * z`. Entry
    /// at `pack(local_xyz)` is `slot.0` when a world segment is bound at
    /// that local position, [`EMPTY_SLOT`] otherwise.
    ///
    /// **This is the buffer the GPU uploads each frame.** Layout is fixed
    /// by [`Self::pack`] and is byte-identical between Rust and WGSL.
    indirection: Vec<u32>,
}
```

#### Concrete sizes (from the existing constants — verified)

- `window_size = UVec3::new(WORLD_SIZE_IN_SEGMENTS.x, WORLD_SIZE_IN_SEGMENTS.y, WORLD_SIZE_IN_SEGMENTS.z) = (16, 2, 16)` per `crates/bevy_naadf/src/lib.rs:247`.
- `capacity = 16 * 2 * 16 = 512`.
- `indirection` buffer size = 512 × 4 B = **2048 B**. Fits in one wgpu uniform block; we use `STORAGE` anyway for parity with the existing layout style.

#### Existing types reused verbatim

- `pub struct SlotIndex(pub u32)` — `crates/bevy_naadf/src/streaming/residency.rs:46`.
- `pub struct WorldSegmentPos(pub IVec3)` — `residency.rs:41` (derives `Clone, Copy, Debug, PartialEq, Eq, Hash` — `Hash` required for `world_to_slot: HashMap`).

#### API surface (closed — every mutation goes through one of these)

```rust
impl WindowedSlotMap {
    /// Build an empty map.
    ///
    /// `window_size` defines the indirection table extent and the capacity
    /// (`= window_size.x * y * z`). For the streaming preset this is
    /// `UVec3::new(16, 2, 16) = 512`.
    pub fn new(window_size: UVec3) -> Self;

    // --- queries (`&self`) -----------------------------------------------
    pub fn capacity(&self) -> u32; // window_size.x * y * z
    pub fn origin(&self) -> IVec3;
    pub fn window_size(&self) -> UVec3;
    /// True iff `world_seg` falls inside `[origin, origin + window_size_signed)`.
    pub fn is_in_window(&self, world_seg: WorldSegmentPos) -> bool;
    /// Window-local position (`world_seg.0 - origin`). Caller must ensure
    /// `is_in_window` — otherwise the result is meaningless / negative.
    pub fn local_of(&self, world_seg: WorldSegmentPos) -> IVec3;
    /// Slot index currently bound to `world_seg`, or `None` if not resident.
    pub fn lookup_slot(&self, world_seg: WorldSegmentPos) -> Option<SlotIndex>;
    /// World-segment bound to `slot`, or `None` if the slot is free.
    pub fn lookup_world(&self, slot: SlotIndex) -> Option<WorldSegmentPos>;
    /// Iterator over every bound (world, slot) pair. Order unspecified.
    pub fn iter_bound(&self) -> impl Iterator<Item = (WorldSegmentPos, SlotIndex)> + '_;
    /// Slice the GPU uploads. `&[u32]` of length `capacity()`.
    pub fn indirection_buffer(&self) -> &[u32];
    /// Number of free slots remaining (≤ `capacity()`).
    pub fn free_count(&self) -> u32;

    // --- pool primitives (`&mut self`) -----------------------------------
    /// Pop a free slot from the pool, returning `None` when the pool is
    /// empty. Does NOT bind — caller follows up with [`bind`].
    pub fn allocate(&mut self) -> Option<SlotIndex>;
    /// Return `slot` to the pool. Panics in debug if `slot` is still bound
    /// (i.e. `slot_to_world[slot.0] != None` or `world_to_slot` still
    /// references it). Encodes the invariant "free slots have no mapping."
    pub fn free(&mut self, slot: SlotIndex);

    // --- mapping mutators (`&mut self`) ----------------------------------
    /// Associate `world_seg → slot` and `slot → world_seg`. Updates the
    /// indirection table at `pack(local_of(world_seg))` to `slot.0`. Panics
    /// in debug if:
    /// - `world_seg` is outside the current window (`!is_in_window`).
    /// - `world_seg` is already bound to a different slot.
    /// - `slot` is already bound to a different world segment.
    pub fn bind(&mut self, world_seg: WorldSegmentPos, slot: SlotIndex);

    /// Clear the binding for `world_seg`. Returns the freed slot for the
    /// caller to either re-`bind` (for an immediate admission) or `free`
    /// (return to pool). The indirection table is updated to
    /// [`EMPTY_SLOT`] at the corresponding local position. Returns `None`
    /// if `world_seg` was not bound.
    pub fn unbind(&mut self, world_seg: WorldSegmentPos) -> Option<SlotIndex>;

    /// Shift the window. Auto-unbinds every segment whose new local
    /// position would be out of window; rebuilds the indirection table
    /// from scratch for all remaining bound segments. Returns the
    /// `(world_seg, slot)` pairs that were unbound — the caller decides
    /// whether to `free()` them (return to pool) or `bind()` them to new
    /// admissions in the same call.
    ///
    /// `new_origin == origin()` is a fast-path no-op (returns empty Vec
    /// without touching the indirection buffer).
    pub fn set_origin(&mut self, new_origin: IVec3) -> Vec<(WorldSegmentPos, SlotIndex)>;

    // --- packing helpers --------------------------------------------------
    /// Pack a `local_xyz: IVec3` into a flat `u32` indirection-table index.
    /// Row-major, X-fastest, matching the rest of the codebase's chunk
    /// indexing (`chunk_calc.wgsl:424-426`, `world_change.wgsl:320-322`,
    /// `bounds_calc.wgsl:365-367`, `ray_tracing.wgsl:290-294`).
    ///
    /// Caller must ensure `0 <= local_xyz.{x,y,z} < window_size.{x,y,z}`.
    /// Debug-asserts the bounds.
    pub fn pack(&self, local_xyz: IVec3) -> u32;

    #[cfg(debug_assertions)]
    /// Verify every invariant (B). Called by `bind` / `unbind` /
    /// `set_origin` at exit. Used directly by unit tests.
    fn audit_invariants(&self);
}
```

#### `new()` body — concrete

```rust
pub fn new(window_size: UVec3) -> Self {
    let capacity = (window_size.x * window_size.y * window_size.z) as usize;
    let mut free_list = Vec::with_capacity(capacity);
    // Push in REVERSE order so `pop()` returns SlotIndex(0) first.
    // Deterministic ordering matches existing residency_driver test fixtures.
    for i in (0..capacity).rev() {
        free_list.push(SlotIndex(i as u32));
    }
    Self {
        free_list,
        world_to_slot: HashMap::with_capacity(capacity),
        slot_to_world: vec![None; capacity],
        origin: IVec3::ZERO,
        window_size,
        indirection: vec![EMPTY_SLOT; capacity],
    }
}
```

#### `pack()` body — concrete

```rust
pub fn pack(&self, local_xyz: IVec3) -> u32 {
    debug_assert!(
        local_xyz.x >= 0 && (local_xyz.x as u32) < self.window_size.x
            && local_xyz.y >= 0 && (local_xyz.y as u32) < self.window_size.y
            && local_xyz.z >= 0 && (local_xyz.z as u32) < self.window_size.z,
        "pack({local_xyz:?}) outside window_size={:?}", self.window_size,
    );
    let lx = local_xyz.x as u32;
    let ly = local_xyz.y as u32;
    let lz = local_xyz.z as u32;
    lx + ly * self.window_size.x
       + lz * self.window_size.x * self.window_size.y
}
```

Row-major-X-fastest verified against:
- `chunk_calc.wgsl:424-426` — `chunk_idx = chunk_pos.x + chunk_pos.y * sx + chunk_pos.z * sx * sy`.
- `world_change.wgsl:320-322` — same formula.
- `bounds_calc.wgsl:365-367` — same formula.
- `ray_tracing.wgsl:290-294` — same formula via `flatten_index(chunk_pos, sx, sx*sy)`.
- `residency.rs:128-132` `slot_index_of([lx, ly, lz])` — same formula (the existing one we're replacing).

### B. Invariant assertions — exact `debug_assert` set

The single source-of-truth `audit_invariants` body (debug-only):

```rust
#[cfg(debug_assertions)]
fn audit_invariants(&self) {
    let cap = self.capacity() as usize;

    // I1 — buffer / pool size match.
    debug_assert_eq!(self.slot_to_world.len(), cap,
        "I1: slot_to_world length must equal capacity");
    debug_assert_eq!(self.indirection.len(), cap,
        "I1: indirection length must equal capacity");

    // I2 — pool + bound = capacity.
    debug_assert_eq!(
        self.free_list.len() + self.world_to_slot.len(), cap,
        "I2: free + bound must equal capacity (free={}, bound={}, cap={})",
        self.free_list.len(), self.world_to_slot.len(), cap,
    );

    // I3 — free_list ↔ slot_to_world[None] consistency.
    let none_count = self.slot_to_world.iter().filter(|s| s.is_none()).count();
    debug_assert_eq!(none_count, self.free_list.len(),
        "I3: slot_to_world None-count must equal free_list length");

    // I4 — every entry in free_list maps to None in slot_to_world.
    for fs in &self.free_list {
        debug_assert!(self.slot_to_world[fs.0 as usize].is_none(),
            "I4: free slot {fs:?} has non-None slot_to_world");
    }

    // I5 — bidirectional mapping consistency (forward → reverse).
    for (w, slot) in self.world_to_slot.iter() {
        debug_assert_eq!(
            self.slot_to_world[slot.0 as usize], Some(*w),
            "I5: world_to_slot {w:?} -> {slot:?} but slot_to_world disagrees",
        );
    }
    // I5b — bidirectional mapping consistency (reverse → forward).
    for (i, w_opt) in self.slot_to_world.iter().enumerate() {
        if let Some(w) = w_opt {
            debug_assert_eq!(
                self.world_to_slot.get(w).copied(), Some(SlotIndex(i as u32)),
                "I5b: slot_to_world[{i}]={w:?} but world_to_slot disagrees",
            );
        }
    }

    // I6 — every bound world_seg is inside the current window.
    for w in self.world_to_slot.keys() {
        debug_assert!(self.is_in_window(*w),
            "I6: bound segment {w:?} out of window (origin={:?}, size={:?})",
            self.origin, self.window_size);
    }

    // I7 — indirection[pack(local_of(w))] == slot for every bound pair.
    for (w, slot) in self.world_to_slot.iter() {
        let local = self.local_of(*w);
        let idx = self.pack(local);
        debug_assert_eq!(self.indirection[idx as usize], slot.0,
            "I7: indirection mismatch at {w:?} local={local:?} pack={idx}: \
             expected slot {slot:?}, found {}", self.indirection[idx as usize]);
    }

    // I8 — every indirection entry that's NOT EMPTY_SLOT corresponds to a
    // bound pair. (Mirror of I7 in the other direction.)
    for (idx, slot_u) in self.indirection.iter().copied().enumerate() {
        if slot_u != EMPTY_SLOT {
            debug_assert!((slot_u as usize) < cap,
                "I8: indirection[{idx}] = {slot_u} out of slot range");
            let w_opt = self.slot_to_world[slot_u as usize];
            debug_assert!(w_opt.is_some(),
                "I8: indirection[{idx}] points to free slot {slot_u}");
        }
    }
}
```

Performance: `audit_invariants` is O(capacity) = O(512). Cheap enough to
call at the exit of every mutator in `cfg(debug_assertions)`. The `release`
build elides the call entirely.

### C. `set_origin` — the load-bearing operation

Algorithm in pseudo-Rust (the impl agent translates to literal code):

```rust
pub fn set_origin(&mut self, new_origin: IVec3) -> Vec<(WorldSegmentPos, SlotIndex)> {
    // Edge case 1 — no shift, no work.
    if new_origin == self.origin {
        #[cfg(debug_assertions)]
        self.audit_invariants();
        return Vec::new();
    }

    // (1) Compute new window AABB. Window-size axes are u32, cast through
    // i32 for the half-open right edge math. `WORLD_SIZE_IN_SEGMENTS = (16,
    // 2, 16)` fits in i32 without overflow.
    let ws = self.window_size;
    let aabb_min = new_origin;
    let aabb_max = IVec3::new(
        new_origin.x + ws.x as i32,
        new_origin.y + ws.y as i32,
        new_origin.z + ws.z as i32,
    );

    // (2) Walk world_to_slot; collect every (world, slot) pair that falls
    // OUTSIDE the new AABB. Cannot mutate `world_to_slot` while iterating;
    // collect into a Vec first.
    let mut evicted: Vec<(WorldSegmentPos, SlotIndex)> = Vec::new();
    for (w, slot) in self.world_to_slot.iter() {
        let p = w.0;
        let inside = p.x >= aabb_min.x && p.x < aabb_max.x
                  && p.y >= aabb_min.y && p.y < aabb_max.y
                  && p.z >= aabb_min.z && p.z < aabb_max.z;
        if !inside {
            evicted.push((*w, *slot));
        }
    }

    // (3) Unbind each evicted pair. DO NOT push slots into free_list —
    // return them so the caller decides free vs immediate-re-bind.
    for (w, slot) in &evicted {
        self.world_to_slot.remove(w);
        self.slot_to_world[slot.0 as usize] = None;
        // indirection cleared in (5), so no per-slot write here.
    }

    // (4) Adopt the new origin.
    self.origin = new_origin;

    // (5) Rebuild `indirection` from scratch. Clear to EMPTY_SLOT, then
    // populate from every REMAINING bound pair.
    for entry in &mut self.indirection {
        *entry = EMPTY_SLOT;
    }
    for (w, slot) in self.world_to_slot.iter() {
        // After (3) every remaining `w` is inside the new window (the
        // evicted set covered everything outside).
        let local = IVec3::new(
            w.0.x - new_origin.x,
            w.0.y - new_origin.y,
            w.0.z - new_origin.z,
        );
        let idx = self.pack(local);
        self.indirection[idx as usize] = slot.0;
    }

    #[cfg(debug_assertions)]
    self.audit_invariants();

    evicted
}
```

#### Edge cases the spec must address

| Scenario | Behaviour |
|---|---|
| `new_origin == old_origin` | Fast-path: return empty Vec, indirection untouched. Caller's typical "no shift this tick" loop incurs zero cost. |
| Shift by 1 segment in X (typical) | Evicts ~`window_size.y * window_size.z = 32` pairs (the X-edge column that just left the window). Remaining ~480 pairs preserved; indirection rebuilt to map each to its new local position (`local.x -= 1`). |
| Shift far enough that ALL slots evict (e.g. new_origin shifted by ≥ window_size in any axis) | Returns all 512 pairs. After (5), indirection is all `EMPTY_SLOT`. `world_to_slot` is empty; `slot_to_world` is all `None`. **Slots have NOT been returned to `free_list`** — caller must `free()` them OR re-`bind()` them to incoming admissions. |
| Camera Y unchanged + origin.y is always 0 (per `residency.rs:191`) | The Y axis never evicts. The dominant traffic is X/Z. The algorithm doesn't special-case this; the O(capacity) iteration covers it uniformly. |

#### Why `set_origin` returns the evicted pairs instead of auto-`free()`-ing them

The caller (Phase-2.5+ residency driver) wants to reuse evicted slots for
the new admissions in the same tick — this avoids a `free → allocate` round
trip and lets us deterministically pair an outgoing world-seg with an
incoming one. The pool primitives stay simple (single-slot allocate/free);
`set_origin` handles the bulk transition.

### D. Concrete GPU bind-group layout

#### Where the indirection buffer fits in the existing bind-group surface

The renderer's `chunks` access lives on `@group(0) @binding(0)` in
`world_data.wgsl:60` (`naadf_world_bind_group_layout` per `mod.rs:2405-2422`).
The bind-group layout has 8 bindings today (binding 0–7):

| Binding | Resource | File |
|---|---|---|
| 0 | `chunks: array<vec2<u32>>` | `world_data.wgsl:60` |
| 1 | `blocks: array<u32>` | `world_data.wgsl:64` |
| 2 | `voxels: array<u32>` | `world_data.wgsl:68` |
| 3 | `voxel_types: array<vec4<u32>>` | `world_data.wgsl:73` |
| 4 | `world_meta: GpuWorldMeta` (uniform) | `world_data.wgsl:76` |
| 5 | `entity_chunk_instances` | `world_data.wgsl:107` |
| 6 | `entity_voxel_data` | `world_data.wgsl:112` |
| 7 | `entity_instances_history` | `world_data.wgsl:?` |

**Decision (D-bind-1):** add the indirection buffer at `@group(0) @binding(8)`
on the renderer's `naadf_world_bind_group_layout`. The same layout already
binds `chunks/blocks/voxels/...`, so the renderer reads of `chunks` and
`indirection` share scope without an extra bind group.

WGSL declaration (added to `world_data.wgsl` after the entity bindings):

```wgsl
// streaming-world Phase 2.6 — window indirection table. `pack(local_xyz) →
// slot index` (or `EMPTY_SLOT = 0xFFFFFFFFu` for "no segment bound"). Size
// is fixed at `WORLD_SIZE_IN_SEGMENTS.x * y * z = 512` u32s. Bound on the
// renderer's @group(0) so ray_tracing.wgsl + bounds_calc.wgsl reads can
// translate window-local chunk coords → slot positions in chunks_buffer.
//
// For non-streaming presets the buffer is bound as a 1-element placeholder
// (identity mapping irrelevant — `streaming_active` uniform gates the
// translation; see § E).
@group(0) @binding(8) var<storage, read> window_indirection: array<u32>;
```

**Rationale for placing on `@group(0)`, not a new group:**
- The single bind group already gates every renderer pass; adding `@group(1)`
  would force every pass to bind+release a separate group every frame.
- The 2 KiB buffer fits in `STORAGE` with negligible overhead.
- The construction-side bind group `construction_world` at
  `mod.rs:2405-2422` mirrors `naadf_world_bind_group_layout`'s shape. Both
  need the same extension so `chunk_calc.wgsl` / `world_change.wgsl` /
  `bounds_calc.wgsl` can see the same indirection table at the same binding
  index. → 1 layout edit, 1 layout-mirror edit, 1 bind-group rebuild on
  each.

The `naadf_world_bind_group_layout` at `mod.rs:2405-2422` is a duplicate of
the canonical `NaadfPipelines::world_layout` (per the comment at 2384-2387).
Both must be extended; pipeline-cache layout equality is by entry-set so
the BindGroupLayout id stays consistent.

#### CPU-side `Buffer` allocation

```rust
// In `prepare_construction`, alongside the noise_terrain_params_buffer
// allocation at mod.rs:1755-1762.
if gpu.window_indirection_buffer.is_none() && streaming_active {
    let buf = render_device.create_buffer(&BufferDescriptor {
        label: Some("naadf_streaming_window_indirection"),
        size: (capacity as u64) * 4,   // 512 × 4 B = 2048 B.
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Initialise to EMPTY_SLOT — the first frame's extract will overwrite.
    let zero = vec![EMPTY_SLOT; capacity as usize];
    render_queue.write_buffer(&buf, 0, bytemuck::cast_slice(&zero));
    gpu.window_indirection_buffer = Some(buf);
    bind_groups.construction_world = None; // rebuild
    // The renderer's world bind group is owned by `prepare::WorldGpu`; the
    // existing rebuild trigger at `mod.rs:2438-2441` extends to include
    // this new binding (additional `as_entire_buffer_binding()` in the
    // `BindGroupEntries::sequential` list).
}
```

**Why a single fixed-size buffer (no `GrowableBuffer`):** capacity is a
compile-time constant (`WORLD_SIZE_IN_SEGMENTS = (16, 2, 16) → 512`); the
buffer never needs to resize. `GrowableBuffer<T>` is explicitly flagged
NOT applicable for the residency slab in `01-context.md` § "Borderline
calls".

For non-streaming presets, allocate a **1-element placeholder buffer**
(per the existing pattern `prepare_world_gpu` uses for the entity buffers
at `mod.rs:?`) so the bind-group layout is always satisfied; the `chunks`
read path gates on a uniform flag (§ E) and never reads `window_indirection`
for non-streaming presets.

#### Per-frame upload

Per-frame upload of `WindowedSlotMap::indirection_buffer()` via
`RenderQueue::write_buffer`. The upload runs in a **new render-app system**
in the `Render::Queue` stage (after `extract_streaming_state` populates
`StreamingExtractRender`, before the producer node runs). System name:
`upload_window_indirection`. Lives in `streaming/noise_dispatch.rs` for
proximity to the existing extract.

```rust
pub fn upload_window_indirection(
    gpu: Option<Res<ConstructionGpu>>,
    streaming_extract: Option<Res<StreamingExtractRender>>,
    render_queue: Res<RenderQueue>,
) {
    let (Some(gpu), Some(s)) = (gpu, streaming_extract) else { return; };
    if !s.streaming_mode_active { return; }
    let Some(buf) = gpu.window_indirection_buffer.as_ref() else { return; };
    render_queue.write_buffer(buf, 0, bytemuck::cast_slice(&s.window_indirection));
}
```

`StreamingExtractRender.window_indirection: Vec<u32>` is a new field
populated by `extract_streaming_state` as
`residency.window.indirection_buffer().to_vec()`. The clone is 2 KiB —
cheap.

Scheduling: register on the render app, with the existing extract pattern
matching `extract_streaming_state`. The upload system reads
`StreamingExtractRender` (already a Resource on the render world via
`init_resource` at `mod.rs:86-87` in `streaming/mod.rs`).

### E. Renderer-side indirection read — every shader site

#### Shader-strategy decision

**Decision (D-shader-1):** add a **`streaming_active: u32` flag** to
`GpuWorldMeta` (the existing `world_meta` uniform at
`world_data.wgsl:30-40`). Each shader site that reads `chunks` branches on
this flag: when `streaming_active != 0`, translate through the indirection
table; otherwise read direct (the existing path, byte-identical for
`Default` / `Vox` presets).

Alternative considered: separate shader compile of every chunks-reading
shader with `#ifdef STREAMING_ACTIVE`. Rejected — Bevy's `Shader::from_wgsl`
+ `naga-oil` compose mode does support shader-defs, but every shader
already runs through composable-module imports of `world_data.wgsl` which
makes the shader-def per-shader gating complex. A single runtime branch on
a uniform `u32` costs one ALU per chunk read (~50 chunk reads per ray in
practice = negligible) and is a one-line edit per shader.

`GpuWorldMeta` extension:

```rust
// crates/bevy_naadf/src/render/gpu_types.rs (existing file — extend)
#[repr(C)]
pub struct GpuWorldMeta {
    pub size_in_chunks: [u32; 3],
    pub streaming_active: u32,      // NEW — `1` for streaming preset, `0` otherwise.
    pub bounding_box_min: [f32; 3],
    pub _pad0: u32,
    pub bounding_box_max: [f32; 3],
    pub _pad1: u32,
}
```

Layout: `streaming_active` takes the previously-implicit `_pad0` slot at
offset 12 in `GpuWorldMeta`. Verify against actual current `GpuWorldMeta`
shape during impl — if different, fit `streaming_active` into the first
free 4-byte slot or extend the struct by 16 B padded.

WGSL mirror at `world_data.wgsl:30-40`:

```wgsl
struct GpuWorldMeta {
    size_in_chunks: vec3<u32>,
    streaming_active: u32,           // NEW
    bounding_box_min: vec3<f32>,
    bounding_box_max: vec3<f32>,
}
```

#### Shared helper function (one place, multiple callers)

Add to `world_data.wgsl`:

```wgsl
// Translate an absolute (or window-local — they're the same on the
// streaming preset because the camera Transform is pre-translated to
// window-local frame by pin_streaming_window_camera) chunk-coord into a
// chunks-buffer index. Returns `0xFFFFFFFFu` ("no chunk") when the local
// coord points at an empty slot, signalling "treat as sky".
//
// Non-streaming preset: pass-through to the flat layout.
fn streaming_chunk_index(chunk_pos: vec3<u32>) -> u32 {
    if (world_meta.streaming_active == 0u) {
        // Direct path (Default / Vox / ProceduralStatic): no indirection.
        return chunk_pos.x
             + chunk_pos.y * world_meta.size_in_chunks.x
             + chunk_pos.z * world_meta.size_in_chunks.x * world_meta.size_in_chunks.y;
    }
    // Streaming path:
    //   1. Translate to slot-local-segment via the indirection table.
    //   2. Compute the chunk index within that slot's segment.
    //   3. Combine: slot * CHUNKS_PER_SEGMENT + chunk_within_segment.
    let chunks_per_seg_x: u32 = 16u; // SEGMENT_CHUNKS
    let chunks_per_seg_y: u32 = 16u;
    let chunks_per_seg_z: u32 = 16u;
    let seg_local = vec3<u32>(
        chunk_pos.x / chunks_per_seg_x,
        chunk_pos.y / chunks_per_seg_y,
        chunk_pos.z / chunks_per_seg_z,
    );
    let chunk_in_seg = vec3<u32>(
        chunk_pos.x % chunks_per_seg_x,
        chunk_pos.y % chunks_per_seg_y,
        chunk_pos.z % chunks_per_seg_z,
    );
    // pack(local_xyz): row-major X-fastest with window_size = (16, 2, 16).
    let local_pack = seg_local.x + seg_local.y * 16u + seg_local.z * (16u * 2u);
    let slot = window_indirection[local_pack];
    if (slot == 0xFFFFFFFFu) {
        return 0xFFFFFFFFu; // empty
    }
    // chunks_per_segment_in_total = 16 * 16 * 16 = 4096.
    let chunks_per_seg_total: u32 = 4096u;
    let chunk_in_seg_idx = chunk_in_seg.x
        + chunk_in_seg.y * chunks_per_seg_x
        + chunk_in_seg.z * chunks_per_seg_x * chunks_per_seg_y;
    return slot * chunks_per_seg_total + chunk_in_seg_idx;
}

fn streaming_chunk_load(chunk_pos: vec3<u32>) -> vec2<u32> {
    let idx = streaming_chunk_index(chunk_pos);
    if (idx == 0xFFFFFFFFu) {
        return vec2<u32>(0u, 0u); // empty chunk
    }
    return chunks[idx];
}
```

The `0u` "empty chunk" return value matches the existing `BLOCK_STATE_UNIFORM_EMPTY = 0`
convention (chunk state `>> 30` returns `0`, which the renderer treats as
"sky-traversed", per `ray_tracing.wgsl:319` `if ((cur_node >> 31u) != 0u)`
falling through to "not mixed = empty").

#### Every read site (verified — Glob `**/*.wgsl` + grep `chunks\[`)

| File:line | Current expression | New expression | Notes |
|---|---|---|---|
| `assets/shaders/ray_tracing.wgsl:290-295` | `let chunk_idx = flatten_index(chunk_pos, ws.x, ws.x*ws.y); let chunk_texel = chunks[chunk_idx];` | `let chunk_texel = streaming_chunk_load(chunk_pos);` | Renderer DDA inner loop — load `.x` + `.y`. |
| `assets/shaders/bounds_calc.wgsl:212-215` | `let neighbour_idx = pos.x + pos.y*sx + pos.z*sx*sy; let neighbour_x = chunks[neighbour_idx].x;` | `let neighbour_x = streaming_chunk_load(neighbour_pos_u).x;` | W3 background bounds — neighbour lookup. **Streaming-active.** |
| `assets/shaders/bounds_calc.wgsl:365-368` | `let chunk_idx = chunk_pos_u.x + …; let cur_chunk_full = chunks[chunk_idx];` | (Read) `let cur_chunk_full = streaming_chunk_load(chunk_pos_u);` ; (Write) `chunks[streaming_chunk_index(chunk_pos_u)] = vec2<u32>(cur_chunk, entity_y);` | Read + write. Write site at line 407. Skip write when `streaming_chunk_index == 0xFFFFFFFFu` (chunk not resident — should never happen because the queue only enqueues groups whose chunks are resident; assert in debug). |
| `assets/shaders/world_change.wgsl:320-323` | `let chunk_idx = chunk_pos.x + …; let cur_chunk_load = chunks[chunk_idx];` | Read via `streaming_chunk_load`. Write via `streaming_chunk_index`. | W2 group-change edit. **Streaming preset DOES use W2 for evictions per `02b-design-plan-b.md` § F.** |
| `assets/shaders/world_change.wgsl:382` (write) | `chunks[chunk_idx] = vec2<u32>(new_chunk_x, cur_chunk_y);` | `let idx = streaming_chunk_index(chunk_pos); if (idx != 0xFFFFFFFFu) { chunks[idx] = vec2<u32>(new_chunk_x, cur_chunk_y); }` | W2 group-change write. |
| `assets/shaders/world_change.wgsl:450-454` | `let chunk_idx = chunk_pos.x + …; let cur = chunks[chunk_idx]; chunks[chunk_idx] = vec2<u32>(change.y, cur.y);` | Use `streaming_chunk_index`. | W2 chunk-change. |
| `assets/shaders/chunk_calc.wgsl:424-428` | `let chunk_idx = chunk_pos.x + …; chunks[chunk_idx] = vec2<u32>(state, 0u);` | `let idx = streaming_chunk_index(chunk_pos); if (idx != 0xFFFFFFFFu) { chunks[idx] = vec2<u32>(state, 0u); }` | chunk_calc producer write. Note: `chunk_pos = group_id + params.chunk_offset` at line 356; for the streaming preset, `chunk_offset` is set per-segment by the streaming dispatch loop at `mod.rs:2817-2821` — currently to `[lx * 16, ly * 16, lz * 16]` (the slot's window-local chunk position). With the new layout we instead set `chunk_offset` to the world-chunk coord (window-local) for which the segment is being generated — then the shader's `streaming_chunk_index(chunk_pos)` translates back through `window_indirection`. **See § F for the full layout shift.** |
| `assets/shaders/entity_update.wgsl:118-119` | `let old = chunks[chunk_idx]; chunks[chunk_idx] = vec2<u32>(old.x, update.y);` | Use `streaming_chunk_index`. **Entity preset does NOT overlap with streaming preset today** — gate this site behind `streaming_active == 0` no-op or assume the install paths are mutually exclusive (they are — see `voxel/grid.rs::setup_test_grid`). Phase 2.6 can either (a) leave `entity_update.wgsl` direct-only and assert the streaming preset never installs entities, OR (b) thread the indirection through anyway for forward-compat. Recommend (a) — current install paths are mutually exclusive (verified — `Residency` and `EntityHandler` resources are inserted by different `setup_test_grid` branches). |
| `assets/shaders/world_change.wgsl:454` | (See above) | (See above) | |

**Decision (D-shader-2):** modify `chunks_buffer` indexing in EVERY chunks-
touching shader EXCEPT `entity_update.wgsl`. The entity preset is mutually
exclusive with the streaming preset at install time, so the entity write
path can stay direct-indexed; if a future feature combines entities with
streaming, this is the one new shader-side change needed.

### F. Mid-segment slot layout — `slot * CHUNKS_PER_SEGMENT + offset` is a layout shift

#### What the current `chunks_buffer` is

Verified at `crates/bevy_naadf/src/render/prepare.rs:271-298`:
- Sized at `chunk_count = WORLD_SIZE_IN_CHUNKS.x * y * z = 256 * 32 * 256 = 2,097,152 chunks`.
- Layout is **flat absolute-world-chunk-coord** at row-major X-fastest:
  `idx = z * sx * sy + y * sx + x`.
- 8 B per chunk (`vec2<u32>`) = **16 MiB total**.

#### What the slot-indexed layout would be

`512 slots × 16³ chunks/slot = 512 × 4096 = 2,097,152 chunks` — **same total
count**, same buffer size (16 MiB), different grouping. Per-slot layout:

```
Slot 0:   chunks[0       .. 4096]   (16³ chunks for slot 0)
Slot 1:   chunks[4096    .. 8192]
...
Slot N:   chunks[N*4096  .. (N+1)*4096]
...
Slot 511: chunks[2092032 .. 2097152]
```

Within each slot, chunks are row-major X-fastest at 16-per-axis:
`chunk_in_seg_idx = cx + cy * 16 + cz * 16 * 16`.

#### Decision (D-layout)

**Adopt the slot-indexed layout for the streaming preset only.**

Reason: under the current absolute-coord layout, two world segments that
are not in the window AT THE SAME TIME (because they're on opposite sides
of an origin shift) would clash at the same `chunks_buffer` index. With
slot-indexed layout, each slot owns 4096 chunks for the duration of its
binding regardless of the binding's world-segment. Stale chunks linger in
freed slots (harmless — `unbind` sets `indirection[…] = EMPTY_SLOT`, which
short-circuits the read to "sky" before reaching the stale data).

For `Default` / `Vox` / `ProceduralStatic` presets, **keep the existing
flat absolute-coord layout** — the `streaming_active == 0` branch of
`streaming_chunk_index` returns the flat-coord index directly. No regression
on those presets.

#### Buffer-allocation impact

`chunks_buffer` size is the SAME (2,097,152 × 8 B = 16 MiB) under both
layouts. **No allocation change needed.** The streaming dispatch's
`chunk_offset` in `GpuConstructionParams` was previously the slot's
window-local chunk offset (`mod.rs:2817-2821`); under the new layout it
becomes **the slot-index expressed as a chunk-offset triple in slot-major
form**. Concretely:

```rust
// crates/bevy_naadf/src/render/construction/mod.rs:2817-2821 — replace this
let [lx, ly, lz] = crate::streaming::Residency::local_of(slot.0);
let group_offset_in_chunks = [
    lx * segment_chunks,
    ly * segment_chunks,
    lz * segment_chunks,
];
// — with this:
let slot_idx = slot.0;
let chunks_per_seg = segment_chunks;          // 16
let chunks_per_seg_total = chunks_per_seg.pow(3);  // 4096
// The chunk_calc shader writes at chunk_pos = group_id + chunk_offset, then
// flattens via streaming_chunk_index. We want chunk_idx in the chunks_buffer
// to equal `slot_idx * chunks_per_seg_total + (gx + gy*16 + gz*16*16)`.
//
// streaming_chunk_index(chunk_pos) = slot_idx * 4096 + chunk_in_seg_idx, so
// the easiest way to make that math line up is to set chunk_offset as the
// CHUNK COORD WITHIN THE SLOT'S WINDOW-LOCAL POSITION:
//   chunk_offset.x = local_x * 16, ...
// then `streaming_chunk_index(chunk_pos)` will hit `window_indirection[
// pack(local)]` = slot_idx, returning `slot_idx * 4096 + …` as desired.
// This is the SAME chunk_offset value as today — the math is unchanged.
// The semantic difference: now `streaming_chunk_index` rewrites the result
// via the indirection table, instead of the flat-coord path.
let [lx, ly, lz] = crate::streaming::Residency::local_of(slot_idx);
let group_offset_in_chunks = [
    lx * chunks_per_seg,
    ly * chunks_per_seg,
    lz * chunks_per_seg,
];
let _ = chunks_per_seg_total;
```

So the actual `chunk_offset` math STAYS the same. **What changes is the
indirection table content:** previously the residency driver's Pass 3
assigned arbitrary slots; now Phase 2.6 bins each admission to the slot
that the previously-allocated slot's local position would have mapped to.

**Effective change in Phase 2.6's `residency_driver`:**

```rust
// Pass 3 was: empty_slots.pop() for each pending world_seg, in arbitrary
// order. Phase 2.6 replaces this with:
for w in pending {
    let local = w.0 - residency.origin;
    // local is guaranteed in-window because target was constructed inside
    // the window.
    // Allocate ANY free slot from the pool — pool-side modulo (the user's
    // explicit directive).
    let Some(slot) = residency.window.allocate() else { break; };
    residency.window.bind(w, slot);
    // ... (existing slot_state / dispatch bookkeeping)
}
```

The `window.bind()` call writes `indirection[pack(local)] = slot.0` —
this is the load-bearing connector. The slot-to-world geometric mapping
the renderer needs is now ENFORCED by `WindowedSlotMap`'s invariants,
not by Pass-3 ordering.

#### Why not (b) keep absolute world-coord layout + indirection-via-modulo?

Alternative considered: keep the flat absolute-world-coord layout; on
window-shifts, instead of memcpy-ing buffer regions, change the camera's
absolute-world frame in the shader (the camera Transform is pre-translated
today, but we'd need to also translate the chunks-buffer index expression
by `+ origin_in_chunks_mod_world_size`). Rejected because:
- The streaming preset's renderer ALREADY pre-translates to window-local
  coords (per `pin_streaming_window_camera`); the absolute-coord layout
  would require UN-translating, which defeats the Q1 design rule
  ("renderer never sees world IVec3, only window-local").
- Modulo-arithmetic in the chunk-fetch is more ALU than a single
  indirection-table lookup, and modulo introduces wrap-around bugs the
  indirection table avoids (slots free'd from one corner cleanly become
  `EMPTY_SLOT` instead of mapping to the buffer position they would have
  wrapped around to).

### G. Migration plan — file by file

| Order | Action | Path | Approx LOC delta | Why |
|---|---|---|---:|---|
| 1 | new | `crates/bevy_naadf/src/streaming/windowed_slot_map.rs` | +320 (~250 impl + ~70 unit tests) | The data structure + audit_invariants + tests (§ H). |
| 2 | edit | `crates/bevy_naadf/src/streaming/mod.rs` | +5 / -1 | `pub mod windowed_slot_map;` + re-export `WindowedSlotMap, EMPTY_SLOT`. |
| 3 | edit | `crates/bevy_naadf/src/streaming/residency.rs` | +30 / -90 | Replace `slot_to_world: Vec<Option<…>>` + `world_to_slot: HashMap<…>` + `slot_state: Vec<SlotState>` fields with a single `window: WindowedSlotMap` field. **Drop `SlotState` entirely** — generating-vs-resident becomes implicit (`admissions_this_frame` contains slots that are generating; everything else bound is resident). `mark_admissions_resident` + `finalise_admissions_as_resident` no longer needed; delete (Phase 2.5's `Last`-stage system is gone). `process_pending_admissions` reads `window.iter_bound()` and filters by membership in `admissions_this_frame`. Update Pass 1 (evict via `set_origin` returning evicted pairs → `window.free()` them). Update Pass 2 (pending = target set minus `window.iter_bound().map(\|(w,_)\| w).collect::<HashSet<_>>`). Update Pass 3 (per-pending `allocate` + `bind`). |
| 4 | edit | `crates/bevy_naadf/src/streaming/mod.rs` | -5 | Remove `add_systems(Last, finalise_admissions_as_resident)`. Remove the corresponding re-export. |
| 5 | edit | `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | +15 | Add `window_indirection: Vec<u32>` field to `StreamingExtractRender` (populated from `residency.window.indirection_buffer().to_vec()`). Add `upload_window_indirection` system + register in `StreamingPlugin::build` on the render-app schedule. |
| 6 | edit | `crates/bevy_naadf/src/render/gpu_types.rs` | +1 / -1 | Add `streaming_active: u32` field to `GpuWorldMeta` (in the previous-`_pad0` slot). |
| 7 | edit | `crates/bevy_naadf/src/render/construction/mod.rs` | +60 | (a) `ConstructionGpu`: add `pub window_indirection_buffer: Option<Buffer>`. (b) `prepare_construction`: allocate the buffer + initial zero-write (only when `streaming_active`). (c) Extend `naadf_world_bind_group_layout` at `:2405-2422` with binding 8 (`storage_buffer_read_only_sized` for the indirection). (d) Extend the `BindGroupEntries::sequential((...))` at `:2427-2436` with `gpu.window_indirection_buffer.as_ref().unwrap().as_entire_buffer_binding()`. (e) Extend the canonical `world_layout` in `NaadfPipelines::from_world` similarly (find: `crates/bevy_naadf/src/render/pipelines.rs`). (f) When the streaming preset is NOT active, install a 1-element placeholder via `gpu.window_indirection_buffer.get_or_insert(...)` so the bind group is always satisfied. |
| 8 | edit | `crates/bevy_naadf/src/render/construction/mod.rs:2806-2904` | +5 / -5 | Remove the existing Pass-3 slot iteration logic. Adjust the admission loop's `group_offset_in_chunks` computation to match § F (no change to the values — the same `[lx*16, ly*16, lz*16]` math, just sourced via `WindowedSlotMap::local_of(slot)` indirectly through the existing `slot.0`). |
| 9 | edit | `crates/bevy_naadf/src/render/pipelines.rs` | +5 | Add binding 8 (storage read) to `NaadfPipelines::world_layout` to mirror the construction-side change. |
| 10 | edit | `crates/bevy_naadf/src/render/prepare.rs` | +20 | Extend `prepare_world_gpu` to allocate a 1-element placeholder for `window_indirection_buffer` when the streaming preset is OFF, and write `streaming_active` into `world_meta` (`= 0` for non-streaming, `= 1` for streaming — read from `streaming_extract`). |
| 11 | edit | `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` | +50 | Add `@binding(8) window_indirection` declaration. Add `streaming_active: u32` to `GpuWorldMeta`. Add `streaming_chunk_index` + `streaming_chunk_load` helpers. |
| 12 | edit | `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:290-295` | +1 / -5 | Replace direct `flatten_index` + `chunks[…]` with `streaming_chunk_load(chunk_pos)`. |
| 13 | edit | `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:212-215, 365-368, 407` | +6 / -8 | Replace direct chunks reads/writes with helper calls. |
| 14 | edit | `crates/bevy_naadf/src/assets/shaders/world_change.wgsl:320-323, 382, 450-454` | +8 / -10 | Replace direct chunks reads/writes with helper calls. |
| 15 | edit | `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:424-428` | +4 / -5 | Replace direct chunks write with helper (guarded by `idx != 0xFFFFFFFFu`). |
| 16 | unchanged | `crates/bevy_naadf/src/assets/shaders/entity_update.wgsl` | 0 | Entity preset and streaming preset mutually exclusive — leave direct-indexed. |
| 17 | unchanged | `crates/bevy_naadf/src/assets/shaders/noise_terrain.wgsl` | 0 | `noise_terrain` writes to `segment_voxel_buffer`, not chunks. Untouched (Phase-1/2 deliverable, ROAD-ONLY per brief). |
| 18 | unchanged | `crates/bevy_naadf/src/e2e/streaming_window.rs` | 0 | Gate's thresholds and walk distance stay the same. The test was already designed to FAIL today and PASS after Phase 2.6 lands. |

Estimated impl LOC total: **+400 / -125 ≈ net +275 LOC** across 18 edits.

#### Phase 2.6 is mostly mechanical

The single thing that requires careful sequencing is the world-bind-group
layout extension (steps 7, 9, 10). The pipeline cache requires the
construction-side `world_layout` and the renderer-side `world_layout` to
have IDENTICAL entry sets (matched by descriptor equality); both must be
updated in the same commit or the pipelines won't validate.

## Unit test plan

Every test lives in `crates/bevy_naadf/src/streaming/windowed_slot_map.rs`
under `#[cfg(test)] mod tests` (matches the `residency.rs` pattern). The
impl agent ships these alongside the data structure.

| # | Test name | Assertion (one sentence) |
|---|---|---|
| T1 | `new_empty_state` | After `new(UVec3::new(16,2,16))`: `capacity() == 512`, `free_count() == 512`, `origin() == IVec3::ZERO`, all `indirection_buffer()` entries are `EMPTY_SLOT`. |
| T2 | `allocate_returns_slots_in_order_starting_from_zero` | First 5 calls to `allocate()` return `SlotIndex(0)..SlotIndex(4)` in order. |
| T3 | `allocate_returns_none_when_pool_empty` | After allocating all 512 slots, `allocate()` returns `None`. |
| T4 | `allocate_free_round_trips` | Allocate 100, then free 100 — `free_count() == 512` again, no invariant violations. |
| T5 | `bind_updates_indirection` | After `bind(WorldSegmentPos(IVec3::new(3,1,5)), slot=0)`, `indirection_buffer()[pack(IVec3::new(3,1,5))] == 0`. |
| T6 | `bind_round_trip_via_lookup` | `bind(w, s)` → `lookup_slot(w) == Some(s)` && `lookup_world(s) == Some(w)`. |
| T7 | `unbind_clears_indirection` | After `bind` then `unbind(w)`, `indirection_buffer()[pack(local_of(w))] == EMPTY_SLOT`. |
| T8 | `unbind_returns_slot_for_caller_disposition` | `unbind(w)` returns the slot it had; calling `free(returned_slot)` restores `free_count()` by 1. |
| T9 | `set_origin_no_shift_returns_empty_vec` | `set_origin(map.origin())` returns `Vec::new()`, doesn't modify the indirection buffer (bit-exact compare before/after). |
| T10 | `set_origin_full_evict_returns_all_pairs` | Bind 5 segments, then shift origin by `IVec3::new(WORLD_SIZE_IN_SEGMENTS.x as i32, 0, 0)` (full X-window) — returns all 5 pairs in some order. After the shift, `indirection` is all `EMPTY_SLOT` and `world_to_slot` is empty. |
| T11 | `set_origin_partial_shift_preserves_in_window` | Bind a 16-wide row in X at (0..16, 0, 0); shift origin to `IVec3::new(1, 0, 0)`. Exactly 1 pair (the one at `x=0`) is returned as evicted. The remaining 15 bindings are at `local.x ∈ [0, 15)` (was `[1, 16)`). |
| T12 | `set_origin_rebuilds_indirection_correctly` | After T11's shift, for each remaining bound (w, slot): `indirection[pack(local_of(w))] == slot.0`. (Direct invariant I7 check on a known fixture.) |
| T13 | `bind_panics_on_out_of_window` *(debug)* | In a `cfg(debug_assertions)` test, `bind(WorldSegmentPos(IVec3::new(100, 0, 0)), slot)` panics. |
| T14 | `bind_panics_on_double_bind_world` *(debug)* | Double-binding the same `world_seg` to a different slot panics. |
| T15 | `bind_panics_on_double_bind_slot` *(debug)* | Double-binding the same `slot` to a different `world_seg` panics. |
| T16 | `free_panics_on_bound_slot` *(debug)* | `free(slot)` on a slot still mapped panics ("free slots have no mapping"). |
| T17 | `indirection_buffer_length_equals_capacity` | `indirection_buffer().len() == capacity() == 512`. |
| T18 | `audit_invariants_after_random_mutations` | Pseudo-random sequence: 200 ops of (bind / unbind / set_origin / allocate / free) on the same map; `audit_invariants()` passes after every op. Uses a fixed RNG seed for reproducibility. |
| T19 | `pack_round_trip_x_fastest` | For every `(lx, ly, lz)` in `[0,16) × [0,2) × [0,16)`: round-trip via `pack` agrees with `Residency::slot_index_of([lx, ly, lz])` (the existing slot-index formula at `residency.rs:128-132`). |
| T20 | `set_origin_idempotent_under_re_derivation` | Calling `set_origin(new_origin)` twice with the same value: the second call returns empty and `indirection_buffer()` is bit-identical to after the first call. |

Each test is < 30 LOC. Total test LOC: ~600 — but unit test code is cheap.

## Decisions & rejected alternatives

### D1 — Pool-side modulo vs explicit indirection table

- **Chose:** explicit `indirection: Vec<u32>` (length 512), uploaded to GPU
  as a storage buffer.
- **Rejected:** modulo-arithmetic in the shader (`chunks[(local + origin_mod_window) mod window_size]`).
- **Why:** (1) The user's explicit directive ("make a concrete data
  structure for this instead of adhocking it on every step of the way")
  points at an explicit indirection layer rather than implicit
  modulo math. (2) The shader-side modulo would require the camera frame
  to revert to absolute-world coords, contradicting Q1 ("renderer never
  sees world IVec3, only window-local") in `01-context.md`. (3) The
  indirection table makes the "EMPTY_SLOT → sky" path trivial; modulo
  wrap-around would have to be detected via a separate residency-bitmask
  binding, ending up at the same buffer count. (4) An indirection table
  is the textbook hardware-friendly form for sliding-window residency
  (cf. virtual memory's page table) — easy to reason about, easy to
  debug.
- **Fact that would flip this back:** if the per-frame 2 KiB upload
  measured to be a bottleneck (it won't — that's `chunks_buffer * 0.012%`,
  one `write_buffer` call), or if a hardware constraint demanded a single
  ALU shader path with no extra binding (none today).

### D2 — Shader strategy: runtime branch on `streaming_active` uniform vs separate shader compiles

- **Chose:** single shader, runtime branch on `world_meta.streaming_active`
  via the `streaming_chunk_index` helper.
- **Rejected:** separate shader compiles per-preset using shader-defs.
- **Why:** (a) one extra ALU per chunk read; on the hot DDA path that's ~50
  chunk reads per ray = 50 extra ALUs, dwarfed by the noise sample cost
  elsewhere. (b) Bevy's naga-oil composable-module imports make per-shader
  shader-def gating fiddly (the `world_data.wgsl` module is imported by
  multiple entry-shaders; each would need the same shader-def to compile
  consistently). (c) The branch is uniform across an entire frame
  (uniform-flow branch in WGSL — wave-coherent), no actual divergence.
- **Fact that would flip:** if profiling shows the branch costs measurably
  on integrated GPUs.

### D3 — `chunks_buffer` layout: slot-indexed for streaming preset only

- **Chose:** slot-indexed layout
  (`chunks_buffer[slot * 4096 + chunk_in_seg]`) for streaming preset;
  flat-absolute-coord layout (current) for everything else.
- **Rejected (a):** uniformly switch the entire codebase to slot-indexed
  (would touch the W5 `.vox` preset's expected geometric layout — out of
  scope, would require re-verifying `validate_gpu_construction` byte-equal
  oracle).
- **Rejected (b):** keep flat-absolute-coord + memcpy ranges of
  `chunks_buffer` on each origin shift (per-shift cost proportional to
  `window_chunks ≈ 16 × 2 × 16 × 16³ = 2M chunks × 8 B = 16 MiB` of GPU
  memcpy per shift — order of magnitude more expensive than the 2 KiB
  indirection upload).
- **Why:** the slot-indexed layout is the cleanest fit for the
  indirection-table model; it preserves Q1 ("renderer never sees world
  IVec3"); it keeps the `Default` / `Vox` / `ProceduralStatic` presets
  byte-identical (the runtime branch via `streaming_active == 0` returns
  the flat index unchanged).
- **Fact that would flip:** if a future feature combines streaming with
  `ModelData` (e.g. streaming a `.vox` world larger than VRAM), the
  streaming-active branch's slot indexing would need to extend to
  `generator_model.wgsl`. That's a small addition (one helper call) but
  out of Phase 2.6 scope.

### D4 — Drop `SlotState` enum entirely

- **Chose:** lifecycle is now implicit:
  - In `free_list` ⟺ empty.
  - Bound and present in `admissions_this_frame` ⟺ Generating.
  - Bound and NOT in `admissions_this_frame` ⟺ Resident.
- **Rejected:** keep `SlotState` enum on `WindowedSlotMap` (or as a
  parallel `Vec<SlotState>` on `Residency`).
- **Why:** the new abstraction's invariants make `SlotState` redundant.
  The `admissions_this_frame` Vec on `Residency` already names the
  generating-vs-resident split. Phase 2.5's `mark_admissions_resident` /
  `finalise_admissions_as_resident` were workarounds for the missing
  `Generating → Resident` transition; under the new model, "marking
  resident" is just "the next frame's `admissions_this_frame` doesn't
  contain this slot" — implicit, no system needed.
- **Cost of removal:** Phase 2.5's `slot_admissions_eventually_drain_to_resident`
  unit test in `residency.rs:594-671` is re-cast against the new model
  (the test's load-bearing assertion — "Generating count strictly
  decreases each tick" — becomes "the count of slots both bound AND in
  admissions_this_frame strictly decreases each tick"). Add this assertion
  to the migrated residency tests.
- **Fact that would flip:** if Phase 3 needs a richer per-slot state
  machine (e.g., "queued for GPU dispatch but params not yet uploaded";
  "dispatched but waiting on bounds chain refresh"), reintroduce
  `SlotState` as a parallel `Vec` on `Residency`. WindowedSlotMap doesn't
  need to know about it.

### D5 — `set_origin` returns evicted pairs (doesn't auto-`free` them)

- **Chose:** return `Vec<(WorldSegmentPos, SlotIndex)>` of evicted pairs;
  caller decides `free()` vs immediate re-`bind()`.
- **Rejected:** auto-push evicted slots into `free_list`.
- **Why:** the typical residency-driver flow on an origin shift is "evict
  N out-of-window segments, admit N new in-window segments." Auto-
  freeing forces a `free → allocate` round trip per pair. With the
  caller controlling disposition, the driver can do
  `for pair in evicted.into_iter().chain(/* freshly-allocated */) {
  window.bind(new_w, pair.1); }` — preserving slot identity across the
  shift where possible (the slot's GPU content is then overwritten by
  the next noise dispatch, no need to evict-then-allocate).
- **Fact that would flip:** if the caller always `free()`s anyway,
  auto-`free` saves boilerplate. Phase 2.6's `residency_driver` reuses
  slots aggressively (admission count ≈ eviction count on typical shift),
  so the manual path is the better fit.

### D6 — Where the indirection-buffer upload runs (render-app `Render::Queue` stage)

- **Chose:** new render-app system `upload_window_indirection` in the
  render-app schedule, between `ExtractSchedule` (which populates
  `StreamingExtractRender`) and `naadf_gpu_producer_node`.
- **Rejected:** upload from inside `prepare_construction` (the buffer-
  allocation site).
- **Why:** `prepare_construction` is a single huge function already
  flagged at `#[allow(clippy::too_many_arguments)]`; adding a per-frame
  write_buffer call there bloats it further and confuses the build-once-
  + reuse pattern that dominates that function. A dedicated upload system
  is the same pattern Phase 2 already uses for `noise_terrain_params_buffer`
  (rewritten per-segment in the producer node).
- **Fact that would flip:** if a future refactoring consolidates render-
  world prepare systems into per-feature modules, the upload may move
  into a `streaming/prepare.rs` module. Same code, different home.

### D7 — Single bind group (extend `naadf_world_bind_group_layout`)

- **Chose:** extend the existing renderer + construction bind groups with
  binding 8.
- **Rejected:** introduce a new `@group(2)` for the indirection table.
- **Why:** (a) reuse of existing layout machinery; (b) avoids a per-pass
  bind cost; (c) the construction-side bind group has the same shape, so
  one layout extension covers both sides.
- **Fact that would flip:** if WebGL2 backends impose a max-bindings-per-
  group limit we exceed (unlikely — WebGL2 caps are 16 storage bindings
  per group, we're at 8 + 1 = 9).

## Assumptions made

1. **`naadf_world_bind_group_layout` and `NaadfPipelines::world_layout` are
   structurally identical** (both define the entry set for the renderer's
   `@group(0)`). The brief surfaces this at `mod.rs:2384-2387` (a comment
   says the layout is "rebuilt inline because BindGroupLayoutDescriptor
   equality is by entry-set"). Step 7 of the migration plan extends BOTH —
   if a third copy lives anywhere else (e.g. an e2e-test-only layout), it
   must be extended too. This is a discovery for the impl agent.

2. **`GpuWorldMeta` has a free 4 B slot at offset 12 (the existing `_pad0`
   per the WGSL comment at `world_data.wgsl:29` `size_in_chunks (0..16)`)**.
   Verified from the WGSL — `size_in_chunks: vec3<u32>` consumes 12 B,
   then 4 B std140 padding to vec3 alignment (16 B). Promoting that pad
   to a real `streaming_active: u32` field is layout-neutral; no buffer
   size change. The Rust mirror at `gpu_types.rs` must agree — verify
   alignment when the impl agent touches that file.

3. **The streaming preset and the entity preset are mutually exclusive at
   install time.** Verified by reading `voxel/grid.rs::setup_test_grid`'s
   match-on-`GridPreset` — `ProceduralStreaming` and the entity-enabled
   presets are different arms; no install path inserts both. If a future
   preset combines them, `entity_update.wgsl` needs the same indirection
   thread as the other shaders.

4. **`chunks_buffer`'s total size in u32-pairs stays unchanged** (= 16 MiB
   under either layout). Verified arithmetic: `512 slots × 16³ chunks/slot
   = 256 × 32 × 256 chunks = 2,097,152 = WORLD_SIZE_IN_CHUNKS.x*y*z`. The
   buffer allocation in `prepare.rs:271-298` doesn't change.

5. **Phase 2.5's `slot_state` enum + the `Last`-stage system can be
   dropped** without a compatibility shim. Verified by Glob: the only
   external readers of `SlotState` are inside `streaming::residency` (the
   driver) and one e2e diagnostic message in `streaming_window.rs:292-303`
   (the wall-clock-budget panic prints a histogram of {Generating,
   Resident, Empty}). The diagnostic message will need a small refactor:
   instead of histogramming `slot_state`, derive the three counts from
   `(window.iter_bound().count(), admissions_this_frame.len(), window.free_count())`.

6. **The renderer's `chunks` read in `ray_tracing.wgsl:295` is the ONLY
   place a SAMPLED-load happens during DDA.** Verified by `grep chunks[`
   across the shaders directory (§ E table). All 9 sites are listed and
   accounted for.

7. **The streaming preset is the ONLY preset whose camera Transform is
   pre-translated to window-local.** Verified at
   `streaming_window.rs:251-258` — `translate_world_to_window_local`
   early-returns when there's no `Residency`. For `Default`, `Vox`,
   `ProceduralStatic`, the renderer reads chunks at absolute-world-chunk
   coords; the `streaming_active == 0` branch keeps that behaviour bit-
   identical.

8. **The bind-group layout change does not exceed wgpu's per-group binding
   cap on any target backend.** Adding binding 8 → 9 total bindings on
   group 0. wgpu's default `Limits::max_bindings_per_bind_group = 65535` —
   we're nowhere near.

## Open questions for the user (if any)

**None at the architectural level.** Every architectural question raised
in the brief is resolved by Assumption (1)–(8) above with verifiable
codebase pointers. The impl agent should validate Assumption 1 (a second
`world_layout` copy may exist in `render/pipelines.rs` — confirm during
impl) and Assumption 2 (`GpuWorldMeta` layout — confirm `_pad0` is at the
expected offset and is uninitialised, not a hidden semantic field) but
neither would block the design; they're impl-discovery items, not
architectural decisions awaiting user input.

A minor scoping question for the orchestrator (NOT a blocker):

- **Q (scope):** should Phase 2.6 raise `STREAMING_MIN_PIXEL_DELTA` /
  `STREAMING_MIN_AFTER_LUM_VARIANCE` thresholds after the fix lands (per
  the rule in `03e` § "Item 2 chosen ... re-tuned to `measured * 0.4`")?
  - **Architect recommendation:** YES — once the gate passes, measure
    Δ and variance and tighten. But this is impl-phase scope (after the
    fix lands), not design-phase. The design itself doesn't change
    thresholds.
