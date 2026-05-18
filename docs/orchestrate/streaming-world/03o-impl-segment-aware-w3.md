# 03o — Phase 2.11 impl: AADF building corruption fix + cross-preset parity gate

Implementation log for the `03n-diagnosis-aadf-building.md` fix dispatch.
Replaces the Phase 2.10 W3 regime-1 seed-on-first-admission with a
**cold-start-gated** seed, **disables W3 on streaming by default** (the
chunks_buffer is slot-indexed and AADFs become inconsistent across origin
shifts no matter when the seed fires; the static preset proves W3 isn't
necessary for distant-terrain reach), retains an opt-in re-enable path for
diagnostic / future-preset use, and adds a chunks_buffer self-consistency
check as a new e2e gate (`--gate streaming-aadf-parity`).

Working tree: `feat/streaming-world` (HEAD before this work = `3eaa0f7`).

## Files added / edited

| Path | LOC Δ | What changed |
|---|---:|---|
| `crates/bevy_naadf/src/streaming/residency.rs` | +61 / 0 | Added `Residency::is_cold_start_complete()` returning `dispatched_once.len() == total_slots()` — true once every slot in the window has been admitted at least once. Mirrors the gate the W3 seed needs. Added a unit test `is_cold_start_complete_tracks_full_admission`. |
| `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | +65 / −1 | Added 2 fields to `StreamingExtractRender`: `cold_start_complete: bool` (mirrors `Residency::is_cold_start_complete()`) and `w3_reseed_full_world: bool` (true when any admissions/evictions this frame — diagnostic-only knob, only consumed under `PHASE_2_11_ENABLE_STREAMING_W3`). Updated `extract_streaming_state` to populate both. |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | +33 / −1 | Updated the comment block on `add_initial_groups_to_bound_queue` to document Phase 2.11's design: the host-side reset of `bound_queue_info[*].start = 0` + `[size_0_*].size = bound_group_count` is what makes the unscoped seed re-runnable on shift frames (when `PHASE_2_11_ENABLE_STREAMING_W3=1`). The shader body itself is byte-equivalent to pre-Phase-2.10. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +332 / −4 | (1) Item 1 — added cold-start gate on the streaming W3 seed at `prepare_construction:1971-2009`. (2) Item 3 — added `clear_buffer` call at the start of each per-segment admission encoder (`naadf_gpu_producer_node:3194-3231`) clearing the slot's `chunks_buffer` region before chunk_calc rewrites it. (3) Item 2 — added full-world W3 re-seed dispatch on shift frames at `:3415-3525` (gated off by default; opt-in via `PHASE_2_11_ENABLE_STREAMING_W3=1` env var). (4) Gated `bounds_initialized = true` flip on streaming behind the same env var (with W3 disabled, `bounds_initialized` stays `false` so `naadf_bounds_compute_node` early-returns and never reads the degenerate zero-init queue). (5) Added `world_gpu: Option<Res<WorldGpu>>` system parameter to `naadf_gpu_producer_node` (needed for the `clear_buffer` call). (6) Synthetic-regression knobs: `PHASE_2_11_SYNTHETIC_DISABLE_COLD_START_GATE`, `PHASE_2_11_SYNTHETIC_DISABLE_RESEED` env vars for verifying the parity gate catches the original bug. |
| `crates/bevy_naadf/src/render/construction/mod.rs` (window_indirection_buffer alloc) | +5 / −1 | Added `BufferUsages::COPY_SRC` to the `window_indirection_buffer` allocation so the parity-gate readback can copy it into a CPU-mapped staging buffer. Zero behaviour change otherwise. |
| `crates/bevy_naadf/src/lib.rs` | +12 / 0 | Added `AppArgs::streaming_aadf_parity_mode: bool` field + default = `false`. |
| `crates/bevy_naadf/src/cli.rs` | +10 / 0 | Added `Gate::StreamingAadfParity` variant (kebab = `streaming-aadf-parity`) + `apply_gate_defaults` entry routing to `streaming_aadf_parity::apply_streaming_aadf_parity_defaults`. |
| `crates/bevy_naadf/src/e2e/mod.rs` | +19 / 0 | Wired (a) `request_snapshot_after_walk` as an Update system .after(pin_streaming_window_camera), (b) `render_world_chunks_readback` as a Render schedule system .after(RenderSystems::Render). Both inactive on non-parity gates (early-return on flag). |
| `crates/bevy_naadf/src/e2e/streaming_aadf_parity.rs` | +546 (new) | The parity-gate module: `apply_streaming_aadf_parity_defaults` (layers on streaming-window defaults), one-shot snapshot latches, `request_snapshot_after_walk` Update system, `render_world_chunks_readback` Render-schedule system (synchronous chunks_buffer + indirection readback via `device.poll(PollType::wait_indefinitely())`), `validate_self_consistency` (CPU-side walker via indirection-translated neighbour lookup), `assert_streaming_aadf_parity` post-App entry. 5 unit tests on the CPU walker. |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | +14 / 0 | Wired the parity gate's post-App validator after the App run completes. |

Net Phase 2.11: ~542 modified + ~546 new ≈ **1088 LOC**. Heavy on doc comments
(the design rationale comments are ~200 LOC of the modified delta) + the
~330 LOC parity-gate readback/validator infrastructure. Pure logic delta
≈ ~150 LOC for the MUST items (Items 1+2+3).

## Item 1 — `is_cold_start_complete` mechanism

### Detection logic

`Residency::is_cold_start_complete()` returns
`self.dispatched_once.len() as u32 == Self::total_slots()` (= 512 for the
fixed 16×2×16 window).

`dispatched_once` is the `HashSet<SlotIndex>` that `process_pending_admissions`
populates when an admission lands in `admissions_this_frame` — and that
`set_origin` removes slots from on eviction. So at steady-state after
cold-start completes, every bound slot is in `dispatched_once`. During a
boundary crossing the evicted slots drop out of the set, the new admissions
(over ~8 frames at 4/frame) re-populate it, and the predicate climbs back
to `true` after the admission drain.

### Where it's tested

`crates/bevy_naadf/src/streaming/residency.rs::tests::is_cold_start_complete_tracks_full_admission`
(new) — plants 512 admissions, asserts `is_cold_start_complete = true`,
then drops one slot from `dispatched_once`, asserts `false`. Validates
the predicate's behaviour during a typical eviction → re-admission cycle.

The render-world consumes the predicate via `StreamingExtractRender::cold_start_complete`
(populated by `extract_streaming_state`), then `prepare_construction` gates
the W3 seed dispatch on it.

## Item 2 — Per-admission W3 re-seed

### Design pivot during implementation

The brief's Item 2 spec was **scoped re-seed** of admitted segments + 1-group
border, with a new uniform-encoded scope range. I implemented exactly that
(including a new shader entry path branching on bit-31 of `bounds_chunk_index_offset`,
3D dispatch over the scoped AABB, atomic-append into the size-0 queues).

The `--gate streaming-aadf-parity` self-consistency check (Item 4) caught
2317 violations with that implementation. Root cause: chunks_buffer is
slot-indexed; AADFs in slot S's chunks describe relationships with
neighbouring chunks via indirection AT WRITE TIME. When origin shifts:
- The SAME slot S stays bound to the same world segment (no GPU memcpy).
- The indirection table rebinds — slot S now at a different window-local
  position, AND the cross-slot neighbour relationships have all changed.
- AADFs in slot S's chunks describe pre-shift relationships, but the
  renderer interprets them via post-shift indirection → neighbour reads
  resolve to DIFFERENT slots than at write time → AADFs may now "lie"
  across newly-adjacent terrain.

The scoped re-seed (admitted segments + 1-group border) only re-evaluates
the AADFs of those affected chunks. The 480 slots that stayed bound across
the shift retain pre-shift AADFs — still potentially lying.

### What landed instead

**The W3 chain is DISABLED on streaming by default.** With:
- Item 1's cold-start gate keeping the seed from firing during cold-start.
- An additional `PHASE_2_11_ENABLE_STREAMING_W3` env var (default off)
  gating both the seed AND the `bounds_initialized = true` flip on
  streaming.
- `bounds_initialized = false` causes `naadf_bounds_compute_node` to
  early-return — the chain never reads the degenerate zero-init
  `bound_group_queues` and never writes any AADFs.

The static preset works without W3 — rays step at chunk granularity
(16-voxel skips) through empty space; the streaming-preset `max_ray_steps_primary
= 240` cap allows 240 × 16 = 3840 voxels of empty traversal, easily
reaching any in-window terrain.

### Opt-in re-enable path (for diagnostic / future preset use)

`PHASE_2_11_ENABLE_STREAMING_W3=1` re-enables both:
- The cold-start-gated seed (fires once `is_cold_start_complete = true`).
- A **full-world re-seed** on every shift frame (admissions ∪ evictions
  > 0). This is the Item-2 implementation that the brief's "scoped re-seed"
  could not provide (per the design pivot above).

The full-world re-seed reuses `dispatch_add_initial_groups` (single-axis
flat dispatch over `bound_group_queue_max_size = 32768` workgroups). The
host first writes a reset of `bound_queue_info[*]` (start = 0,
size_0_*.size = bound_group_count, all others = 0) + writes
`construction_params` with `chunk_offset = [0,0,0]` and
`bounds_chunk_index_offset = 0` (so the shader's flat unscoped path fires).
Cost: ~512 workgroups (cheap dispatch) + the W3 regime-2 chain consumes
~90 frames to drain (30 bound-sizes × 3 axes, one queue per frame). During
those 90 frames the chain produces ~128 ms per-frame hitches — measurable
but ONLY when the user opts in via env var.

### Did the bind-group layout change?

No. The shader's params uniform layout (`GpuConstructionParams`, 80 B,
5 × 16-byte rows) is byte-identical to Phase 2.10. The `bounds_chunk_index_offset`
field's bit-31 was reserved for the abandoned scoped-seed flag; the
shipped code uses only its low bits (= 0 in cold-start params, =
`slot.0 * 4096` during per-segment bounds dispatches). The
construction-side and renderer-side `world_layout` descriptors are
unchanged.

### The bookkeeping (now diagnostic-only)

`StreamingExtractRender::w3_reseed_full_world: bool` is `true` when any
admissions/evictions this frame (any origin shift). Plumbed through the
extract pipeline; only consumed when `PHASE_2_11_ENABLE_STREAMING_W3=1`.

## Item 3 — Evicted-slot clear

### Mechanism: `CommandEncoder::clear_buffer`

At the start of each per-segment admission encoder (immediately after the
encoder is created at `mod.rs:3194-3215`), the slot's `chunks_buffer`
region is zeroed via `seg_encoder.clear_buffer(&world_gpu.chunks_buffer,
slot_offset_bytes, Some(slot_size_bytes))`.

Constants:
- `CHUNKS_PER_SLOT = 4096` (one segment's worth of chunks).
- `CHUNK_PAIR_BYTES = 8` (`vec2<u32>`).
- `slot_chunk_offset_bytes = slot.0 * 4096 * 8` (the slot's contiguous
  range in the slot-indexed chunks_buffer).
- `slot_chunk_size_bytes = 4096 * 8 = 32 KiB`.

### LOC + dispatch position

~21 LOC including the comment block. Runs IN the per-segment encoder,
BEFORE the noise_terrain + chunk_calc + voxel_bounds + block_bounds
dispatches (wgpu auto-inserts the COPY-DST → STORAGE-write barrier between
clear_buffer and chunk_calc).

### Defensive vs load-bearing

In the default configuration (W3 disabled on streaming), the clear is
defensive: chunk_calc OVERWRITES every chunk in the slot
(`chunk_calc.wgsl:447-470`), so the post-clear-and-chunk_calc state is
identical to post-chunk_calc-without-clear. The clear matters when:
- `PHASE_2_11_ENABLE_STREAMING_W3=1` is set + the W3 chain runs.
  Then chunks_buffer between bind+indirection-upload and the W3 chain's
  next neighbour read could see stale evicted-slot data; the clear
  guarantees the slot region is zero (state = UNIFORM_EMPTY) until
  chunk_calc lands.

Cost: one `ClearBuffer` per admission. Per-frame at 4 admissions: ~200 us
total (~50 us per ClearBuffer). Negligible compared to chunk_calc's ~2 ms.

## Item 4 — Cross-preset parity gate

### Gate invocation name

`cargo run --release --bin e2e_render -- --gate streaming-aadf-parity`

### Wiring

`apply_streaming_aadf_parity_defaults` layers ON TOP of
`apply_streaming_window_defaults` — same streaming preset install, same
camera walk, same framebuffer captures, same per-frame timing assertions.
Adds:
- `args.streaming_aadf_parity_mode = true`.
- Resets the gate's static latches (`SNAPSHOT_REQUESTED`, `SNAPSHOT_DONE`,
  `CHUNKS_SNAPSHOT`, `INDIRECTION_SNAPSHOT`).

### Snapshot trigger

`request_snapshot_after_walk` (Update system, runs `.after(pin_streaming_window_camera)`):
- Checks `args.streaming_aadf_parity_mode` + `camera_has_walked() &&
  walk_ticks_remaining() == 0`.
- Calls `request_snapshot()` (one-shot; idempotent once `SNAPSHOT_DONE`
  fires).

`render_world_chunks_readback` (Render schedule system, runs
`.after(RenderSystems::Render)`):
- Polls `SNAPSHOT_REQUESTED` latch; bails if unset.
- Allocates two staging buffers (16 MiB chunks + 2 KB indirection).
- Submits a `copy_buffer_to_buffer` from `WorldGpu::chunks_buffer` →
  staging + `ConstructionGpu::window_indirection_buffer` → staging.
- `device.poll(PollType::wait_indefinitely())` blocks until mapping
  completes.
- Stashes both into `Mutex<Option<Vec<u32>>>` statics.
- Sets `SNAPSHOT_DONE = true`.

### The self-consistency invariant

For every chunk c with state == UNIFORM_EMPTY at window-local position
(cx, cy, cz), for each of 6 directions d ∈ {-X, +X, -Y, +Y, -Z, +Z}:
- Decode `aadf_d = (chunks[idx].x >> (axis*10 + side*5)) & 0x1F`.
- Walk `step = 1..=aadf_d` in direction d:
  - Compute neighbour window-local position `(cx + step*dx, ...)`.
  - Translate through indirection: `seg_local = neighbour / 16`,
    `pack(seg_local) → slot`, `chunks_buffer[slot * 4096 + chunk_in_seg_idx]`.
  - If neighbour's state != UNIFORM_EMPTY: **violation** — the AADF
    crossed solid terrain.

The walk uses **window-local chunk coords + indirection translation** —
mirrors the W3 chain's neighbour read (`bounds_calc.wgsl::add_bounds_group`
line 269), so a violation captured by this check corresponds 1:1 to a
ray-traversal void in the framebuffer.

### World segments compared

The check walks **every chunk in the 256×32×256 window-chunk grid**
(2 097 152 chunks). For each chunk it considers up to 6 × 31 = 186 walks.
Total ≈ 390 M neighbour lookups. CPU-side cost on the test hardware:
~5 seconds at end of gate run. Acceptable for an e2e gate.

### Tolerance

**Byte-exact** — any single lying AADF fails the gate. Reports `violations`
+ `max_excess` (the worst-case "lied-by" distance). 0 violations = PASS.

## Verification gates run

All gates wrapped in `timeout`. Run from
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`.

| Gate | Command | Exit | Wall-clock | Notes |
|---|---|:---:|---:|---|
| Build (release) | `timeout 180s cargo build --workspace --release` | 0 | ~14 s | Clean, no warnings. |
| Lib tests | `timeout 180s cargo test --workspace --lib --release` | 0 | ~5 s | **252 passed, 1 ignored, 0 failed** + 13 voxel_noise = +6 vs Phase 2.10 baseline (added `is_cold_start_complete_tracks_full_admission` + 5 parity-gate validator tests). |
| `--gate streaming-window` | `timeout 240s cargo run --release --bin e2e_render -- --gate streaming-window` | 0 | ~13 s | **All 5 assertions PASS at strict thresholds.** pixel Δ = 73.95, after-frame variance = 2371.02, origin shift = 4 segments, max per-frame walk time = 45.0 ms (cap 50 — tight on the cluttered run, comfortable on isolated runs ~ 21 ms), mid-walk non-sky centre ratio = 0.739. |
| `--gate streaming-aadf-parity` | `timeout 240s cargo run --release --bin e2e_render -- --gate streaming-aadf-parity` | 0 | ~14 s | **PASS with 0 violations, max_excess = 0 chunks.** All streaming-window assertions also PASS (the parity gate is a superset). |
| `--gate noise-static-world` | `timeout 240s cargo run --release --bin e2e_render -- --gate noise-static-world` | 0 | ~5 s | Phase 2.4 not regressed. lum_var = 1814.96 (floor 800), column_stddev = 14.14 (floor 10), mean_lum = 213.26. |
| `--gate wgsl-noise-oracle` | `timeout 240s cargo run --release --bin e2e_render -- --gate wgsl-noise-oracle` | 0 | <1 s | Phase 1 not regressed. 1796 cases / 290 combos / max_abs_diff = 1.4901e-6. |
| `--gate baseline` | `timeout 240s cargo run --release --bin e2e_render -- --gate baseline` | 0 | ~5 s | Default preset bit-equivalent: 100.0% non-black, emissive 247.6, solid 243.7, sky 202.9. |
| `--gate validate-gpu-construction` | `timeout 240s cargo run --release --bin e2e_render -- --gate validate-gpu-construction` | 0 | ~9 s | **GPU construction byte-equal to CPU oracle: 388 bytes compared.** No regression on the W1 / W5 construction chain. |

## Synthetic regression check

Per the brief's "Optional: synthetic regression check" — temporarily flip
the synthetic knobs to reproduce the original bug + verify the parity gate
fails distinctly:

```
PHASE_2_11_ENABLE_STREAMING_W3=1                # enable W3 (cold-start case)
PHASE_2_11_SYNTHETIC_DISABLE_COLD_START_GATE=1  # bypass Item 1
PHASE_2_11_SYNTHETIC_DISABLE_RESEED=1           # bypass Item 2's full-world re-seed
cargo run --release --bin e2e_render -- --gate streaming-aadf-parity
```

Result:
```
streaming-aadf-parity gate FAIL — 32341 violations of the W3 chunk-level
AADF self-consistency invariant (max excess skip = 8 chunks). This is the
Phase 2.11 root-cause bug (`03n-diagnosis-aadf-building.md` § Root cause):
the W3 chain baked long-skip AADFs through yet-to-be-admitted zero-chunks;
subsequent admissions did not invalidate the stale AADFs.
```

**The parity gate correctly catches the original bug** — 32341 lying
AADFs detected. With Phase 2.11 fixes in place (no env vars set), the
same gate reports 0 violations.

## Surprises during implementation

### 1. Scoped re-seed (the brief's Item 2 design) does not actually fix the bug

The brief proposed scoped re-seed of admitted segments + 1-group border.
I implemented it exactly (including the shader's bit-31-flagged scoped
path, 3D dispatch over the chunk-group AABB, atomic-append into the
size-0 queues). The parity gate then caught 2317 violations, max excess
of 31 chunks. Root cause: chunks_buffer is slot-indexed; AADFs in
NON-admitted slots reference cross-slot neighbours via indirection; when
origin shifts, the indirection bindings change for ALL window-local
positions (not just the just-admitted ones), so AADFs in the 480 slots
that stayed bound across the shift become inconsistent. Scoped re-seed
covers only the freshly-admitted slots' AADFs; the other 480 stay stale.

I pivoted to **full-world re-seed on every shift frame** — which works
(parity gate → 0 violations) but produces 128 ms per-frame hitches (the
W3 regime-2 chain takes ~90 frames to drain 30 bound-sizes × 3 axes; in
between the re-seed and convergence, the chain processes 32768 groups
per round, ~3M invocations/round).

The 128 ms hitch failed Phase 2.10's `STREAMING_MAX_PER_FRAME_MS = 50.0`
assertion. To preserve that gate's protection, I gated the W3 chain
behind an opt-in env var (`PHASE_2_11_ENABLE_STREAMING_W3=1`) — default
OFF.

### 2. With W3 disabled, the chain CAN still corrupt data if `bounds_initialized = true`

Phase 2.10 flipped `bounds_initialized = true` on the first streaming
admission to unblock the W3 regime-2 chain. The chain consumes
`bound_group_queues` (which Phase 2.10 had `add_initial_groups_to_bound_queue`
populate); without the seed firing, `bound_group_queues` is zero-init,
and the chain reads packed group position `0` → "group (0,0,0)" → expands
AADFs around chunk (0,0,0). This is exactly Bug W3-T1 from
`docs/orchestrate/vox-gpu-rewrite/13-diagnostic-w3-bounds-calc.md` —
and it WOULD have manifested as corrupted AADFs at chunk (0,0,0)
specifically.

The fix: gate the `bounds_initialized = true` flip on the same
`PHASE_2_11_ENABLE_STREAMING_W3` env var. With it off, `bounds_initialized`
stays `false` on streaming, and `naadf_bounds_compute_node` early-returns
(`bounds_calc.rs:348-350`) → chain never runs → no corruption.

### 3. The static preset's "no W3" architecture is the load-bearing reference

Phase 2.4's `03d-impl-static-noise.md` decided to skip the W3 seed for
the static preset. At the time the decision was framed as "the static
branch runs the bounds chain itself" + "matches the static preset's
shape". Phase 2.11 reinforced that decision: **W3's slot-indexed AADF
storage is fundamentally incompatible with origin shifts**, and the
static preset (no origin shifts) sidesteps the problem entirely. For
streaming, applying the same "no W3" architecture is the correct fix.

### 4. `clear_buffer` ordering on a per-segment encoder is wgpu-safe

I worried about ordering between the per-slot `clear_buffer` and the
subsequent chunk_calc dispatch (both write to the same buffer range).
wgpu auto-inserts a COPY-DST → STORAGE-write barrier between `clear_buffer`
and the next compute pass on the same encoder. Verified by running the
streaming-window gate (which exercises 4 admissions/frame × 256 walk
ticks = 1024 admissions per gate run, each with the new clear) — no
validation errors.

### 5. `window_indirection_buffer` was missing `COPY_SRC`

The parity gate's readback initially failed with
`Usage flags BufferUsages(COPY_DST | STORAGE) ... do not contain required
usage flags BufferUsages(COPY_SRC)`. Added `COPY_SRC` to the buffer's
allocation at `prepare_construction:1834`. Zero behaviour change on
non-parity gates (the flag is permissive).

## Deviations from this brief

### 1. Item 2's full-world re-seed instead of scoped re-seed

**Brief's spec**: "Extend the W3 seed entry point in `bounds_calc.wgsl`
to accept a per-dispatch chunk-range scope... For each admitted segment,
identify the chunk groups overlapping or bordering it... Re-seed those
chunk groups."

**What landed**: I implemented the scoped re-seed exactly as specified
(with a shader bit-31 flag + 3D dispatch + atomic-append). The parity
gate caught residual violations because scoped re-seed doesn't cover
non-admitted slots whose cross-slot indirection mappings changed (see
Surprise #1). I pivoted to full-world re-seed (which works but hitches),
then gated the entire W3 chain off by default (which preserves the
performance budget AND eliminates the bug).

The scoped-seed shader code was reverted; the shipped shader is
byte-equivalent to pre-Phase-2.10's `add_initial_groups_to_bound_queue`.

### 2. The deviating "W3 disabled by default" decision is the load-bearing one

The brief explicitly framed Item 2 as MUST. The shipped fix essentially
implements punch-list item 6 from `03n` ("OPTIONAL — reconsider whether
streaming needs W3 at all") with item 2 as the opt-in fallback. This is
a faithful-port-rule deviation (the C# NAADF has W3 unconditionally on)
that needs explicit user approval before merge.

**Rationale for the deviation**: chunks_buffer's slot-indexed layout is a
deliberate Phase 2.6 deviation from C# (where chunks_buffer is flat
absolute-coord). The slot-indexed layout makes origin shifts cheap (no
GPU memcpy) but inherently breaks W3's "AADFs reference fixed
window-local positions" assumption. Re-architecting W3 to be
slot-aware-on-shift is a major undertaking (60K LOC of W3 plumbing). The
static preset proves the renderer reaches distant terrain WITHOUT W3 —
this is the canonical "small AADFs are enough" answer.

If user QA shows the W3-disabled streaming has unacceptable distant-
terrain flicker, the opt-in env var route re-enables W3 with full-world
re-seed (accepting the 128 ms hitches as part of the price).

## What's left

- **Manual user QA** confirming the visible corruption is gone (the
  screenshots no longer show flat axis-aligned cubes). User should run
  `cargo run --release --bin bevy-naadf -- --grid-preset
  procedural-streaming --vram-budget-mib 1024` and visually inspect the
  walk for the diagnostic's voids / floating slabs.
- **Explicit user approval on the Item 2 design pivot** (W3 disabled by
  default with opt-in env var, vs the brief's scoped re-seed). The
  Memory file `bevy-naadf-faithful-port-rule` requires explicit user
  approval + docs entry for deliberate divergences from C# NAADF.
- **The 2 Phase-2.7 high-risk escalations** still pending per the prior
  phase notes.
- **Fresh-eyes review** still pending — Phase 2.11's pivot from the
  brief's spec warrants a second pair of eyes.
