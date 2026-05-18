# 03n — Diagnosis: AADF building corruption on streaming admissions

Read-only investigation, post-Phase 2.10 (`03m-impl-bounds-and-w3.md`).
Working tree: `feat/streaming-world` (HEAD = post-Phase 2.10).

User-visible symptom: rays follow `chunks_buffer[idx]` 5-bit chunk-level AADF
skip-distance hints, skip over space that contains real voxels, and emerge on
the far side without rendering them. Visible as flat axis-aligned voids and
floating slabs that should be solid (screenshots 4/5/6/7).

## TL;DR

**Root cause:** the W3 regime-1 seed (`add_initial_groups_to_bound_queue`)
fires ONCE, over the FULL world bound-group count (32 768 groups covering all
256×32×256 chunks), AT THE INSTANT the first ~4 admissions have landed —
while the other ~508 segments are still un-admitted (their `chunks_buffer`
slot regions are zero-initialised but their `window_indirection` entries are
already bound from Pass 3 of `residency_driver`). The W3 regime-2 background
loop then iteratively grows chunk-level 5-bit AADFs *over those un-admitted
zero-chunks*, treating them as `BLOCK_STATE_UNIFORM_EMPTY` (because
`(0u32 >> 30u) == 0u` = `UNIFORM_EMPTY`). The chain BAKES IN a long skip
distance through real-future-terrain segments. The W3 chain re-enqueues
a group only as bound size grows (0→30) and stops once mask bits are set —
**no re-expansion fires when those segments later receive their real noise +
chunk_calc + per-segment bounds dispatches.** The 5-bit AADF stays large and
lies. Rays follow the lie and skip past terrain.

`chunk_calc.wgsl::compute_voxel_bounds` and `compute_block_bounds` (the W1
2-bit voxel + 2-bit block AADFs) are **CORRECT** — they operate on `cached_cell`
(workgroup-shared, scope = one 4³ block), see Task A below. The Phase 2.10
per-segment dispatch via `bounds_chunk_index_offset` is sound for THOSE passes.
The bug is solely in the W3 5-bit chunk-level AADF path.

## What the screenshots show

Reading images 6, 7, 4, 5 in turn:

- **Image 6** (`/home/midori/.claude/image-cache/e4081ed2-be75-401d-a246-bb2dcded1571/6.png`):
  terrain at top of frame, then a sharp axis-aligned vertical cliff DROP, then
  more terrain visible below. The cliff is not voxel geometry — it is the
  silhouette of the W3 skip cuboid. The ray, while traversing the upper-terrain
  region, reads `chunks[idx]` for an empty-above-terrain chunk; the 5-bit AADF
  in the (-Y, +X, ...) direction tells it to skip 8-15 chunks ahead. The skip
  carries the ray PAST real solid voxels that should have shadowed it.
- **Image 7**: classic stair-step axis-aligned voids — every chunk-sized step
  is exactly the AADF skip granularity (`16 voxels = 1 chunk`). The ray
  follows decreasing-AADF chunks until it exits the area, then steps to the
  next chunk where the AADF is also lying but a different magnitude → stair
  pattern.
- **Image 4**: dense terrain (snow-noise pattern) sits on top of HUGE solid
  white slabs. The slabs are voxels that DO exist (the noise generated them);
  they are visible at their tops because rays from above hit them. But rays
  trying to cross the slab from the side skip past them entirely (AADF lies).
  The slabs are real; the void to their +X / -X / -Z sides is a fiction.
- **Image 5**: similar dual-layer: rough top terrain plus axis-aligned cliff
  faces. The cliff-face SURFACE is correctly hit when rays approach from
  outside, confirming the voxel data is correct in the chunks the ray reaches
  via a non-lying AADF chain.

**The voxels ARE in `voxels_buffer` / `blocks_buffer`** (visible at frame
edges, visible from below, visible everywhere a ray didn't follow a lying
AADF). The fault is in the `chunks_buffer[idx].x` 5-bit AADF skip-distance
fields, populated by the W3 chain at
`bounds_calc.wgsl::compute_group_bounds:233-284`.

## Static vs streaming AADF dispatch — side-by-side

| Aspect | Static preset (a0b) — KNOWN-GOOD | Streaming preset (a0) — BROKEN |
|---|---|---|
| **Per-segment noise + chunk_calc** | 512 segments × 1 pass each, one-shot at startup (`mod.rs:2853-2949`) | 4 segments/frame × 128 admission frames = 512 segments cold-start, then 4 segments per boundary-crossing thereafter (`mod.rs:3055-3232`) |
| **W1 voxel + block bounds** (2-bit AADFs in `voxels[]` + `blocks[]`) | ONE dispatch, full-world workgroup extent, ON THE RENDER-CONTEXT ENCODER, after the per-segment loop (`mod.rs:2955-2976`) | Per-admission inline on the per-segment encoder, scoped via `bounds_chunk_index_offset = slot.0 × 4096` (`mod.rs:3217-3230`) |
| **W1 cross-chunk reads?** | NONE — `compute_voxel_bounds` / `compute_block_bounds` read only from `cached_cell` (workgroup-local 64-element array; scope = ONE 4³ block of voxels OR ONE 4³ block of blocks within one chunk). Confirmed at `chunk_calc.wgsl:182-274` (the helpers `add_bounds_voxels_or_blocks` and `compute_bounds_4`). | Same — workgroup-local; no cross-chunk hazards regardless of dispatch shape. |
| **W3 regime-1 seed** (`add_initial_groups_to_bound_queue`) | Suppressed (`bounds_initialized` gets set inside the `(a0b)` branch directly; the post-loop full-world W1 bounds chain already populated 2-bit AADFs, and the static preset never fires the W3 seed) — see Phase 2.4 § "Skip the bounds-init seed for both streaming and static modes" (`03d-impl-static-noise.md:18`, item 3). The static preset has NO W3 chunk-level 5-bit AADF AT ALL. | ONE-SHOT, FIRES ONCE, full-world `bound_group_count = 32768` (`mod.rs:1942-1948`). Fires the first frame `bounds_initialized = true` (i.e., first frame after first 4 admissions have landed). |
| **W3 regime-2 background loop** (`naadf_bounds_compute_node`) | Gates on `bounds_initialized` (true after the W1 bounds chain); regime-2 finds every queue empty (the seed was never dispatched) and is a no-op every frame. The chunk-level 5-bit AADFs at `chunks[idx].x[0..30)` stay ZERO forever — but rays still work because empty/full chunks are correctly classified by W1 (state bits at `[30..32)`). | Same gate, but the seed DID populate the queues with 32 768 groups × 3 axes. Regime-2 expands `n_bounds_rounds = 1` round per frame, growing chunk-level 5-bit AADFs across the world. |
| **Outcome** | No chunk-level 5-bit AADF, BUT ray traversal still works because the chunks the ray encounters are correctly classified UNIFORM_FULL / UNIFORM_EMPTY / CHILD by the W1 chain. Rays step at chunk granularity (16-voxel-skips, the `+ offset` term in `ray_tracing.wgsl:367-376`) through empty space — slower, but correct. | W3 chain populates 5-bit AADFs based on a partially-admitted world snapshot (yet-to-be-admitted segments have zero chunks_buffer bytes → state UNIFORM_EMPTY → expansion thinks they are empty space → AADF skip distances grow long). Once a chunk has been processed by the W3 chain at every bound size (0..30), its mask bits are fully set and it never re-enters the queue (`bounds_calc.wgsl:474-493`). **Subsequent admissions of the formerly-zero segments do not invalidate or re-expand neighbour chunks' AADFs.** Rays follow the long, lying skip distances and miss real terrain. |

**The architectural delta**: the static preset doesn't use the W3 chunk-level
AADF chain at all (per 2.4's "Skip the bounds-init seed for both streaming
and static modes" decision — `03d-impl-static-noise.md:18`), so the static
preset never has a bake-in-stale-AADF window. The streaming preset's Phase
2.10 RESTORED the W3 seed (to recover distant-terrain visibility, per
`03l` item 3), but the W3 chain assumes the entire world is populated and
consistent at the moment of seeding — which is NOT TRUE during the streaming
cold-start.

## Bounds shader neighbour-read analysis (Task A)

Three bounds passes participate in AADF construction. Per-pass neighbour-read
audit:

### W1.A — `chunk_calc.wgsl::compute_voxel_bounds` (lines 497-552)

- Workgroup shape: `@workgroup_size(64, 1, 1)` — 64 threads per workgroup.
- `block_index = bounds_chunk_index_offset + group_id.x + group_id.y * nx + group_id.z * nx * ny`.
- Each thread reads ONE voxel via `voxels[voxel_index / 2u]`, decodes upper-or-
  lower-half u16, sets `cached_cell[local_index] = cur_voxel`.
- Calls `compute_bounds_4(local_index, voxel_pos_in_block, ...)`. The helper
  reads only from `cached_cell[local_index ± {1,4,16}]` — these are all
  workgroup-local indices in `[0, 64)`. No reads of `voxels[]` past
  the initial decode.
- WRITE: `voxels[voxel_index / 2u] = lo | (hi << 16u)` — strictly within the
  segment's voxel range (because `block_index ∈ [bounds_chunk_index_offset
  × 64, (bounds_chunk_index_offset + 4096) × 64)`).

**Cross-segment neighbour reads: NONE.** Per-segment scoping is sound for
this pass. Voxel-level 2-bit AADFs are correct.

### W1.B — `chunk_calc.wgsl::compute_block_bounds` (lines 559-596)

- Workgroup shape: `@workgroup_size(64, 1, 1)` — 64 threads per workgroup
  process one chunk's 64 blocks.
- `chunk_index = bounds_chunk_index_offset + group_id.x + group_id.y * nx + group_id.z * nx * ny`.
- Each thread reads ONE block via `blocks[chunk_index * 64 + local_index]`,
  sets `cached_cell[local_index] = cur_block`.
- Calls `compute_bounds_4(local_index, block_pos_in_chunk, 30u, 0x3u, ...)`.
  Reads `cached_cell[local_index ± {1,4,16}]` only.
- WRITE: `blocks[chunk_index * 64 + local_index] = cached_cell[local_index]`.

**Cross-segment neighbour reads: NONE.** Block-level 2-bit AADFs are correct.

### W3 — `bounds_calc.wgsl::compute_group_bounds` (lines 372-494)

- Workgroup shape: `@workgroup_size(4, 4, 4)` — 64 threads per workgroup
  process one 4³-chunk "bound group" (64 chunks).
- Decodes `gp` from the queue: a (4³-chunk) group position in world chunk-grid
  coords.
- `chunk_pos = (gp.x * 4 + local_id.x, gp.y * 4 + local_id.y, gp.z * 4 + local_id.z)`.
  This is a world-chunk-grid coord in `[0, 256) × [0, 32) × [0, 256)` on the
  streaming preset (window-local because the world IS the window — 16×2×16
  segments fill the full `WORLD_SIZE_IN_SEGMENTS`).
- READ: `cur_chunk = streaming_chunk_load_bc(chunk_pos_u).x` (`bounds_calc.wgsl:422-423`).
- If `chunk_state == UNIFORM_EMPTY`, calls `add_bounds_group(chunk_pos,
  ±dir_abs, ...)` for each `-direction` and `+direction` on the queue's axis
  (`bounds_calc.wgsl:442-448`).
- `add_bounds_group` (lines 233-284) does:
  - `neighbour_chunk_pos = chunk_pos + direction_offset` — coord in `[-1, 256]`
    range.
  - Out-of-world-bounds neighbours: permissive expansion (line 253-262 — bumps
    the bound when the chunk is at the queue's current bound size on that
    side).
  - In-bounds neighbours: `neighbour_x = streaming_chunk_load_bc(neighbour_pos_u).x`
    (line 269) — **this is a cross-chunk read indexed by world-chunk-grid
    coord through the window indirection table**.
  - `if (state != UNIFORM_EMPTY) { return cur_chunk; }` — only expands through
    empty neighbours.
  - Mask check + grow bound by 1.

**Cross-chunk neighbour reads: YES, at world-chunk-grid resolution, through
the streaming indirection table.**

The neighbour coord is in `[0, 256)` (after the out-of-bounds gate), so it
indexes ALL chunks in the world. The indirection table has every slot bound
(Pass 3 of `residency_driver` runs before any dispatch). For a yet-to-be-
admitted segment, `streaming_chunk_load_bc` returns `chunks[slot * 4096 + ...]`
= `vec2<u32>(0u, 0u)` (zero-init from `prepare_world_gpu`).

Decoded: `state = 0 >> 30 = 0 = BLOCK_STATE_UNIFORM_EMPTY`. The check at line
276 of `bounds_calc.wgsl` (`if (state != BLOCK_STATE_UNIFORM_EMPTY) { return; }`)
**lets the expansion proceed**. The 5-bit AADF grows under the false premise
that the neighbour chunk is real empty space.

## Hypothesis verdicts

### H_neighbor_read — bounds shaders read out-of-segment neighbours when scoped → incorrect AADF — **CONFIRMED, but only for the W3 chunk-level pass.**

- W1 voxel + block bounds: workgroup-local reads only — no cross-segment hazard.
- W3 compute_group_bounds: reads neighbour chunks across the FULL world chunk grid
  via the indirection table (`bounds_calc.wgsl:269`). When neighbours are
  yet-to-be-admitted segments, the read returns 0 → state UNIFORM_EMPTY →
  expansion proceeds → AADF lies.

### H_offset_apply — `bounds_chunk_index_offset` applied to writes but not to neighbour reads — **N/A (refuted as the proximate cause)**.

- The W1 passes that use `bounds_chunk_index_offset` do NOT have neighbour
  reads — `cached_cell` is workgroup-local. The offset is applied identically
  to read + write because the read at line 518 uses `voxel_index = block_index * 64 + local_index`
  (block_index already includes the offset), and the write at line 550 uses the
  same `voxel_index / 2u`. Same for `compute_block_bounds`: read + write both
  use `block_index = chunk_index * 64 + local_index` with the offset baked in.
- The W3 pass does NOT use `bounds_chunk_index_offset` at all (see
  `bounds_calc.wgsl:75-86` — the field is documented as carried in the struct
  for layout-equality only; W3 entry points do not read it).
- The mechanism `bounds_chunk_index_offset` exists is sound for the W1 passes;
  it just doesn't address the W3 cross-chunk hazard.

### H_w3_seed — W3 regime-1 seed runs over inconsistent data and propagates corruption — **CONFIRMED, this is the load-bearing root cause.**

`mod.rs:1917-1958`:

```rust
let want_w3_seed_streaming = streaming_active
    && gpu.bounds_initialized
    && !gpu.streaming_w3_seed_dispatched;
...
bounds_calc::dispatch_add_initial_groups(
    &mut encoder,
    initial_pipeline,
    world_bg,
    bounds_bg,
    bound_group_count,  // = 32 768 — full world bound groups
);
```

`bounds_initialized` flips `true` the first frame **any** admission's bounds
chain runs (`mod.rs:3258-3260`: "After segments_dispatched > 0 && !gpu.bounds_initialized
=> set bounds_initialized = true"). At that instant, only 4 of 512 segments
have real data in `chunks_buffer`. The other 508 slots are zero-init from
`prepare_world_gpu` (`prepare.rs:295-305`).

The seed dispatches `add_initial_groups_to_bound_queue` over the FULL
`bound_group_count` (32 768 groups, covering every chunk in the world).
Every group's mask bit is set for bound_size = 0 on all 3 axes (X/Y/Z),
and packed group positions are written into the 3 size-0 queues.

The W3 regime-2 loop (`naadf_bounds_compute_node`, `bounds_calc.rs:316-390`)
then runs once per frame, popping `max_group_bound_dispatch` groups per
round and growing them by 1 bound-size per round. Within ~16 frames after
the seed, many groups in the un-admitted region have already grown to
bound_size = 4-8 (skip distance 64-128 voxels). After ~30 frames any given
group has been processed at every bound size from 0 to 30 and its mask is
fully set; it will never be re-expanded.

**The streaming cold-start drains over ~128 frames at 4 admissions/frame.**
By the time most segments are admitted with their real noise/chunk_calc/W1
bounds data, the W3 chunk-level AADFs that were computed against
yet-to-be-admitted neighbours have ALREADY BEEN BAKED IN — and the AADFs
say "skip ahead, it's all empty" while the truth is now solid voxels.

Steady-state regression: every segment-boundary crossing evicts 32 segments
and admits 32 more (over 8 frames). The freshly-admitted segments' chunks
are written with correct state bits + correct W1 AADFs. BUT the W3 chunk-
level 5-bit AADFs of neighbouring chunks were computed against the
PREVIOUSLY-evicted segment's data and stay stale.

### H_chunk_calc_offset — `chunk_calc.wgsl` per-segment offset is wrong on reads — **REFUTED.**

`chunk_calc.wgsl::calc_block_from_raw_data` (lines 389-478):
- Reads `segment_voxel_buffer[voxel_index_in_segment + i]` — the per-segment
  noise pass output; correct addressing.
- Writes `chunks[chunk_idx] = vec2<u32>(state, 0u)` where `chunk_idx =
  streaming_chunk_index_cc(chunk_pos)`; `chunk_pos = group_id + params.chunk_offset`.
  The `chunk_offset` is set per-admission by `mod.rs:3162` (`group_offset_in_chunks
  = [local_x * 16, local_y * 16, local_z * 16]`); the indirection translation
  emits `slot.0 * 4096 + chunk_in_seg_idx` (the slot's contiguous range).
- Writes `blocks[base + local_index]` for mixed chunks — `base` is from
  `atomicAdd(&block_voxel_count[1], 64)`, a global cursor; correct.
- Writes `voxels[voxel_u32_start + i]` — `voxel_u32_start` from
  `atomicAdd(&block_voxel_count[0], 64u) >> 1u`, global cursor; correct.

No cross-chunk neighbour reads in `chunk_calc.calc_block_from_raw_data`.

## Root cause

`crates/bevy_naadf/src/render/construction/mod.rs:1917-1958` —
**The W3 regime-1 seed
(`bounds_calc.wgsl::add_initial_groups_to_bound_queue`) on streaming fires
ONCE over the FULL world bound-group extent (32 768 groups) the first frame
ANY admission's bounds chain has landed, but at that frame 508 of 512
segments are still zero-initialised. The W3 regime-2 background loop then
expands chunk-level 5-bit AADFs through those zero chunks (which decode as
`BLOCK_STATE_UNIFORM_EMPTY`), producing long skip distances through
yet-to-be-admitted segments. Subsequent admissions populating those
segments with real noise + chunk_calc + W1 bounds do NOT invalidate or
re-expand the stale W3 AADFs (the chunk's mask bits are set; the chain
short-circuits at `bounds_calc.wgsl:480-481`). Rays following the lying
AADFs skip past real terrain — the visible voids in screenshots 4/5/6/7.**

Citations:
- W3 seed dispatch: `mod.rs:1942-1948`.
- W3 seed gate (one-shot, streaming-only): `mod.rs:1917-1922`.
- `bound_group_count` = full world: `mod.rs:1924`, `bounds_calc.rs:400-408`.
- `bounds_initialized = true` on first admission: `mod.rs:3258-3260`.
- `chunks_buffer` zero-init at allocation: `prepare.rs:285-305`.
- Indirection bound for ALL pending on first frame: `residency.rs:364-382`
  (Pass 3, `window.bind` for all `pending`); admissions throttled to 4/frame
  in Pass 4 (`residency.rs:386, 418-447`).
- W3 expansion treats zero-chunk neighbour as empty:
  `bounds_calc.wgsl:269-282`.
- W3 chain stops re-enqueueing once mask bits are set:
  `bounds_calc.wgsl:474-493`.

## Punch-list for the fix dispatch

Ordered, most likely to least.

1. **(MUST) Seed-then-defer W3 on streaming until ALL admissions land.**
   Delay the `streaming_w3_seed_dispatched` flip until the cold-start
   admission burst has fully drained (all 512 segments admitted at least
   once). Use a counter or a `residency.is_cold_start_complete()` query.
   Cost: 5-15 LOC in `mod.rs:1917-1958`. **This is necessary but NOT
   sufficient** — steady-state boundary crossings still produce stale W3
   AADFs in evicted regions.
   - LOC estimate: ~20 (counter + gate + plumbing).

2. **(MUST) Per-admission W3 re-seed for the freshly-admitted segment AND
   its neighbours.** When a segment is admitted, identify the chunk groups
   that overlap or border the segment, re-seed THOSE groups at bound_size = 0
   (clear their mask bits, re-enqueue in size-0 X/Y/Z queues, optionally
   zero their existing 5-bit AADFs in `chunks_buffer[idx].x`). The W3 chain
   will re-expand only those groups, picking up the now-correct
   neighbour state.
   - Needs a new dispatch entry point in `bounds_calc.wgsl` (or extend
     `add_initial_groups_to_bound_queue` with per-segment scope via a new
     uniform `seed_chunk_range_min/max`).
   - Needs streaming-side bookkeeping: list of chunk-groups to re-seed
     after admission/eviction; passed via `StreamingExtractRender`.
   - LOC estimate: 80-140 (shader entry point + Rust dispatch + extract + bookkeeping).

3. **(MUST) Zero out evicted-slot chunk AADF bits before that slot's
   `chunks_buffer` region is rewritten by the next admission.** Even with
   the per-admission re-seed above, the OLD (pre-eviction) AADF bits on
   the evicted slot persist between eviction and re-population if the W3
   re-seed runs LATER than the W1 per-segment bounds. Easier: at the start
   of the per-segment admission encoder (`mod.rs:3186`), prepend a
   `ClearBuffer` or a tiny compute clear of the slot's `chunks_buffer`
   range. Cost: ~15 LOC.
   - LOC estimate: ~15.

4. **(SHOULD) Add an e2e gate that catches this regression.** See "What
   test would catch this" below — a byte-for-byte comparison of
   `chunks_buffer` between static and streaming presets at matching camera
   poses, OR an e2e visual diff of the rendered framebuffer.
   - LOC estimate: 50-100 (gate scaffolding).

5. **(SHOULD) Revisit the EMPTY_SLOT semantic in `streaming_chunk_load_bc`.**
   Currently it returns `vec2(0u, 0u)` on EMPTY_SLOT → state = UNIFORM_EMPTY.
   If the W3 chain encounters EMPTY_SLOT during the bounds expansion (which
   shouldn't happen since all 512 slots are bound, but a robustness
   fix), returning `vec2((BLOCK_STATE_CHILD << 30u), 0u)` instead would
   force the expansion to stop (treats EMPTY_SLOT as mixed → not eligible
   for expansion through). Or hard-code a STOP sentinel.
   - LOC estimate: ~5.

6. **(OPTIONAL) Reconsider whether streaming needs W3 at all.** The static
   preset works correctly WITHOUT W3 chunk-level 5-bit AADFs — rays step at
   chunk granularity (16-voxel) through empty space. The MAX_RAY_STEPS_PRIMARY
   cap (240 on streaming per Phase 2.10 item 2) allows 240 × 16 = 3840 voxels
   of empty traversal, comfortably reaching any in-window terrain. **If the
   per-segment W1 bounds chain is correct (which Task A confirms it is), the
   W3 chain may be entirely optional** — disabling it would revert
   `mod.rs:1917-1922` to never fire on streaming.
   - LOC estimate: ~5 to disable the gate; nets ~600 LOC of W3 plumbing if
     fully removed, but a 5-LOC disable is the right size for this fix.
   - Caveat: the user diagnosed "blocks far-away appear briefly for one
     frame and disappear" in `03l`, attributing it to stale chunk-level
     AADF. If reverting W3 on streaming brings back that flicker, item 6
     is not viable. A measurement is needed to disambiguate.

**Strongly recommended fix shape**: combination of (1) + (2) + (3) — keep
the W3 chain on streaming, but make it segment-aware. OR fix (6) first if
the static-preset evidence holds that W3 isn't load-bearing for distant-
terrain reach.

**Total LOC estimate for items (1)+(2)+(3)+(4): ~165-275 LOC** plus a new
shader entry point in `bounds_calc.wgsl`. This exceeds the typical
small-fix budget; a fresh-eyes review of items (2) + (6) before
implementation would save throwaway work.

## What test would catch this

The `--validate-gpu-construction` gate compares the GPU-built `chunks_buffer`
byte-for-byte against a CPU oracle for a small world (4×2×4 chunks). It
PASSES at 388 bytes (`03m` line 233) — proving the W1 path is byte-correct.
It does NOT exercise the streaming/W3 path because the test world is too small
(`bound_group_count = 0` per `bounds_calc.rs:400-408` for 4×2×4).

**Three test gates would catch this regression:**

1. **Streaming-vs-static `chunks_buffer` byte-diff at matching camera poses.**
   Install both presets in turn at the same camera pose (`(2048, 288, 2048)`
   looking +X — both presets use this same install pose). Let cold-start
   complete (~128 frames on streaming, 1 frame on static). Read back
   `chunks_buffer`. For every chunk that is RESIDENT in both presets, assert
   the byte representation matches (the W3-AADF bits will differ if W3 ran
   on streaming with stale neighbours — the static preset's chunk AADF bits
   stay zero, so the streaming preset's chunk AADF bits must also stay zero
   for byte-equality, or item 6's "disable W3 on streaming" must be in
   effect).
2. **Streaming-vs-static framebuffer diff.** Install each preset, let
   cold-start complete, capture a framebuffer at the matching camera pose,
   compute SSIM or per-pixel diff. A perfect AADF would produce
   bit-identical framebuffers; tolerance for floating-point + DDA stepping
   is required. Currently the screenshot evidence suggests the diff would
   be **dramatic** — entire visible regions vary by hundreds of luminance
   units.
3. **Mid-cold-start AADF consistency check.** During the cold-start, after
   N=64 admission frames (half of 128), snapshot `chunks_buffer`. For every
   chunk c in a resident segment with neighbours in YET-TO-BE-ADMITTED
   segments, assert the chunk-level 5-bit AADF in the direction of the
   un-admitted neighbour is ≤ 0 (i.e., no skip). Catches the live propagation
   of the bug at the moment it happens.

Either (1) or (2) is sufficient as a CI gate. (3) is the diagnostic-grade
catcher for development.

## Hard one-off observation

Captured from the brief's Task E with `--grid-preset procedural-streaming
--vram-budget-mib 1024`:

```
streaming-world: ProceduralStreaming preset installed — noise_preset=0,
  seed=1337, sea_level=256.0, terrain_amplitude=64.0, vram_budget_mib=1024,
  max_segments_per_frame=4; camera spawn at Vec3(2048.0, 288.0, 2048.0)
  looking at Vec3(2148.0, 240.0, 2048.0)
[...]
residency shift: cam_seg=IVec3(8, 1, 8), new_origin=IVec3(0, 0, 0),
  evictions=0, bound_segments=512, admissions_this_frame=4
[at frame 1: 512 segments are bound in indirection, 4 are dispatching]
[...]
residency shift: cam_seg=IVec3(7, 1, 8), new_origin=IVec3(-1, 0, 0),
  evictions=32, bound_segments=512, admissions_this_frame=4
streaming-world Phase 2.10: W3 regime-1 seed dispatched (one-shot — chunk-
  level 5-bit AADF queue now active).
[W3 seed fires AT THE INSTANT THE FIRST BOUNDARY CROSSING completes — long
 before cold-start has fully drained; the user moved the camera 1 segment
 before all 512 of the original 512-segment cold-start admissions had run]
```

**Confirms**: the W3 seed fires on `bounds_initialized = true` — which is the
FIRST admission frame's W1 bounds finish, not the LAST. At that moment most
of the world is zero-chunks. The cold-start does drain over the next ~127
frames but the W3 seed has already snapshotted the partially-admitted world
state into its bound-group queues.

Additionally `bound_segments=512` on EVERY shift line confirms the indirection
table is fully bound from frame 1 onward — never EMPTY_SLOT. The W3 chain's
`streaming_chunk_load_bc` calls therefore always hit a non-EMPTY indirection
entry → always return `chunks[slot * 4096 + ...]` → return zero for un-admitted
slots → state UNIFORM_EMPTY → expansion proceeds incorrectly.
