# 04 — Fresh-eyes review brief

You are the fresh-eyes reviewer for Phase M of this orchestration. **You have not seen the design rationale, the diagnostic, or the prior iteration logs. By design.** Your job is to read the design as a verbatim artifact + the existing code, and verify it against the success criteria below.

You **MUST NOT** read `01-context.md`, `06-diagnostic-investigation.md`, or the implementation logs (`05-*.md`). Reading them would defeat the fresh-eyes pass — the architect's assumptions would leak into your review and you'd rubber-stamp them.

## What you ARE allowed to read

- This file (`04-review.md`).
- The design artifact: `docs/orchestrate/taa-hash-world-identity/02-design.md`.
- The actual code in the worktree: `crates/bevy_naadf/src/**/*.rs` and `crates/bevy_naadf/src/assets/shaders/*.wgsl` — verify any file:line the design cites is correct.
- The project's `CLAUDE.md` rules.

## What you MUST NOT read

- `01-context.md` — contains the orchestration's working assumptions.
- `00-reuse-audit.md` — contains the prior reuse decision rationale.
- `05-impl-taa-hash-world-identity.md` — contains the two prior failed iterations.
- `06-diagnostic-investigation.md` — contains the diagnostic that informed the current design.

## The artifact

`docs/orchestrate/taa-hash-world-identity/02-design.md` — written by Phase K's `delegate-architect` agent.

## Success criteria

The design must demonstrate, with **concrete file:line evidence in the existing code**, that:

1. **The structural fix correctly rebases `CameraHistory.positions[]` on origin shift.** The design must specify (a) WHICH past entries get rebased (all? only those still alive in the ring?), (b) WHEN the rebase fires relative to `update_camera_history` writing this frame's slot (must be BEFORE, otherwise the just-written slot also gets the offset and double-corrects), (c) HOW the residency origin-shift event is communicated from main-world to render-world (a new field on `Residency`? a Bevy event? a resource diff?). Each of these has a wrong answer — the design must commit to a correct one.

2. **The rebase logic preserves the int+frac split's precision invariants.** `PositionSplit` is an int+frac decomposition. The design must show that adding `delta_segments × SEGMENT_VOXELS` (an integer multiple of 256, a power-of-2 multiple) to a `PositionSplit` does NOT change the `.frac` field, only `.pos_int`. If the design uses any `f32` arithmetic or any path that could land the frac field at a different value, that is a precision regression.

3. **The cross-world signal handoff is sound.** Main-world `residency_driver` detects origin shifts. Render-world `update_camera_history` needs to know. The design must commit to a mechanism (new `Residency` field? `Events<OriginShifted>`? Render-world resource copy via `ExtractResource`?) and demonstrate that the timing guarantees the rebase fires for the SAME frame as the shift, not the next.

4. **The new `streaming-taa-shift-noise` e2e gate captures the artefact analytically.** The design must specify: (a) camera path that triggers an origin shift, (b) the per-pixel variance measurement (shadowed-band selector, e.g. luminance < 0.1), (c) the frame indices captured (the shift frame, frames N+1..N+5), (d) the threshold separating the artefact transient from the post-recovery steady state, (e) a sanity check that the gate FAILS in the current (pre-fix) state — demonstrating the gate captures the artefact rather than merely passing under any code state.

5. **The 8-neighbour hash fallback fix at `taa.wgsl:281-283` is correct.** The current code uses centre `ray_dir` to reconstruct neighbour positions. The fix needs each neighbour's own ray. The design must specify: (a) where in the precompute loop each neighbour's `ray_dir` comes from (recomputed via `get_ray_dir(params.inv_view_proj, cur_pixel_pos, …)`? cached from somewhere?), (b) what existing helpers/buffers are reused, (c) cost (the precompute loop already runs once per output pixel — recomputing 8 ray-dirs per pixel is fine, but verify the design doesn't do more).

6. **`data_id_lo13` made truly world-absolute.** The current helper at `taa.wgsl:215-225` (or wherever it landed after iteration 2) uses `cam_pos_int` which is window-local. The design must specify: (a) what world-absolute reference is added (`residency.origin × SEGMENT_VOXELS`? a new uniform?), (b) how that value reaches the shader (uniform field, push constant?), (c) consistency between the read site and write site (they MUST compute identical world-absolute positions).

7. **Verification plan.** The design must enumerate the gates that will run, in order, with timeouts, including the new `streaming-taa-shift-noise` gate. Per memory `feedback-e2e-gates-must-fail-fast.md`, all wrapped in `timeout 180s`. Per memory `subagent-gpu-app-verification-loop.md`, one smoke per failing gate.

8. **No forbidden actions.** Per worktree `CLAUDE.md`: NO `cargo run --bin bevy-naadf` as a verification step. The implementation phase must use only build + lib tests + named e2e gates.

## Deliverable shape

Append `## delegate-reviewer findings (<ISO date>)` to `docs/orchestrate/taa-hash-world-identity/04-review.md` (this file). Structure:

For EACH of the 8 success criteria:
- **Criterion N: <one-line restatement>**
  - **Verdict**: PASS / PARTIAL / FAIL / NOT ADDRESSED.
  - **Evidence in design (cite section + line of `02-design.md`)**:
  - **Evidence in code (cite file:line if you verified)**:
  - **If FAIL or PARTIAL: what's missing or wrong, with code refs to the actual existing code**:

Then a final section:

- **`## Highest-risk thing in the design`** — the one item most likely to silently break on landing. Be specific.
- **`## Recommended next action`** — proceed to implementation as designed / proceed with named amendments / send back to architect for redesign / something else.

## Hard rules

- Read `02-design.md` ONCE in full. Don't skim.
- Verify every file:line reference in the design by reading the actual code. The architect may have referenced lines that don't exist or have shifted.
- For criterion 2 (precision invariants): trace the `PositionSplit` arithmetic by reading `crates/bevy_naadf/src/camera/position_split.rs`. Don't take the architect's word for it.
- For criterion 3 (cross-world handoff): trace the system schedule. Bevy main-world systems do not directly write to render-world resources; the channel is `ExtractResource` / `ExtractSchedule` / a render-world equivalent. If the design hand-waves this, that's a FAIL on criterion 3.
- For criterion 6 (world-absolute hash): verify by reading the shader that the architect's chosen reference value (`residency.origin × SEGMENT_VOXELS` or whatever) is available at the shader call sites. If it's not bound, that's a FAIL.
- Use neutral language. You are not the implementer — you are catching things before they get implemented.

## delegate-reviewer findings (2026-05-19)

Verified design at `02-design.md` (1728 lines) against the eight success
criteria in this brief. Every file:line claim in the design was checked
against the worktree code; deviations are noted inline.

---

### Criterion 1 — structural rebase rebases `CameraHistory.positions[]` on origin shift

- **Verdict**: PASS.
- **Evidence in design**: §"Design — structural rebase" (lines 95-246).
  - (a) WHICH entries: design § "WHICH entries get rebased" (lines 209-228)
    commits to *all 128 slots* and reasons through (i) the artefact spanning
    the full 32-frame TAA ring + 64-frame ReSTIR ring, (ii) default/un-
    written slots being harmless rebase targets because they remain rejected
    by the existing `sample_age`/dist/screen-pos tests, (iii) the
    just-written slot being overwritten after the bulk rebase.
  - (b) WHEN: design lines 99-124 commits to "inside `update_camera_history`,
    BEFORE the writes at `positions[slot] = current`". Ordering is
    `rebase 128 slots → overwrite slot taa_index`, which is correct: the
    new write goes in *after* the rebase, so the current frame's slot
    receives the post-shift window-local value uncorrupted.
  - (c) HOW the signal travels: design § "Detection mechanism" (lines
    126-146) uses a `Local<LastOriginSeen>` storing the prior frame's
    `Residency::origin()`. No cross-world handoff — the rebase runs in
    main-world `Update` where the resource is read-accessible.
- **Evidence in code**: confirmed at `crates/bevy_naadf/src/render/taa.rs:188-193`
  that `update_camera_history` already takes `residency:
  Option<Res<crate::streaming::residency::Residency>>` as a parameter, so the
  plumbing the design relies on already exists. `Residency::origin()` is at
  `streaming/residency.rs:173-175`. The system's schedule ordering is
  registered at `lib.rs:898-901` as `Update,
  update_camera_history.after(camera::sync_position_split)`. The "rebase
  BEFORE write" sequencing is achievable because the existing write to
  `history.positions[slot]` is at `render/taa.rs:290`, well after the
  insertion point the design picks (just above line 214).
- **Notes**: the design correctly identifies that the C# instrumentation
  block at `render/taa.rs:215-287` is to be deleted in the same edit. The
  WHEN/WHICH reasoning is sound; the rebase-all-128 choice is the
  defensible one (any selective subset would require a per-slot
  "written-this-run" marker the design correctly notes the renderer
  already lacks).

---

### Criterion 2 — rebase preserves `PositionSplit` int+frac precision invariants

- **Verdict**: PASS.
- **Evidence in design**: §"The rebase call" (lines 148-186) and § "Precision
  invariant" (lines 188-206). The chosen op is `ps.pos_int += voxel_delta;`
  where `voxel_delta = -delta_segments * SEGMENT_VOXELS`. `SEGMENT_VOXELS`
  is `256`, so `voxel_delta` is an integer multiple of 256. The design
  explicitly states `.pos_frac` is untouched.
- **Evidence in code**: `PositionSplit` at
  `crates/bevy_naadf/src/camera/position_split.rs:21-28` is
  `{ pos_int: IVec3, pos_frac: Vec3 }`; `normalise` at lines 51-55 folds
  `floor(frac)` into `pos_int`. The invariant is "`pos_frac ∈ [0,1)³`
  after every mutating op". Adding an integer-only delta to `pos_int` does
  not touch `pos_frac`, so the post-op state is still canonical. No
  `normalise()` call is needed and no f32 path is introduced. The Rust unit
  tests in § "Verification plan" (design lines 1360-1408) explicitly assert
  `ps.pos_frac == frac_before[i]` for every slot, including default-zero
  slots and slots seeded with non-zero frac — that pins the invariant to
  the test surface.
- **Notes**: the design also flags (lines 200-206) that using the
  `PositionSplit + PositionSplit` operator overload would route through
  `Add::add` → `normalised()` (`position_split.rs:65-77`) — also correct
  in this case because the rhs has zero frac, but the chosen `IVec3 += IVec3`
  is the cleaner contract (the precision-relevant `pos_frac` never enters
  the arithmetic). Good architectural call.

---

### Criterion 3 — cross-world signal handoff is sound

- **Verdict**: PASS.
- **Evidence in design**: § "Design — cross-world wiring" (lines 249-308).
  The design picks **option zero**: no cross-world handoff needed because
  the rebase lives in main-world `update_camera_history`, and
  `extract_camera_history` (which already mirrors `positions[..]` byte-for-
  byte to the render world via `render/extract.rs:370-385`) consumes the
  ALREADY-rebased state. The design explicitly enumerates the three
  alternatives the brief named and rejects each with reasoning (lines
  263-291).
- **Evidence in code**: confirmed at
  `crates/bevy_naadf/src/render/extract.rs:370-385` that
  `extract_camera_history` is a render-world `ExtractSchedule` system that
  copies `history.positions` verbatim. Schedule ordering: `PreUpdate
  residency_driver` (`streaming/mod.rs:268-274`) → `Update
  track_and_pin_camera` (`streaming/mod.rs:284-297`) → `Update
  sync_position_split` (`lib.rs:850-857`) → `Update update_camera_history`
  (`lib.rs:898-901`) → `ExtractSchedule extract_camera_history`
  (`render/mod.rs:155-160`). The new origin is observable in
  `Residency::origin()` by the time `update_camera_history` runs because
  `PreUpdate` strictly precedes `Update`. The rebase fires for the SAME
  frame as the shift, not the next.
- **Notes**: the alternative-rejection reasoning is solid. In particular,
  rejecting render-world-only rebasing (lines 286-291) is correct because
  `extract_camera_history` runs in `ExtractSchedule` *after* the main-world
  `Update`, so anything written render-side would race against the
  already-extracted snapshot.

---

### Criterion 4 — new `streaming-taa-shift-noise` e2e gate captures the artefact analytically

- **Verdict**: PARTIAL.
- **Evidence in design**: § "Design — new e2e gate" (lines 834-1090). The
  design specifies:
  - (a) Camera path: layered on `streaming-window`'s additive +X walk
    (`STREAMING_WALK_TICKS = 256`, `STREAMING_WALK_VOXELS_PER_TICK = 4.0`,
    1024 voxels total, ≥ 4 segment shifts). Verified the constants at
    `e2e/streaming_window.rs:360-362` and the segment-shift minimum at
    `streaming_window.rs:113-116`.
  - (b) Per-pixel variance: temporal variance over frames N..N+3
    (4-sample per-pixel variance), shadowed-band selector `luminance <
    SHADOWED_BAND_LUMA_MAX = 30.0`, must be shadowed in all 5 captured
    frames to enter the metric.
  - (c) Frame indices: N (the shift frame), N+1, N+2, N+3, N+5. Frame N+4
    captured as "filler" (design line 897). N+5 = baseline.
  - (d) Threshold: `STREAMING_TAA_SHIFT_NOISE_RATIO_MAX = 3.0` — `var_transient
    / max(var_baseline_spatial, 1.0)`.
  - (e) Sanity check: design § "One-smoke pre-fix verification" (lines
    1070-1076) AND § "Verification plan" (lines 1339-1343) BOTH commit to
    running the gate ONCE on the pre-fix worktree before landing the
    structural rebase, with the requirement "Step 6 MUST FAIL". If pre-fix
    PASSES, the gate is sent back to the architect for re-calibration.
- **What's missing / wrong**:
  1. **`var_baseline` is spatial not temporal** (design lines 942-979).
     Pre-fix the artefact is a *temporal* burst — `var_transient` is per-
     pixel-temporal. `var_baseline` is the spatial variance of the
     shadowed-band luminance across N+5 alone. These two quantities are
     dimensionally compatible (both are luminance² units) but they
     measure different things. A scene where the post-recovery shadowed
     band has high spatial variance (e.g. dappled GI bounce against rock)
     would *inflate* `var_baseline` and let a real artefact pass; a scene
     with smooth shadowed regions deflates it and could false-fail.
     The design acknowledges this in line 942 ("Actually that's two
     different things") and pivots to "the cleaner formulation" but
     never demonstrates the formulation is robust. A more defensible
     formulation: capture a *post-recovery temporal window* (N+5..N+8)
     and use temporal variance for both, so pre/post-fix are dimensionally
     identical.
  2. **N+4 is captured-but-unused** (design line 897). Either include it
     in the transient window (making it a 5-sample variance) or capture
     N..N+3 + N+5 (4 transients + 1 baseline). The "capture but discard"
     pattern is a footgun for the implementer.
  3. **Shift-frame detection is mid-walk** (design § "Capture timing —
     the shift-frame detection", lines 893-906). The system observes
     `Residency::origin().x` each tick in an `Update` system; on the tick
     `origin.x` changes, that tick is `N`. The walk goes through the e2e
     state machine and `pin_streaming_window_camera` adds 4 voxels/tick;
     the FIRST shift happens around the half-window threshold. The design
     line 889 estimates "first shift fires after ~64 voxels of +X walk =
     ~16 ticks" — but with `STREAMING_WALK_VOXELS_PER_TICK = 4.0`, that's
     16 ticks of walk, which is plausible. Implementer needs to verify the
     `Update`-order observability of `origin.x` is *before* the screenshot
     request system fires.
  4. **Threshold (3.0) is unmeasured** (design lines 1015-1020 admits
     this — "Pre-fix verification is what locks in the choice"). The
     gate's analytical power is contingent on the pre-fix run producing
     a clear separation. The design's "if pre-fix barely fails (3.1×),
     the threshold needs widening" path is reasonable but means the gate
     is provisional until the empirical pre-fix run.
- **Evidence in code**: design's claim of inheriting from `streaming_window`
  is verified: the design adds `streaming_taa_shift_noise_mode` as a layer
  on `streaming_window_mode` (design lines 866-873), which is the same
  pattern `streaming_cold_start` uses at `cli.rs:290-292`. The
  `Mutex<Image>` stash pattern is identical to `MID_WALK_IMAGE` at
  `streaming_window.rs:213`.
- **Notes**: the gate design is largely sound. The metric formulation
  needs one more pass to put the baseline on the same dimensional footing
  as the transient; per the brief's criterion 4 spec ("the threshold
  separating the artefact transient from the post-recovery steady state"),
  the design's choice of spatial-baseline weakens the analytical contract.

---

### Criterion 5 — 8-neighbour hash fallback fix at `taa.wgsl:281-283` is correct

- **Verdict**: PASS.
- **Evidence in design**: § "Design — 8-neighbour hash fallback fix" (lines
  743-830).
  - (a) Each neighbour gets its own `ray_dir` via a per-iteration
    `get_ray_dir(params.inv_view_proj, cur_pixel_pos, screen_width,
    screen_height, vec2<f32>(0.0, 0.0))` inside the loop.
  - (b) Helper reuse: `get_ray_dir` is already imported at
    `taa.wgsl:78-83`; verified at the actual code. No new imports needed.
  - (c) Cost: 8 extra `get_ray_dir` per output pixel — design correctly
    notes this is one matrix-vector multiply + perspective divide +
    normalise. The centre's `ray_dir` (`taa.wgsl:245`) is kept for
    `pos_virtual = ray_dir * first_hit_dist` (line 346) which is centre-
    only and unaffected.
- **Evidence in code**: the current loop at `taa.wgsl:269-327` does indeed
  call `get_hit_data_from_planes(cur_first_hit, cam_pos_int, cam_pos_frac,
  ray_dir)` at lines 281-283 with the centre `ray_dir` from line 245.
  Verified by reading the actual shader. The bug is real.
  `get_ray_dir`'s signature at `render_pipeline_common.wgsl:207-219` matches
  the design's call shape exactly.
- **Notes**: clean fix. The reasoning at design lines 769-775 about why
  the centre-`ray_dir` reuse was benign pre-iteration-2 (hash didn't
  depend on `pos`) and broken post-iteration-2 (hash now uses `pos`) is
  the smoking gun.

---

### Criterion 6 — `data_id_lo13` made truly world-absolute

- **Verdict**: PARTIAL — design is structurally correct, but contains
  contradictory layout reasoning that the implementer must resolve.
- **Evidence in design**: § "Design — hash world-absolute correction" (lines
  311-740).
  - (a) World-absolute reference: `residency_origin_voxels = residency.origin
    × SEGMENT_VOXELS` added to `cam_pos_int` at the floor() inside
    `taa_data_id_lo13` (design lines 661-674).
  - (b) Mechanism for shader-side access: repurpose `GpuTaaParams` trailing
    padding. After lengthy back-and-forth, design settles (line 590) on
    "Option C — split into two separate fields", with the final Rust
    layout (lines 593-608) putting `residency_origin_voxels: IVec3` at
    offset 176 and `sample_age: u32` at offset 188 — both fit inside the
    existing 192-byte size.
  - (c) Read/write consistency: both `taa.wgsl:317` (reproject pass) and
    `taa.wgsl:517` (calc_new_taa_sample) go through the same helper, both
    pass the same `residency_origin_voxels` local. Design § "Read/write
    consistency" (lines 731-740) explicitly affirms this.
- **What's missing / wrong**:
  1. **Layout reasoning is contradictory.** Design lines 365-444 contains
     a series of layout attempts, some of which are explicitly wrong
     (one declares `vec4<i32>` would push the struct to 208 bytes — true
     for one variant — and others use `vec3<i32>` to keep 192). The
     final § "Final recommendation — Option C" (lines 590-621) settles
     on the right answer (`IVec3 + u32` packs to 16 bytes via std140's
     `vec3 + scalar = vec4` packing), but the design then immediately
     muddles the WGSL declaration in § 4.a (line 1502-1556) by oscillating
     between `vec4<i32>` and `vec3<i32>` again — eventually landing back
     on the precedent of `cam_pos_int (IVec3 + u32 padding) → vec4<i32>`,
     but with the trailing slot now holding `sample_age` instead of pad.
     The implementer is left to reconcile this against the actual
     `taa.wgsl:114-124` struct, which currently declares `cam_pos_int:
     vec4<i32>`, `cam_pos_frac: vec4<f32>`, then a flat sequence of u32s
     ending in `sample_age` (no explicit padding). The migration shape
     must add `residency_origin_voxels` such that `params.cam_pos_int.xyz`
     keeps its current offset 128, `params.cam_pos_frac.xyz` keeps offset
     144, and the u32 block (`screen_width..sample_age`) keeps its existing
     positions 160..180. There are two clean shapes that achieve this:
     (i) WGSL `residency_origin_voxels: vec4<i32>` declared after
     `taa_index` with `.xyz` = voxels and `.w` = `sample_age` (eliminating
     the `sample_age: u32` line) — Rust mirror `IVec3 + u32` at offset
     176/188; OR (ii) keep `sample_age` declared in WGSL but use the
     `vec3<i32>` ↔ Rust `IVec3 + u32` trick. The design ultimately
     recommends shape (i) (lines 538-562 / 615-637) but in the file-by-file
     change list at § 4.a (lines 1502-1556) starts to backtrack toward
     keeping `sample_age` declared, then re-confirms shape (i). The
     implementer should treat the final file-by-file changelist (item 4)
     as authoritative and ignore the intermediate dead-ends.
  2. **Hash-helper signature pollutes call sites with an extra param.**
     Adding `residency_origin_voxels: vec3<i32>` to `taa_data_id_lo13`'s
     signature (design lines 663-666) forces both call sites at
     `taa.wgsl:317` (reproject pass) and `taa.wgsl:517` (calc_new_taa_sample)
     to pass it. Lower-friction alternative: read the uniform inside the
     helper. The design's choice (pass-as-arg) is defensible — it keeps
     the helper context-free — but worth flagging as a minor footgun if a
     future caller forgets to pass the new arg.
  3. **`StreamingExtractRender.window_origin` is the only chosen source**
     (design § "Plumbing", lines 339-411 / § "Non-streaming preset", lines
     689-728). Verified at `streaming/noise_dispatch.rs:269-277,346,596`
     that `window_origin: bevy::math::IVec3` is the render-world mirror,
     refreshed each `ExtractSchedule` from `residency.window.origin()`.
     For non-streaming presets, `StreamingExtractRender` exists (always
     `init_resource::<StreamingExtractRender>()` at `streaming/mod.rs:323`)
     with default `window_origin: IVec3::ZERO`, so the shader expression
     `cam_pos_int + residency_origin_voxels` degenerates to `cam_pos_int`
     — bit-identical to current behaviour. Good.
- **Evidence in code**: confirmed every cited line. `taa_data_id_lo13`
  is at `taa.wgsl:215-225` (matches design's claim). The shader call
  sites are at `taa.wgsl:317` (reproject) and `taa.wgsl:517` (calc-new).
  `GpuTaaParams` struct at `gpu_types.rs:195-231` has trailing `_pad2/_pad3/_pad4`
  (`gpu_types.rs:226-230`). Size assertion at `gpu_types.rs:874` is
  `assert!(std::mem::size_of::<GpuTaaParams>() == 192)`. WGSL struct at
  `taa.wgsl:114-124` does NOT currently declare `_pad2/_pad3/_pad4` (the
  trailing pad is implicit) — confirmed.
- **Notes**: the design's *substance* is correct (the world-absolute
  composition is the right fix); the *form* of the WGSL/Rust layout
  presentation is meandering and an implementer following the design
  linearly is at high risk of landing the wrong layout. The implementer
  should treat § "File-by-file change list" item 4.a (lines 1502-1556)
  as the canonical layout spec and ignore the §"Plumbing" earlier
  excursions.

---

### Criterion 7 — verification plan

- **Verdict**: PASS.
- **Evidence in design**: § "Verification plan" (lines 1321-1351). Six
  ordered steps, each prefixed with `timeout 180s` (matching the
  `feedback-e2e-gates-must-fail-fast.md` rule):
  1. `cargo build --workspace`
  2. `cargo test --workspace --lib` (with timeout 300s — explicitly
     called out at line 1330 because the test suite is heavier)
  3. `cargo run --release --bin e2e_render -- --gate streaming-cold-start`
  4. `... --gate streaming-window`
  5. `... --gate oasis-edit-visual`
  6. NEW: `... --gate streaming-taa-shift-noise`
  The "one smoke per failing gate" rule is explicitly applied at lines
  1349-1350.
- **Evidence in code**: the named gates exist as `Gate::StreamingColdStart`
  (`cli.rs:413`), `Gate::StreamingWindow` (`cli.rs:397`),
  `Gate::OasisEditVisual` (`cli.rs:457`). The new gate variant addition
  at design § 6 follows the same shape (variant in `Gate` enum,
  `as_kebab_str` arm, `apply_gate_defaults` arm).
- **Notes**: the timeout 180s wrapper is consistent across all gates.
  The pre-fix verification step (line 1339) explicitly runs Step 6
  BEFORE landing the rebase — this is the analytical-validation step
  per `feedback-primitives-then-analytical-invariants.md`. Good.

---

### Criterion 8 — no forbidden actions

- **Verdict**: PASS.
- **Evidence in design**: the verification plan uses ONLY:
  `cargo build`, `cargo test --workspace --lib`, and
  `cargo run --release --bin e2e_render -- --gate <name>`. There is no
  `cargo run --bin bevy-naadf` smoke-test. The "one smoke per failing
  gate" rule is correctly applied to the gate runs only.
- **Evidence in code**: project `CLAUDE.md` (read) explicitly forbids
  `cargo run --bin bevy-naadf` as a verification step; the design respects
  this. `subagent-gpu-app-verification-loop.md` is cited at line 1349.
- **Notes**: the design also avoids any "boot the binary for N seconds
  and confirm clean exit" pattern. It correctly delegates the visual check
  to the user.

---

## Highest-risk thing in the design

The **`var_baseline` formulation in the new e2e gate** (Criterion 4,
point 1). The design pivots mid-section from a temporal-temporal
comparison to a temporal-spatial comparison without empirically
demonstrating that the temporal/spatial dimensional mismatch is robust
across scenes with varying spatial-noise characteristics in the shadowed
band. The threshold `3.0` was picked qualitatively, not measured. On
landing, the most likely silent failure mode is: pre-fix the gate fails
loudly (good); post-fix the gate passes (good); but the gate's
analytical power then degrades silently as the scene's shadowed-band
spatial variance changes between runs (e.g. a different `noise_seed` or
a tweak to the GI bounce intensity), making the gate either flap or
mask future regressions. The mitigation the design promises ("if pre-fix
barely fails, widen the threshold") only locks in the threshold once;
it does not address the dimensional mismatch.

Second-highest risk: the **WGSL/Rust layout for the `residency_origin_voxels`
field** (Criterion 6, point 1). The design contains multiple internally
contradictory layout attempts before landing on the final shape; an
implementer reading top-to-bottom rather than treating § 4.a's
file-by-file changelist as the canonical spec could land the wrong
layout (a struct that compiles but writes the WGSL uniform at offsets
the Rust struct doesn't read from). The 192-byte size assertion at
`gpu_types.rs:874` will catch a Rust-side mistake at compile time, but
a WGSL/Rust offset mismatch (the same trap that produced the Batch-6
black-frame bug, documented at `taa.wgsl:95-105`) would compile cleanly
and silently mis-read values at the read site.

## Recommended next action

**Proceed with two named amendments**, applied during implementation:

1. **Pin the `var_baseline` formulation to a temporal window** (capture
   N+5..N+8 = a 4-sample post-recovery window, compute its per-pixel
   temporal variance over the same shadowed-band selector, and use that
   as `var_baseline`). This makes the ratio dimensionally consistent and
   removes the scene-dependent spatial-variance sensitivity. If the
   implementer cannot make this change without significantly more capture
   ticks, fall back to the design's spatial baseline but log both metrics
   in the assertion output for the user's live inspection.
2. **Treat § 4.a (`File-by-file change list` item 4, design lines
   1502-1556) as the canonical WGSL/Rust layout spec** and ignore the
   meandering analysis in § "Plumbing" (lines 335-687). The implementer
   should add the size assertion + a one-shot CPU-side test that writes a
   sentinel `residency_origin_voxels = IVec3(0x11111111, 0x22222222,
   0x33333333)` into the uniform, dispatches a trivial compute, and reads
   back via a debug output buffer to confirm the WGSL sees the same byte
   pattern. This catches the WGSL/Rust offset-mismatch class of bugs at
   land-time instead of at "Why is the post-shift hash zero?" diagnosis-
   time.

With those two amendments, the design is sound and the implementation
should proceed. Verdict: **fix-then-ship**.

