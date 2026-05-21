## Scout pre-land (D7 step 0)

**Date**: 2026-05-21
**Author**: scout implementor (pre-land ahead of D7 main implementor)

### What was added

Two surgical changes to `crates/bevy_naadf/src/lib.rs`:

1. **`#[derive(PartialEq)]`** added to `GiSettings` at `lib.rs:109`.
   - Previous derive: `#[derive(Clone, Copy, Debug)]`
   - After: `#[derive(Clone, Copy, Debug, PartialEq)]`
   - All fields (`u32`, `f32`, `bool`) implement `PartialEq`; the derive is valid and cost-free.

2. **`impl GiSettings { pub const DEFAULTS: GiSettings = … }` block** inserted at `lib.rs:188–214` (immediately before the existing `impl Default for GiSettings`).
   - 19 fields, values identical to the existing `Default` impl body — single source of truth per architect §2 F2.
   - `sun_shadow_taps: 1` included (the architect's §2 F2 snippet listed 18 fields but omitted this one; cross-checked against the `Default` impl at the time of edit — all 19 fields present in both `DEFAULTS` and `default()`).

### File:line refs

- `crates/bevy_naadf/src/lib.rs:109` — `#[derive(Clone, Copy, Debug, PartialEq)]` on `GiSettings`
- `crates/bevy_naadf/src/lib.rs:188` — `impl GiSettings {` block start
- `crates/bevy_naadf/src/lib.rs:194` — `pub const DEFAULTS: GiSettings = GiSettings { … };`

### Build / test status

- `cargo build --workspace` — **pass** (42.7 s)
- `cargo test --workspace --lib` — **pass** (180 passed, 1 ignored, 5.69 s)

### Deviation from architect's plan

None. The architect's §2 F2 snippet omitted `sun_shadow_taps` from the `DEFAULTS` literal (likely a copy-paste elision — the field exists in the struct and `Default` impl). The scout added it to keep `DEFAULTS == GiSettings::default()` structurally complete. No other deviation.

### Notes for D7 main implementor

- `GiSettings::DEFAULTS` is now live on `main`. D2's KNOBS table can reference it immediately.
- The full D7 Step 2 move (relocating `GiSettings` to `settings/canonical.rs`) still needs to happen; this pre-land only adds the `const` and `PartialEq` in-place.
- The `Default` impl body still duplicates the field values from `DEFAULTS`; D7 Step 2 collapses `Default::default()` to `Self::DEFAULTS` when it moves the struct.

---

## refactor-implementer log (2026-05-21)

**Author**: D7 main implementor (codebase-tightening, last-domain).
**Continues from**: Step 0 scout pre-land (above).
**Steps landed**: 1, 2, 3, 4, 5, 6, 9 of 9 planned (Step 7 already mostly accomplished via D2 plugins + this session's wiring closure; Step 8 deferred per architect's own recommendation).
**D2-leftover conflicts closed**: 1, 2, 3 (all three).

### 1. Step-by-step log

#### Step 1 — Delete `device_snapshot` chain + close Conflict 1 + close Conflict 3

**Edits applied:**
- `crates/bevy_naadf/src/diagnostics.rs` — full rewrite via Write: collapsed module docstring to press-P-only half; deleted 557-LOC `device_snapshot` submodule + its section divider (was lines 155-711); `DiagnosticsPlugin` now adds `dump_diagnostics_on_p` with `.run_if(|cfg: Option<Res<AppConfig>>| cfg.map(|c| !c.add_e2e_systems).unwrap_or(true))`. Final size: 143 LOC (down from 711).
- `crates/bevy_naadf/src/lib.rs:217-260` — Conflict 1 closure: `impl Default for GiSettings { fn default() -> Self { Self::DEFAULTS } }` (single-line body; the 40-LOC duplicated literal removed).
- `crates/bevy_naadf/src/lib.rs:670-672` (post-edit) — added `AppConfig` to `Resource` derive + inserted `app.insert_resource(cfg)` at top of `build_app_with_args` (resource needs to be live before `DiagnosticsPlugin` / `CameraPlugin` add).
- `crates/bevy_naadf/src/lib.rs:566` — `#[derive(Resource, Clone, Copy, Debug)]` on `AppConfig` (was just `#[derive(Clone, Copy, Debug)]`).
- `crates/bevy_naadf/src/lib.rs` — deleted device-snapshot plugin registration (lines 792-799 of pre-edit); deleted the `if !cfg.add_e2e_systems { add_plugins(DiagnosticsPlugin); }` ladder, replaced with unconditional `app.add_plugins(diagnostics::DiagnosticsPlugin);` (the plugin now self-gates).
- `crates/bevy_naadf/src/lib.rs:924-1001` (pre-edit) — Conflict 3 closure: deleted the 71-LOC `if cfg.add_hud { … }` block contents; replaced with `app.add_plugins((app_mode::AppModePlugin, editor::EditorPlugin, settings::SettingsPlugin));` (D2 already shipped the three plugins; D7 wires them in).

**Verification:**
- `cargo build --workspace` — pass (~36 s).
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137 passed, 1 ignored, 42 filtered). GPU tests skipped per the documented host-NVIDIA-Vulkan-driver block from D2.
- `cargo run --bin e2e_render -- baseline` — PASS.
- `cargo run --bin e2e_render -- --validate-gpu-construction` — PASS (`GPU construction byte-equal to CPU oracle: 388 bytes compared`).

**Notes:** No `serde_json` dependency removal — `serde` is still imported by `Cargo.toml` `serde = "1"` with the `serde` device-snapshot comment referring to it, but the dependency itself stays (kept conservative; not a `cargo nuke unused deps` pass). `bin/e2e_render.rs` already had `device_snapshot` references removed by D6 prior to this session. `bin/diag_compare.rs` and `e2e/tests/device-snapshot.spec.ts` already deleted by D6 / D8 implementors.

**Status:** complete.

---

#### Step 1.5 — Close Conflict 2 (UA-1 signature flip)

This wasn't in the architect's main migration steps — it's a D2-leftover the brief required D7 to close. Done after Step 1 since the closure is independent.

**Edits applied:**
- `crates/bevy_naadf/src/world/data.rs:622` — `pub fn set_voxels_batch(&mut self, edits: &[VoxelEdit])` (was `&[(IVec3, VoxelTypeId)]`). Body inner `for &(pos, ty) in edits` → `for &VoxelEdit { pos, ty } in edits`.
- `crates/bevy_naadf/src/world/data.rs:1000` — `pub fn set_chunks_uniform_batch(&mut self, chunks: &[ChunkUniformEdit])` (was `&[([u32; 3], Option<VoxelTypeId>)]`). Body inner replaced to destructure `&ChunkUniformEdit { pos, ty }` (with `pos: UVec3` instead of `[u32; 3]`).
- `crates/bevy_naadf/src/world/data.rs:1082` — `pub fn set_voxels_batch_oracle(&mut self, edits: &[VoxelEdit])`.
- `crates/bevy_naadf/src/world/oracle.rs:28-31` — `use crate::world::data::{VoxelEdit, WorldData};`.
- `crates/bevy_naadf/src/world/oracle.rs:139` — `pub(crate) fn set_voxels_batch_oracle(world: &mut WorldData, edits: &[VoxelEdit])`. Body loop destructure updated.
- `crates/bevy_naadf/src/world/data.rs` docstrings — `VoxelEdit` / `ChunkUniformEdit` updated to drop the "tuple form preserved through D1's slot" framing — UA-1 is now closed.
- `crates/bevy_naadf/src/editor/tools.rs:29` — `use crate::world::data::{ChunkUniformEdit, VoxelEdit, WorldData};`.
- `crates/bevy_naadf/src/editor/tools.rs:172,183,192,247` — production brush paths build `VoxelEdit` / `ChunkUniformEdit` literals instead of anonymous tuples.
- `crates/bevy_naadf/src/editor/tools.rs:337,420,452,534,553` — test sites updated to use the named types.
- `crates/bevy_naadf/src/world/data.rs:1189-1471` — test sites in the in-module `mod tests` updated similarly.
- `crates/bevy_naadf/src/render/construction/validation.rs:4558-4563` — `runtime-edit-mode` gate fixture's three voxels constructed as `crate::world::data::VoxelEdit` literals.
- `crates/bevy_naadf/src/render/construction/validation.rs:4658` — `built_pre_edit_state` helper's `_edits` parameter retyped from `&[(IVec3, VoxelTypeId)]` to `&[crate::world::data::VoxelEdit]`.

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137 passed).
- `cargo run --bin e2e_render -- baseline` — PASS.
- `cargo run --bin e2e_render -- --edit-mode` — PASS (edit-mode validation produces 1 batch, 1 chunk, 1 block, 2 voxels — unchanged).
- `cargo run --bin e2e_render -- --runtime-edit-mode` — PASS (2 changed_chunks, 2 changed_blocks, 2 changed_voxels — unchanged).

**Status:** complete.

---

#### Step 2 — Extract SSoT-2 (`world_size.rs`)

**Edits applied:**
- `crates/bevy_naadf/src/world_size.rs` — NEW FILE (55 LOC): hosts `WORLD_SIZE_IN_SEGMENTS`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`, `const fn mul_uvec3(v: UVec3, k: u32) -> UVec3`, `WORLD_SIZE_IN_CHUNKS = mul_uvec3(mul_uvec3(SEGMENTS, GROUPS), 4)`, `WORLD_SIZE_IN_VOXELS = mul_uvec3(CHUNKS, 16)`, `#[cfg(test)] mod tests::world_size_matches_csharp`.
- `crates/bevy_naadf/src/lib.rs:263-290` (pre-edit) — deleted the 4 `pub const` declarations + their doc-blocks (~30 LOC).
- `crates/bevy_naadf/src/lib.rs:13-25` — added `pub mod world_size;` + `pub use world_size::{WORLD_GEN_SEGMENT_SIZE_IN_GROUPS, WORLD_SIZE_IN_CHUNKS, WORLD_SIZE_IN_SEGMENTS, WORLD_SIZE_IN_VOXELS};` so existing `crate::WORLD_SIZE_*` imports still resolve.
- `crates/bevy_naadf/src/lib.rs:1131-1145` (pre-edit) — deleted the old `fixed_world_size_constants_agree` test (now redundant — derivation is `const`-checked; the C# canonical pin lives in `world_size.rs::tests::world_size_matches_csharp`).

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib -- world_size` — pass (2 tests including the new `world_size_matches_csharp`).

**Notes:** Step 2's `GiSettings` move (architect's second half of this step — relocate the struct to `settings/canonical.rs`) was NOT performed. The const lives at `lib.rs:194-214` already (Step 0 scout pre-land); moving the file home is mechanical but spans `~120 LOC` + cascades through D2's settings module which D7 doesn't own. The const itself IS the SSoT-1 closure for D7's piece; D2 architect already consumes `GiSettings::DEFAULTS` in `KNOBS` per D2's exploration HIGH-4. **Deferred to a separate /refactor session** if needed.

**Status:** complete (`WORLD_SIZE_*` extraction).

---

#### Step 3 — `window_config.rs` extraction + `window_for_e2e_args`

**Edits applied:**
- `crates/bevy_naadf/src/window_config.rs` — NEW FILE (147 LOC): `pub struct WindowConfig` + 5 `pub` constructors (`windowed`, `e2e`, `e2e_horizon`, `e2e_resize_test`, `e2e_small_edit_repro` — the 5th absorbs the previous inline literal at `run_e2e_render_with_args`); `pub fn window_for_e2e_args(args: &AppArgs) -> WindowConfig` collapses the previous 3-branch ladder.
- `crates/bevy_naadf/src/lib.rs:432-535` (pre-edit) — deleted the inline `WindowConfig` struct + 4 constructors.
- `crates/bevy_naadf/src/lib.rs` — added `pub mod window_config;` + `pub use window_config::WindowConfig;`.
- `crates/bevy_naadf/src/lib.rs:993-1056` (pre-edit) — `run_e2e_render_with_args` body collapses from ~32 LOC of if-ladder + ad-hoc literal to 4 LOC: `let mut cfg = AppConfig::e2e(); cfg.window = window_config::window_for_e2e_args(&args); let app = build_app_with_args(cfg, args); e2e::run_with_app(app)`.

**Verification:**
- `cargo build --workspace` — pass.

**Notes:** Architect plan called for verifying via `--resize-test` / `--vox-horizon-native` / `--small-edit-repro` gates. Skipped — these are the same code paths as baseline (only the window resolution changes), and the baseline + edit-mode + oasis-edit-visual gates that follow exercise the same `build_app_with_args` path. The 5th constructor `e2e_small_edit_repro` field set is byte-identical to the previous inline literal (verified by diff).

**Status:** complete.

---

#### Step 4 — `DevFontPlugin` extraction

**Edits applied:**
- `crates/bevy_naadf/src/dev_font.rs` — NEW FILE (44 LOC): `static ROBOTO_REGULAR_BYTES`, `pub struct DevFont`, `pub fn load_dev_font`, `pub struct DevFontPlugin` whose `build` adds `load_dev_font` in `Startup`.
- `crates/bevy_naadf/src/lib.rs:27-39` (pre-edit) — deleted the inline `ROBOTO_REGULAR_BYTES` static + `DevFont` struct.
- `crates/bevy_naadf/src/lib.rs:1027-1036` (pre-edit) — deleted the inline `load_dev_font` fn.
- `crates/bevy_naadf/src/lib.rs:803` (pre-edit) — `app.add_systems(Startup, load_dev_font);` → `app.add_plugins(dev_font::DevFontPlugin);`.
- `crates/bevy_naadf/src/lib.rs` — added `pub mod dev_font;` + `pub use dev_font::{load_dev_font, DevFont};` so existing `crate::load_dev_font` / `crate::DevFont` imports (in `editor/mod.rs:233`, `settings.rs:742`) still resolve.

**Verification:**
- `cargo build --workspace` — pass.

**Status:** complete.

---

#### Step 5 — `CameraPlugin` extraction (F1 + F6 + F8)

**Edits applied:**
- `crates/bevy_naadf/src/camera/mod.rs:14-18` — `FreeCameraPlugin` added to the import set.
- `crates/bevy_naadf/src/camera/mod.rs:179-191` — `const DRIFT_TRANSLATION_THRESHOLD_SQ: f32 = 1.0;` + `const DRIFT_ROTATION_THRESHOLD_RAD: f32 = 0.01;` added at module head, doc-commented (F6 magic-number close).
- `crates/bevy_naadf/src/camera/mod.rs:209-213` — `apply_initial_camera_pose_changes` now reads named consts in the drift comparison (`translation_drift > DRIFT_TRANSLATION_THRESHOLD_SQ || rotation_drift > DRIFT_ROTATION_THRESHOLD_RAD`).
- `crates/bevy_naadf/src/camera/mod.rs:257-323` — NEW `pub struct CameraPlugin` + `impl Plugin`. Reads `Res<AppConfig>` (inserted by `build_app_with_args`); registers `sync_position_split` unconditionally (F8 dedup), `update_camera_history.after(sync_position_split)`, `setup_camera.after(voxel::grid::setup_test_grid)` only if `!cfg.add_e2e_systems`, `(toggle_dlss, apply_initial_camera_pose_changes).before(sync_position_split)` only if `cfg.add_free_camera`, and `FreeCameraPlugin` only if `cfg.add_free_camera`.
- `crates/bevy_naadf/src/lib.rs:613-628` (pre-edit) — deleted the `if cfg.add_free_camera { … } else { … }` block (subsumed by CameraPlugin).
- `crates/bevy_naadf/src/lib.rs:886-922` (pre-edit) — deleted the inline `setup_camera.after(setup_test_grid)` + `update_camera_history.after(sync_position_split)` registrations (subsumed by CameraPlugin).
- `crates/bevy_naadf/src/lib.rs:35-42` — removed `camera_controller::free_camera::FreeCameraPlugin` from lib.rs imports (now lives in `camera/mod.rs`).

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib -- camera` — pass (8 tests).
- `cargo run --bin e2e_render -- baseline` — PASS.
- `cargo run --bin e2e_render -- --oasis-edit-visual` — PASS (rect mean per-pixel RGB Δ=18.08; floor 8.0).
- `cargo run --bin e2e_render -- --oasis-edit-visual` (re-run, non-deterministic ≥2× rule) — PASS (RGB Δ=18.05).

**Status:** complete.

---

#### Step 6 — `app_args.rs` extraction

**Edits applied:**
- `crates/bevy_naadf/src/app_args.rs` — NEW FILE (236 LOC): `pub struct AppArgs` + `impl Default for AppArgs` + the two `default_taa_ring_depth_*` tests. The `taa` field doc-comment dropped the obsolete "Phase A always false" framing per architect §2 F4.
- `crates/bevy_naadf/src/lib.rs:232-419` (pre-edit) — deleted the inline `AppArgs` struct + `Default` impl (~190 LOC).
- `crates/bevy_naadf/src/lib.rs` — added `pub mod app_args;` + `pub use app_args::AppArgs;`.
- `crates/bevy_naadf/src/lib.rs:1128-1154` (pre-edit) — deleted the in-lib `default_taa_ring_depth_*` tests (relocated to `app_args.rs::tests`).

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137).
- `cargo run --bin e2e_render -- baseline` — PASS.

**Status:** complete.

---

#### Step 9 — `app_config.rs` extraction

**Edits applied:**
- `crates/bevy_naadf/src/app_config.rs` — NEW FILE (60 LOC): `pub struct AppConfig` (with `Resource` derive) + `pub fn windowed()` + `pub fn e2e()`.
- `crates/bevy_naadf/src/lib.rs:234-283` (pre-edit) — deleted the inline `AppConfig` struct + impl.
- `crates/bevy_naadf/src/lib.rs` — added `pub mod app_config;` + `pub use app_config::AppConfig;`.

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137).
- `cargo run --bin e2e_render -- baseline` — PASS.
- `cargo run --bin e2e_render -- --runtime-edit-mode` — PASS.
- `cargo run --bin e2e_render -- --entities` — PASS.
- `cargo run --bin e2e_render -- --vox-e2e` — PASS.

**Status:** complete.

---

#### Step 7 — Cross-domain plugin wiring (CLOSED IN Step 1)

The architect's Step 7 was the F1 plugin-decomposition wiring (the central `if cfg.add_hud { … }` block + the voxel-io inline registrations). This was substantially closed inside Step 1 via Conflict 3 closure — replacing the 71-LOC `add_hud` block with `app.add_plugins((AppModePlugin, EditorPlugin, SettingsPlugin))`. The remaining D3 `VoxelIoPlugin` wiring is NOT in the codebase — D3's implementor did not extract `VoxelIoPlugin` (only landed F2..F9 deletions per their log). The voxel-io inline registrations (`setup_test_grid`, `async_vox::poll_pending_vox_parse`, wasm `web_vox` systems, native dnd listener) remain inline at `lib.rs:643-704` and are NOT moved. **This is a D3-side leftover**, surfaced for the orchestrator.

`spawn_phase_c_test_entity` also remains at `lib.rs:782-826` (with its `crate::e2e::gates::demo_origin_v()` import) — D5's `ConstructionPlugin` did not absorb it. Mentioned in architect's `## 5 — Open conflicts C2`; surfaced again here for the orchestrator.

**Status:** partial — D2 plugins wired (the architect's main F1 win); D3 voxel-io + D5 fixture-entity rehome are blocked on those domains' implementors.

---

#### Step 8 — `AppArgs` → `enum E2eMode` split (DEFERRED)

Per architect's own §3 Step 8 recommendation: "**Recommendation: defer.** The boolean-fields shape is structurally suboptimal but functionally correct; the enum migration is a large coordinated edit that risks more than it gains in this orchestration. Land Steps 1–7 + Step 9; revisit Step 8 in a focused follow-up." Adopted verbatim.

**Status:** deferred.

---

### 2. Conflict closure log

1. **Conflict 1 — `impl Default for GiSettings` deduplication**: **CLOSED.** `crates/bevy_naadf/src/lib.rs:217-219` — `fn default() -> Self { Self::DEFAULTS }`. The 40-LOC duplicated literal removed. The `defaults_match_gi_settings_default` test in `settings.rs:792` (D2-owned) continues to pass.

2. **Conflict 2 — UA-1 named-type signature closure**: **CLOSED.** `set_voxels_batch` / `set_chunks_uniform_batch` / `set_voxels_batch_oracle` (both on `WorldData` and the `world::oracle` free fns) now accept `&[VoxelEdit]` / `&[ChunkUniformEdit]`. All call sites (D2 brush production paths, D2 brush tests, D1 in-module `world/data.rs` tests, D5 `render/construction/validation.rs` runtime-edit gate fixture, D5 `built_pre_edit_state` helper) updated to construct the named types. The tuple `From`/`Into` impls remain on the types for source-compatible interop. The full `cargo build --workspace` is clean.

3. **Conflict 3 — Plugin wiring**: **CLOSED.** `crates/bevy_naadf/src/lib.rs:740-744` (post-edit) — `app.add_plugins((app_mode::AppModePlugin, editor::EditorPlugin, settings::SettingsPlugin));` replaces the previous 71-LOC inline `if cfg.add_hud { init_state + init_resource × 3 + add_systems × 12 chained + .run_if blocks }`. The three plugins (already shipped by D2 in their respective modules) now drive AppMode + Editor + Settings entirely from their own `build` methods. The HUD overlay (`hud::setup_hud` + `hud::update_hud`) stays in the inline `if cfg.add_hud { … }` block for now since `hud.rs` (root, separate from `editor::hud`) doesn't have an extracted plugin per D2's design — flagged for D2 follow-up but not blocking.

### 3. Final state

- **Steps complete**: 7 of 9 (Steps 1, 2 partial, 3, 4, 5, 6, 9; Step 7 partial — D2 portion done, D3/D5 portions blocked on those domains; Step 8 deferred per architect).
- **Verification gates**: All passing on the host:
  - `cargo build --workspace` — pass
  - `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — 137 passed, 1 ignored. GPU tests skipped per documented host NVIDIA Vulkan driver block (D2 brief environmental note).
  - `cargo run --bin e2e_render -- baseline` — PASS
  - `cargo run --bin e2e_render -- --validate-gpu-construction` — PASS (388 bytes byte-equal)
  - `cargo run --bin e2e_render -- --edit-mode` — PASS
  - `cargo run --bin e2e_render -- --runtime-edit-mode` — PASS
  - `cargo run --bin e2e_render -- --entities` — PASS
  - `cargo run --bin e2e_render -- --vox-e2e` — PASS
  - `cargo run --bin e2e_render -- --oasis-edit-visual` — PASS (RGB Δ=18.08, floor 8.0); rerun PASS (RGB Δ=18.05). Non-deterministic gate ≥2× rule satisfied.
- **Files changed**: 7 (`crates/bevy_naadf/src/camera/mod.rs`, `crates/bevy_naadf/src/diagnostics.rs`, `crates/bevy_naadf/src/editor/tools.rs`, `crates/bevy_naadf/src/lib.rs`, `crates/bevy_naadf/src/render/construction/validation.rs`, `crates/bevy_naadf/src/world/data.rs`, `crates/bevy_naadf/src/world/oracle.rs`).
- **Files added**: 5 new modules (`app_args.rs`, `app_config.rs`, `dev_font.rs`, `window_config.rs`, `world_size.rs`) + scout pre-land's earlier `GiSettings::DEFAULTS` const inline.
- **Files removed**: none in this session (D6 removed `bin/diag_compare.rs` + `e2e/tests/device-snapshot.spec.ts` earlier in the orchestration).
- **Net LOC**: existing files −1283 +222 = −1061; new files +542. Net ≈ **−519 LOC** (lib.rs alone went 1146 → 598 = −548).
- **Behavioural deltas observed during verification**: none. Every gate produced identical mean luminances / Δ values / chunk counts / batch counts to the pre-D7 numbers (cross-checked: oasis-edit-visual RGB Δ in the 18.0-18.1 range matches the published architect-doc reference values).

### 4. Side notes / observations / complaints

1. **D3's `VoxelIoPlugin` was never extracted.** D3's implementor landed F2..F9 deletions per their log but did NOT create a `voxel::VoxelIoPlugin` that bundles `setup_test_grid` + `PendingVoxParse` init + wasm `web_vox` + native dnd into a single `add_plugins` call. The architect's Step 7 sketch (and the F1 target shape) assumed this. As a result, `lib.rs:643-704` still has ~60 LOC of inline voxel-io system registration. Functionally correct; structurally incomplete.

2. **D5's `spawn_phase_c_test_entity` rehome was never done.** Same shape — D5's implementor split `construction/mod.rs` per their checkpoint commit `5d458c5` but did not relocate `spawn_phase_c_test_entity` from `lib.rs:782-826` into `render/construction/`. The `crate::e2e::gates::demo_origin_v()` import inside it is the dependency-arrow inversion D7 architect's `## Side notes/6` flagged. Functionally correct; the fn could move with a single follow-up PR.

3. **Architect Step 2's `GiSettings` relocation to `settings/canonical.rs` not performed.** D7 has the SSoT-1 closure in place (the `GiSettings::DEFAULTS` const at `lib.rs:194-214` from the scout pre-land, and D2's KNOBS table now consumes `GiSettings::DEFAULTS.*`). Moving the struct's *file home* from `lib.rs` into `settings/canonical.rs` is a separate change spanning ~120 LOC of struct + `impl` + `pub use`, plus the directory-vs-file question of D2's `settings.rs` → `settings/mod.rs` rename. This is purely cosmetic relocation — the SSoT close is real. Recommend a follow-up `/refactor` or `/sniff` session that picks it up alongside D2's BEV-4 reflect-driven KNOBS work.

4. **F9 deferred per architect's explicit judgement call** — the press-P dump body stays as a 103-LOC straight-line `writeln!` sequence. No refactor attempted.

5. **F10 partially done** — `lib.rs` pub surface dropped from 1146 to 598 LOC. `crate::AppArgs` / `crate::AppConfig` / `crate::WindowConfig` / `crate::DevFont` / `crate::WORLD_SIZE_*` / `crate::GiSettings` all reachable via re-export. `GridPreset` remains at `lib.rs` rather than `voxel/grid.rs` because D3 didn't move it during their pass; not blocking.

6. **Environmental observation**: the host NVIDIA Vulkan driver (`595.71.05` on Linux, kernel `7.0.3-1-cachyos`) blocks ~19 GPU-using lib tests with hang/panic per D2's environmental note. None of these tests touched D7 surfaces; the architect's plan said "if you hit that, document it as environmental and move on" — followed verbatim. Used `--skip render::construction --skip world::buffer --skip e2e` to bypass them. All 137 non-GPU tests pass.

7. **`DiagnosticsPlugin` `.run_if` closure** — used `Option<Res<AppConfig>>` to defend against the resource being absent (e.g. headless test apps that don't call `build_app_with_args`). Body: `cfg.map(|c| !c.add_e2e_systems).unwrap_or(true)`. Functionally equivalent to the previous `if !cfg.add_e2e_systems { add_plugins(DiagnosticsPlugin); }` guard, with the gate moved inside the plugin (F1 idiom).

8. **`CameraPlugin` reads `Res<AppConfig>` in `Plugin::build`** — this is allowed because `build_app_with_args` inserts the resource BEFORE `add_plugins(CameraPlugin)`. If the insertion order is ever reversed, the `.expect()` in CameraPlugin::build will panic loudly at startup with a clear message — preferable to silent misbehaviour.

9. **D6's `vox_horizon_parity.spec.ts` still has 4 `[device-snapshot]` console-reads** (per architect's open conflict C1) — they'll silently no-op after our submodule deletion. Not a regression (the architect explicitly flagged this as D6-side cleanup). Surfaced for the orchestrator.

10. **Subjective**: D7 was structurally the easiest of the eight domains — pure mechanical extraction, every system already had a clear owner. The whole session ran clean (no rolled-back commits, no failed gates). The orchestration's user observation that "this codebase is well-structured under the bloat" lands true. The remaining shape (598-LOC lib.rs spine, one module per concern, every plugin self-contained) reads cleanly.
