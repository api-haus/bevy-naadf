# 03l — Diagnosis: post-Phase-2.9 FPS hitch + view-distance corruption

Read-only investigation produced after Phase 2.9 (the camera-nudge fix) shipped.
Two user-visible bugs remain on `--grid-preset procedural-streaming`:

- **Bug 1** — visible FPS hitch every time the camera crosses a segment boundary.
- **Bug 2** — "blocks far-away appear briefly for one frame and disappear" —
  rays terminate too early; distant terrain flickers in and out as the camera
  moves.

Both bugs trace to the Phase 2.8 deferred-idle-flush mechanism. Working tree:
`feat/streaming-world` (HEAD = Phase 2.9 commit `fcfcd37` series).

## TL;DR

- **Bug 1 root cause: confirmed.** Every segment-boundary crossing sets
  `streaming_bounds_dirty = true`. The bounds chain dispatches on the FIRST
  idle frame after the admission burst ends (drained at 4 segments/frame).
  The dispatch is full-world worst-case extent
  (`voxel_workgroups = 134 M`, `block_workgroups = 2.1 M`), measured at
  **~300 ms per dispatch** on the test hardware
  (`03c-diagnosis.md` § "Root cause: minutes-long hang", lines 220-244;
  reproduced in `03i-impl-dirty-segments-bounds.md` line 168 — "Pre-fix:
  ~310 ms per admission frame"). This 300 ms single-frame hitch IS the user's
  visible stutter on every boundary crossing.

- **Bug 2 root cause: confirmed (stale AADF) + supporting cause
  (low MAX_RAY_STEPS_PRIMARY = 120).** Freshly-admitted segments carry
  `bounds = 0` (zero-init from `prepare_world_gpu`) until the deferred bounds
  chain fires. With AADF = 0, the ray DDA cannot use the chunk-skip
  optimisation; it walks 1 voxel at a time. The `MAX_RAY_STEPS_PRIMARY = 120`
  cap (`ray_tracing.wgsl:133`, runtime knob in `GpuRenderParams`) means a
  ray can step at most 120 voxels before terminating without a hit → terrain
  beyond ~120 voxels through any zero-AADF region is invisible. Each idle
  frame the bounds chain fires → AADF refreshes → distant terrain
  reappears for one frame. The next admission burst restarts the cycle.

- **Punch-list size estimate: ~75-130 LOC** in `mod.rs` plus an optional
  `bounds_calc.wgsl`/`chunk_calc.wgsl` shader-offset uniform (~30 LOC).
  Most-likely path: dispatch the bounds chain EVERY admission frame (not
  deferred) BUT scoped to the just-affected segments — exactly the path
  `03i-impl-dirty-segments-bounds.md` deferred as "per-segment scoped
  dispatch via a new shader-side offset uniform" (lines 225-228, marked
  "Not in scope this session").

---

## Bug 1 — Hitch root cause

### Trigger sequence

`feat/streaming-world` HEAD; trace through
`crates/bevy_naadf/src/render/construction/mod.rs:2944-3233`:

1. Camera moves; `track_and_pin_camera`
   (`crates/bevy_naadf/src/streaming/camera.rs:137-182`) updates
   `CameraAbsolutePosition`.
2. `residency_driver`
   (`crates/bevy_naadf/src/streaming/residency.rs:266-397`) runs in
   `PreUpdate`. Detects the segment-boundary crossing
   (`do_shift = true`, line 297-300). Computes `new_origin`
   (line 309), evicts the segments that fell out of the window
   (lines 316-330), enqueues the freshly-in-window segments as
   `Generating` (lines 343-382). Pushes up to `max_segments_per_frame = 4`
   into `admissions_this_frame` (line 386, `process_pending_admissions`
   at lines 418-447).
3. `extract_streaming_state`
   (`crates/bevy_naadf/src/streaming/noise_dispatch.rs:310-382`) mirrors
   the admissions/evictions deltas + the indirection table into
   `StreamingExtractRender`.
4. Render-graph node `naadf_gpu_producer_node`
   (`crates/bevy_naadf/src/render/construction/mod.rs:2944` "streaming-world
   Phase 2 branch") sees non-empty admissions → runs the per-segment
   noise + chunk_calc dispatch (lines 3003-3139). Sets
   `gpu.streaming_bounds_dirty = true` (line 3175). **Skips the bounds
   chain** (the `else if` at line 3176 doesn't fire because
   `any_admissions_or_evictions = true`).
5. The "shift" produces a number of new segments equal to the
   shift-width-in-segments × window-perpendicular-face-area-in-segments.
   For a 1-segment shift in X on the 16×2×16 window: 1 × 2 × 16 = 32
   segments. At 4 admissions/frame, that's **8 admission frames** to drain
   the queue.
6. After the queue drains (frame N+8), `admissions_this_frame.is_empty()`
   = true AND `streaming_bounds_dirty` = true → the `else if` branch at
   line 3176 fires → bounds chain dispatched on the render-context
   encoder.

### Cost of the deferred dispatch

From `mod.rs:3182-3190`:

```rust
let world_chunks = crate::WORLD_SIZE_IN_CHUNKS.x  // = 256
    * crate::WORLD_SIZE_IN_CHUNKS.y               // ×  32
    * crate::WORLD_SIZE_IN_CHUNKS.z;              // × 256
    // = 2,097,152
let max_blocks_u64 = (world_chunks as u64) * 64;  // = 134,217,728
let max_voxels_u64 = max_blocks_u64 * 32;         // = 4,294,967,296
let voxel_workgroups = ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
    // = 134,217,729 voxel workgroups
let block_workgroups = ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
    // =   2,097,153 block workgroups
```

Both `compute_voxel_bounds` and `compute_block_bounds` are `@workgroup_size(64,1,1)`
(`crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:488,540`). Total
threads:
- `compute_voxel_bounds`: 134 M wg × 64 threads = **8.59 B thread invocations**
- `compute_block_bounds`: 2.1 M wg × 64 threads = **134 M thread invocations**

Measured cost on the test machine (RTX 5080):

| Source | Value | Citation |
|---|---|---|
| Pre-Phase-2.8 per-frame cost | ~310 ms/frame | `03c-diagnosis.md` line 244 ("310-330 ms between consecutive `dispatched 4 segment(s)` lines × 300 frames"). |
| Pre-Phase-2.8 admission count | 128 frames × 310 ms | `03c-diagnosis.md` line 230, 244. |
| Phase 2.8 cold-start improvement | ~40 s → 1.02 s (single 300 ms dispatch) | `03i-impl-dirty-segments-bounds.md` line 145, 167 (`Window-create → bounds-flush = 1.021 s`). |

So one full-extent bounds chain = **~300 ms**.

### Why this manifests as a per-boundary hitch (not a one-time cold-start)

Phase 2.8 fixed the cold-start (where 128 consecutive admission frames each
re-dispatched the full chain). But the deferred-latch ALSO fires on **every
subsequent segment-boundary crossing** — `streaming_bounds_dirty` is set every
admission/eviction frame regardless of cold-start vs runtime.

Trace:
- Camera walks +1 segment → 32 new segments enter the window → 8 admission
  frames at 4/frame.
- Frames N..N+7: admissions present, `dirty = true`, bounds skipped (5-8 ms/frame).
- Frame N+8: idle (admissions empty for one frame because the queue drained) →
  bounds chain runs on the render-context encoder → ~300 ms frame.
- User experiences: ~8 quick frames + 1 huge 300 ms frame on every boundary
  crossing.

The hitch frequency matches user observation ("every reposition"). Camera at
walking speed traverses one segment (256 voxels) per ~N seconds; each
traversal triggers exactly one deferred bounds dispatch.

### Code citations

| Line | Mechanism |
|---|---|
| `mod.rs:281` | `pub streaming_bounds_dirty: bool` field. |
| `mod.rs:3173-3175` | `if any_admissions_or_evictions { gpu.streaming_bounds_dirty = true; }`. |
| `mod.rs:3176-3212` | `else if gpu.streaming_bounds_dirty { /* dispatch bounds */; gpu.streaming_bounds_dirty = false; }`. |
| `mod.rs:3182-3190` | Full-world workgroup count computation. |
| `mod.rs:3192-3203` | `dispatch_compute_voxel_bounds` + `dispatch_compute_block_bounds` on the render-context encoder. |
| `chunk_calc.rs:282-322` | The dispatch helpers — `@workgroup_size(64,1,1)`, 3D-split for the 65535-per-axis cap. |
| `chunk_calc.wgsl:488,540` | Shader entry points — full-flat indexing. No per-segment scoping. |

---

## Bug 2 — View-distance corruption root cause

### Verdict on each candidate cause

1. **Stale AADF on newly-admitted segments — CONFIRMED ROOT CAUSE.**

   Trace:
   - `prepare_world_gpu`
     (`crates/bevy_naadf/src/render/prepare.rs:271-298` per Phase 2.5
     diagnostic): the `chunks_buffer` is zero-initialised at allocation.
   - The streaming dispatch loop (`mod.rs:3003-3139`) writes:
     `noise_terrain` → fills voxel buffer; `chunk_calc.calc_block_from_raw_data`
     → fills `blocks` + `chunks[idx].x`'s state bits (`>> 30u`). But neither
     pass writes the AADF skip-distance bit-fields. Those are owned by
     `compute_voxel_bounds` + `compute_block_bounds` (on `voxels` + `blocks`
     respectively) and `compute_group_bounds` (on `chunks.x` — the
     chunk-level 5-bit AADFs at bits `[0..30)`).
   - During an admission burst, the bounds chain is DEFERRED → freshly-
     admitted chunks carry the zero-init state: `chunks.x = state<<30 | 0`,
     `blocks[i] = state<<30 | 0`, `voxels[i] = state<<15 | 0`. The
     chunk-level 5-bit-per-axis AADF bits at `chunks.x[0..30)` are all
     zero.

   Ray-tracing consequence — `ray_tracing.wgsl:367-376`:
   ```wgsl
   // Chunk is *not* mixed (uniform-empty). 5-bit AADF in chunk units.
   bounds_in_dir = offset + 16u * vec3<u32>(
       (cur_node >> shift_chunk.x) & 0x1Fu,   // ZERO for stale chunks
       (cur_node >> shift_chunk.y) & 0x1Fu,   // ZERO for stale chunks
       (cur_node >> shift_chunk.z) & 0x1Fu,   // ZERO for stale chunks
   );
   ```
   With zero AADF, `bounds_in_dir = offset + 0` = `15 - voxel_pos_in_chunk`
   (or `voxel_pos_in_chunk` for negative directions) — i.e. the ray skips
   only to the FAR FACE OF THE CURRENT CHUNK (≤16 voxels), then enters the
   next chunk at one voxel per step. Without AADF, the chunk-skip
   acceleration collapses to per-chunk-boundary steps.

   At `MAX_RAY_STEPS_PRIMARY = 120`
   (`ray_tracing.wgsl:133` const + `gpu_types.rs:79-87` runtime knob;
   `prepare.rs:744`):
   - With proper AADF (one chunk-skip per step at e.g. 16-voxel granularity),
     a ray can reach ~120 × ~16 = ~1920 voxels of empty space before the
     step cap.
   - With AADF = 0 (zero-skip), a ray can reach ~120 × 16 = 1920 voxels
     of *chunk-boundary* steps (still 1920 voxels — because each step
     advances at least one chunk), BUT only on uniform-empty chunks.
     On a mixed chunk (one that contains some terrain), `cur_node >> 31` =
     1 → descends to block layer → `bounds_in_dir = (cur_node >> shift_voxel_block) & 0x3u`
     (`ray_tracing.wgsl:342-346`). With stale block-level AADF (also zero
     after streaming admission), the block-level skip-distance is 0 →
     each step advances one block (4 voxels). So through a mixed chunk
     with stale AADF, max ray range = 120 × 4 = 480 voxels.
   - 480 voxels at standard sea_level 256 with terrain_amplitude 64 covers
     the height range but only a tiny portion of horizontal span before
     hitting the step cap → distant terrain unreachable.

   **The "blocks appear briefly for one frame" pattern**: when the deferred
   bounds chain finally fires on an idle frame, AADF refreshes globally →
   that frame's rays reach distant terrain → distant blocks visible. The
   next frame is an admission frame (next 4 segments of the boundary
   crossing) → AADF zeroed for the newly-admitted segments → those
   neighboring segments' chunks read zero AADF → ray-marching collapses
   in their direction.

2. **`EMPTY_SLOT` returning sky vs treating as empty chunk — VERIFIED:
   "empty chunk", NOT sky.**

   `world_data.wgsl:205-211`:
   ```wgsl
   fn streaming_chunk_load(chunk_pos: vec3<u32>) -> vec2<u32> {
       let idx = streaming_chunk_index(chunk_pos);
       if (idx == 0xFFFFFFFFu) {
           return vec2<u32>(0u, 0u);
       }
       return chunks[idx];
   }
   ```
   When the indirection table maps a position to `EMPTY_SLOT`, the load
   returns `(0u, 0u)` — `cur_node = 0u`. In `ray_tracing.wgsl:317`:
   `if ((cur_node >> 31u) != 0u)` → 0 → fall through to the "not mixed"
   branch at line 360. At line 380: `if ((cur_node & 0x40000000u) != 0u)`
   → 0 → not a uniform-full hit. The ray treats the empty slot as
   uniform-empty with zero AADF, skips to the far face of that chunk,
   and continues marching.

   **The design `02c-design-windowed-slot-map.md` § E proposed
   `if slot == EMPTY_SLOT { return SKY; }`** (lines 633-637, 787-790).
   What was actually shipped is "treat as uniform-empty with zero AADF"
   — NOT "return sky immediately". This is a design deviation but the
   correctness implication is benign: rays continue marching through
   the empty region; if they exit the bbox they MISS into the
   atmosphere; if they hit a still-resident chunk past the empty slot
   they hit that. The cost is per-chunk traversal through the empty
   slots (no skip-distance), eating step budget.

   On the streaming preset, **window-local positions outside the
   resident window (≥ 256 voxels on X or Z, ≥ 32 on Y) read past the
   indirection table extent (512 u32s)**. WGSL bounds-clamping on
   storage-buffer reads makes those reads return either 0 or
   implementation-defined garbage clamped to slot 0. This is harmless
   in practice because the camera Transform is window-local-centered
   so rays from the camera typically stay inside `[0, 4096)` window-
   local coords corresponding to in-window segments — but it's a
   latent footgun.

3. **`MAX_RAY_STEPS_PRIMARY` cap — CONTRIBUTING CAUSE.**

   `ray_tracing.wgsl:133`: `const MAX_RAY_STEPS_PRIMARY: i32 = 120;`
   Runtime override: `GpuRenderParams.max_ray_steps_primary`
   (`gpu_types.rs:79-87`, default 120 at `lib.rs:230`, used at
   `naadf_first_hit.wgsl:181`).

   120 steps is the canonical NAADF value. It's tuned for *good* AADF
   (each step skips ≥ 1 chunk on average through empty space). With
   stale AADF on freshly-admitted streaming chunks, 120 steps is not
   enough to reach across a 4096-voxel window. This isn't a bug in the
   cap; it's the AADF staleness amplifying through the cap.

   No view-distance override / ray-length cap besides this — verified
   via the grep:

   ```
   $ grep -n 'view_distance' crates/bevy_naadf/src/  # zero hits.
   ```

4. **`bounding_box_max` in `GpuWorldMeta` — NOT the cause.**

   `prepare.rs:520-527`:
   `bounding_box_max = size_in_voxels - Vec3::splat(0.1)` =
   `(4095.9, 511.9, 4095.9)` (the full world extent). The ray's
   bbox-exit test (`ray_tracing.wgsl:269`:
   `if (any(vec3<f32>(cur_cell) >= bbox_max)) break;`) only fires
   when the cell exits the FULL world bbox in window-local frame.
   Since the camera Transform is window-local-pinned at the center
   of the 16×2×16-segment window (= 2048 voxels from each X/Z edge),
   rays from the camera have ~2048 voxels of bbox before clipping —
   far more than `MAX_RAY_STEPS_PRIMARY = 120` allows even with
   perfect AADF. Bbox is not load-bearing for the view-distance
   complaint.

5. **W3 background bounds-compute (regime-2) — VERIFIED NOT RUNNING ON
   STREAMING.**

   `bounds_calc.rs:316-390`: `naadf_bounds_compute_node` gates on
   `if !construction_gpu.bounds_initialized { return; }`. Phase 2.8
   sets `bounds_initialized = true` on the first idle flush
   (`mod.rs:3221-3223`). After that, the W3 background loop runs each
   frame.

   BUT: `mod.rs:1876-1886` gates the regime-1 seed
   (`add_initial_groups_to_bound_queue`) on `&& !noise_dispatch_active`.
   On the streaming preset, `noise_dispatch_active = true` always →
   regime-1 seed NEVER runs → the W3 bound-queue family is never
   populated. The W3 regime-2 `naadf_bounds_compute_node` then
   `prepare_group_bounds` finds every queue empty,
   `compute_group_bounds` runs over count = 0 with a single
   no-op workgroup (per the `max(1, group_amount)` floor at
   `bounds_calc.wgsl:361`). So **the W3 chunk-level 5-bit AADFs are
   never updated by the background queue on the streaming preset**.

   The ONLY path that updates AADF on streaming is the deferred
   `compute_voxel_bounds + compute_block_bounds` flush — which writes
   voxel-level (2-bit) and block-level (2-bit) AADFs only. The
   chunk-level (5-bit) AADFs at `chunks.x[0..30)` are NEVER computed
   on streaming. This is a SEPARATE BUG masked by the view-distance
   complaint:

   - Voxel-level (2-bit) AADF → max skip in mixed chunks = 3 voxels.
   - Block-level (2-bit) AADF → max skip in mixed chunks = 3 blocks
     (12 voxels).
   - Chunk-level (5-bit) AADF (NEVER populated on streaming) → max
     skip on uniform-empty chunks = 31 chunks (496 voxels).

   With zero chunk-level AADF, even AFTER the deferred-idle bounds
   flush, uniform-empty chunks contribute only `offset + 0 * 16` =
   ≤16-voxel skips per step. So distant terrain in uniform-empty
   directions (sky above terrain) is reachable only via 120 ×
   ~chunk-boundary-step ≈ 120 chunks worst-case, but in practice
   bounded much lower because the `+ offset` term adds 0..15. The
   uniform-empty regions are the LARGEST in a terrain world (sky
   above the heightmap = vast empty space). With chunk-level AADF =
   0, rays going up-and-forward expensively chunk-step through that
   empty space.

### Frame-by-frame model

Consider the camera walking +X at moderate speed, crossing one segment
boundary every ~120 frames. A boundary crossing produces 32 new segments
(1 × 2 × 16) drained at 4/frame over 8 admission frames.

**Frames 0..7: admission burst (8 frames at ~5-8 ms each, per
`03i-impl-dirty-segments-bounds.md` line 168).**

- `residency_driver` doesn't shift origin again (no new crossing yet); 4
  Generating slots picked by `process_pending_admissions`
  (`residency.rs:418-447`).
- Render: `naadf_gpu_producer_node` streams 4 segments — noise_terrain +
  chunk_calc per segment (`mod.rs:3003-3139`). Each segment write
  populates `chunks[slot * 4096 .. (slot+1) * 4096]` (state bits at
  `[30..32)` + entity counts; AADF bits in `[0..30)` stay ZERO).
  `voxels`, `blocks` written by `chunk_calc.calc_block_from_raw_data`
  (`chunk_calc.wgsl:380` workgroup_size 4³, processes per-block voxel
  data; populates state bits but NOT the AADF skip bits — those are
  the job of the `compute_*_bounds` passes).
- `streaming_bounds_dirty = true` set (`mod.rs:3175`).
- BOUNDS CHAIN SKIPPED.
- Rays during these frames: freshly-admitted segments + freshly-evicted
  segments BOTH have AADF = 0 in their chunks. Rays crossing those
  segments collapse to chunk-by-chunk stepping. Distant terrain
  (>120 chunks away through empty regions) becomes UNREACHABLE.

**Frame 8: idle frame (admission queue drained).**

- `residency_driver` runs but no shift (`do_shift = false`,
  `residency.rs:297`); `process_pending_admissions` finds 0
  candidates (all marked dispatched). `admissions_this_frame = []`,
  `evictions_this_frame = []`.
- Render: `any_admissions_or_evictions = false`,
  `streaming_bounds_dirty = true` → the else-if at `mod.rs:3176`
  fires → 300 ms BOUNDS CHAIN dispatch.
- User experiences the visible hitch.
- After dispatch, all voxel-level (2-bit) and block-level (2-bit)
  AADFs are up to date. Chunk-level (5-bit) AADFs at `chunks.x[0..30)`
  remain ZERO (the W3 chain is not dispatched here — bug masked).
- Rays during this frame: AADF acceleration works at the block + voxel
  level. **Distant terrain visible** through the freshly-bounded
  segments. User sees blocks "appear".

**Frame 9..N: steady state until next boundary crossing.**

- No admissions / evictions until camera reaches the next boundary.
- `streaming_bounds_dirty = false` → bounds chain DOESN'T run.
- AADF stable. Distant terrain stays visible.

**Frame N+1: camera crosses next boundary.**

- Cycle repeats: 8 admission frames + 1 hitch frame.
- During the 8 admission frames, distant terrain DISAPPEARS in the
  direction of the new segments. User sees blocks "disappear".

This matches the user's description: "blocks far-away appear briefly for
one frame and disappear ... it looks like something is corrupted in a
view-distance and it terminates rays too early".

### Code citations (Bug 2)

| Line | Mechanism |
|---|---|
| `ray_tracing.wgsl:133` | `MAX_RAY_STEPS_PRIMARY = 120`. |
| `gpu_types.rs:79-87`, `prepare.rs:744` | Runtime knob; default 120. |
| `ray_tracing.wgsl:256-399` | Main DDA loop; only termination conditions are step-cap (`:257-259`), bbox-max (`:269-271`), bbox-min (`:280-282`), uniform-full hit (`:380-385`). No view-distance cap. |
| `ray_tracing.wgsl:317-376` | AADF expansion. `bounds_in_dir` = bit-extraction of AADF fields × 4 or × 16. With AADF = 0, this collapses to per-cell stepping. |
| `world_data.wgsl:205-211` | `streaming_chunk_load` returns `vec2(0u, 0u)` on `EMPTY_SLOT` — NOT sky-early-return. |
| `mod.rs:1876-1886` | `add_initial_groups_to_bound_queue` (the W3 5-bit chunk-level AADF seed) is skipped when `noise_dispatch_active` (streaming OR static). |
| `bounds_calc.rs:316-390` | W3 background regime-2 — runs but has no work because the seed never fires. |
| `prepare.rs:271-298` (per `03c-diagnosis.md` line 138) | `chunks_buffer` zero-init at allocation. |
| `mod.rs:3128-3139` | Streaming per-segment dispatch — populates voxels/blocks/chunks-state but NOT chunk-level AADF nor voxel/block AADF (deferred). |
| `mod.rs:3176-3212` | Deferred AADF dispatch — voxel-bounds + block-bounds, but NOT the chunk-level W3 pass. |

---

## Punch-list for the fix dispatch

Estimated total LOC: ~75-130 (within the LOC budget; not a >200 escalation).

### MUST (load-bearing fixes)

1. **Replace deferred-idle-flush with per-affected-segment bounds dispatch on
   EVERY admission frame.** Scope the dispatch to the just-affected segments
   only, instead of the full-world worst-case extent.

   - File: `crates/bevy_naadf/src/render/construction/mod.rs:3141-3212`.
   - Mechanism: introduce a `chunk_offset` / `chunk_count` push-constant or
     uniform for the bounds passes; dispatch over the smaller range every
     frame admissions or evictions land. Per-frame cost target: ~10 ms
     (`affected_segments × 4096 chunks/segment × 64 voxel-workgroups-per-chunk`
     ≈ 1 M voxel workgroups for a 4-segment frame, vs 134 M for full-world).
   - LOC budget: ~50-80 LOC in `mod.rs` (rebuild the workgroup count math
     per-segment + loop the dispatch over `admissions + evictions` lists)
     plus an optional shader-side offset uniform if the existing dispatch
     helpers can't be adapted.

2. **Decide & document `EMPTY_SLOT` handling.** The design said
   "EMPTY_SLOT → sky"; the implementation does "EMPTY_SLOT → uniform-empty
   chunk with zero AADF". Pick one and align design + code.

   - File: `crates/bevy_naadf/src/assets/shaders/world_data.wgsl:205-211`.
   - Recommended: keep current "uniform-empty" behavior (rays exit the
     window cleanly), but add a `MAX_RAY_STEPS_PRIMARY = 240` runtime
     bump on streaming for safety until per-segment bounds is online.
     LOC budget: ~5 LOC for the optional cap bump + a comment.

3. **Restore chunk-level 5-bit AADF via the W3 chain on streaming.** The
   regime-1 seed (`add_initial_groups_to_bound_queue`) is currently
   `noise_dispatch_active`-gated off; this leaves chunk-level AADF at zero
   forever on streaming.

   - File: `crates/bevy_naadf/src/render/construction/mod.rs:1885-1886`
     (the `&& !noise_dispatch_active` gate).
   - Two options:
     - (a) Run the regime-1 seed on the first idle frame after the first
       admission burst (one-shot, full-world). Cost: ~1 ms per
       `bound_group_queue_max_size` count. Same as the static preset.
     - (b) Run a SCOPED regime-1 seed per admission frame, seeding ONLY
       the freshly-admitted segments' bound groups. ~10-30 LOC.
   - Either path lets the W3 background queue run; once seeded, regime-2
     incrementally expands the chunk-level 5-bit AADF over many frames
     (`n_bounds_rounds = 1` round per frame at NAADF defaults).
   - LOC budget: ~10-30 LOC.

### SHOULD (verification / hardening)

4. **Add a per-frame timing assertion to `--gate streaming-window`.** After
   the camera walks, expect NO frame > 50 ms across the post-walk wait
   phase. The existing gate just checks aggregate variance + origin shift;
   it would PASS on a 300 ms-per-frame regression.

   - File: `crates/bevy_naadf/src/e2e/streaming_window.rs` (the existing
     `ShootBefore` / `WaitPostEdit` / `ShootAfter` driver phases at
     `oasis_edit_visual.rs:113-125`).
   - Mechanism: collect `Instant::now()` between
     `OasisShootBefore`...`OasisShootAfter`; assert
     `max_frame_time < 50 ms`.
   - LOC budget: ~30 LOC in the e2e gate.

5. **Add a `--validate-streaming-distant-terrain` gate.** After camera walk,
   raycast from camera forward and verify a non-skybox hit at some `t > 256`
   voxels (i.e., the ray reached past the camera-segment-distance threshold
   into resident-but-just-loaded territory).

   - File: `crates/bevy_naadf/src/e2e/streaming_window.rs` (new assertion).
   - Mechanism: read back framebuffer; count pixels matching the sky-gradient
     at the lower-half of the frame; fail if >50% (sky-only after a walk
     means rays terminated too early). Or sample a specific pixel near
     screen center after the walk; check it's not sky-blue.
   - LOC budget: ~30-50 LOC.

### OPTIONAL

6. **Reconsider `MAX_RAY_STEPS_PRIMARY` cap on streaming preset.** 120 is
   the C# / NAADF canonical value tuned for full AADF acceleration. While
   AADF on streaming is recovering, this could be temporarily bumped to
   240-360 to mask transient stale-AADF artifacts. Once item (1) lands and
   AADF stays at-most-1-frame stale, the 120 cap is sufficient.

   - File: `crates/bevy_naadf/src/lib.rs:230`
     (`max_ray_steps_primary: 120`).
   - Recommendation: keep at 120 (don't paper over the real fix).

7. **Demote the per-frame `info!` log at `mod.rs:3225-3232`.** During
   active traversal, this logs at every admission frame — noisy in the
   user's terminal. Demote to `debug!` post-fix.

---

## What test would catch this

Extend `--gate streaming-window` with:

- **Per-frame timing assertion** — assert `max(frame_time) < 50 ms` across
  the post-walk wait phase (300 frames). Catches Bug 1 (any frame > 50 ms
  = a deferred bounds dispatch crept back in).
- **Mid-walk distant-terrain visibility check** — capture a framebuffer
  during the camera walk (frame 60 of 120, when admissions are actively
  draining); assert >X% of non-skybox pixels at screen center. Catches
  Bug 2 (stale AADF mid-walk should NOT collapse the ray range to
  per-block stepping).

The existing gate's strict assertions (pixel Δ ≥ 3.0; variance ≥ 800;
origin shift = 4) all measure POST-walk state when admissions have
drained AND bounds has flushed. They don't catch transient mid-walk
stale-AADF artifacts.

---

## Hard one-off observation

Not performed. The hypothesis is confirmable from code reading alone
(the deferred-flush mechanism is explicit in `mod.rs:3173-3212`, the
ray shader's AADF consumption is explicit at `ray_tracing.wgsl:317-376`,
and the `EMPTY_SLOT → (0u, 0u)` behavior is explicit at
`world_data.wgsl:205-211`). Adding a single runtime measurement would
not change any conclusion.

If the fix dispatch wants a confirmation pass: log
`info!("bounds dispatched at frame={}, admissions=0", frame)` whenever
the else-if at `mod.rs:3176` fires, then walk the camera with
`--gate streaming-window` and grep for the frequency — it will print
exactly once per N frames where N = (8 admission frames + 1 idle
frame) per segment crossing.
