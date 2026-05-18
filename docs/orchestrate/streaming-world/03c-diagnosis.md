# 03c — Diagnosis: `--streaming-window` false-pass + minutes-long hang

Read-only diagnostic produced by a Phase-2.5 investigation agent. Traces the
streaming-world e2e gate (`--streaming-window`) end-to-end against the source
at HEAD `66d1b939`. No source edits.

The two observed symptoms — the gate's reported `PASS` despite the user
seeing only skybox AND the binary taking ~minutes to finish — are both
**genuine code-level defects**, not impl-agent confusion. They have distinct
root causes; both are reproduced below.

## Hard one-off observation

Run via `cargo run --release --bin e2e_render -- --streaming-window` from
worktree root (the impl agent's verification command), captured stdout/stderr
tail:

```
[~120 + ~300 frames of per-frame logging like:]
streaming-world: dispatched 4 segment(s) this frame (0 evictions); bounds chain WAS run.
streaming-world residency shift: cam_seg=IVec3(12, 1, 8), new_origin=IVec3(4, 0, 0), evictions=128, pending Generating slots=512, admissions_this_frame=4
e2e_render --oasis-edit-visual: after-capture 256x256
e2e_render --streaming-window: screenshot saved to target/e2e-screenshots/streaming_window_after.png
e2e_render --streaming-window: streaming-window: mean pixel Δ = 0.00 (floor = 0.00); after-frame luminance variance = 242.05 (floor = 50.00); residency origin shift in X = 4 segments (floor = 4)
e2e_render --streaming-window: streaming-window gate PASS — streaming-window: mean pixel Δ = 0.00 (floor = 0.00); after-frame luminance variance = 242.05 (floor = 50.00); residency origin shift in X = 4 segments (floor = 4)
e2e_render: streaming-window PASS — 120 warmup + 300 post-walk wait frames; camera walked +1024 voxels in X; residency window followed.
```

- Exit code: **0 (PASS)**.
- Wall-clock: ~95 seconds for the post-walk wait phase alone (per-frame
  log timestamps show 310-330 ms between consecutive `dispatched 4 segment(s)`
  lines, × 300 frames). Total run ~2 minutes.
- Reproduces the impl log's reported "PASS / variance 242.05 / pixel Δ 0.0".
- Reproduces the user's "hangs for minutes". (The timeout did not hit — the
  run finished naturally.)
- Reproduces the user's "skybox only" — both
  `target/e2e-screenshots/streaming_window_{before,after}.png` are pure
  sky-gradient (top brighter to dark navy at bottom, no terrain anywhere).
  Inspected visually.

A second run launching the binary DIRECTLY (`./target/release/e2e_render
--streaming-window` from worktree root, not via `cargo run`) instead exited
with `FAIL — after-frame luminance variance 0.11 below floor 50.00` in ~3
seconds — because the asset path is resolved relative to the binary's CWD
and the shaders couldn't be found, so the framebuffer was pure black
(variance ~0). This asset-CWD quirk is unrelated to the gate logic but is
what makes the "ran the binary directly" repro look different from the
"cargo run" repro; both reveal the same underlying bug (no terrain visible)
in different ways. See `crates/bevy_naadf/src/lib.rs:674` for the
`file_path: "src/assets"` config.

## Root cause: false pass

The gate's exit code is gated by three assertions in
`crates/bevy_naadf/src/e2e/streaming_window.rs:289-326`:

| # | Assertion | Type | Behaviour on skybox-only |
|---|---|---|---|
| (a/b) | `pixel_delta >= STREAMING_MIN_PIXEL_DELTA` where `STREAMING_MIN_PIXEL_DELTA = 0.0` (line 62) | LOOSE — any value passes | Pass (0.0 ≥ 0.0) |
| (a)   | `after_lum_var >= STREAMING_MIN_AFTER_LUM_VARIANCE = 50.0` (line 68) | LOOSE — sky gradient has variance ~242 | Pass (sky alone reaches 242) |
| (d)   | `origin_shift_x_seg.abs() >= STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS = 4` (line 291) | STRICT — residency-side invariant | Pass — the residency manager IS shifting correctly |

The (d) assertion is the **only strict assertion**, and it tests the
**residency bookkeeping layer**, not the rendered output. Per the live
log:

```
streaming-world residency shift: cam_seg=IVec3(12, 1, 8), new_origin=IVec3(4, 0, 0), ...
```

the residency origin moves from `0 → 4` as designed. So the residency
manager itself is correct; the gate proves the residency-manager invariant
and nothing else.

What it does NOT prove:

- That `chunks_buffer` actually got populated with terrain content.
- That the renderer can read that content.
- That the camera's local frame matches what the renderer dereferences.
- That any pixel of the before/after frames contains anything other than sky.

The threshold floors (a/b) at `0.0` and (a) at `50.0` are both loose enough
that two identical pure-skybox frames pass.

Comment at `streaming_window.rs:54-61` explicitly flags
`STREAMING_MIN_PIXEL_DELTA = 0.0` as a *temporary* value with a TODO
("Bumping this to ≥ 3.0 once the translation glue lands…"). The
translation glue HAS landed (see § "Verification of impl-log claims" below)
but the floor was never raised.

The `STREAMING_MIN_AFTER_LUM_VARIANCE = 50.0` floor is also too loose. The
impl log itself says (line 67): *"The sky gradient alone … produces
variance ~200; flat-black would be near 0. This threshold catches 'every
pixel is identical' failures."* — so by the gate author's own admission,
50 only catches pure-flat output, not sky-only output. The 0.11 result on
the asset-path-broken (flat-black) run confirms the 50.0 floor is wired
correctly for the "every pixel identical" case; it just doesn't catch the
"every pixel is sky" case.

**Why the render is skybox-only**: the streaming dispatch IS running every
frame (the log says `dispatched 4 segment(s)` every frame). But the
admissions list never recycles. In
`crates/bevy_naadf/src/streaming/residency.rs:408-433`,
`process_pending_admissions` picks the **4 camera-closest `Generating`
slots** every frame:

```rust
fn process_pending_admissions(residency: &mut Residency) {
    let cap = residency.max_segments_per_frame as usize;
    let mut candidates: Vec<...> = residency
        .slot_state
        .iter()
        .filter_map(|(i, st)| match st {
            SlotState::Generating { .. } => Some(...),
            _ => None,
        })
        .collect();
    candidates.sort_by_key(|c| c.2);  // by cam-distance squared
    for (slot, world, _dsq) in candidates.into_iter().take(cap) {
        residency.admissions_this_frame.push((world, slot));
    }
}
```

But `Generating → Resident` is never set anywhere in the running code. The
helper `mark_admissions_resident` exists at
`crates/bevy_naadf/src/streaming/residency.rs:438-447` and is re-exported
from `streaming/mod.rs:45`, but `grep -rn mark_admissions_resident
crates/` shows **only the definition and re-export — zero call sites.** So
`SlotState::Resident` is never assigned (lines 444 alone, never executed).

Consequence: every frame, `process_pending_admissions` re-picks the SAME
4 slots — the 4 closest to the current camera segment. The other 508 in
the window stay in `Generating` and never enter the budgeted-admissions
list, so the GPU dispatch never writes their `chunks_buffer` content.
Those slots' content stays at the zero-initialised pattern from
`crates/bevy_naadf/src/render/prepare.rs:271-282`, where
`chunk_data_single.resize(chunk_count, 0)` zero-fills the buffer at
allocation time.

This is consistent with the per-frame log: `pending Generating slots=512,
admissions_this_frame=4` after the shift. 512 candidates, of which the
SAME 4 nearest are picked, dispatched, and remain `Generating` for the
next frame.

The camera at `(2048, 288, 2048)` looks `+X` with a downward angle (look
target Y = `sea_level - 16 = 240` per
`crates/bevy_naadf/src/e2e/streaming_window.rs:124-135` and
`crates/bevy_naadf/src/voxel/grid.rs:212-219`). Rays go forward into the
+X half of the window. The 4 dispatched slots are camera-centric (segments
adjacent to cam_seg = `(8, 1, 8)`), not where the rays travel — the forward
rays land in segments far from the 4 dispatched ones, hit zero-filled
chunks, miss everything → sky.

After the +1024 camera walk, the same logic holds at the new pose: the 4
dispatched slots track the camera, the forward rays land elsewhere, sky
again. Pixel-delta between two near-identical sky-only frames = 0.0.

### Citations

- Loose floor `STREAMING_MIN_PIXEL_DELTA = 0.0`:
  `crates/bevy_naadf/src/e2e/streaming_window.rs:62`.
- Loose floor `STREAMING_MIN_AFTER_LUM_VARIANCE = 50.0`:
  `crates/bevy_naadf/src/e2e/streaming_window.rs:68` (sky-only produces 242).
- Strict floor `STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS = 4`:
  `crates/bevy_naadf/src/e2e/streaming_window.rs:78`.
- The three assertions in `assert_streaming_window_landed`:
  `crates/bevy_naadf/src/e2e/streaming_window.rs:307-326`.
- `process_pending_admissions` picks `Generating` candidates only and
  caps at `max_segments_per_frame`:
  `crates/bevy_naadf/src/streaming/residency.rs:408-433`.
- `SlotState::Generating` set at `:383-384`; `SlotState::Resident` only
  ever set in unreached `mark_admissions_resident` at `:444`.
- `mark_admissions_resident` has zero call sites
  (re-exported at `crates/bevy_naadf/src/streaming/mod.rs:45`).

## Root cause: minutes-long hang

The gate state machine itself is **fully bounded**. Driver phase budgets
in `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:113,121,125`:

| Phase | Frame cap | Wall-clock cap |
|---|---:|---|
| `OasisWarmup` | 120 | none |
| `OasisShootBefore` | 1 | none |
| `OasisDrainBefore` | 16 | none |
| `OasisApplyEdit` | 1 | none |
| `OasisWaitPostEdit` | 300 | none |
| `OasisShootAfter` | 1 | none |
| `OasisDrainAfter` | 16 | none |
| `OasisAssert` | 1 | none |

Total: ~455 frames. Every phase increments `state.phase_ticks` unconditionally
and advances when `phase_ticks >= cap`
(`crates/bevy_naadf/src/e2e/driver.rs:884-894, 901-953, 955-1011, 1013-1021,
1023-1080, 1082-1173`). No phase has a *wait-for-convergence* condition;
there is no `wait_for_slot_state_resident` style loop. So no logical
deadlock.

The hang is **frame-rate-driven, not deadlock-driven**: every frame, the
streaming branch in `naadf_gpu_producer_node`
(`crates/bevy_naadf/src/render/construction/mod.rs:2551-2754`) issues a
full-world bounds-chain dispatch any time any admission or eviction
happened that frame:

```rust
// crates/bevy_naadf/src/render/construction/mod.rs:2706-2734
if any_admissions_or_evictions {
    let encoder = render_context.command_encoder();
    let world_chunks = WORLD_SIZE_IN_CHUNKS.x * WORLD_SIZE_IN_CHUNKS.y * WORLD_SIZE_IN_CHUNKS.z;
    let max_blocks_u64 = (world_chunks as u64) * 64;
    let max_voxels_u64 = max_blocks_u64 * 32;
    let voxel_workgroups = ((max_voxels_u64 / 32 + 1).max(1)).min(u32::MAX as u64) as u32;
    let block_workgroups = ((max_blocks_u64 / 64 + 1).max(1)).min(u32::MAX as u64) as u32;
    chunk_calc::dispatch_compute_voxel_bounds(encoder, p_voxel, world_bg, voxel_workgroups);
    chunk_calc::dispatch_compute_block_bounds(encoder, p_block, world_bg, block_workgroups);
}
```

With `WORLD_SIZE_IN_CHUNKS = (256, 32, 256)` = 2,097,152 chunks
(`crates/bevy_naadf/src/lib.rs:246`):

- `max_blocks_u64 = 2,097,152 × 64 = 134 M`.
- `max_voxels_u64 = 134 M × 32 = 4.29 B`.
- `voxel_workgroups = 134 M + 1`.
- `block_workgroups = 2.1 M + 1`.

The `voxel_workgroups` count packed via `split_3d_dispatch` represents
~134 million workgroups × 64 threads/wg = ~8.5 billion thread-invocations
per frame, on the worst-case-sized buffer (sized for the full world, even
though only 4 chunks of new content land each frame). Even on RTX 5080
this measures ~300 ms/frame in practice (per-frame log timestamps confirm
this: each "dispatched 4 segment(s)" line is 300-330 ms apart on the
measured machine).

And — because of the `Generating → Resident` bug above — `any_admissions =
true` on EVERY frame, indefinitely. So the bounds chain fires every frame
of the gate, not just on segment-boundary crossings.

Frame-rate breakdown:
- 120 warmup frames × ~310 ms = **~37 s**.
- 300 wait frames × ~310 ms = **~93 s**.
- ~50 drain/shoot/etc frames × ~310 ms = **~15 s**.
- Total: **~2 minutes**. Matches the user's "hangs for minutes" report.

So the gate does eventually exit (with the false `PASS`); it does not
deadlock. The "hang" is real but is slow-per-frame caused by per-frame
worst-case bounds-chain dispatch, compounded by the `Generating → Resident`
defect that forces every frame to count as "had an admission".

### Citations

- Worst-case bounds-chain dispatch on every per-frame admission:
  `crates/bevy_naadf/src/render/construction/mod.rs:2706-2734`.
- World-size constants:
  `crates/bevy_naadf/src/lib.rs:230, 236, 246, 249` (`WORLD_SIZE_IN_SEGMENTS
  = (16, 2, 16)`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4`, `WORLD_SIZE_IN_CHUNKS
  = (256, 32, 256)`, `WORLD_SIZE_IN_VOXELS = (4096, 512, 4096)`).
- `any_admissions` true every frame because process_pending_admissions
  always finds Generating candidates:
  `crates/bevy_naadf/src/streaming/residency.rs:408-433`.
- Driver phases all bounded (no wait-for-condition loops):
  `crates/bevy_naadf/src/e2e/driver.rs:884-1173`.
- Phase frame caps `OASIS_WARMUP_FRAMES = 120`, `OASIS_POST_EDIT_WAIT_FRAMES
  = 300`, `OASIS_DRAIN_FRAMES = 16`:
  `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:113, 121, 125`.

## Verification of impl-log claims

The impl log `03b-impl-residency.md` is partially stale; some claims do not
match the code at HEAD `66d1b939`.

| Claim in `03b-impl-residency.md` | Reality | Notes |
|---|---|---|
| `STREAMING_MIN_PIXEL_DELTA = 0.0` is the "temporary" value pending translation glue | TRUE at source | `streaming_window.rs:62`. Translation glue HAS landed but the floor was not raised; impl log's "Bumping this to ≥ 3.0 once the translation glue lands" note (line 218-220) was not followed through. |
| Camera-to-window-coords translation glue is "not yet wired" / "infrastructure-level wiring that wasn't in Phase 2's scope" (line 96-109) | FALSE — translation IS wired | `translate_world_to_window_local` exists and is called from `pin_streaming_window_camera` at `streaming_window.rs:158-196`. Camera Transform is correctly pre-translated by `-origin * SEGMENT_VOXELS` each tick. The impl agent's narrative description of this gap is out of date relative to their own code. |
| 12 new unit tests added | FALSE — actually 16 | `noise_dispatch.rs`: 2. `residency.rs`: 8. `streaming_window.rs`: 6 (the impl log listed 2: `camera_walk_latch_round_trip`, `streaming_window_pose_x_shifts_on_walk`; the actual file has 4 more: `pin_translates_world_to_window_local_origin_zero`, `pin_translates_world_to_window_local_origin_shifted`, `pin_translation_no_residency_is_identity`, `pin_translation_is_idempotent_under_re_derivation` at `streaming_window.rs:375-438`). Total 16. |
| `--streaming-window` exit 0 with PASS | TRUE — reproduced | But the PASS is a false pass per § "Root cause: false pass" above. |
| After-frame luminance variance = 242.05 | TRUE — reproduced. | But the value is from sky gradient alone, not terrain. The impl log's own statement at line 46 — "After-frame luminance variance: 242.05 (sky gradient — see limitation below)" — already acknowledged this. |
| Pixel Δ = 0.0 from "TAA fully converged" | FALSE explanation | Pixel Δ is 0.0 because both frames are skybox-only (the camera-translation pre-cancels the walk in window-local frame, AND the un-dispatched 508/512 slots show zero terrain in both poses). TAA may also factor in, but is not the load-bearing reason. |
| Residency origin shift = 4 segments | TRUE — reproduced | This single residency invariant IS verified, by the only strict assertion in the gate. |
| All 6 verification gates green | UNVERIFIED | Did not re-run `--wgsl-noise-oracle`, baseline, or `--validate-gpu-construction`. The impl log lists them passing; the diagnostic agent did not re-run them. Out-of-scope here. |
| "Per-frame admission count: 4 (default); ~1200 dispatches over the 300-frame wait = 5× the 512-slot window" (line 44) | MISLEADING | 1200 dispatches over 300 frames is arithmetically right, but those dispatches re-target the **same 4 slots** every frame (the 4 closest to the camera segment), not 1200 distinct slots. So while the dispatch fires 1200 times, only 4 unique slots receive content. The bounds-chain runs over the worst-case buffer extent regardless. |
| `slot_state` advances `Generating → Resident` once dispatched (implied by `Mark a slot Resident once the render-world has actually dispatched its noise + chunk_calc passes` in `residency.rs:435-447`) | FALSE | `mark_admissions_resident` is defined but never called. Slots remain `Generating` forever, breaking per-slot progression. |

## Punch-list for the fix dispatch

Ordered (independent items can be parallelised — the residency-state
transition is the load-bearing fix; others are regression-catching scaffolding):

1. **(MUST) Transition `Generating → Resident` after dispatch.**
   - `crates/bevy_naadf/src/render/construction/mod.rs:2700` (per-segment
     dispatch loop in the streaming branch) — after `render_queue.submit`,
     the slot just dispatched must be marked Resident.
   - The naive path: mark on the *render-world* side and extract back.
     Cleaner: emit a per-frame counter / use a `RenderApp → MainApp`
     event mirror. Simplest immediate fix: call
     `mark_admissions_resident` on the main-world `Residency` *after*
     `extract_streaming_state` has read the admissions list this frame
     — i.e., from a `Last`-stage system in the main world that reads
     `Residency::admissions_this_frame` and marks them Resident,
     trusting the render-world dispatch ran. This is racy by Bevy's
     async-rendering model but harmless: at worst, a slot is marked
     Resident one frame too early; the next admission re-Generating-s
     it if eviction picks it up.
   - File: `crates/bevy_naadf/src/streaming/residency.rs` — add a
     `Last`-stage system `finalise_admissions_as_resident` that calls
     `mark_admissions_resident(&mut residency,
     &admissions_this_frame.clone())` and clears `admissions_this_frame`.
     Wire it in `StreamingPlugin` (`crates/bevy_naadf/src/streaming/mod.rs`).
   - This is the root-cause fix. Without it the visible streaming will
     never populate beyond 4 slots regardless of any other change.

2. **(MUST) Raise `STREAMING_MIN_PIXEL_DELTA` from 0.0 to a real floor.**
   - File: `crates/bevy_naadf/src/e2e/streaming_window.rs:62`.
   - Impl log suggests `≥ 3.0`. After (1) lands and the visible streaming
     works, measure the actual Δ between a Pose-A capture and a Pose-B
     capture and pick a value with reasonable margin.

3. **(MUST) Raise `STREAMING_MIN_AFTER_LUM_VARIANCE` from 50.0 to a
   floor that fails on sky-only.**
   - File: `crates/bevy_naadf/src/e2e/streaming_window.rs:68`.
   - The 50.0 floor is too loose — sky-gradient alone produces ~242
     (per impl log). After (1) lands, a populated terrain frame should
     produce >> 242 (terrain pixels vary more than sky pixels). Pick
     a floor north of the sky-only baseline (e.g., 400 — chosen so
     that pure-sky-with-242 fails and a moderately-textured-terrain frame
     passes). Final value to be measured against a real terrain frame
     after (1) lands.

4. **(SHOULD) Add a wall-clock budget to the gate.**
   - The gate's frame-cap budget (~455 frames) becomes a 2-minute wall
     clock under the current per-frame dispatch load. Even with (1)
     fixed, the post-walk wait phase will continue to dispatch the
     bounds chain over the worst-case buffer extent.
   - Adding a wall-clock cap (e.g., 30 s) per phase would fail the gate
     LOUDLY when the frame rate drops — instead of taking 2 minutes
     and exiting `PASS`.
   - Files: each of the OasisXxx phase branches in
     `crates/bevy_naadf/src/e2e/driver.rs:884-1173`. Add a `started_at:
     Instant` to `E2eState` (or per-phase) and compare against a
     per-phase `MAX_WALL_CLOCK_SECS` constant.
   - Alternative: gate-level only — record start time at `OasisWarmup`
     entry, abort with explicit FAIL if total exceeds e.g. 60 s.

5. **(SHOULD) Move the bounds-chain dispatch out of every-frame.**
   - File: `crates/bevy_naadf/src/render/construction/mod.rs:2706-2734`.
   - After (1) lands, `any_admissions_or_evictions` will become FALSE
     on frames where no segment crossed in or out — and the per-frame
     bounds chain will stop firing. This is a load-bearing
     consequence of fixing (1); test the frame timestamps after (1) +
     verify the bounds chain only runs on segment-crossings, not every
     frame.
   - If frame rate is still bad, consider a dirty-segments
     optimisation (only re-bound the affected segments — impl log §
     "Surprises" line 130-134 flags this as a Phase-2.5 perf win).

6. **(SHOULD) Update `03b-impl-residency.md` to reflect reality.**
   - The "Camera-to-window-coords translation glue is not yet wired"
     narrative at lines 96-126 is stale — the glue IS wired.
   - The "12 new unit tests" tally at line 33 is incorrect — there are
     16 (the 4 `pin_translation*` tests were added after the impl log
     was written).
   - The "Per-frame admission count: 4 … ~1200 dispatches over the
     300-frame wait = 5× the 512-slot window" claim at line 44 is
     misleading — the 1200 dispatches target only 4 unique slots, not
     1200 distinct slots.

7. **(OPTIONAL) Demote per-frame `info!` logs to `debug!`.**
   - The impl log itself flags this as cleanup at line 137-139.
   - File: `crates/bevy_naadf/src/streaming/residency.rs:394-402`
     (`residency shift:` log) and
     `crates/bevy_naadf/src/render/construction/mod.rs:2745-2752`
     (`streaming-world: dispatched N segment(s)`).
   - Low priority — but at production frame rates these logs are noisy.

8. **(OPTIONAL) Document the asset-path CWD requirement.**
   - The binary's asset path is `src/assets` relative to the binary
     CWD (`crates/bevy_naadf/src/lib.rs:674`). Running via `cargo run`
     uses `CARGO_MANIFEST_DIR` (`crates/bevy_naadf/`) implicitly, so
     it works; running the bare binary from any other directory
     looks for assets at `<cwd>/src/assets` and silently produces a
     pure-black framebuffer.
   - This is a long-standing project quirk, not Phase-2-specific —
     but it makes "did the impl agent run a different binary?"
     ambiguous. The diagnostic agent confirms: both runs were the
     same binary; the CWD determined whether the assets resolved.
   - Possible fix (out-of-scope for this dispatch): set `file_path`
     via `BEVY_ASSET_ROOT` env var with a sensible default, or
     resolve `crates/bevy_naadf/src/assets` from an
     executable-relative path.
