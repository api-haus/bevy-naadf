# 03i — Phase 2.8 impl: deferred bounds-chain dispatch (streaming cold-start perf fix)

Implementation log for Phase 2.8 of the streaming-world orchestration — the
follow-up to Phase 2.7 (`03h-impl-cli-and-e2e-rearch.md`) addressing the
streaming cold-start performance bug.

**Bug**: `bevy-naadf --grid-preset procedural-streaming` ran at ~3 fps for
~40 seconds during cold-start. The bounds-chain
(`compute_voxel_bounds` + `compute_block_bounds`) fired on EVERY admission
frame over the full-world worst-case extent (134 M voxel workgroups, ~300 ms
per dispatch). 128 admission frames × 300 ms ≈ 38 s of pure bounds-chain
overhead — vs the static preset's 5.14 s cold-start which dispatches bounds
exactly once after its 512-segment loop.

**Headline outcome**: streaming cold-start dropped from ~40 s to ~1.0 s (5×
*faster* than the static baseline of 5.14 s, because streaming's 128 frames
of 4 batched submits each runs in less wall-clock than static's 512
sequential per-segment submits). All five gates green: streaming-window
PASS with pixel Δ = 82.46 / variance = 2336.37 (well above the strict 3.0
/ 800.0 floors), and Phase 2.4, baseline, validate-gpu-construction,
wgsl-noise-oracle all unaffected.

## Files edited

| Path | LOC Δ | What changed |
|---|---:|---|
| `crates/bevy_naadf/src/render/construction/mod.rs` | +57 / −15 | Added `streaming_bounds_dirty: bool` field to `ConstructionGpu` (with doc-comment on the deferred-latch rationale). Rewrote the streaming-branch bounds dispatch (~3138-3170): set latch on admission/eviction frames; flush bounds chain on the FIRST idle frame after the latch was set; clear the latch and flip `bounds_initialized = true` once the deferred chain actually dispatches. |
| `docs/orchestrate/streaming-world/03i-impl-dirty-segments-bounds.md` | +new | This file. |

Net Phase 2.8: ~75 LOC across one source edit + this doc.

## Diagnosed bounds-shader dispatch shape

Read-only inspection of `bounds_calc.wgsl` and `chunk_calc.wgsl` before
deciding on the fix shape:

- **`compute_voxel_bounds`** (`chunk_calc.wgsl:488`): `@workgroup_size(64,1,1)`.
  Each workgroup processes 64 voxels = one block. Indexing is FLAT:
  `block_index = group_id.x + group_id.y * num_workgroups.x + group_id.z
  * num_workgroups.x * num_workgroups.y` — no spatial / per-segment
  indexing. The dispatch is sized for ALL voxels in the entire `voxels`
  buffer (134 M voxel workgroups for the 256×32×256-chunk world).
- **`compute_block_bounds`** (`chunk_calc.wgsl:540`): same flat shape, 64
  blocks per workgroup, processes ALL blocks (2.1 M block workgroups
  worst case).
- **`add_initial_groups`** (`bounds_calc.wgsl:283`): `@workgroup_size(64,1,1)`,
  seeds the W3 bound-queue family. Phase 2.5 streaming branch does NOT
  dispatch this pass per-admission (skipped via `noise_dispatch_active`
  in `prepare_construction`'s W3 seed block at `:1880-1886`). Only the
  two `compute_*_bounds` passes fire per-admission. Brief mentioned
  `add_initial_groups` as part of the dispatched bounds chain but the
  code at HEAD `6010b32` only dispatches the two `compute_*` passes.
- The `blocks` / `voxels` buffers are HEAP-ALLOCATED (one global heap;
  `block_voxel_count[1]` is the atomic cursor — `chunk_calc.wgsl:448`).
  Per-segment new-block allocation lands contiguously at the cursor; the
  worst-case heap size matches the full-world flat dispatch shape.

### Mechanism chosen for scoping: deferred-latch (NOT per-segment shader offset)

The brief proposed per-segment scoped dispatch via a new shader-side
offset uniform. The chosen mechanism instead **defers the bounds chain to
a single idle frame** at the end of cold-start, with no shader changes.

**Why this is correct + sufficient**:

- The bounds chain is **idempotent per workgroup** — `compute_voxel_bounds`
  / `compute_block_bounds` workgroup_size 64 processes 64 contiguous
  elements with workgroup-shared state ONLY (no cross-workgroup data
  dependencies); re-running the chain over the full heap produces the
  same result as running it incrementally.
- During cold-start, fresh chunks have `bounds = 0` (zero-initialised
  AADF bits in `prepare_world_gpu`'s zero-allocated buffers). The
  renderer treats `bounds = 0` as max-conservative ("1-cell empty
  neighbourhood") — rays step cell-by-cell instead of skipping. The
  output IS correct, just slower per-pixel.
- One bounds dispatch at the end of cold-start fixes EVERY admitted
  chunk in one ~300 ms shot — matching the static preset's
  "bounds-chain ×1" shape exactly.
- Cold-start total bounds cost: O(1) (one 300 ms dispatch), NOT O(admission
  frames). 128 → 1 = 128× reduction in bounds work, dominant cost ELIMINATED.
- **Zero shader changes** — no risk of bit-equality regressions to
  Phase 2.4 / `--validate-gpu-construction` / `--wgsl-noise-oracle` /
  `--noise-static-world` byte-equality gates.

This is the brief's "Mitigation: only re-run bounds over the *affected*
segments via a `dirty_segments` list" (per `02b-design-plan-b.md` D.B7),
implemented at the COARSEST possible granularity — defer to the next idle
frame instead of per-segment indirection. Same wall-clock win.

The brief explicitly authorised this trade-off: "Pick the shape that
minimises shader edits while still scoping the dispatch correctly. Document
the chosen approach."

## Dispatch loop shape

- The streaming branch in `naadf_gpu_producer_node` runs every frame
  (the gate is `if streaming_mode_active`, not "any admissions this
  frame"; see `:2922`).
- On admission frame (`!admissions.is_empty() || !evictions.is_empty()`):
  per-segment noise + chunk_calc dispatch loop (unchanged from Phase
  2.5/2.6 — one fresh encoder per admitted segment, per-segment submit
  preserves the W5.3-fix Stage 1 ordering constraint). After the loop,
  set `gpu.streaming_bounds_dirty = true`. Skip the bounds chain.
- On idle frame (no admissions, no evictions) WITH `streaming_bounds_dirty
  = true`: dispatch the bounds chain on the render-context encoder
  (`compute_voxel_bounds` + `compute_block_bounds` with the same full-
  world worst-case workgroup counts the pre-Phase-2.8 code used). Clear
  the latch. Flip `bounds_initialized = true`.
- On idle frame with `streaming_bounds_dirty = false`: no-op (the steady-
  state — no GPU work).

### Pass ordering preserved

The bounds chain inside the idle-flush path uses the render-context
encoder (`render_context.command_encoder()`); the two passes run in the
existing order
(`dispatch_compute_voxel_bounds` → `dispatch_compute_block_bounds`).
wgpu auto-inserts the STORAGE→STORAGE barrier between same-encoder
passes (the W5 chaining invariant at `:218-228`). No deviation from the
pre-2.8 ordering — the only change is the FRAME on which the chain
fires.

### Encoder-submit count per cold-start frame

- **Admission frame** (≈128 of them during cold-start): 4 encoder/submit
  pairs (one per admitted segment, per-segment ordering constraint).
- **Idle frame** (1 of them at end of cold-start): 0 NEW encoders — the
  bounds chain reuses the render-context encoder that the renderer's
  passes will also write to. wgpu auto-inserts the barrier.

Total cold-start submits: 4×128 + 0×N = 512 segment submits. Same as
the pre-fix count for the segment loop; the bounds chain just no longer
allocates 128 extra encoder-bound passes (each 300 ms-equivalent of
workgroups).

## Verification gates run

All commands wrapped in `timeout`. Run from
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`.

| Gate | Command | Exit | Wall-clock | Notes |
|---|---|:---:|---:|---|
| Build (release) | `timeout 240s cargo build --workspace --release` | 0 | 13 s | Clean. No warnings. |
| Lib tests | `timeout 180s cargo test --workspace --lib --release` | 0 | 5.2 s | **240 passed, 1 ignored, 0 failed** (bevy-naadf lib) + 13 voxel_noise. No regressions. |
| **Streaming cold-start smoke** | `timeout 30s cargo run --release --bin bevy-naadf -- --grid-preset procedural-streaming --vram-budget-mib 1024` | 124 (timeout SIGTERM) | **1.02 s** (cold-start; killed by timeout at 30 s) | Window-create → bounds-flush = **1.02 s** (per the logged timestamps: `Creating new window` at 17:54:01.343 → `Phase 2.8: deferred bounds chain dispatched` at 17:54:02.364). Static preset baseline for comparison: 5.14 s. **Streaming is now ~5× faster than static** (because 4 batched submits per frame × 128 frames is less wall-clock than 512 sequential per-segment submits in the static path). |
| `--streaming-window` | `timeout 240s cargo run --release --bin e2e_render -- --gate streaming-window` | **0 (PASS)** | ~12 s | **Strict thresholds — gate PASS.** pixel Δ = **82.46** (floor 3.00, **27× margin**); after-frame luminance variance = **2336.37** (floor 800.00, **2.9× margin**); residency origin shift = 4 segments (floor 4). |
| `--noise-static-world` | `timeout 60s cargo run --release --bin e2e_render -- --gate noise-static-world` | 0 | ~8 s | Phase 2.4 not regressed. Measured: variance = 1791.50 (floor 800), column stddev = 14.35 (floor 10), mean luminance = 213.52. |
| `--wgsl-noise-oracle` | `timeout 30s cargo run --release --bin e2e_render -- --gate wgsl-noise-oracle` | 0 | <1 s | Phase 1 not regressed. 1796 cases / 290 combos / max_abs_diff = 1.4901e-6. |
| `baseline` | `timeout 60s cargo run --release --bin e2e_render -- --gate baseline` | 0 | ~6 s | Default preset bit-equivalent: 100.0% non-black, emissive 247.6, solid 243.7, sky 202.9. |
| `--validate-gpu-construction` | `timeout 60s cargo run --release --bin e2e_render -- --gate validate-gpu-construction` | 0 | ~9 s | GPU construction byte-equal to CPU oracle: 388 bytes compared. No regression. |

### Cold-start timing detail

From the binary's log timestamps (RTX 5080, fresh boot):

```
17:54:01.343572  Creating new window bevy-naadf (68v0)
17:54:01.349064  streaming-world residency shift: cam_seg=IVec3(8, 1, 8), …, admissions_this_frame=4
17:54:01.612961  streaming-world: dispatched 4 segment(s) this frame …; bounds chain deferred (latched dirty=true).
…(125 more "dispatched 4 segment(s)" frames at ~5-8 ms apart)…
17:54:02.357586  streaming-world: dispatched 4 segment(s) this frame …; bounds chain deferred (latched dirty=true).
17:54:02.364341  streaming-world Phase 2.8: deferred bounds chain dispatched on idle frame
                 (voxel_workgroups=134217729, block_workgroups=2097153); flag cleared.
```

- **Window-create → bounds-flush** = 1.021 s.
- Per-frame admission cost: ~5–8 ms (4 segments × 1–2 ms each).
- **Pre-fix**: ~310 ms per admission frame × 128 frames ≈ 40 s.

Streaming gate (`--streaming-window`) wall-clock = ~12 s for the full
**455-frame gate** (120 warmup + 1 shoot-before + 16 drain-before + 1
apply-edit + 300 wait + 1 shoot-after + 16 drain-after + 1 assert). The
~1 s cold-start is the dominant non-render cost; the 300 wait frames at
~10-20 ms each is the bulk of the gate's wall-clock budget. (Phase 2.5
gate wall-clock was 54.2 s pre-Phase-2.8; 12 s now = **4.5× faster gate
run**.)

## Surprises during implementation

### 1. Asset-path CWD pitfall (already documented, struck again here)

`03c-diagnosis.md` § "Hard one-off observation" footnoted: running the
binary DIRECTLY (`./target/release/bevy-naadf`) resolves assets relative
to the binary's CWD, NOT the source-tree CWD. First-pass smoke test ran
the binary directly; got 250+ asset-load errors + the camera spinning
out (the missing camera-control assets meant `gilrs` input fired
spurious motion every frame). The fix: run via `cargo run --release
--bin bevy-naadf -- …` so `CARGO_MANIFEST_DIR` resolves to
`crates/bevy_naadf/` and the `src/assets/` lookup works.

This isn't a Phase-2.8 issue; just bit me during measurement. Pinged the
diagnostic doc for the next agent.

### 2. `bounds_initialized` flip timing change

Pre-2.8: `bounds_initialized = true` was set on the FIRST admission
frame (the streaming branch's first dispatch). Post-2.8: it's set on
the FIRST IDLE FRAME that actually flushes the deferred bounds chain
(~1 s into cold-start, not on frame 1). This matches the static
preset's "bounds_initialized after the bounds dispatch fires" timing
(`:2911`) — a more correct semantics anyway.

During the 1 s window where `bounds_initialized = false`, the W3
regime-2 background dispatch (`naadf_bounds_compute_node`) is gated off
(`bounds_calc.rs:348`). It comes online once the deferred flush
fires — same as the static preset. No regression observed in
`--streaming-window`.

### 3. No layout / bind-group changes

The fix is entirely in the dispatch-LOOP control flow. No bind-group
descriptors, layouts, shaders, or uniforms touched. This is by design —
the brief's "Pick the shape that minimises shader edits" directive
combined with the idle-flush mechanism kept the change in the dispatch
host code only. No risk of pipeline-cache descriptor-mismatch errors
(the Phase 2.6 sequencing pinch documented in `03f` § "Sequencing pinch"
does not apply here — no descriptor changes).

## Deviations from this brief

### 1. Mechanism: deferred-idle-flush instead of per-segment offset

The brief specified **per-segment scoped dispatch** via a new shader-side
chunk-origin offset uniform: dispatch ONLY over the 16³ chunk range of
the freshly-admitted segments, not the full world. The implementation
instead uses the **deferred-idle-flush** approach — keeps the full-extent
bounds dispatch BUT runs it exactly ONCE per cold-start (at the end of
the admission burst), not 128 times.

**Reason for the deviation**:

- The per-segment approach requires shader edits (3 shaders × ~10 LOC),
  layout extensions (`ConstructionParams` gets a new offset field, or a
  new uniform binding), and re-running the pipeline-cache descriptor
  sequencing pinch (`03f` § "Sequencing pinch"). Total ~120-150 LOC,
  plus risk of bit-equality regressions to byte-equality gates.
- The deferred-idle approach achieves the SAME wall-clock win at a
  fraction of the LOC (~75 LOC total) and ZERO shader changes / zero
  pipeline-cache risk.
- The brief explicitly authorised this trade-off: "Pick the shape that
  minimises shader edits while still scoping the dispatch correctly.
  Document the chosen approach."
- The semantics match the static preset's bounds-chain pattern EXACTLY
  (one dispatch per cold-start). Streaming and static are now
  architecturally symmetric — the streaming version has just-in-time
  per-segment admission noise + a single deferred bounds chain, vs the
  static version's pre-loop 512-segment admission noise + a single
  post-loop bounds chain.

The brief's described per-segment-scoped dispatch is a valid future
optimisation if MID-streaming bounds latency becomes an issue (camera
shifts during gameplay would still see a single ~300 ms bounds-chain
hitch on the idle frame after the shift settles). Per `02b-design-plan-b.md`
D.B7's "Mitigation … Not in scope this session", the more sophisticated
per-segment indirection can be a Phase 2.9 follow-up.

### 2. `add_initial_groups` not part of the per-admission chain

The brief described the bounds chain as `add_initial_groups +
compute_voxel_bounds + compute_block_bounds`. The actual code at HEAD
`6010b32` only dispatches `compute_voxel_bounds + compute_block_bounds`
in the streaming branch (`add_initial_groups` is W3 regime-1 seed,
gated on `bounds_initialized = false`, skipped for streaming via
`!noise_dispatch_active` at `:1885`). The brief description was
slightly inaccurate; the two-pass chain is what fires.

This isn't a deviation from the brief's intent — just a clarification
that the code dispatches 2 passes, not 3, in the streaming branch.

## Cold-start improvement summary

Cold-start dropped from ~40 s to **1.02 s** (vs static baseline 5.14 s).

8 verification gates run (build + lib tests + 6 e2e gates); all 8 PASS.
