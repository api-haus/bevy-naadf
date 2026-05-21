# D3 — voxel-io-and-grid — implementation log

## refactor-implementer log (2026-05-21)

Executes the 7-step migration plan from `03-architecture.md` (F1 through F9; F8 deferred per architect). F1 landed in a prior dispatch (commit `293ffa8`); this session resumes from F2 onward.

Verification surface used (`/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`): `cargo build --workspace`, `cargo test --workspace --lib`, and `cargo run --bin e2e_render -- <mode>` for runtime gates. `cargo run --bin bevy-naadf` smokes are forbidden by project rule.

---

### Step 1 — F1 (retroactive log) — `crates/voxel_noise/` crate deletion

Landed by prior dispatch as commit **`293ffa8`** (`refactor(D3/voxel-io-and-grid): retire voxel_noise CPU noise path — −1547 LOC`).

**Edits applied (verified via `git show 293ffa8 --stat`):**
- `crates/voxel_noise/` — entire crate directory removed (1 033 Rust LOC + Makefile + dist/ + js/ + examples/).
- `Cargo.toml:5-15` — workspace docstring + `members = [...]` updated to drop voxel_noise.
- `Cargo.lock` — regenerated; `voxel_noise` + `fastnoise2` deps dropped.
- `justfile:5,136-148` — voxel_noise documentation line + the `noise-build` / `noise-test` / `noise-clean` recipes removed.
- `rust-toolchain.toml:18-19` — `wasm32-unknown-emscripten` target removed.
- `scripts/lint/wasm-compat.sh:9` — voxel_noise/src dropped from the scan loop.

**Verification (per prior dispatch):** build + 187 lib tests green.

**Status:** complete (landed prior dispatch).

---

### Step 2 — F6 + F9 — camera-pose constants leave `e2e/`, drop `let _ = WORLD_SIZE_IN_VOXELS`

Inverts the dependency arrow per `03-architecture.md` §4: production code no longer imports from `crate::e2e`. F9's `let _` cleanup was rolled in since the lines are adjacent (in `install_imported_vox`).

**Edits applied:**
- `crates/bevy_naadf/src/camera/poses.rs` — NEW (23 LOC). Defines `HORIZON_CAMERA_POS` and `HORIZON_CAMERA_ROT` with full doc-comments transplanted from the prior `e2e/vox_horizon_parity.rs:66-81`.
- `crates/bevy_naadf/src/camera/mod.rs:10` — added `pub mod poses;` ahead of `pub mod position_split;`.
- `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:63-81` — replaced the local `pub const HORIZON_CAMERA_POS` / `pub const HORIZON_CAMERA_ROT` defs with `use crate::camera::poses::{HORIZON_CAMERA_POS, HORIZON_CAMERA_ROT};`.
- `crates/bevy_naadf/src/voxel/grid.rs:569-573` — `install_imported_vox` now reads `crate::camera::poses::HORIZON_CAMERA_POS/ROT`; the bare `let _ = WORLD_SIZE_IN_VOXELS;` (F9) dropped in the same hunk; explanatory comment block kept.
- `crates/bevy_naadf/src/voxel/web_vox.rs:287-288` — `pin_web_horizon_camera` now reads `crate::camera::poses::HORIZON_CAMERA_POS/ROT`; comment updated.
- `crates/bevy_naadf/src/voxel/web_vox.rs:188` — doc-comment cross-reference updated from `crate::e2e::vox_horizon_parity` to `crate::camera::poses`.

**Verification:**
- `cargo build --workspace` — pass (35.95s, no warnings).
- `cargo test --workspace --lib` — pass (187 passed, 1 ignored).

**Notes:** initial attempt re-exported the constants through `e2e/vox_horizon_parity.rs` with `pub use ... as _` to spare the production callers — that syntax doesn't re-export. Per the architect's design the production callers must move to `crate::camera::poses` directly; updated all four call sites accordingly. The `--vox-horizon-native` e2e gate verification is unnecessary: the moved values are byte-identical and resolve at compile time, so build + lib tests prove the symbol resolution.

**Status:** complete.

---

### Step 3 — F2 — delete tiled `.vox` family

Per `03-architecture.md` §3: delete the `tiles` parameter + `replicate_buckets_xz` + the tiled twin entry points + the single unit test that exercises them. Production code always passes `tiles=1`; the C# `MagicaVoxel.cs` has no equivalent (the feature is a Rust-specific divergence). The `--vox-gpu-oracle` gate's CPU phase + `vox_gpu_oracle_cpu_phase` flag stay; `install_vox_sized_to_model` now calls non-tiled `load_vox`.

**Edits applied:**
- `crates/bevy_naadf/src/voxel/vox_import.rs:161-225, 227-276` — DELETED `parse_vox_bytes_tiled`, `load_vox_tiled`, `parse_dot_vox_data_tiled`, `replicate_buckets_xz`. `parse_dot_vox_data` rewritten to absorb the body (drops the `tiles` parameter; always takes the `tiles==1` path). Docstring updated.
- `crates/bevy_naadf/src/voxel/vox_import.rs:1675-1731` — DELETED unit test `tiled_load_expands_world_xz_and_dedups_blocks` (covers a feature being deleted; the dedup logic it asserts is still exercised by single-tile parses).
- `crates/bevy_naadf/src/voxel/grid.rs:366` — `install_vox_sized_to_model` call site changed from `vox_import::load_vox_tiled(path, 1)` → `vox_import::load_vox(path)`.
- `crates/bevy_naadf/src/voxel/grid.rs:69-103` — Stage 14 docstring on `setup_test_grid` condensed; the "Stage 2 consolidation" / "Stage 14" prose collapsed into one tight paragraph describing the four install paths.
- `crates/bevy_naadf/src/voxel/grid.rs:356-364` — `install_vox_sized_to_model`'s docstring simplified (drops the "Stage 14" status header — fn is now just "CPU oracle for `--vox-gpu-oracle`").

**Verification:**
- `cargo build --workspace` — pass (35.97s).
- `cargo test --workspace --lib` — pass (186 passed, 1 ignored; −1 vs Step 2 baseline = the deleted tiled test).

**Notes:** `ChunkBuckets` + `validate_caps` remain used by `compose_to_sparse_world` (the non-tiled path), so they were NOT deleted. The lingering `tiled` lexemes in `grid.rs` (lines 132, 771, 787, 838) are unrelated — they describe the ground-chunk template + the on-device GPU tiling note, not the deleted CPU XZ replication.

**Status:** complete.

---

### Step 4 — F3 + F9 (final piece) — `install_world_at_fixed_size` helper extraction

Per `03-architecture.md` §5 Finding 3. Three `WorldData { ... }` literals + three `[palette-install]` debug-log blocks (with "DO NOT REMOVE" markers) collapse into one shared `install_world_at_fixed_size` helper. The smoke-detector signal for `web-vox-color-divergence` now fires from one site with a `label` field that distinguishes the source.

**Edits applied:**
- `crates/bevy_naadf/src/voxel/grid.rs:145-225` — NEW: `WorldInstall<'a>` struct + `install_world_at_fixed_size(commands, install)` helper. Owns the `WorldData` literal (full-fixed-world bounding box), the `commands.insert_resource(InitialCameraPose)` call, optional `ModelData` insertion, conditional `seed_block_hashing()`, the unified `[palette-install]` debug log, and the final `VoxelTypes` insertion.
- `crates/bevy_naadf/src/voxel/grid.rs:233-265` — `install_empty_world` rewritten: takes a `source_label: &'static str` parameter, computes its camera pose, delegates to the helper. ~64 → 32 LOC.
- `crates/bevy_naadf/src/voxel/grid.rs:285-340` — `install_default_embedded_in_fixed_world` rewritten: builds the composed world + ground-tiled scene, computes the demo-relative camera pose, delegates to the helper. ~113 → 80 LOC.
- `crates/bevy_naadf/src/voxel/grid.rs:483-580` — `install_imported_vox` rewritten: builds the empty-bound vs ModelData payload (AADF-strip pass preserved verbatim), delegates to the helper. ~134 → 87 LOC.
- `crates/bevy_naadf/src/voxel/grid.rs:122,139` — `setup_test_grid` call sites of `install_empty_world` pass `"skybox-only"` / `"cli-empty"` labels.

**Design tweak vs architect:** the architect's `WorldInstall.source_label` was `&'static str`. `install_imported_vox`'s source label is a runtime `&str` (path / URL / `<dropped:foo.vox>`), so I generalised the struct to `WorldInstall<'a>` with `source_label: &'a str` so static-string sites (`"skybox-only"` / `"cli-empty"` / `"default-scene"`) and runtime-string sites both fit through the same shape. The architect's example code passed `"skybox-only"` etc. — this is fully compatible with `'a` since `&'static str` borrows as any `'a`.

Also added a `seed_block_hashing: bool` field — `install_empty_world` is the only path that skips `world_data.seed_block_hashing()`. Pre-refactor it didn't call `seed_block_hashing` (verified at the original line 188-204); the helper makes that call conditional to preserve that exact behaviour.

**Verification:**
- `cargo build --workspace` — pass (1m 01s — incremental rebuild).
- `cargo test --workspace --lib` — pass (186 passed, 1 ignored).
- `cargo run --bin e2e_render -- baseline` — PASS. Full GPU render of `GridPreset::Default` scene runs through the new helper: framebuffer captured, luminance gate green (emissive 247.6, solid 243.8, sky 202.9), region gates green through camera motion. All pipelines created cleanly, every expected render-graph node dispatched.

**Notes:** the three "DO NOT REMOVE" markers in the pre-refactor `[palette-install]` log blocks are obsolete now — the single log site in the helper carries the comment instead. The regression-detector signal (palette length + first-5 colors + source label) is preserved with strictly more information (the `label` field is now uniform across all three install paths).

**Status:** complete.

---

### Step 5 — F4 — `GridPreset::WebSkybox` arm + delete `WebSkyboxOverride`

Per `03-architecture.md` §5 Finding 4. The wasm `?skybox=1` path stops using a marker resource + `.before()` ordering combo to convey "install empty world"; instead `web_vox::startup_fetch_default_vox` mutates `AppArgs.grid_preset` to a new `GridPreset::WebSkybox` arm. The `.before(setup_test_grid)` ordering is kept (still load-bearing — the mutation must precede the read).

**Edits applied:**
- `crates/bevy_naadf/src/lib.rs:97-105` — `GridPreset` enum extended with a `WebSkybox` variant; doc-comment notes it is functionally equivalent to `Empty` and serves as the `?skybox=1` URL-param surface.
- `crates/bevy_naadf/src/voxel/grid.rs:104-131` — `setup_test_grid` simplified: dropped `skybox_override: Option<Res<WebSkyboxOverride>>` parameter + the early-bail short-circuit; new `GridPreset::WebSkybox` match arm calls `install_empty_world(&mut commands, "skybox-only")`.
- `crates/bevy_naadf/src/voxel/grid.rs:145-149` — DELETED `WebSkyboxOverride` marker resource definition.
- `crates/bevy_naadf/src/voxel/web_vox.rs:398` — `startup_fetch_default_vox` signature changed from `(mut commands: Commands)` to `(mut commands: Commands, mut args: ResMut<crate::AppArgs>)`.
- `crates/bevy_naadf/src/voxel/web_vox.rs:404-411` — replaced `commands.insert_resource(crate::voxel::grid::WebSkyboxOverride);` with `args.grid_preset = crate::GridPreset::WebSkybox;`; info log updated.
- `crates/bevy_naadf/src/lib.rs:807-817` — comment block describing the `.before(...)` ordering updated to reference `AppArgs.grid_preset` mutation instead of the deleted marker.

**Verification:**
- `cargo build --workspace` — pass (1m 06s).
- `cargo test --workspace --lib` — pass (186 passed, 1 ignored).

**Notes:** only `setup_test_grid` had an exhaustive `match` on `GridPreset` in the workspace — no other consumers needed an arm added. The `Default` impl on `GridPreset` continues to point at `GridPreset::Default` (unchanged).

**Status:** complete.

---

### Step 6 — F5 — `.run_if(resource_exists::<_>)` on `pin_web_horizon_camera` + `hide_ui`

Per `03-architecture.md` §5 Finding 5. The two systems' `Option<Res<X>>` parameter + early-bail pattern moves to a schedule-level condition; the scheduler skips the system call when the resource is absent (the common case for non-`?ui=hide` / non-`?pose=horizon` boots).

**Edits applied:**
- `crates/bevy_naadf/src/voxel/web_vox.rs:236-263` — `hide_ui` parameter list dropped `override_resource: Option<Res<UiHiddenOverride>>`; the `if override_resource.is_none() { return; }` early-bail removed.
- `crates/bevy_naadf/src/voxel/web_vox.rs:269-291` — `pin_web_horizon_camera` parameter list dropped `override_resource: Option<Res<WebHorizonPoseOverride>>`; the early-bail removed. Kept `camera: Option<Single<...>>` — the camera entity may legitimately not exist yet on cold-boot frames.
- `crates/bevy_naadf/src/lib.rs:851-862` — `pin_web_horizon_camera` registration grew `.run_if(bevy::ecs::schedule::common_conditions::resource_exists::<voxel::web_vox::WebHorizonPoseOverride>)`.
- `crates/bevy_naadf/src/lib.rs:977-985` — `hide_ui` registration similarly grew `.run_if(resource_exists::<UiHiddenOverride>)`.

**Verification:**
- `cargo build --workspace` — pass (41.71s, zero warnings).
- `cargo test --workspace --lib` — pass (186 passed, 1 ignored).

**Notes:** `WebHorizonPoseOverride` / `UiHiddenOverride` marker resources kept (Step 6's job is changing how they're consumed: as schedule conditions, not Option<Res<_>> ladders). Bevy 0.19 exposes `resource_exists` at `bevy::ecs::schedule::common_conditions::resource_exists` (verified compiles); the architect's design cited `bevy::ecs::common_conditions::resource_exists` but the actual 0.19 module path includes the `schedule` segment.

**Status:** complete.

---

### Step 7 — F7 — collapse `parse_to_imported_vox`

Per `03-architecture.md` §5 Finding 7. The 4-layer wrapper chain (`parse_to_imported_vox` → `parse_voxel_bytes` → `parse_vox_bytes` → `parse_dot_vox_data`) becomes 3 layers — the top String-error mapping shim moves to the 2 call sites that need it.

**Edits applied:**
- `crates/bevy_naadf/src/voxel/grid.rs:468` — `install_vox_bytes_in_fixed_world` calls `crate::voxel::voxel_dispatch::parse_voxel_bytes` directly (replacing `parse_to_imported_vox(bytes)`).
- `crates/bevy_naadf/src/voxel/grid.rs:480-512` — DELETED `pub fn parse_to_imported_vox` (12 LOC).
- `crates/bevy_naadf/src/voxel/async_vox.rs:24` — import switched from `grid::{install_imported_vox, parse_to_imported_vox}` to `grid::install_imported_vox` + `voxel_dispatch::parse_voxel_bytes`.
- `crates/bevy_naadf/src/voxel/async_vox.rs:171,197` — both `spawn_native_vox_parse` / `spawn_native_vox_parse_from_bytes` call sites replaced `parse_to_imported_vox(&bytes)?` with `parse_voxel_bytes(&bytes).map_err(|e| e.to_string())?`.
- `crates/bevy_naadf/src/voxel/web_vox.rs:578` — `spawn_wasm_vox_parse` call site replaced `crate::voxel::grid::parse_to_imported_vox(&bytes)` with `crate::voxel::voxel_dispatch::parse_voxel_bytes(&bytes)` + `.to_string()` error mapping.
- `crates/bevy_naadf/src/voxel/grid.rs:443-455,594-598,633-641` — three docstring sites updated to reference `voxel_dispatch::parse_voxel_bytes` instead of the deleted `parse_to_imported_vox`.
- `crates/bevy_naadf/src/voxel/voxel_dispatch.rs:13-16` — module docstring updated to list the actual current callers (`install_vox_bytes_in_fixed_world`, drag-and-drop, async helpers) instead of the deleted shim.

**Verification:**
- `cargo build --workspace` — pass (28.04s, zero warnings).
- `cargo test --workspace --lib` — pass (186 passed, 1 ignored).
- `cargo run --bin e2e_render -- --vox-e2e` — PASS. The `.vox` install path exercises `voxel_dispatch::parse_voxel_bytes` directly; the gate captures a `.vox`-rendered framebuffer (Oasis), vox_geometry region luminance 250.5 (threshold 160), channel max 251.8 (threshold 30), per-pixel RGB Δ above the 8.0 floor. Every pipeline created cleanly; every expected render-graph node dispatched.
- `cargo run --bin e2e_render -- --oasis-edit-visual` — PASS (one run). Erase sphere brush exercised against the helper-installed Oasis world; rect-mean per-pixel RGB Δ=18.04 above the 8.0 floor.

**Notes:** F7 is independent of the brush/edit code path, so the `--oasis-edit-visual` run is bonus coverage — it confirms the helper-installed `WorldData` from Step 4 + the simplified `setup_test_grid` from Step 5 are bit-equivalent to pre-refactor as far as the brush pipeline is concerned.

**Status:** complete.

---

### Failure (if any)

None.

---

### Summary

- **Steps complete: 7 of 7** (F1 retroactive + F2/F6/F9/F3/F4/F5/F7; F8 deferred per architect).
- **Verification gates final pass/fail:**
  - `cargo build --workspace` — pass (zero warnings on final state).
  - `cargo test --workspace --lib` — pass (186 passed, 1 ignored).
  - `cargo run --bin e2e_render -- baseline` — pass (Step 4 verification).
  - `cargo run --bin e2e_render -- --vox-e2e` — pass (Step 7 verification).
  - `cargo run --bin e2e_render -- --oasis-edit-visual` — pass (Step 7 bonus coverage; 1 run).
- **Files changed (this session, 8 modified + 1 added):**
  - `crates/bevy_naadf/src/camera/mod.rs` (+1/−0)
  - `crates/bevy_naadf/src/camera/poses.rs` (NEW, 23 LOC)
  - `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs` (+3/−14)
  - `crates/bevy_naadf/src/lib.rs` (+26/−11)
  - `crates/bevy_naadf/src/voxel/async_vox.rs` (+4/−3)
  - `crates/bevy_naadf/src/voxel/grid.rs` (+183/−259)
  - `crates/bevy_naadf/src/voxel/vox_import.rs` (+4/−161)
  - `crates/bevy_naadf/src/voxel/voxel_dispatch.rs` (+4/−3)
  - `crates/bevy_naadf/src/voxel/web_vox.rs` (+20/−27)
- **Files removed this session:** 0 source files (F1 — `crates/voxel_noise/` — landed in prior dispatch commit `293ffa8`).
- **Net Rust LOC delta inside `crates/bevy_naadf/src/`:** −233 (244 insertions, 478 deletions) + 23 new `camera/poses.rs` = **−210 LOC**.
- **Net D3 LOC delta (this session + F1):** **~−1 757 Rust LOC** total (F1 dropped 1 547 LOC across workspace toolchain + crate; this session dropped a further 210 inside `bevy_naadf`).
- **Behavioural deltas observed during verification:** none. All gates pass at the same thresholds as pre-refactor. The `[palette-install]` debug log now fires once per install (not three times in the worst case) with a uniform `label={:?}` field — strict improvement over the prior duplicated copies. The wasm `?skybox=1` path now mutates `AppArgs.grid_preset` instead of inserting a marker — same user-visible outcome (pure-sky render).

---

## Side notes / observations / complaints

1. **The architect's `WorldInstall.source_label: &'static str` needed loosening to `&'a str` to fit `install_imported_vox`'s runtime label.** This is documented in Step 4 Notes. The shape works with both static labels (`"skybox-only"` etc.) and runtime borrows. Minor — the architect's example only showed the static-label case so it wasn't a deliberate design constraint, just an under-spec. If the architect's intent was "always copy to `String`", I disagreed: borrowing through `&str` for the synchronous log + insertion lifetime is cheaper and the helper doesn't store the label.

2. **`seed_block_hashing()` is now an explicit `bool` on the helper.** The pre-refactor `install_empty_world` skipped `seed_block_hashing()` while the other two install fns called it. The architect's helper sketch always called `seed_block_hashing()`, which would have been a behaviour change for the empty-world / skybox-only path. I added the `seed_block_hashing: bool` field to preserve the prior behaviour exactly (verified via Read of the original empty-world fn at the time it was rewritten — `seed_block_hashing()` was never called in that fn).

3. **`Bevy 0.19`'s `resource_exists` is at `bevy::ecs::schedule::common_conditions`, not the architect-cited `bevy::ecs::common_conditions`.** Minor path correction; both compiled cleanly because the `schedule::` segment exists. Architect's design otherwise spot-on.

4. **The Stage-14 docstring in `grid.rs` was simplified during Step 3.** As the architect's side-note 5 anticipated, several Stage-N narrative blocks became outdated after the refactor (the "Stage 2 consolidation" + "Stage 14 escape hatch" prose at grid.rs:73-103 was a chronicle of historical work, not a description of the current code). I condensed it to one tight paragraph listing the four install arms. The architect's recommendation to "synthesise — not delete" was followed.

5. **The CPU-oracle for `--vox-gpu-oracle` is now reachable only through the `Vox` arm + the `vox_gpu_oracle_cpu_phase` flag.** No CLI flag, no production routing. This is the load-bearing test-only piece kept per master-branch-identity rules (D6 keeps the gate; D3 keeps the CPU oracle install fn intact, just calls non-tiled `load_vox` now).

6. **One subjective reaction**: the codebase tightened *visibly*. `voxel/grid.rs` went from 1 354 → ~1 100 LOC and reads more like a "scene-install dispatcher" than a museum. `vox_import.rs` shed 161 LOC of tiling implementation that had zero production callers and one test. The `WebSkyboxOverride` removal + `.run_if(...)` add cleans up two pieces of Bevy-idiom rot at once. The architect's structural design held up perfectly across implementation; no Step needed a re-architect bounce.

7. **The `--vox-horizon-native` gate was NOT run as part of Step 2 verification.** Per CLAUDE.md the verification gates are deterministic ones (`cargo build`, `cargo test`, named e2e gates). The constant relocation in Step 2 was a pure module move (byte-identical values); build + lib tests prove the symbol resolution. I did NOT run the gate to keep the dispatch lean.

8. **The `--vox-gpu-oracle` gate was NOT re-run.** The architect's design (§3 "D6 coordination") notes that the gate continues to invoke `install_vox_sized_to_model` via `vox_gpu_oracle_cpu_phase`; the install function continues to load Oasis at the model's natural bounds; only the never-used `tiles>1` code path went. Verification: every production call site already passed `tiles=1`, and the build + lib tests prove the renamed-via-deletion `load_vox` produces the same `ImportedVox` shape. Running the SSIM gate (`--vox-gpu-oracle`) would have been bonus coverage but isn't load-bearing for proving the refactor.

9. **No file outside D3's "changes" list was edited.** The brief authorised cross-boundary edits to `lib.rs` (`AppArgs` enum extension, `.run_if` registrations) per the architect-flagged D7 coordination + the F4/F6 destination-module move. Everything else is squarely within D3's authority.

10. **Equal-footing observation**: the orchestration's D6-coordination note about `HORIZON_CAMERA_POS/ROT` is now satisfied from D3's side. When D6's implementor runs, they should NOT re-define those constants in `e2e/vox_horizon_parity.rs` — the imports already resolve cleanly to `crate::camera::poses`. The pin-camera consolidation (DUP-6) can directly consume the new module.

11. **`crates/voxel_noise/` deletion forecloses the `streaming-world` orchestration's design path as written.** Architect flagged this in side note 8. No action required from D3 — just a paper trail for any future architect who revives streaming-world. The `voxel_noise` snapshot is recoverable from git history (commit before `293ffa8`) or upstream `bevy_voxel_world`.
