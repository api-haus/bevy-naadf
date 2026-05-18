# 01-context — web-vox-color-divergence

Canonical context bundle for the diagnose-first investigation of a
web-only voxel color divergence. Every non-review agent reads this file
first, end-to-end, before any other action.

All paths in this file are absolute and refer to the worktree at
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`.

---

## Goal (restated verbatim from the originating handoff)

> The /delegate orchestration at `docs/orchestrate/web-vox-async-loading/`
> delivered async `.vox` loading on web + native with closed-loop e2e
> gates. **The async-loading half is complete**: all 9
> architect-designed implementation steps (Q1–Q7) landed, the
> wasm-bindgen-rayon `no-bundler` fix is in, the downstream W2/W5
> label-leak re-allocation gates are in, and 7/8 verification gates pass
> (the 8th is unrelated R2-404 CORS — out of scope).
>
> Then the user ran the headed web build and observed:
>
> - ✓ The `.vox` loads (overlay clears; HTTP fetch + rayon parse + async
>   GPU readback all complete).
> - ✓ Hovering on voxels reveals each voxel has distinct types; the
>   types look correct (40, 45 prominent on rooftops — matches expected
>   `dot_vox` palette indices for the Oasis fixture).
> - ✗ But voxel colors render as **near-black** (dark blue-gray tones
>   across the whole scene, sky gradient unaffected).
>
> Native renders the same fixture with **full colors** (sandy beige
> buildings, green palm trees, dark roof tiles, doors, ladders, all the
> expected Oasis aesthetic). So this is a **web-only material/palette
> divergence** introduced somewhere in the async parse+install split —
> not a fundamental rendering bug.

The user's exact words after observing the bug live: *"it actually
loaded the vox! hovering on voxels reveals that every voxel has distinct
type and the types look correct from what i see - 40, 45 are prominent
on rooftops and i managed to catch same types. colors are black tho for
some reason"*.

So the bug is real, narrow (just colors), and the async-loading half of
the goal is unambiguously won. The diagnose session is for the
color/material divergence only — do not pull it back into a larger
scope.

---

## Visual evidence

**Bug — web build, near-black voxel render:**
- Absolute path: `/home/midori/.claude/image-cache/54dc586b-43a7-4520-ba35-363920ed03a7/1.png` (142 KB PNG, verified to exist).
- Prose: dark blue-gray cityscape silhouette. Building outlines,
  structures, towers visible by their darker shapes against the lighter
  sky. Sky gradient at top correct. Scene is mostly very dim with
  subtle shading — geometry is present, lighting is present, but
  materials/colors are flat-dark.

**Correct reference — native gate's `--vox-web-parity-loaded` capture:**
- Absolute path: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/target/e2e-screenshots/vox_web_parity_loaded.png` (139 KB PNG, verified to exist).
- Prose: same Oasis fixture, fully colorful — sandy beige building
  walls, green palm trees, dark stone-tile roofs, wooden doors,
  ladders. This is what the web build *should* look like.

Diff between the two: geometry identical, voxel types identical (user
verified by hovering), lighting/exposure comparable (web isn't pitch
black — there's shading), but **the per-voxel material/color lookup is
broken on web**.

Sub-agents that need to compare these visually MUST use the `Read` tool
on the absolute paths above; conversation-relative identifiers like
"image 1" or "the screenshot" resolve to nothing for a fresh agent.

---

## Binding decisions from Step 4 Q&A

Each decision is cited with the question header + the option label the
user chose. Treat each as a hard constraint on agent behaviour.

### Decision 1 — Mode: Distributed

> **Q (header "Mode"):** The Step 2.5 analysis recommends distributed
> mode … Confirm mode?
> **A:** Distributed (recommended).

Phase gates between research / design / impl / review. Each phase
dispatch must Write its findings to its group file under
`docs/orchestrate/web-vox-color-divergence/` before returning. Agent
return text is for short status only.

### Decision 2 — Diagnose posture: Instrument first, then fix

> **Q (header "Diagnose posture"):** The audit's hypothesis (build-once
> gate × async install collision) is strong but unverified by
> observation. Diagnose-first protocol options:
> **A:** Instrument first, then fix (recommended).

The research agent (Step 6a) must:

1. Add a **one-shot diagnostic `info!` log** at the GPU palette-upload
   site (`crates/bevy_naadf/src/render/prepare.rs:487`, the
   `voxel_types.upload_all(...)` site) that emits a palette signature:
   palette length + the first N (suggest N=5) entries' `color_base`
   channels as f16-decoded RGB triplets. Label the log with a stable
   prefix like `[palette-upload]` so it can be grepped from `wasm-pack`
   / `trunk` console output and from native `cargo run` stdout.
2. Add a matching log at the `install_imported_vox` palette-write site
   (`crates/bevy_naadf/src/voxel/grid.rs:580`) reporting the palette
   signature being inserted into the main world. Same `[palette-...]`
   prefix scheme.
3. Run the headed web build (`cd crates/bevy_naadf && trunk serve` on
   port 8080; open `http://localhost:8080/?vox=/test-fixtures/oasis_hard_cover.vox`
   in headed Chrome) AND the native gate
   (`timeout 120s cargo run --bin e2e_render -- --vox-web-parity` is
   the closest existing capture; the audit notes `assert_vox_geometry_visible`
   already logs per-channel mean).
4. Observe and report: does `prepare_world_gpu` upload exactly once?
   With the default palette or the .vox palette? If once-with-default,
   does any later `[palette-upload]` log line appear after the .vox
   install? The answer to this triplet **confirms or refutes** the
   audit's hypothesis.

**Do not propose any fix in the research phase.** Even if the
instrumentation makes the answer obvious. The architect's brief
(Step 6b) is the place for fix shapes; the user has a hard gate
between the two.

### Decision 3 — Fix shape: Architect picks; user reviews at design hard gate

> **Q (header "Fix direction"):** Once root cause is confirmed, which
> fix-shape direction should the architect explore?
> **A:** Architect picks; I'll review the design.

The architect surveys the three candidate shapes from the audit:

1. **Re-buildable extract path with `Changed<T>` queries.** Add
   `Changed<VoxelTypes>` / `Changed<WorldData>` (and possibly
   `Changed<ModelData>`) queries to `stage_world_gpu_buildonce`
   (`crates/bevy_naadf/src/render/extract.rs:191-227`) and
   `stage_model_data_buildonce` (`crates/bevy_naadf/src/render/extract.rs:242-259`).
   On change, remove `WorldGpu` so the prepare-stage build-once gate
   re-opens. Cleanest Bevy idiom; minimal new machinery; the audit
   notes zero `Changed<T>` queries exist on these resources today.
2. **Cache-invalidate at install site.** In `install_imported_vox`
   (`crates/bevy_naadf/src/voxel/grid.rs:480-581`), before the
   `commands.insert_resource(...)` calls, schedule a
   `commands.remove_resource::<WorldGpu>()` (and possibly the
   `WorldGpuStaging` extract) so the next ExtractSchedule re-runs from
   scratch. More explicit, less idiomatic; the audit notes
   `remove_resource::<WorldGpu>` is grep-zero across the crate today.
3. **Suppress default scene during pending .vox.** Add a
   `WebAsyncVoxPending` marker resource around the
   `startup_fetch_default_vox` flow; have `setup_test_grid` short-circuit
   when the marker is present (and have `apply_pending_vox` /
   `poll_pending_vox_parse` install the scene only after the .vox lands).
   Addresses the root cause but leaves the build-once architectural gap
   unfixed — future "live re-import" / "scene reload" features would
   re-encounter it.

Recommendation + decision matrix (trade-offs across correctness,
performance impact, idiom-fit, residual architectural risk) goes in
`03-design.md`. The user reviews and confirms the shape before
implementation begins. **Do not pre-commit to a shape in the
research-phase findings.**

### Decision 4 — Gate extension: Yes, in this orchestration

> **Q (header "Gate extension"):** The audit found that `--vox-web-parity`
> (SSIM-only) would NOT have caught this regression … Extend gate(s)
> with per-channel color-spread assertion in this orchestration?
> **A:** Yes — extend within this orchestration (recommended).

The implementation phase (Step 6c) lands two gate extensions alongside
the color fix:

1. **`assert_vox_geometry_visible`** (`crates/bevy_naadf/src/e2e/vox_e2e.rs:402-433`):
   the function already calls `framebuffer.region_mean(...)` which
   returns `[f32; 4]` per-channel. Promote the assertion from
   luminance-only to require per-channel max ≥ floor (suggest 20.0 on
   the 0–255 scale, but architect picks the exact threshold based on
   the native reference capture). Continue to log the full RGBA mean.
2. **`vox_web_parity` `loaded` phase** (`crates/bevy_naadf/src/e2e/vox_web_parity.rs:117-190`):
   in addition to the SSIM compare, assert per-channel mean spread is
   non-trivial on the `vox_web_parity_loaded.png` capture (the audit
   notes SSIM-on-skybox-baseline misses color-bleed regressions
   entirely).

The architect picks exact thresholds + the helper function shape (likely
new helper in `crates/bevy_naadf/src/e2e/framebuffer.rs` building on
`region_mean`). Implementation must keep the existing gates green and
make both extended gates **fail** on a near-black render.

---

## Reuse audit summary (read `00-reuse-audit.md` for the full 24-row
table + borderline calls)

**Smoking-gun finding:**

- `stage_world_gpu_buildonce` (`crates/bevy_naadf/src/render/extract.rs:191-227`,
  gate at `:201-203`) and `prepare_world_gpu`
  (`crates/bevy_naadf/src/render/prepare.rs:184-583`, gate at
  `:201-203`) are **build-once** — gated on `is_some()` of the staging /
  output resource. The build-once docstring at `extract.rs:60-66`
  explicitly admits: *"If a future feature ever needs a whole-world
  re-upload (e.g. world reload or live re-import), it re-creates this
  resource at that boundary — but no such code path exists today."*
- On web: `setup_test_grid`
  (`crates/bevy_naadf/src/voxel/grid.rs:104-143`) dispatches on
  `args.grid_preset == GridPreset::Default` and calls
  `install_default_embedded_in_fixed_world`
  (`crates/bevy_naadf/src/voxel/grid.rs:220-313`), which inserts
  `WorldData + VoxelTypes` at lines 311-312 with a 13-entry default
  palette. This runs at Startup, BEFORE the first
  `ExtractSchedule`. Schedule ordering at
  `crates/bevy_naadf/src/lib.rs:758-791` confirms.
- The first `ExtractSchedule` runs `stage_world_gpu_buildonce` with the
  default `VoxelTypes`, then `prepare_world_gpu` uploads the default
  palette to the GPU `GrowableBuffer<GpuVoxelType>` at
  `prepare.rs:487` (`voxel_types.upload_all(...)`). Build-once gate
  closes.
- N frames later, the rayon parse completes; `poll_pending_vox_parse`
  (`crates/bevy_naadf/src/voxel/async_vox.rs:81-153`) calls
  `install_imported_vox` (`crates/bevy_naadf/src/voxel/grid.rs:480-581`)
  via `Commands`. `install_imported_vox` overwrites the main-world
  `VoxelTypes` at `grid.rs:580` with the .vox palette. **No system
  re-extracts.**
- Native doesn't hit this: `GridPreset::Vox { path }` in
  `setup_test_grid` calls `install_vox_in_fixed_world` synchronously at
  Startup — `WorldData + VoxelTypes` with the .vox palette are inserted
  BEFORE the first `ExtractSchedule` runs.

**Negative findings (also load-bearing):**

- Zero `Changed<VoxelTypes>` / `Changed<WorldData>` /
  `Changed<ModelData>` queries exist anywhere in the codebase.
- `commands.remove_resource::<WorldGpu>()` is grep-zero across the
  crate.
- The only `ResMut<WorldData>` is the editor's brush at
  `crates/bevy_naadf/src/editor/mod.rs:140`.
- No invalidation / re-build plumbing exists for the world GPU mirror.

**Failure mechanic confirmed in shader code:**

- `crates/bevy_naadf/assets/shaders/render_pipeline_common.wgsl:102-114`
  `decompress_voxel_type(comp)`: unpacks the 4-u32 entry back to
  `color_base: vec3<f32>` etc.
- `crates/bevy_naadf/assets/shaders/naadf_first_hit.wgsl:228-235`:
  `let voxel_type = decompress_voxel_type(voxel_types[ray_result.hit_type])`
  then `acc.absorption = acc.absorption * voxel_type.color_base`. If
  `voxel_types[i]` is the empty-default (all-zero) entry,
  `color_base = Vec3::ZERO` and every absorbed ray multiplies by zero →
  near-black output.

The audit's full candidate table + borderline calls are in
`00-reuse-audit.md`.

---

## Required reading (per-phase)

### Research agent (Step 6a) — read in order:

1. `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/01-context.md` (this file).
2. `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/00-reuse-audit.md` (the full audit).
3. **Required source reading with line ranges** (the audit cites
   precise lines — verify before instrumenting):
   - `crates/bevy_naadf/src/voxel/grid.rs:104-143` — `setup_test_grid`
     dispatch on `GridPreset`. Why: confirms default-scene runs at
     Startup on web.
   - `crates/bevy_naadf/src/voxel/grid.rs:220-313` —
     `install_default_embedded_in_fixed_world`. Why: where the default
     palette gets inserted.
   - `crates/bevy_naadf/src/voxel/grid.rs:480-581` —
     `install_imported_vox`. Why: the post-parse install site; the
     palette-write at line 580 is one of the two instrumentation points.
   - `crates/bevy_naadf/src/voxel/async_vox.rs:81-153` —
     `poll_pending_vox_parse`. Why: confirms commands-based dispatch
     into `install_imported_vox`.
   - `crates/bevy_naadf/src/voxel/web_vox.rs:361-413` and `:437-455` —
     `apply_pending_vox` + `spawn_wasm_vox_parse`. Why: web async entry.
   - `crates/bevy_naadf/src/render/extract.rs:60-66`, `:191-227`,
     `:242-259` — `stage_world_gpu_buildonce` + the docstring;
     `stage_model_data_buildonce`. Why: the build-once gates.
   - `crates/bevy_naadf/src/render/prepare.rs:184-583`, especially
     `:201-203` (gate) and `:487` (palette upload) and `:380-388`
     (palette conversion). Why: the GPU upload site to instrument.
   - `crates/bevy_naadf/src/lib.rs:758-791` — startup/update system
     registration. Why: schedule ordering on web.

### Architect agent (Step 6b) — read in order:

1. `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/01-context.md` (this file).
2. `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/00-reuse-audit.md`.
3. `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/docs/orchestrate/web-vox-color-divergence/02-research.md`,
   especially its `## Decisions & rejected alternatives` and
   `## Assumptions made` sub-sections (the load-bearing trace from the
   research phase).
4. All source files cited in the research's confirmed root-cause
   analysis (the research will name them).

### Implementer agent (Step 6c) — read in order:

1. This file.
2. `00-reuse-audit.md`.
3. `02-research.md` — same sub-section emphasis.
4. `03-design.md`, **especially** its `## Decisions & rejected alternatives`
   and `## Assumptions made` sub-sections.

### Review agent (Step 6d) — read ONLY:

- `05-review.md`. Do **not** read `01-context.md`, `02-research.md`, or
  `03-design.md`. The reviewer is deliberately denied the design
  rationale so it can catch assumptions the implementer silently baked
  in. The orchestrator reconciles the reviewer's flags against full
  context at the Step 7 synthesis.

---

## Forbidden moves (binding for every agent)

Verbatim from the handoff's "What NOT to do":

1. **Do NOT add a `#[cfg(target_arch = "wasm32")]` branch in the render
   path.** Decision 2 from the prior orchestration's `01-context.md` is
   binding: web identical to native. The fix is to find the asymmetry
   in the install/upload path, not to special-case the renderer.
2. **Do NOT undo any of the async-loading work.** It works. The
   parse-off-thread, async GPU readback, no-bundler rayon, removed
   interim wasm32 hack, new e2e gate, Playwright SSIM spec — all proven
   by the verification gates.
3. **Do NOT introduce a sync `.vox` path on web as a workaround.** The
   point was to remove the UI freeze.
4. **Do NOT use `cargo run --bin bevy-naadf` as a verification step.**
   Project rule (`CLAUDE.md` at worktree root). Use the e2e gates and
   the `trunk serve` headed Chrome window only for live visual
   inspection.
5. **Do NOT mock GPU work in tests.** Both targets run real
   wgpu/WebGPU pipelines.
6. **Do NOT use headless Playwright.** Memory
   `playwright-e2e-must-be-headed.md`: bevy-naadf `e2e/` suite must
   always run `--headed`; headless Chromium WebGPU dies with
   `DeviceLost` mid-render and hides real failures.
7. **No ranked-hypothesis lists in the research output.** Pick one
   hypothesis after observation, verify, move on.
8. **No `--no-verify` on commits.**

Additions specific to this orchestration:

9. **Do NOT speculatively-fix in the research phase.** Even when the
   instrumentation makes the answer obvious. Hard gate between research
   and design — see Decision 3.
10. **Do NOT widen scope beyond the color divergence.** The R2-404 CORS
    bug (out-of-scope per handoff) and the fresh-eyes reviewer dispatch
    of the prior `web-vox-async-loading` orchestration are explicitly
    excluded.
11. **Do NOT delete the diagnostic logs after the fix lands.** Convert
    them to `debug!` (so they're off by default but available with
    `RUST_LOG=bevy_naadf=debug`) — they catch the regression class for
    future bug reports. The architect picks `debug!` vs `trace!`; the
    implementer applies it.
12. **Do NOT loop on GPU-app gates as a sub-agent.** Memory
    `subagent-gpu-app-verification-loop.md`: sub-agents must not
    rebuild→rerun GPU app gates in a loop. One smoke-run max; visual
    checks are the user's.

---

## Verification surface for the fix (handoff verbatim + gate-extension addition)

Once a hypothesis-then-fix is applied:

1. `cargo build --workspace` — must remain green.
2. `cargo build --target wasm32-unknown-unknown --bin bevy-naadf --no-default-features --features webgpu` — must remain green.
3. `cargo test --workspace --lib` — must remain green (184 tests passed
   at last run).
4. `timeout 120s cargo run --bin e2e_render -- --vox-web-parity` — must
   remain green AND now exercise the new per-channel color-spread
   assertion (Decision 4).
5. `timeout 120s cargo run --bin e2e_render -- --vox-e2e` — must remain
   green AND the upgraded `assert_vox_geometry_visible` per-channel
   assertion must pass on the native capture.
6. `timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual` —
   must remain green.
7. `timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle` — must
   remain green.
8. `timeout 300s just test-wasm` — the .vox-loading tests must remain
   green AND the loaded-phase canvas screenshot must now show colorful
   materials (not the near-black current state).

After the fix lands, the extended gate(s) must be **verified to fail on
the pre-fix state** — temporarily revert the fix, run the gate, confirm
the assertion fires. This proves the gate actually catches the
regression class. Architect/implementer must include this verification
in `04-impl.md`.

---

## Worktree state

- **Path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`
- **Branch:** `feat/web-vox-streaming`
- **HEAD at orchestration start:** `okd76481c` — `docs(web-vox-async-loading): close orchestration — async-loading deliverable shipped; web-color divergence ha…`
- **Working tree:** clean.

All the async-loading work (commits `1ac6f0b6` → `4e54c7a7` → `7dc739a`
→ `162c40b8` → `okd76481c`) is committed. The diagnose investigation
builds **on top of** this state.

---

## Out of scope (do not touch)

- `wasm-smoke.spec.ts` CORS-on-404 failure (pre-existing in this
  branch; tackled in a separate session).
- Fresh-eyes reviewer dispatch of the prior `web-vox-async-loading`
  orchestration. If wanted, dispatch separately against
  `docs/orchestrate/web-vox-async-loading/05-review.md`.
