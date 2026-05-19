# 03r — Cold-start inner-chunks gap diagnostic

[work in progress as of 2026-05-19]

## TL;DR

Cold-start admission FAILS for the FIRST `4 × K` slots picked by
`process_pending_admissions`, where K is the number of frames it takes
the producer node's `noise_terrain_pipeline` to finish ASYNCHRONOUS
WGSL compilation through `PipelineCache::get_compute_pipeline`. Empirically
K is "several frames" (WGSL pipeline-cache compile of two combined
shaders, ~3-6 frames on first run). The lost slots are the
camera-nearest segments (sorted by `dsq` first), producing the visible
rectangular gap around the spawn point in Image 8.

**Root cause** (HIGH confidence): `process_pending_admissions`
(`residency.rs:486-515`) does TWO unconditional things every time it
picks a candidate:

```rust
residency.admissions_this_frame.push((world, slot));
residency.dispatched_once.insert(slot);      // <-- premature
```

It marks the slot as Resident (`dispatched_once`) BEFORE the
render-world producer node has done any GPU work. The producer node
(`render/construction/mod.rs:2839-3392`) has 17 distinct early-return
paths between its entry and the per-segment encoder. Specifically the
first few frames after install_procedural_streaming_world will
early-return at one of:

- `let Some(p_noise_id) = gpu.noise_terrain_pipeline` (mod.rs:3136) —
  Frame 0: `noise_terrain_pipeline` is `None` (queued by
  `prepare_construction` only AFTER the extract has mirrored
  `noise_terrain_shader` from the main-world `StreamingShaderHandle`).
- `let Some(p_noise) = pipeline_cache.get_compute_pipeline(p_noise_id)`
  (mod.rs:3139) — WGSL pipeline compile is asynchronous; returns `None`
  while compiling. The combined noise_terrain shader is ~1500 LOC of
  WGSL (FastNoiseLite + terrain entry); compile takes multiple frames
  on a cold pipeline cache.
- `let (Some(p_calc), Some(p_voxel), Some(p_block)) = (...)` (lines
  2903-2912) — same async-pipeline situation for the chunk_calc +
  voxel_bounds + block_bounds pipelines.
- `let Some(world_gpu) = world_gpu.as_deref()` (mod.rs:3114) — the
  Phase 2.12 § Surprise #3 1-3 frame race for `WorldGpu`.

For EACH frame the producer early-returns:
- Main-world `process_pending_admissions` ran and inserted 4 slots into
  `dispatched_once` (residency.rs:513).
- Render-world producer did NOTHING.
- Phase 2.12's `clear_streaming_bound_slots` may have cleared those
  slots' chunks regions to zero (UNIFORM_EMPTY = sky).

After the producer pipelines finally compile, the residency driver's
`process_pending_admissions` filters by
`!dispatched_once.contains(slot)` — and skips the slots from prior
frames. The slots stay sky-coloured, indirection-pointed-at,
permanently — until evicted.

**The user's "slide and return" mitigation works** because
`set_origin` evicts segments outside the new window; eviction calls
`dispatched_once.remove(slot)` (residency.rs:381) and frees the slot.
On the return trip the segment is re-allocated to a (possibly
different) slot, fresh in `dispatched_once`, and gets picked again by
Pass 4 — this time on a frame where all pipelines are compiled. The
dispatch fires and the terrain appears.

**Punch-list summary**: defer `dispatched_once.insert(slot)` until the
render world ACKs the dispatch (cross-world accumulator, same pattern as
Phase 2.12's `PENDING_CLEAR_ON_BIND_SLOTS`). Alternatively: gate
admission picking on producer-ready state (all required pipelines
compiled + WorldGpu present), so cold-start doesn't "burn" admission
slots into a queue the GPU never processes.

**Pre-sort verdict**: NO — sort order is correct
(camera-distance-squared, ascending). The bug is downstream of sorting.
Any pre-sort scheme picks the same 4-32 camera-nearest slots; they all
get burned by the same race. Pre-sorting cannot save you because the
issue is "slot marked Resident before dispatch completes," not "wrong
slots picked first."

## Image-8 segment identification (Task A)

Image 8 (read via Read tool):
- A clear rectangular trough/pit in voxel terrain, sky-blue (navy
  background colour) showing through.
- The pit is oriented along one horizontal axis (appears to run
  left-to-right, with a notch extension at far-right end).
- Width ≈ 1 segment thick (~256 voxels at typical FOV).
- Length ≈ 3-4 segments long along the elongation axis.
- Surrounded by white voxel terrain (snow/sand-coloured).
- Slightly visible sky in upper area too, suggesting overall pose looks
  forward-and-down at terrain.

**Camera spawn pose** (`voxel/grid.rs:211-222`):
- `cam_pos = (2048, sea_level + 32, 2048) = (2048, 288, 2048)`.
- `cam_look = (cx + 100, sea_level - 16, cz) = (2148, 240, 2048)`.
- Looking forward in **+X direction**, slightly down.

**Camera segment** = `cam_pos / 256 = (8, 1, 8)`.

**Window origin** (after first `target_origin_for_camera_seg`) =
`(8 - 8, 0, 8 - 8) = (0, 0, 0)`. So `window_size = (16, 2, 16)` covers
world-segments `[0, 16) × [0, 2) × [0, 16)`.

**The 4 inner-ring segments closest to camera** (dsq sorting at
`residency.rs:419-422` and `:509`):
- `(8, 1, 8)` — dsq = 0 — camera's own segment.
- `(7, 1, 8)` — dsq = 1.
- `(9, 1, 8)` — dsq = 1.
- `(8, 1, 7)` — dsq = 1.
- `(8, 1, 9)` — dsq = 1.
- `(8, 0, 8)` — dsq = 1.

(6 candidates tie at dsq = 1; 4 get picked in HashMap iter order via
`iter_bound`, since `residency.window.iter_bound()` walks
`world_to_slot: HashMap` whose iteration order is non-deterministic.)

**The 8-16 next-ring segments**: dsq = 2 ring:
- `(7, 0, 8), (9, 0, 8), (8, 0, 7), (8, 0, 9)`,
- `(7, 1, 7), (7, 1, 9), (9, 1, 7), (9, 1, 9)`. 8 segments at dsq=2.

**Visible terrain at gap edge** in Image 8 looks like terrain a few
segments away (full white voxels). The gap is **roughly the size of a
small handful of segments** — consistent with the camera-near segments
being the ones that failed to dispatch.

The gap's "rectangular along one axis" shape is consistent with the
+X look direction: failed segments near the camera in +X form an
elongated visible hole because the camera's frustum cuts along +X
through the missing region.

## Cold-start admission lifecycle (Task B)

Walk-through of Frame 0 for an inner-ring segment (e.g. `(7, 1, 8)`):

### Frame 0 (PreUpdate)

1. `install_procedural_streaming_world` (`voxel/grid.rs:145-244`) ran
   during `Startup`. Inserted `Residency::empty(4)` —
   `WindowedSlotMap::new` allocates 512 free slots (free_list contains
   `SlotIndex(511) .. SlotIndex(0)` so `pop()` returns `SlotIndex(0)`
   first), `world_to_slot` empty, `indirection` all `EMPTY_SLOT`.

2. `residency_driver` (`PreUpdate`, `residency.rs:318-465`) fires.

3. `last_camera_seg == None` → `do_shift = true`.

4. `target_origin_for_camera_seg(IVec3(8, 1, 8)) = IVec3(0, 0, 0)`.

5. Pass 1 (`set_origin(IVec3(0, 0, 0))`): origin was `IVec3(0,0,0)` already
   (`WindowedSlotMap::new` sets `origin = IVec3::ZERO`). **Fast-path
   no-op** (`windowed_slot_map.rs:262-268`) — returns empty Vec, no
   evictions.

   Wait — actually `set_origin` checks `if new_origin == self.origin`
   — and `IVec3::ZERO == IVec3::ZERO`. So **no work**.

   `evictions_this_frame` stays empty. **CONFIRMED — Pass 1: no
   evictions on cold-start.**

6. Pass 2 (target set computation, `residency.rs:392-422`):
   `resident = HashSet::from(window.iter_bound())` = empty (nothing
   bound yet). `pending` collects ALL 512 world-segments
   `(lx, ly, lz) ∈ [0,16) × [0,2) × [0,16)` since none are resident.
   `pending.sort_by_key` by dsq vs camera seg `(8, 1, 8)`.

7. Pass 3 (allocate + bind, `residency.rs:431-450`): for each of 512
   pending in distance order, `window.allocate()` returns next free
   slot (pool LIFO order: slot 0, slot 1, ..., slot 511), `window.bind(w,
   slot)` writes indirection. **ALL 512 slots bound in Frame 0.**

   `clear_on_bind_queue` gets all 512 slot indices.

8. Pass 4 (`process_pending_admissions`, `residency.rs:486-515`):
   - cap = 4.
   - `cam_seg = last_camera_seg.unwrap_or(IVec3::ZERO)` — at this point
     in the function we have already set `last_camera_seg = Some(cam_seg_world.0)`
     at line 370 BEFORE the call (line 454). So `cam_seg = IVec3(8, 1, 8)`. **Good.**
   - `candidates = window.iter_bound().filter(!dispatched_once.contains)
     .map(|(w, slot)| (slot, w, dsq))`. All 512 pass the filter
     (dispatched_once is empty). Order from `iter_bound()` is HashMap
     iter order — **non-deterministic**.
   - `candidates.sort_by_key(|c| c.2)` — sorted by dsq ascending.
   - First 4: the camera-closest segments (dsq = 0 then dsq = 1 × 3 from
     the 5 dsq=1 candidates).
   - For each: push to `admissions_this_frame`, INSERT slot into
     `dispatched_once`.

### Frame 0 (ExtractSchedule)

9. `extract_streaming_state` (`noise_dispatch.rs:376-517`) reads main
   world, atomically takes `clear_on_bind_queue` and pushes to
   `PENDING_CLEAR_ON_BIND_SLOTS` (512 slot indices). Clones
   `admissions_this_frame` (4 entries) + indirection + window_origin
   + `is_cold_start_complete()` (returns false:
   `dispatched_once.len() = 4 != 512`).

### Frame 0 (Render::Queue)

10. `upload_window_indirection` writes the indirection buffer to GPU.
11. `clear_streaming_bound_slots` (`noise_dispatch.rs:595-645`): **gates
    on `world_gpu` being available**. If `WorldGpu` is allocated by
    `prepare_world_gpu` at this point, drain pending and clear 512
    slot regions. If NOT yet allocated, early-return (line 601-608);
    the pending slot ids stay in `PENDING_CLEAR_ON_BIND_SLOTS` for the
    next frame.

### Frame 0 (Core3d::PostProcess)

12. `naadf_gpu_producer_node` (`render/construction/mod.rs:3109-3392`):
    - Top of streaming branch (line 3114): `let Some(world_gpu) =
      world_gpu.as_deref() else { return; };` — **early-return if
      WorldGpu not yet allocated**.
    - If WorldGpu IS allocated: iterate `admissions_this_frame` (4
      entries). For each, build params, build per-segment encoder,
      `seg_encoder.clear_buffer(chunks_buffer, slot_offset, size)`,
      dispatch noise_terrain + chunk_calc + voxel_bounds + block_bounds,
      `render_queue.submit([seg_encoder.finish()])`. 4 segments
      dispatched.

### Frame 0 — KEY OBSERVATION

**The producer node has MULTIPLE early-return gates** that fire while
GPU resources are still asynchronously initializing. From
`render/construction/mod.rs:2874-3152`:

1. `construction_config` not yet there (`:2874`).
2. `construction_pipelines` not yet there (`:2895`).
3. `construction_bind_groups` not yet there (`:2896`).
4. `world_bg` (`construction_world` bind group) not yet built (`:2900`).
5. **`p_calc` / `p_voxel` / `p_block` pipeline compilation pending**
   (`:2903-2912`) — `PipelineCache::get_compute_pipeline()` returns
   `None` while WGSL compile runs. Async; multi-frame.
6. **`world_gpu`** not yet present (`:3114`) — Phase 2.12 § Surprise #3.
7. `streaming_extract` resource not yet present (`:3133`) — first
   `ExtractSchedule` race.
8. **`gpu.noise_terrain_pipeline`** not yet queued (`:3136`) — only
   queued by `prepare_construction:1772-1785` AFTER
   `streaming_extract.noise_terrain_shader` is mirrored.
9. **`p_noise` pipeline compile pending** (`:3139`) — async; the
   combined-source `noise_terrain` shader is large (FastNoiseLite +
   the noise_terrain entry, ~1500 LOC of WGSL); compile is slow on
   first cache.
10. `construction_noise_terrain` bind group not yet built (`:3142`).
11. `noise_terrain_params_buffer` / `bounds_params_buffer` not yet
    allocated by `prepare_construction` (`:3147` / `:3150`).

The early-returns are SILENT — no log, no defer-queue mechanism. Every
frame the early-return fires:

- Main-world `process_pending_admissions` ALREADY inserted up to 4 slot
  ids into `dispatched_once` BEFORE the render-world had a chance to
  run.
- Render-world producer node early-returned — no GPU work done.
- Phase 2.12's `clear_streaming_bound_slots` MAY have run (it has its
  OWN early-return for missing `WorldGpu` — but its cross-world
  accumulator means even if it can't run yet, it WILL run later).

**Cumulative effect**: `(K frames of race) × (4 admissions per frame) =
4 × K slots burned`. With K ≈ 3-6 frames for first-time pipeline
compile, that's 12-24 slots. Image 8's gap looks larger than 4
segments — consistent with multi-frame race.

**The slots are marked "dispatched" without the dispatch having fired**,
and the filter at `residency.rs:502` excludes them from future re-pick.

`clear_on_bind_queue` is more careful: it uses a cross-world
accumulator that survives the race. But `dispatched_once` is
main-world-only and gated by main-world residency_driver logic alone.

### Frame 1, 2, ... (Pre-shift, do_shift=FALSE)

- `residency_driver`: `last_camera_seg == Some(IVec3(8,1,8))` and
  `cam_seg_world.0 == IVec3(8,1,8)` → `do_shift = FALSE`.
- Branch at `residency.rs:362-367`: call
  `process_pending_admissions` and return. NO Pass 1-3 work.
- `process_pending_admissions` picks next 4 candidates,
  filtered by `!dispatched_once.contains(slot)`. The 4 slots from
  Frame 0 are EXCLUDED.
- These 4 new candidates ARE dispatched if WorldGpu is now allocated.

### Outcome

- Frame 0's 4 "missed" slots stay marked `dispatched_once` forever
  (until eviction).
- Their `chunks_buffer` region was cleared by `clear_streaming_bound_slots`
  (Phase 2.12 cross-world accumulator survives), so renderer reads
  UNIFORM_EMPTY = sky.
- 508 OTHER slots get correctly admitted over the next 127 frames.
- The user sees a rectangular gap at the camera-closest segments
  (which are the FIRST 4 picked by Pass 4 sort).

Result: **the gap is exactly the 4 segments nearest to camera at
`(8, 1, 8)` — the same dsq=0/dsq=1 ring identified in Task A.**

This matches Image 8's geometry: the gap is the camera-adjacent region.

## Re-admission lifecycle (Task C)

User says "slid view far to one side and then returned to origin —
that gap filled completely."

Slide far = camera crosses many segment boundaries, e.g. ends at
camera_seg `(12, 1, 8)`. Sequence:

- Each segment-crossing frame: `do_shift = TRUE`. Pass 1 evicts segments
  outside new window (e.g. one stripe of low-X segments). For each
  evicted `(_w, slot)`: `dispatched_once.remove(slot)` at
  `residency.rs:381`. The slot returns to the free pool.

- Camera returns to origin: `(8, 1, 8)` again. Window shifts back —
  same eviction-and-readmission dance. Each time the previously-broken
  slots get evicted (or rather: their world_segment, which was bound
  but never dispatched, now gets unbound), and on the return shift the
  SAME segment is re-allocated to a (possibly different) slot,
  `dispatched_once` is fresh, and Pass 4 picks them again. THIS time
  `WorldGpu` is long-since allocated, the dispatch fires, and the
  terrain appears.

The decisive lifecycle difference between cold-start and re-admission:

| Step | Cold-start (Frame 0) | Re-admission (Frame N >> 3) |
|---|---|---|
| `world_gpu` allocated? | NO (1-3 frame race) | YES |
| Producer early-returns? | YES | NO |
| `dispatched_once.insert()` fires? | YES (before GPU) | YES |
| GPU dispatch fires? | NO | YES |
| Final state | sky-coloured slot, marked Resident | populated slot, marked Resident |

## Root cause

**`process_pending_admissions` inserts slot indices into
`dispatched_once` (`residency.rs:513`) BEFORE the render-world producer
node has confirmed it actually ran the dispatch.**

The producer node has 11+ early-return paths that can fire while async
GPU init (`prepare_world_gpu`, WGSL pipeline compile, bind-group
build) is in flight. For each early-return frame:

- Main world: 4 candidates pushed to `admissions_this_frame`, 4 slot
  ids inserted into `dispatched_once`.
- Render world: producer early-returns silently. Zero GPU work.
- Main world next frame: filter at `:502` excludes those 4 slots from
  re-pick. They stay sky-coloured forever (until evicted).

The slots end up:

- Marked Resident in main world (`dispatched_once` membership).
- Cleared to UNIFORM_EMPTY in render world (Phase 2.12 clear-on-bind
  cross-world accumulator) — this part DOES survive race (sticky
  accumulator).
- Never dispatched (no chunk_calc → no terrain content).
- Excluded from the re-pick filter at `residency.rs:502`.

This is symmetric with the Phase 2.12 design correction for
`clear_on_bind_queue`: that field was originally per-frame but was made
sticky to survive the Frame-0 race. `dispatched_once` needs the same
treatment — either:

1. Don't insert until the render world ACKs the dispatch (round-trip
   through an extract resource / cross-world accumulator).
2. Mark the slot as dispatched only conditionally on a "all-ready"
   predicate (all pipelines compiled + WorldGpu present + bind groups
   built) checked from main world — fragile because main world can't
   inspect render-world resources directly.
3. Use a sticky pending list: `admissions_this_frame` becomes
   `admissions_pending`; only remove an entry from `admissions_pending`
   after the producer node ACKs it (same accumulator pattern as
   `PENDING_CLEAR_ON_BIND_SLOTS`).

### Cited file:line

- `crates/bevy_naadf/src/streaming/residency.rs:486-515`
  (`process_pending_admissions`).
- `crates/bevy_naadf/src/streaming/residency.rs:502` (filter:
  `.filter(|(_, slot)| !residency.dispatched_once.contains(slot))`).
- `crates/bevy_naadf/src/streaming/residency.rs:513`
  (`residency.dispatched_once.insert(slot);` — the premature insertion).
- `crates/bevy_naadf/src/render/construction/mod.rs:3114-3116` (the
  early-return that misses 4 admissions).
- Phase 2.12 § Surprise #3 (`03q-impl-phase-2-12.md` lines 269-270):
  "`prepare_world_gpu` is asynchronous build-once (takes 1-3 frames).
  Required cross-world static accumulator `PENDING_CLEAR_ON_BIND_SLOTS`
  to survive the Frame-0 race."

### Confidence

HIGH. The mechanism is mechanically derivable from the cited lines.
The user's reproduction (cold-start gap fills after evict + re-admit)
matches step-by-step.

### Why this wasn't caught before

- The Phase 2.12 fix focused on `clear_on_bind_queue` (Frame-0 race for
  the clear queue). The author CORRECTLY identified that "WorldGpu is
  asynchronous build-once" and CORRECTLY designed the cross-world
  accumulator pattern — but applied it only to clear-on-bind, not to
  the `dispatched_once`/admissions race.
- The `streaming-framebuffer-diff` gate (Phase 2.12 MUST-3) uses a
  384-frame warmup which is plenty of time for the missed 4 segments
  to NOT be re-tried (filter excludes them). The relaxed SSIM threshold
  (0.05) tolerates "some sky pixels visible" — the gate passes despite
  the gap.
- The `streaming-window` gate captures `before/mid_walk/after`
  framebuffers but its mid-walk centre-ratio threshold (0.30) and
  variance assertion don't specifically detect "a uniform-sky hole in
  the camera-nearest segments" at cold-start. The pre-walk
  `streaming_window_before` framebuffer at the SAME pose could
  visually show this gap (`03p` § "streaming_window_before.png" notes
  "smooth beige plateau" — that may actually be terrain BEHIND the gap
  rather than the gap itself, depending on view).

## User hypothesis verdicts

### "Pool slot missing issue"

REFUTED in literal form. `WindowedSlotMap::new` initialises ALL 512
slots as free (`windowed_slot_map.rs:80-94`: free_list seeded in
reverse, slot_to_world all `None`, indirection all `EMPTY_SLOT`). Pass 3
succeeds for ALL 512 binds in Frame 0 (`residency.rs:431-450` —
`window.allocate()` returns `Some(slot)` 512 times then `None`; the
512th allocate succeeds because pool capacity is exactly 512). No slots
are missing from the pool.

But CONFIRMED in spirit: the user's intuition is right that the
problem is "some slots end up in a broken state on cold-start." The
broken state isn't "missing from the pool" — it's "in
`dispatched_once` without actual chunks_buffer content."

### "Pre-sorting fix"

REFUTED. The sort order is correct (camera-distance-squared,
ascending). The 4 camera-nearest segments ARE picked first by
`process_pending_admissions`. The bug is downstream of the picking: the
picked slots get their `dispatched_once.insert(slot)` fire even when
the actual GPU dispatch does not.

Changing the sort order would change WHICH 4 segments fail to admit on
cold-start, but the user would still see a 4-segment-sized hole
somewhere. The actual fix is to defer `dispatched_once.insert` until
the render world confirms the dispatch ran.

A partial mitigation: SHOULD-2 in `03p-diagnosis-remaining-bugs.md`
proposed a "Bigger admission budget on cold-start" with
`max_segments_per_frame_cold_start = 32` (1 frame to fill the entire
window). If `WorldGpu` is allocated by Frame 1 (most likely — `prepare_world_gpu`
is best-case 1 frame), the cold-start hitch is one frame; if the race
extends to Frame 2-3, 32 of 512 still miss, but a re-pick on Frame 4
catches anything not dispatched. Doesn't fully fix the race but
narrows the window.

The real fix requires lifecycle separation between "queued for
admission" and "GPU-dispatched" — exactly what Phase 2.5's
`SlotState::Generating` vs `SlotState::Resident` enum was meant to
express. Phase 2.6 collapsed those states into "in admissions_this_frame
vs in dispatched_once" and lost the safety property: a slot can be in
`dispatched_once` without GPU work having completed.

## Captured log evidence (Task F if run)

SKIPPED — code reading was decisive. The mechanism is mechanically
derivable from `residency.rs:486-515` + `mod.rs:3114-3116` + Phase 2.12's
own documentation of the `WorldGpu` 1-3 frame race. A cargo run would
only confirm a known mechanism; the diagnostic budget is better spent
on the punch-list.

## Punch-list for fix dispatch

Ordered. Per faithful-port discipline + the user's no-microoptimization
rule, prefer the simplest fix that survives the race.

### MUST-1 — Defer `dispatched_once.insert(slot)` until GPU ACK

Mechanism (3 sub-steps):

1. Move `dispatched_once.insert(slot)` OUT of
   `process_pending_admissions` (`residency.rs:513`). Replace with a new
   per-frame Vec `dispatched_this_frame: Vec<SlotIndex>` on
   `Residency` that the producer node populates AFTER each successful
   dispatch.

2. The render world signals dispatch completion via a cross-world
   accumulator (same pattern as `PENDING_CLEAR_ON_BIND_SLOTS` at
   `noise_dispatch.rs:373`). Producer node pushes slot indices into
   `DISPATCH_ACK_SLOTS: Mutex<Vec<SlotIndex>>` after each per-segment
   encoder submit.

3. A new main-world system `apply_dispatch_acks` (PreUpdate before
   residency_driver, or Last) drains `DISPATCH_ACK_SLOTS` and inserts
   into `residency.dispatched_once`. **Now a slot is only marked
   Resident if its dispatch actually fired.**

LOC estimate: ~80 (analogous to Phase 2.12's clear-on-bind plumbing,
~135 LOC; this is simpler — one HashSet insertion per ack vs one GPU
clear_buffer per slot).

### MUST-2 — Regression catcher gate

Add an e2e gate that catches "any slot bound but not dispatched after N
frames of cold-start" as a hard FAIL.

Shape:

1. Boot the streaming preset with default params.
2. Wait 200 frames (much more than the 128-frame
   `512 / max_segments_per_frame = 4` cold-start drain).
3. Read back via the existing `streaming_aadf_parity` readback infrastructure:
   - `chunks_buffer` (slot-indexed).
   - `window_indirection_buffer`.
4. For each indirection entry that's NOT `EMPTY_SLOT`, decode the slot's
   first chunk: `chunks_buffer[slot * 4096].x >> 30u`. Should be ONE OF:
   - `0b00 = UNIFORM_EMPTY` (legitimately empty above-terrain segment).
   - `0b01 = UNIFORM_FULL` (legitimately full underground segment).
   - `0b10 = MIXED` (the common case at sea-level segments).
5. ASSERT: for every slot bound to a world_seg with Y == 1 (the
   sea-level row), at LEAST 1 chunk in the slot has state != UNIFORM_EMPTY.
   This is the "the slot has terrain content" predicate.
6. Asserts FAIL if any slot in the camera-row is uniformly empty
   (= sky-only); detects the cold-start gap bug.

LOC estimate: ~150 (mirror of `streaming_aadf_parity.rs` ~546 LOC, but
simpler — no CPU walker, just per-slot first-chunk inspection).

### SHOULD-1 — `clear_buffer` deduplication

Phase 2.12's clear-on-bind clears 512 slots on Frame 0 (~25 ms hitch).
With MUST-1's defer-ack pattern, the per-admission `clear_buffer` at
`mod.rs:3341-3345` is now redundant (the cross-world clear has already
fired before the per-segment encoder runs). Remove to save ~50 us per
admission.

LOC estimate: -20 (deletion).

### SHOULD-2 — Defensive log when producer early-returns due to missing `WorldGpu`

Producer at `mod.rs:3114-3116` early-returns silently. Add a
`bevy::log::warn_once!` so the race is visible in logs.

LOC estimate: ~5.

## What test would catch this

The streaming preset's existing gates all rely on **post-cold-start**
state. Phase 2.12's `streaming-framebuffer-diff` waits 384 frames before
shooting; `streaming-window` waits 120 frames + walk + 300 frames.
Both are run with `WorldGpu` long-since allocated, so they don't
exercise the Frame-0 race.

Specifically:

- `streaming-window` measures `mid_walk_terrain_ratio` (centre-pixel
  non-sky ratio). At cold-start that ratio for the same scene is
  visually 70-80% terrain (per `streaming_window_before.png`); a 4-of-
  512-slot gap is ~0.78% of the window's segment count, much smaller
  than the gate's noise floor.

- `streaming-aadf-parity` post-Phase-2.11 is tautologically PASS (W3
  disabled, no AADFs to walk). It doesn't inspect "is this slot's
  first chunk uniformly empty?"

- `streaming-framebuffer-diff` compares static vs streaming framebuffers
  at the same pose. SSIM at 0.05 threshold is very loose; a small
  rectangular hole at the camera-nearest segments may not push the
  comparison past the threshold.

**The gate that WOULD catch this**: MUST-2 above — an explicit
"every slot bound to the camera row has non-empty chunks content at
frame 200" assertion. The per-slot first-chunk decode is the
load-bearing check.

A second catcher worth considering: a `dispatched_once` size assertion
after the cold-start drain. If `dispatched_once.len() == 512` but the
number of slots with non-empty chunks content is LESS than 512, the
race fired. This is detectable via the existing main-world
`is_cold_start_complete()` predicate + a render-world chunks-content
audit.
