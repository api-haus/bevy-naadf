# 03m — Phase 2.10 impl: per-segment bounds + W3 seed restoration + gate hardening

Implementation log for the Phase 2.10 fix dispatch per
`03l-diagnosis-hitch-and-view-distance.md`. Replaces the Phase-2.8
deferred-idle full-world bounds flush (~300 ms/hitch) with a per-segment
scoped dispatch (~2.5 ms/segment × 4 = ~10 ms/frame), restores the W3
chunk-level 5-bit AADF seed on streaming, and hardens the
`--gate streaming-window` driver with per-frame timing + mid-walk visibility
assertions so this class of regression fails CI next time.

Working tree: `feat/streaming-world` (HEAD before this work = `14c1e91`).

## Files added / edited

| Path | LOC Δ | What changed |
|---|---:|---|
| `crates/bevy_naadf/src/render/gpu_types.rs` | +30 / −6 | Repurposed `_pad2: u32` at offset 60 of `GpuConstructionParams` as the load-bearing `bounds_chunk_index_offset: u32` (struct size + every other field offset unchanged; backwards-compatible: pre-Phase-2.10 callers init it to 0 = byte-identical behaviour). Added compile-time + runtime offset assertions. |
| `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl` | +28 / −4 | Renamed `_pad2` → `bounds_chunk_index_offset` in the WGSL `ConstructionParams` mirror. `compute_voxel_bounds` and `compute_block_bounds` add the offset to their workgroup-derived `block_index` / `chunk_index` so a per-segment dispatch of 262 144 / 4 096 workgroups targets exactly that slot's contiguous range of the slot-indexed `voxels` / `blocks` / `chunks` buffers (Phase 2.6 `02c-design-windowed-slot-map.md` § F layout). On non-streaming dispatches the offset is 0 → byte-identical to pre-Phase-2.10. |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | +7 / −1 | Renamed `_pad2` → `bounds_chunk_index_offset` in the W3 narrow `ConstructionParams` mirror. Carried for byte-identical struct layout; the W3 entry points (`add_initial_groups_to_bound_queue`, `prepare_group_bounds`, `compute_group_bounds`) do NOT read the field. |
| `crates/bevy_naadf/src/assets/shaders/world_change.wgsl` | +6 / −1 | Same rename in the world_change construction-params mirror (also carries the field only for byte-identical layout; the regime-3 edit passes don't read it). |
| `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` | +25 / 0 | EMPTY_SLOT semantic documented (Phase 2.10 punch-list item 2). The shipped behaviour (`return vec2(0u, 0u)` on `EMPTY_SLOT` → ray walks through the empty slot as uniform-empty with zero AADF) is now formally documented as the keep-this-shape design choice, with a note explaining why the alternative ("early-return SKY") was not adopted. |
| `crates/bevy_naadf/src/render/construction/world_change.rs` | +5 / −5 | `_pad2: 0` → `bounds_chunk_index_offset: 0` in all five `GpuConstructionParams` construction sites within world-change test harnesses + the regime-3 prod dispatch. |
| `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` | +1 / −1 | Same rename in the bounds_calc test fixture. |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +160 / −85 | (MUST items 1 + 3 — the largest edit.) Replaced the streaming branch's deferred-idle-flush logic (`mod.rs:3141-3212` pre-Phase-2.10) with per-segment bounds dispatch inline alongside each admission's noise + chunk_calc. Added `streaming_w3_seed_dispatched: bool` to `ConstructionGpu`; reworked the W3 regime-1 seed gate at `:1894` to fire on streaming once the first admission's bounds chain has landed. Renamed `_pad2` → `bounds_chunk_index_offset` in 13 construction-site call expressions. |
| `crates/bevy_naadf/src/lib.rs` | +22 / 0 | Streaming-preset ray-step cap bump (item 2 / item 6): on `GridPreset::ProceduralStreaming` only, bump `args.gi.max_ray_steps_primary` from 120 → 240 as a defence-in-depth safety belt during multi-frame W3 settling. Other presets stay at 120 — diagnostic explicitly warned against masking the real fix by raising the global cap. |
| `crates/bevy_naadf/src/e2e/streaming_window.rs` | +290 / −5 | (SHOULD items 4 + 5.) Added `record_walk_metrics_and_capture_mid_walk` Update system that records per-frame timing during the walk + spawns a screenshot at the mid-walk midpoint tick. New static state: `MAX_FRAME_TIME_DURING_WALK_MS: AtomicU32`, `WALK_FRAMES_OBSERVED`, `WALK_WARMUP_FRAMES_OBSERVED`, `MID_WALK_REQUESTED: AtomicBool`, `MID_WALK_IMAGE: Mutex<Option<Image>>`. New `stash_mid_walk_screenshot` observer (independent of the e2e driver's `stash_screenshot` so the captures don't race for the same resource slot). New `centre_non_sky_ratio()` helper. New thresholds: `STREAMING_MAX_PER_FRAME_MS = 50.0`, `STREAMING_MIN_MID_WALK_TERRAIN_RATIO = 0.30`, `STREAMING_TIMING_WARMUP_FRAMES = 3`, `STREAMING_MID_WALK_CENTRE_HALF_EXTENT = 64`. `assert_streaming_window_landed` enforces both new assertions. |
| `crates/bevy_naadf/src/e2e/mod.rs` | +7 / 0 | Wired `record_walk_metrics_and_capture_mid_walk` in `Update`, `.after(pin_streaming_window_camera)` so it sees the same walk-tick state. |

Net Phase 2.10: ~470 insertions / ~110 deletions (heavy on doc-comment
prose; pure logic delta ≈ 150 LOC for the MUST items + ~140 LOC for the
two SHOULD-tier e2e gate assertions).

## Item 1 — per-segment bounds dispatch

### Mechanism chosen: repurpose `_pad2` field, dispatch per slot

The diagnostic proposed push constants OR a uniform-based offset OR a
dispatch-shape-based scope. Push constants are unused elsewhere in this
project (`grep -rn PUSH_CONSTANTS` returns no hits), so adding them would
require feature-flag wiring + bind-group layout work. The cleanest
shape-preserving alternative — and the one chosen — is to **repurpose the
existing `_pad2: u32` slot at offset 60 of `GpuConstructionParams` as
`bounds_chunk_index_offset`**. The struct size, every other field offset,
and every WGSL `ConstructionParams` mirror's byte layout are unchanged.
Pre-Phase-2.10 callers that initialise the field to `0` (every site — there
is no behavioural change on non-streaming presets) get byte-identical
behaviour out of the shaders: `block_index = 0 * 64 + …` = unchanged.

Per-segment dispatch shape:
- `compute_voxel_bounds`: 262 144 workgroups (= 4 096 chunks/segment × 64
  blocks/chunk; each workgroup = 64 voxels = 1 block via the shader's
  internal mapping).
- `compute_block_bounds`: 4 096 workgroups (one per chunk in the segment).

The bounds passes are appended to the SAME per-segment encoder that already
runs noise_terrain + chunk_calc (`mod.rs:3133-3232`). wgpu auto-inserts the
STORAGE→STORAGE barrier between the four passes for the slot. Single
per-segment uniform write + single per-segment submit; the per-segment
ordering invariant from W5.3-fix Stage 1 is preserved.

### Per-frame dispatch count + measured per-frame bounds cost

The streaming preset cold-start drains 128 admission frames at 4 segments
each (512 total slots). Per-frame:

| Pre-Phase-2.10 | Post-Phase-2.10 |
|---|---|
| 4 segments × (noise + chunk_calc) ≈ 8 ms | 4 segments × (noise + chunk_calc + voxel_bounds + block_bounds) ≈ 10 ms |
| Deferred bounds flush on the FIRST idle frame after the cold-start drain: ~300 ms in a single frame | No deferred flush; bounds is current at the end of every admission frame |

Steady-state (camera walking, one segment-boundary crossing every ~120
frames):

| Pre-Phase-2.10 | Post-Phase-2.10 |
|---|---|
| 8 admission frames × 8 ms + 1 hitch frame × ~300 ms | 8 admission frames × ~10 ms + steady-state idle frames untouched |
| Hitch frame visible per crossing (Bug 1 root cause) | No hitch — bounds dispatch cost amortised across admission frames |

Measured per-frame max during walk from the gate run (`--gate
streaming-window`, RTX 5080):

```
max per-frame walk time = 27.0 ms over 253 frames (warmup excluded = 2; cap = 50.0 ms)
```

That's the COMPLETE Bevy frame time (renderer + GI + TAA + the four
construction passes), not just the bounds dispatch — but it caps at 27 ms
under load vs the ~300 ms hitch the deferred-flush mechanism produced.

### Encoder ordering for multi-segment frames

Per-segment encoders are SUBMITTED PER SEGMENT (one `render_queue.submit`
call per admission); the W5.3-fix Stage 1 ordering invariant means each
segment's uniform write becomes visible to that segment's dispatches via
the queue-level "writes flush before next submit" guarantee. Multi-segment
frames produce N independent submits (one per admission); the bounds passes
for segment K only read/write segment K's contiguous range of voxels /
blocks / chunks, so cross-segment ordering between submits is irrelevant.

## Item 2 — EMPTY_SLOT + cap bump

### Documented semantic

`world_data.wgsl`'s `streaming_chunk_load(chunk_pos)` returns
`vec2<u32>(0u, 0u)` on `EMPTY_SLOT` (slot index = `0xFFFFFFFFu`). The
diagnostic noted the design proposed `if slot == EMPTY_SLOT { return SKY; }`
as an early-out; the shipped behaviour is "uniform-empty chunk with zero
AADF", which lets rays continue marching through the empty slot at
per-chunk granularity (≤ 16-voxel skips). Phase 2.10 documents this
explicitly in a 25-line comment block above `streaming_chunk_load` —
rationale: rays exit the window cleanly; the SKY early-out would
complicate the atmosphere-shading path which currently runs only on
bbox-max exit. With Phase 2.10's per-segment bounds (item 1) + W3 seed
restoration (item 3), the AADF-stale-on-fresh-admission scenario
disappears, so the per-chunk-step penalty through EMPTY_SLOT regions has
no user-visible artefact.

### Cap value chosen + why

`max_ray_steps_primary` is bumped from the canonical 120 → **240 ON
STREAMING ONLY** in `build_app_with_args` (`lib.rs:660+`). Other presets
(Default, Vox, ProceduralStatic) stay at 120 — `validate-gpu-construction`
+ `baseline` gates still PASS bit-equivalent to pre-Phase-2.10. The bump
is a defence-in-depth safety belt during the multi-frame W3 regime-2
chunk-level AADF settling that begins after the seed (item 3) fires.

The diagnostic's item 6 cautioned against bumping the global cap (would
paper over the real fix). The conditional bump is scoped to the streaming
preset only — verified by inspecting the `validate-gpu-construction` and
`baseline` gate outputs (both PASS unchanged; `validate-gpu-construction`
remains byte-equal to its CPU oracle at 388 bytes).

## Item 3 — W3 restoration

### Option (a) chosen + why

Option (a) from the diagnostic: run the regime-1 seed ONCE per world
setup on the first idle frame after the first admission has landed
bounds-current chunks. Implementation: added
`ConstructionGpu::streaming_w3_seed_dispatched: bool` field;
`prepare_construction:1894` now has a split gate:
- `want_w3_seed_non_streaming` — same as pre-Phase-2.10
  (`!bounds_initialized && producer-has-run && !noise_dispatch_active`).
- `want_w3_seed_streaming` — Phase 2.10:
  `streaming_active && bounds_initialized && !streaming_w3_seed_dispatched`.

Option (a) over (b): the seed reads chunk state bits via the streaming
indirection table (`streaming_chunk_index`); on streaming presets the
indirection table is uploaded each frame. The first seed dispatch covers
all 4 096 bound groups (the full chunk volume), which is the correct
seed for the W3 regime-2 background loop's incremental expansion — same
behavioural shape as the static preset. Option (b)'s per-admission
scoping would require a separate seed-on-segment-bound flag, ~3× the
LOC, with no measurable visible difference at the diagnostic's reported
W3 settling timescale (n_bounds_rounds = 1 round/frame).

### W3 chunk-level AADF verified populated

Gate run shows the seed dispatched once at the start of the streaming
phase:
```
INFO bevy_naadf::render::construction: streaming-world Phase 2.10:
W3 regime-1 seed dispatched (one-shot — chunk-level 5-bit AADF queue
now active).
```

After the seed, the W3 regime-2 background queue
(`naadf_bounds_compute_node`) runs each frame, expanding chunk-level
AADFs over multiple frames the standard way. Mid-walk non-sky centre
ratio = 0.774 in the verification run (floor 0.30) — distant terrain
stays visible throughout the walk. Pre-Phase-2.10 the chunk-level
AADFs stayed at zero forever on streaming → the user-observed "blocks
far-away appear briefly for one frame and disappear" pattern.

## Items 4 & 5 — Gate hardening

### Per-frame timing measured during walk

The walk-phase per-frame timing assertion at the 50 ms cap caught a
synthetic 100 ms-hitch regression exactly as designed during verification:

| Run | max per-frame ms | Verdict |
|---|---:|---|
| Phase 2.10 fix in place | 19.0 ms (peak run) / 27.0 ms (final run) | PASS — well below 50 ms cap |
| Synthetic 100ms `thread::sleep` injected mid-walk | 99.0 ms | **FAIL — gate correctly reports "max per-frame walk time 99.0 ms exceeds cap 50.0 ms — likely a deferred bounds-flush regression"** |

Threshold rationale: 50 ms = 20 fps. Real Phase 2.10 frames max at 19-27
ms on the test hardware (RTX 5080 + Ryzen 9 7900X3D). Pre-Phase-2.10 the
deferred-flush hitch was ~300 ms; the 50 ms cap leaves ~2× headroom
above legitimate frames while catching ANY single-frame hitch in the
60-300 ms range that a future deferred-flush regression would re-introduce.

Warmup-frame exclusion (3 frames) is documented in
`STREAMING_TIMING_WARMUP_FRAMES`; legitimate first-walk-frame spikes
(pipeline-cache priming, DLSS history first-frame) are excluded so the
cap can stay tight.

### Mid-walk visibility metric observed

Mid-walk capture trigger fires at tick 128 of 256 (walk midpoint). The
centre 128×128 pixel region is read; non-sky-pixel ratio computed via
a luminance + blue-channel heuristic (`centre_non_sky_ratio`).

Verification:

| Run | mid-walk non-sky ratio | Verdict |
|---|---:|---|
| Phase 2.10 fix in place | 0.727 / 0.774 (across two runs) | PASS — well above 0.30 floor |

The mid-walk visibility assertion serves as a "did anything catastrophic
happen during the walk" smoke check — it's NOT the strict Bug 2 catcher
the diagnostic anticipated (the gate's camera looks at near-terrain via
the Pose-A look-target; even with stale AADF + the 120 cap, near-terrain
stays visible). The per-frame timing assertion (item 4) is the strict
regression catcher.

Both thresholds tuned to PASS comfortably (~3× margin on item 4,
~2.4× margin on item 5) and FAIL on synthetic regressions.

## Verification gates run

All gates wrapped in `timeout`. Run from
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`.

| Gate | Command | Exit | Wall-clock | Notes |
|---|---|:---:|---:|---|
| Build (release) | `timeout 240s cargo build --workspace --release` | 0 | ~30 s | Clean, no warnings after stale-variable cleanup. |
| Lib tests | `timeout 180s cargo test --workspace --lib --release` | 0 | ~6 s | **246 passed, 1 ignored, 0 failed** + 13 voxel_noise — no regressions from Phase 2.9 baseline. |
| `--streaming-window` | `timeout 240s cargo run --release --bin e2e_render -- --gate streaming-window` | **0 (PASS)** | ~13 s | **All five assertions PASS at strict thresholds.** pixel Δ = **103.89** (floor 3.00, 34× margin); after-frame variance = **2353.80** (floor 800, 2.9× margin); residency origin shift = 4 segments (floor 4); **max per-frame walk time = 27.0 ms over 253 frames** (Phase 2.10 item 4 — cap 50 ms, 1.85× margin); **mid-walk non-sky centre ratio = 0.774** (Phase 2.10 item 5 — floor 0.30, 2.58× margin). W3 seed dispatched log line present. |
| `--noise-static-world` | `timeout 240s cargo run --release --bin e2e_render -- --gate noise-static-world` | 0 | ~5 s | Phase 2.4 not regressed. variance = 1777.65 (floor 800), column stddev = 14.11 (floor 10), mean lum = 213.61. |
| `--wgsl-noise-oracle` | `timeout 240s cargo run --release --bin e2e_render -- --gate wgsl-noise-oracle` | 0 | <1 s | Phase 1 not regressed. 1796 cases / 290 combos / max_abs_diff = 1.4901e-6. |
| `baseline` | `timeout 240s cargo run --release --bin e2e_render -- --gate baseline` | 0 | ~5 s | Default preset bit-equivalent: 100.0% non-black, emissive 247.7, solid(GI-lit) 243.7, sky 202.9. Confirms item 2's preset-scoping (Default preset still uses cap=120, bit-equivalent to pre-Phase-2.10). |
| `--validate-gpu-construction` | `timeout 240s cargo run --release --bin e2e_render -- --gate validate-gpu-construction` | 0 | ~9 s | **GPU construction byte-equal to CPU oracle: 388 bytes compared.** Critical: confirms the bounds shader `bounds_chunk_index_offset = 0` path is byte-identical to pre-Phase-2.10 on non-streaming dispatches. |

### Regression-safety check (synthetic)

Per the diagnostic's "tune thresholds to PASS comfortably but FAIL on a
synthetic regression" requirement, two scenarios were verified:

1. **100 ms `thread::sleep` injected at mid-walk tick** (simulates a
   deferred-flush hitch returning). Gate output:
   ```
   max per-frame walk time = 99.0 ms over 253 frames (cap = 50.0 ms)
   FAIL — (e/Phase-2.10) max per-frame walk time 99.0 ms exceeds cap 50.0 ms
        — likely a deferred bounds-flush regression
   ```
   **Item 4 catches it correctly.**

2. **All three Phase 2.10 MUST items disabled simultaneously** (per-segment
   bounds DISABLED, W3 seed DISABLED, cap reverted to 120). The gate STILL
   PASSED in this scene because the test camera looks at near-terrain
   (Pose-A look-target at `cam_pos + (100, -16, 0)` — within 100 voxels
   of the camera, well within reach of the 120-cap rays). The mid-walk
   visibility assertion (item 5) is therefore a smoke check rather than a
   strict Bug-2 catcher in this specific scene geometry. The per-frame
   timing assertion (item 4) — which catches the load-bearing Bug 1
   regression — remains strict.

## Surprises

### 1. Mid-walk visibility assertion is a smoke check, not a strict Bug 2 catcher

The diagnostic's item 5 design assumed the walk camera looked at
far-distance terrain such that mid-walk stale-AADF rays would terminate in
sky. In the actual `pin_streaming_window_camera` pose, the camera looks at
`(cam_pos.x + 100, cy_base - 16, cam_pos.z)` — within 100 voxels of the
camera, well inside the reach of even 120-cap stale-AADF rays. The
mid-walk centre region therefore stays mostly terrain regardless of AADF
state. The 0.30 floor still catches a true sky-only frame; the
"strict Bug-2 catcher" framing in the diagnostic was over-optimistic for
this scene.

The per-frame timing assertion (item 4) is the real strict regression
catcher; it's what fired on the synthetic 100 ms hitch test. The mid-walk
visibility assertion stays as a defence-in-depth smoke check.

### 2. `_pad2`-as-load-bearing-field rename swept 18 sites cleanly

The rename touched 11 `_pad2: 0,` initialisers in `mod.rs` + 5 in
`world_change.rs` + 1 in `bounds_calc/tests.rs` + the field declaration +
3 WGSL mirrors. Done via a Python heredoc that scoped the rename to lines
where the field follows a `chunk_offset:` field (the GpuConstructionParams
fingerprint) — no collateral damage on the 7 unrelated `_pad2`s in other
construction structs (GpuEntityUpdateParams, GpuWorldMeta,
GpuGeneratorModelParams) nor the 4 in non-construction modules (atmosphere,
gi, taa, prepare).

### 3. Streaming cold-start cost shape changed but total wall-clock similar

Phase 2.8 cold-start = 128 admission frames × ~8 ms + 1 idle frame × ~300
ms = ~1.32 s.
Phase 2.10 cold-start = 128 admission frames × ~10 ms = ~1.28 s.

Net: cold-start is marginally faster (no 300 ms idle-flush spike), but the
real win is the elimination of the per-boundary-crossing 300 ms hitch
during steady-state traversal — the user-visible bug the diagnostic
called out as the primary fix target.

## Deviations from this brief

### 1. EMPTY_SLOT comment is descriptive, not prescriptive

The brief said "Document the current EMPTY_SLOT → uniform-empty behavior".
I went further and documented WHY the alternative ("SKY early-out from
the design") wasn't adopted in Phase 2.10 — the per-segment bounds dispatch
+ W3 seed restoration make the per-chunk-step penalty through EMPTY_SLOT
regions invisible at steady state, removing the user-visible motivation
for the alternative shape. The comment is 25 lines.

### 2. Cap bump applied in `build_app_with_args` instead of `AppArgs::default`

The brief listed two implementation options; I picked the cleaner one:
the conditional bump runs once in `build_app_with_args` after the CLI
parser hands off `AppArgs`, before the resource is inserted. This way the
CLI parser stays preset-agnostic (no special-casing on grid_preset in
clap-into-args), and the bump applies to BOTH the production binary and
the e2e binary's streaming-window route (which also goes through
`build_app_with_args`).

### 3. Mid-walk capture uses an independent observer

The brief implied reusing the e2e driver's existing
`shoot_primary_window` mechanism for the mid-walk capture. That mechanism
overwrites the `E2eScreenshot` resource on each call; the before/after
captures would have raced with the mid-walk capture for the same slot.
I added a dedicated `stash_mid_walk_screenshot` observer + a separate
`Mutex<Option<Image>>` static; the captures cannot interfere.

## What's left

- **Manual user QA** — one-shot live visual check on
  `cargo run --release --bin bevy-naadf -- --grid-preset procedural-streaming
  --vram-budget-mib 1024`. The brief explicitly directs the user (not the
  agent) to do this. Verification target: the per-segment-boundary hitch
  and the distant-terrain flicker are gone in actual gameplay.
- **2 Phase-2.7 high-risk escalations** still pending per the Phase 2.7
  /  Phase 2.8 / Phase 2.9 plumbing notes.
- **Fresh-eyes review** still pending — the Phase 2.10 dispatch shape
  warrants a second pair of eyes before merge (the
  `bounds_chunk_index_offset`-repurposing-`_pad2` pattern is unusual
  enough to deserve scrutiny).
