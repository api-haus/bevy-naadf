# 03p — Diagnosis: remaining streaming-world bugs after Phase 2.11

Read-only investigation. Post-Phase-2.11 HEAD (`fcfcd37` ← `feat/streaming-world`,
`PHASE_2_11_ENABLE_STREAMING_W3` opt-in W3 chain). All 8 e2e gates pass
including the new `streaming-aadf-parity` gate — the user reports the
visible bug nonetheless persists.

## Summary

Three bugs remain after Phase 2.11. They are caused by ONE root architectural
issue plus two emergent symptoms:

| Bug | Root cause | Mechanism |
|---|---|---|
| AADF-skip-style corruption during traversal | **Indirection-table upload races chunks_buffer rewrites on shift** | Slot's `chunks_buffer` region holds OLD evicted segment's data; indirection now points new window-local position at that slot; renderer reads OLD data interpreted at NEW position |
| Chunk pop-in on traversal | Same: `max_segments_per_frame = 4` means 32 evicted/admitted = 8 frames to drain | While draining, renderer sees mixed "old slot data + new noise+chunk_calc partials" |
| Slight lag regression vs Phase 2.10 | Phase 2.11's `clear_buffer` + `cold_start_complete` plumbing add minor steady-state cost; the W3-disable gate ALSO suppresses correct distant-AADF — making rays step chunk-by-chunk through more empty space (more steps/ray, higher pixel-shader cost) | Per-frame trade — defensive zeroing + extra branch per admission |

**The diagnostic the parity gate was designed to catch IS a real bug class
— but Phase 2.11's fix (disable W3) sidesteps the parity-gate measurement
without solving the underlying visual corruption.** The visual corruption
has a different root cause than the W3-stale-AADF bug `03n` originally
diagnosed; the W3 disable was correct but insufficient.

## Framebuffer evidence

### Gates executed

```bash
timeout 240s cargo run --release --bin e2e_render -- --gate noise-static-world
# → PASS; saves target/e2e-screenshots/noise_static_after.png
timeout 240s cargo run --release --bin e2e_render -- --gate streaming-window
# → PASS; saves target/e2e-screenshots/streaming_window_{before,mid_walk,after}.png
```

Both gates reported PASS. Wall-clock ~13 s each. All 5 streaming-window
assertions passed at strict thresholds (pixel Δ 73.68, after-frame variance
2334.47, origin shift 4 segments, max walk-frame 22.0 ms, mid-walk centre
ratio 0.736).

### On-disk paths

- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/target/e2e-screenshots/noise_static_after.png` — static preset, full cold-start in one frame.
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/target/e2e-screenshots/streaming_window_before.png` — streaming preset, 120 warmup frames (480 / 512 admissions completed; far-corner segments still un-admitted).
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/target/e2e-screenshots/streaming_window_mid_walk.png` — streaming preset, mid-walk (camera moving +X through shift-induced eviction+admission churn).
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/target/e2e-screenshots/streaming_window_after.png` — streaming preset, post-walk + 300 frames wait.

### Visual observations (Read tool — images rendered visually)

**`noise_static_after.png`** — sharp voxel terrain, multiple visible voxel
cubes at varying distances, depth-of-field haze at far reach, characteristic
snow/sand voxel material. The reference for "clean rendering at this camera
pose with this noise seed."

**`streaming_window_before.png`** — at cold-start spawn pose (no walk yet).
Visible features:
- Upper third: rough textured terrain (snow voxels) on left and right,
  resembling proper voxel geometry.
- Centre & lower-left: large smooth beige plateau — appears to be the
  near-face of a multi-chunk-tall voxel mass viewed from within the
  terrain layer.
- Lower-right: dark navy square — atmospheric sky visible through what
  looks like a notch in terrain.
- Sharp vertical seam at horizontal-centre, around mid-frame — a
  segment boundary (X=2304 = world-segment 9 edge) bisects the visible
  geometry.

Reading this WITHOUT prior visual ground truth: it could PLAUSIBLY be
correct — looking out of inside-terrain at a near-face. The Phase 2.10
gate's pixel-Δ floor is `STREAMING_MIN_PIXEL_DELTA = 3.0`; the measured
`Δ = 73.68` says before/after differ substantially. Variance after = 2334.47
(static preset = 1820.17) — streaming has MORE variance than static.
On the surface this looks healthier than static.

**However**: the LOWER half of `before` is much flatter than the
corresponding region in `noise_static_after`. The static preset shows
multiple chunk-scale features in the lower frame; streaming-before shows
a smooth slab. That IS a visible difference — and a faithful port should
match.

**`streaming_window_mid_walk.png`** — mid-walk (camera moving +X). DRAMATIC
visible corruption:
- Granular dithered appearance across the whole frame — pixels in random
  light/dark patterns where smooth voxel surfaces should be.
- Visible chunk-boundary stair-stepping with INCONSISTENT face shading
  per chunk (adjacent chunks of the same voxel material rendering with
  different luminance).
- This is the user-reported "AADF skip corruption" pattern — rays are
  hitting different surfaces per pixel due to the rendered chunks_buffer
  content being inconsistent during the admission drain.

**`streaming_window_after.png`** — after 4-segment +X walk plus 300 frames
of wait — clean voxel terrain, no granular artefacts, no corruption.
Multiple voxel cubes visible at varying distances. Matches the visual
quality of `noise_static_after.png` (clean voxel rendering, depth
information present).

### Side-by-side conclusion

The visual diff `streaming_window_mid_walk.png` vs `streaming_window_after.png`
IS the bug. Both presets resolve to clean rendering POST-DRAIN. The bug
surfaces DURING admission churn (8 frames per shift × repeated shifts
during walk = continuous churn for ~32 walk frames).

The `streaming_window_before.png` is borderline — it MAY be a slightly
incomplete cold-start (32 un-admitted slots at far corners; they're
beyond the camera frustum looking +X but contribute through any rays
that reflect or sky-leak). It MAY also just be a legitimate close-up of
a voxel mass.

**The decisive evidence is the mid-walk capture: dramatic visible
corruption that the parity gate didn't catch.**

## Parity gate post-mortem

### The gate compares internal buffers, not framebuffers

Confirmed by reading `crates/bevy_naadf/src/e2e/streaming_aadf_parity.rs:1-80`:
the gate snapshots `WorldGpu::chunks_buffer` + `ConstructionGpu::window_indirection_buffer`
after the camera walk completes, then walks every UNIFORM_EMPTY chunk's
6-direction 5-bit AADF on the CPU and asserts each `aadf_d` step lands
within still-empty terrain.

### With W3 disabled, both sides are zero by construction

Phase 2.11 (`03o` § Item 2) disabled W3 on streaming by default
(`PHASE_2_11_ENABLE_STREAMING_W3 != "1"`):
- `bounds_initialized` stays `false` (see `mod.rs:3384-3394`).
- `naadf_bounds_compute_node` early-returns (`bounds_calc.rs:348-350`).
- The W3 chunk-level 5-bit AADF chain **never writes**.
- The 5-bit AADF fields in every `chunks_buffer` entry stay at their
  chunk_calc-written value: `chunk_calc.wgsl:468` writes
  `chunks[chunk_idx] = vec2<u32>(state, 0u)`. The `state` u32's bits 0..30
  ARE the 5-bit AADF fields (in chunk encoding). chunk_calc writes
  either `first_voxel_type | (s << 30u)` (uniform) or `new_base | (CHILD << 30u)`
  (mixed). The low 30 bits are the type-id (uniform) or block-pointer
  (mixed), NOT the AADF fields.

**Wait — those fields DO populate to non-zero on uniform-full chunks.**
For a uniform-full chunk with type 1, `state = 1`. Bits 0..30 = 1. The
renderer reads `bounds_in_dir = (state >> shift_chunk.{x,y,z}) & 0x1F`.
Most shifts will yield 0, since `(1 >> 5) & 0x1F = 0`, etc. Only the
shift_chunk.x case (bit 0 only) yields 1.

For a uniform-EMPTY chunk, state = 0, all AADFs = 0. The walker walks
6 directions × up-to-31 steps. Each step decodes `(0 >> shift) & 0x1F = 0`.
The loop's stopping condition is `step <= aadf_d = 0` — **the inner walk
loop never executes**. The gate trivially passes 0 violations.

For uniform-FULL (state ≠ 0 in bits 0..30), the chunk is non-empty and
the OUTER loop skips it (the invariant only applies to UNIFORM_EMPTY
chunks).

For mixed chunks (bit 31 set, state bits 30..32 = 0b10), the OUTER loop
also skips them (not UNIFORM_EMPTY).

**Conclusion: with W3 disabled, the gate has nothing to walk. The check
is tautologically zero. The gate's "PASS — 0 violations" is meaningless.**

This matches the user-feedback memory `feedback-parity-gate-must-not-be-tautological`:
the gate was designed to catch W3 stale-AADF corruption; Phase 2.11's
fix eliminated the W3 chain entirely; the gate's measurement plane is
empty.

### What the synthetic regression check actually proves

Phase 2.11's `03o` § "Synthetic regression check" set
`PHASE_2_11_ENABLE_STREAMING_W3=1` + bypass flags and re-ran the parity
gate — it caught 32341 violations. That proves the gate WORKS WHEN W3
runs. It does NOT prove the gate would catch the user-visible visual
bug, because the bug isn't W3-AADF-related when W3 is disabled.

## Bug 1 — AADF skip corruption (the real one)

### Refuting the "W3 stale AADF" hypothesis as the current visible cause

With W3 disabled on streaming (Phase 2.11 default), the chunks_buffer's
5-bit chunk-level AADF fields STAY ZERO. The W3 chain that previously
populated them with "skip 8/15/31 chunks" hints does not run. Rays
reading these zero AADFs always interpret the chunk as "1-chunk skip
distance maximum" — they step ONE chunk per loop iteration.

**With AADFs zeroed**, the bug CANNOT be "ray follows lying AADF and
skips past terrain". A ray stepping 1-chunk-at-a-time cannot skip past
any chunk it traverses.

### The actual mechanism — indirection-table races chunks_buffer

The bug is in the SEQUENCE of frame-level dispatch:

1. **`PreUpdate::residency_driver`** (`crates/bevy_naadf/src/streaming/residency.rs:291-422`):
   - On segment-boundary crossing, calls `window.set_origin(new_origin)`,
     which evicts 32 slots returning `(world, slot)` pairs.
   - For each evicted pair: `dispatched_once.remove(slot)`, then later
     `window.free(slot)` puts the slot back on the free list.
   - Pass 2 builds the new `pending` list (32 world segments not yet bound).
   - Pass 3 (`residency.rs:396-407`): for each pending segment, calls
     `window.allocate()` (returns one of the just-evicted slots from the
     free list) + `window.bind(world_seg, slot)`. **`bind()` writes the
     indirection-table entry IMMEDIATELY** (`windowed_slot_map.rs:182-202`
     direct write to `self.indirection[pack(local_of(world_seg))]`).
   - Pass 4 (`process_pending_admissions`): picks 4 nearest from the
     newly-bound 32, pushes to `admissions_this_frame`.

2. **`ExtractSchedule::extract_streaming_state`** (`noise_dispatch.rs:351-440`):
   - Clones `admissions_this_frame` (4 segments) + `window.indirection_buffer()`
     (entire 512-entry table — INCLUDING the 32 freshly-rebound slots).
   - Mirror is now visible to the render world.

3. **`Render::Queue::upload_window_indirection`** (`noise_dispatch.rs:459-477`):
   - Calls `render_queue.write_buffer(buf, 0, ... indirection)`.
   - **The GPU `window_indirection_buffer` is now consistent with the
     post-shift binding for ALL 32 evicted-and-rebound slots.**

4. **`Core3d::PostProcess::naadf_gpu_producer_node`** (`mod.rs:3069-3402`):
   - Iterates `admissions_this_frame` (only 4 of the 32 newly-rebound
     slots).
   - For each, creates a per-segment encoder, calls
     `seg_encoder.clear_buffer(&world_gpu.chunks_buffer, slot_offset, ...)`
     to zero the slot's 32 KiB chunks region, then dispatches
     noise_terrain + chunk_calc + voxel_bounds + block_bounds.
   - `render_queue.submit([seg_encoder.finish()])` — encoder is committed.

5. **Renderer (subsequent nodes in PostProcess)**:
   - The renderer's `ray_tracing.wgsl::shoot_ray` reads `chunks[indirection[local]]`.
   - For the 4 admitted-this-frame slots, the new chunks_buffer content
     is correct.
   - **For the OTHER 28 evicted-and-rebound slots, the indirection points
     the new window-local position at a slot whose chunks_buffer region
     STILL CONTAINS THE EVICTED SEGMENT'S OLD CHUNK DATA.**
   - Rays at those window-local positions read OLD chunks_buffer data
     describing a DIFFERENT world segment (the one previously bound here
     before the shift).

This is the ghost-of-old-terrain pattern. Visible per-pixel as
inconsistent chunk content (the visible "dithered" or "granular" frame
in `streaming_window_mid_walk.png`).

### Phase 2.11's clear_buffer fix doesn't address this

Phase 2.11 added `seg_encoder.clear_buffer(...)` at the start of each
per-segment admission encoder (`mod.rs:3301-3305`). That clears the slot
ONLY when its admission encoder runs — i.e., when the admission appears
in `admissions_this_frame`.

A slot that is freshly-rebound but NOT in `admissions_this_frame` does
not get cleared this frame. It will get its clear+chunk_calc on a LATER
frame (one of the next 7 frames, at 4 admissions per frame).

### Confidence: HIGH

The mechanism is mechanically derivable from the code:
- `bind()` writes indirection synchronously (`windowed_slot_map.rs:188-218`).
- Per-segment encoder runs only for `admissions_this_frame` entries
  (`mod.rs:3138`).
- `admissions_this_frame.len() <= max_segments_per_frame = 4`
  (`residency.rs:444`).
- Therefore on a shift frame with 32 evictions, 28 slots have
  `bind()` complete but no encoder → indirection updated but chunks_buffer
  unchanged.

The `streaming_window_mid_walk.png` framebuffer evidence corroborates the
mechanism (granular per-pixel inconsistency during the walk = adjacent
chunks reading from different slots with stale content).

### Other candidate causes — refuted

- **chunk_calc's per-segment offset wrong on reads**: refuted. chunk_calc
  reads only from `segment_voxel_buffer` (per-admission scratch); no
  cross-slot reads. The slot-indexed write uses `streaming_chunk_index_cc(chunk_pos)`
  correctly (`chunk_calc.wgsl:466-469`).
- **`streaming_chunk_load` returns "uniform-empty" mid-build**: partially
  contributory but not the dominant bug. The `streaming_chunk_load`
  helper returns `vec2(0u, 0u)` for EMPTY_SLOT (`world_data.wgsl:230-236`),
  but the indirection-race scenario means slots are NOT EMPTY_SLOT —
  they're a valid slot index pointing at stale data.
- **W1 per-segment bounds has a bug**: refuted by `03n` Task A analysis
  + `03o` verification + `--validate-gpu-construction` gate continuing
  to pass byte-exact against the CPU oracle.

## Bug 2 — Pop-in (chunks appear only after camera approaches)

The pop-in is a direct consequence of `max_segments_per_frame = 4`.

- On a 1-segment camera crossing: 32 evictions + 32 admissions, processed
  4/frame → 8 frames for full re-population.
- On a faster camera (multi-segment crossing): admissions accumulate.

The user's expected behaviour ("chunks already visible from cold-start
residency") would require either:
- All 32 admissions to dispatch in ONE frame (would re-introduce the
  ~300 ms hitch Phase 2.10 fixed by spreading admissions across frames).
- A predictive prefetch (admit segments in the camera's direction-of-travel
  BEFORE the camera enters the crossing zone).
- A bigger `max_segments_per_frame` budget (= more hitch per shift but
  faster convergence).

The pop-in is partially the same root issue as Bug 1: a slot whose
indirection points at it is reading stale or zero chunks data until
its own admission encoder lands. The user perceives this as "chunks
appearing as the camera gets close" because near-camera slots are
admission-priority-sorted first.

### Confidence: HIGH

Mechanism is documented + measured: `STREAMING_MIN_MID_WALK_TERRAIN_RATIO = 0.30`
gate passes at 0.736 (Phase 2.10 boast) → 73.6% of mid-walk centre pixels
have non-sky content → 26.4% are sky-or-stale. That 26.4% IS the pop-in
gap.

## Bug 3 — Lag regression vs Phase 2.10

Phase 2.10 measured max-walk-frame at 22.0 ms in its gate output. Phase
2.11's same gate also reports 22.0 ms (today's run). The walk's per-frame
budget hasn't visibly regressed.

The user's "slightly slower" perception is probably driven by:
1. **Per-admission `clear_buffer` call** (`mod.rs:3301-3305`) — fires
   once per admission. Cost ≈ 50 us × 4 admissions = 200 us per frame.
2. **Per-admission `dispatched_once` HashSet membership tracking** —
   minimal but non-zero.
3. **`is_cold_start_complete()` predicate** — O(1) since-Rust-1.83
   `len()` on a HashSet. Cheap.
4. **`w3_reseed_full_world` cloned bookkeeping field** even when W3
   disabled — minimal (one bool clone per frame).

None of these should be user-perceptible at the per-frame scale. The
user's observation is plausibly attributable to the **W3 disable itself**:
- Phase 2.10 had W3 firing and producing 5-bit chunk AADFs that gave
  rays "skip up to 31 chunks" hints in empty space → 240 ray-steps
  could reach 240 × 31 × 16 = 119,040 voxels.
- Phase 2.11 has W3 disabled → AADFs zero → rays step 1 chunk at a time
  → 240 ray-steps reaches 240 × 1 × 16 = 3,840 voxels. Inside the
  4096-voxel window this is fine for direct vision, but each ray now
  takes MORE steps in the inner DDA loop to reach the same hit point.

If pixel-shader cost is dominated by inner DDA steps, **Phase 2.11
SHOULD be slightly slower per-pixel.** That matches the user's
perception.

### Confidence: MEDIUM-HIGH

The mechanism is correct; the magnitude is what's uncertain. The
streaming-window gate doesn't measure pixel-shader wall-clock directly
— it measures per-frame `Time::delta_secs()` which lumps Bevy update +
extract + GPU dispatch + presentation together.

## Punch-list for the next fix dispatch

Ordered. **Acknowledge: prior two fix attempts (Phase 2.10's per-segment
bounds + Phase 2.11's W3 disable + clear_buffer) have not closed the
user-visible bug. Confidence in the diagnosis matters more than
implementation speed.** If the orchestrator's next dispatch agent finds
contradicting evidence, the punch-list below is wrong and needs
reconsideration.

### MUST-1 — Stop indirection from pointing at uninitialised slots

Two equivalent shapes, pick one:

**Shape A: Deferred indirection upload**
- residency_driver still binds new segments synchronously (the main-world
  invariants need it for VRAM accounting).
- A NEW render-world flag (`PendingAdmissionsCount`) tracks how many
  of the just-shifted slots have a complete chunks_buffer.
- `upload_window_indirection` writes a STAGED indirection table that
  keeps the just-evicted indirection entries pointing at EMPTY_SLOT
  until the new admission's encoder completes.
- Tracked via a per-slot dirty flag updated by the producer node post-dispatch.

LOC estimate: ~80-120.

**Shape B: Immediate clear-on-bind**
- residency_driver's `bind()` path schedules a clear-buffer for the
  just-evicted slot region as part of an "eviction work queue."
- An EARLIER-in-frame render system processes that queue with a single
  command encoder calling `clear_buffer` on all evicted slot regions
  before the producer node runs.
- Trade: 32 KiB × 32 slots = 1 MiB clear per shift frame ≈ 200 us.
- Outcome: indirection points at NEW local positions, but slot data
  is freshly zeroed → renderer sees UNIFORM_EMPTY (correct sky) for
  the un-admitted-yet slots until their producer encoder runs.

LOC estimate: ~50-80. Simpler than Shape A.

**Recommended: Shape B.** It matches the user's expectation ("sky where
nothing's loaded yet" is better than "ghost of old terrain"). It also
preserves the smooth pop-in transition.

### MUST-2 — Decide the W3-on-streaming policy explicitly

Per `bevy-naadf-faithful-port-rule`: Phase 2.11's W3 disable is a
deliberate divergence from C# NAADF (which has W3 unconditionally on).
This requires:
- Explicit user approval (currently MISSING — `03o` "What's left" item
  2 flagged it).
- An entry in `naadf-bevy-port/12-alignment-gap.md` documenting the
  divergence.

Either:
- (a) **User approves the divergence**: codify it (move
  `PHASE_2_11_ENABLE_STREAMING_W3` from env var to config, document in
  alignment gap, KEEP the disable). Bug 3 (lag) is then a documented
  accepted cost.
- (b) **User rejects the divergence**: re-enable W3 by default, accept
  the 128 ms hitch on shift frames, redesign the per-admission scoped
  re-seed properly. Bug 3 disappears, Bug 1's W3-leakage component
  re-emerges as the dominant cause.

The orchestrator should NOT autonomously decide this. The user must
weigh "ghost-terrain at distance flicker" (option a residual Bug 1
mitigation via Shape-B clear) vs "128 ms hitches on segment crossings"
(option b cost).

LOC estimate: ~50 (either path).

### MUST-3 — Add a framebuffer-diff e2e gate

The parity gate as-built is tautological in the shipped configuration.
Replace OR augment with a framebuffer-diff gate:

- Boot the streaming preset at the canonical camera pose.
- Wait 128 frames (full cold-start drain at 4 admissions/frame).
- Capture framebuffer A.
- Boot the static preset at the SAME canonical camera pose with the
  same FnlState/seed/sea_level/amplitude.
- Capture framebuffer B.
- Compute SSIM(A, B) — assert > 0.7 (tunable; static and streaming have
  legitimate differences from per-segment vs full-world dispatch
  rounding, but the overall composition should match).

Implementation: cannot be done in one App run (Bevy isn't reliably
re-initialisable). Either:
- Run two harness invocations + compare PNGs in a post-process script.
- Or build a multi-pass harness: install preset A, capture, tear down
  the App, install preset B, capture, compare.

**Critically: the gate must check FRAMEBUFFERS, not internal buffers**
— per the `feedback-parity-gate-must-not-be-tautological` memory.

LOC estimate: ~150-250 (depending on harness shape).

### SHOULD-1 — Bigger admission budget on cold-start

Cold-start is a one-time event per session boot. Letting it dispatch
`max_segments_per_frame_cold_start = 32` (1 frame to fill the entire
window, at the cost of one ~300 ms boot hitch) is arguably better UX
than 128 frames of empty-looking screen.

- Adds a cold-start vs steady-state branch in residency_driver +
  producer node.
- LOC estimate: ~30.

### SHOULD-2 — Per-segment encoder ordering reconsideration

The current per-segment encoder submits 4× per frame (one per
admission). Coalescing them into ONE encoder (4 dispatches, single
submit) may reduce CPU-side overhead.

- LOC estimate: ~20.
- Speed-up: probably 0.5-1 ms per admission frame. Marginal.

## What the gate SHOULD have been

A faithful regression catcher for "streaming preset renders the same
geometry as static preset at the same camera pose with the same seed":

```
new_gate streaming-static-framebuffer-parity:
    1. Install static preset at canonical pose (Vec3(2048, 288, 2048),
       look-at Vec3(2148, 240, 2048)).
    2. Run 1 + 300 frames (1 dispatch + 300 wait for TAA + GI to settle).
    3. Capture framebuffer B.
    4. Tear down App.
    5. Install streaming preset at the SAME canonical pose, SAME seed,
       SAME sea_level, SAME amplitude.
    6. Run 128 frames (cold-start drain).
    7. Capture framebuffer A.
    8. Compute SSIM(A, B). Assert SSIM > 0.7.
    9. Compute per-pixel max-luminance-Δ. Assert max < 80.
    10. Compute mean-luminance-Δ. Assert mean < 15.

    Tolerance band rationale:
      - SSIM 0.7 — sky regions are bit-identical, terrain may shift
        due to dispatch-ordering effects (mixed-block atomic cursor seed
        offset, +64 vs +0).
      - max-luminance-Δ 80 — accommodates a single bright voxel
        rendering at slightly different brightness due to TAA history
        differences.
      - mean-luminance-Δ 15 — STRICT global bar. The corruption bug's
        granular dithered frames would show mean Δ in the 60-100 range
        — far above 15.
```

This gate CAN'T be silenced by disabling W3 (because the static preset
also doesn't use W3 — both presets produce framebuffers from the SAME
W1 + W4 chain; the framebuffers should match).

If the streaming preset's framebuffer regresses (e.g. via the
indirection-race bug above), the SSIM tanks, the gate FAILS. The gate
is independent of the chunks_buffer internals — it can't be sidestepped
by a fix that changes the construction-pipeline architecture.

**Spec hint for the dispatch agent**: the existing `e2e_render` binary
runs one App per invocation. Either (a) accept that this gate requires
TWO sequential invocations + an out-of-process PNG-diff script, or
(b) build a teardown-and-reinit harness inside one process. (a) is
substantially simpler.

## Honest uncertainties

- **The "beige slab" in `streaming_window_before.png` may be legitimate
  rendering of an inside-terrain view.** I cannot definitively call it
  corruption without a side-by-side static-vs-streaming framebuffer
  comparison at the EXACT cold-start spawn pose. The MUST-3 gate would
  resolve this.

- **The "granular dithering" in `streaming_window_mid_walk.png` is
  definitively corruption.** The pattern (per-pixel inconsistent voxel
  surface shading) cannot happen on a correct DDA traversal. This is
  the load-bearing evidence.

- **Bug 3 (lag) may not be a regression at all** — the user's "slightly
  slower" is subjective and the gate's `max_walk_frame = 22.0 ms` is
  the same as Phase 2.10. If the user re-measures after MUST-1 lands
  and the perception persists, the W3 disable trade-off (MUST-2) is the
  next thing to revisit.

- **MUST-1 Shape B may be insufficient** if the bug's mechanism extends
  beyond the indirection-race I diagnosed. If the user reports the bug
  still visible AFTER MUST-1 Shape B lands, the next investigation
  should snapshot `chunks_buffer + indirection` PER FRAME during the
  walk and replay the rendering CPU-side to identify which exact chunks
  are inconsistent.
