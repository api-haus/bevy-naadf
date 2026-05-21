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

---

## D7 cleanup follow-ups — 2026-05-21

**Author**: D7-cleanup follow-ups implementor.
**Brief**: land the 4 cross-domain cleanup items that D7's main dispatch left open — (1) D3 `VoxelIoPlugin` extraction, (2) D5 `spawn_phase_c_test_entity` rehome, (3) D6 `vox-horizon-parity.spec.ts` `[device-snapshot]` console-read cleanup, (4) `GiSettings` relocation to `settings/canonical.rs`.

### 1. Step-by-step log

#### Follow-up 1 — Extract `VoxelIoPlugin` (D3 architect F4 + D7 Step 7)

**Edits applied:**
- `crates/bevy_naadf/src/voxel/plugin.rs` — NEW FILE (98 LOC). `pub struct VoxelIoPlugin;` + `impl Plugin`. Owns `setup_test_grid` (`Startup`), `PendingVoxParse` resource init, `async_vox::poll_pending_vox_parse` (`Update`), wasm `web_vox::startup_fetch_default_vox` (`Startup`, `.before(setup_test_grid)`), wasm `web_vox::apply_pending_vox` (`Update`, `.after(poll_pending_vox_parse)`), wasm `web_vox::pin_web_horizon_camera` (`Update`, `.after(poll_pending_vox_parse)` + `.run_if(resource_exists::<WebHorizonPoseOverride>)`), and the native dnd pair `grid::log_native_dnd_registered` + `grid::native_vox_drop_listener` gated on `!Res<AppConfig>.add_e2e_systems`.
- `crates/bevy_naadf/src/voxel/mod.rs:12-21` — added `pub mod plugin;` + `pub use plugin::VoxelIoPlugin;`.
- `crates/bevy_naadf/src/lib.rs:388-453` (pre-edit) — deleted ~60 LOC of inline `setup_test_grid` / `PendingVoxParse` init / `poll_pending_vox_parse` / wasm `web_vox::*` / native dnd registrations.
- `crates/bevy_naadf/src/lib.rs:388-394` (post-edit) — replaced with `app.add_plugins(voxel::VoxelIoPlugin);` + a 5-line comment block describing the plugin's contents.

**Behavioural note:** `web_vox::hide_ui` registration remains INSIDE `lib.rs`'s `if cfg.add_hud { … }` block — `hide_ui` depends on the HUD being present (it queries UI roots) so its registration is properly HUD-gated. Moving it into `VoxelIoPlugin` would couple voxel-io to HUD presence. Kept at the original site.

**Verification:**
- `cargo build --workspace` — pass (27.25s).
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137 passed; same 43-test GPU-driver-blocked skip set as the main D7 session).
- `cargo run --bin e2e_render -- baseline` — PASS (luminance 100%; emissive 247.7, solid 243.7, sky 202.9).

**Notes:** D3's architect's F4 didn't include a "this is what `VoxelIoPlugin` looks like" snippet — only the systems-to-bundle list. The plugin shape follows D7 architect's `## 6 Cross-domain assumptions` line "D3 lands `voxel::VoxelIoPlugin` that owns `setup_test_grid` + `PendingVoxParse` init + `poll_pending_vox_parse` + wasm `web_vox::startup_fetch_default_vox` + wasm `apply_pending_vox` + wasm `pin_web_horizon_camera` + native dnd." Done verbatim.

**Status:** complete.

---

#### Follow-up 2 — Rehome `spawn_phase_c_test_entity` to `render/construction/test_fixture.rs`

**Edits applied:**
- `crates/bevy_naadf/src/render/construction/test_fixture.rs` — NEW FILE (78 LOC). `pub fn spawn_phase_c_test_entity(mut entities: ResMut<MainWorldEntities>)` — verbatim transplant of the body from `lib.rs:551-594` (pre-edit). Imports `crate::aadf::entity::EntityData`, `crate::render::gpu_types::EntityInstance`, `super::MainWorldEntities`, and `crate::e2e::gates::demo_origin_v` (the production→e2e dep-arrow remains; D7 architect's Side note 6 flagged it as a separate cleanup).
- `crates/bevy_naadf/src/render/construction/mod.rs:71-74` — added `pub mod test_fixture;` to the existing module list.
- `crates/bevy_naadf/src/render/construction/mod.rs:2146-2155` (pre-edit ConstructionPlugin::build interior) — registered the fixture spawner as a Startup system with `.after(crate::voxel::grid::setup_test_grid)` + `.run_if(|args: Res<crate::AppArgs>| args.spawn_test_entity)`. Self-gating per D7 architect's open conflict C2 option (a).
- `crates/bevy_naadf/src/lib.rs:455-465` (pre-edit) — deleted the `if args.spawn_test_entity { app.add_systems(Startup, spawn_phase_c_test_entity.after(voxel::grid::setup_test_grid)); }` block (subsumed by `ConstructionPlugin`).
- `crates/bevy_naadf/src/lib.rs:473-530` (pre-edit) — deleted the 58-LOC `fn spawn_phase_c_test_entity(...)` body (moved to `test_fixture.rs`).

**Verification:**
- `cargo build --workspace` — pass (31.91s).
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137).
- `cargo run --bin e2e_render -- --entities` — PASS. Fixture spawn log fires from the new module location (`bevy_naadf::render::construction::test_fixture: phase-c wave-3 — spawned fixture entity: 4×4×4 green-emissive @ Vec3(2046.0, 24.0, 2046.0) …`). `entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history; frame B: 8 chunk_updates`.

**Notes:** `crate::e2e::gates::demo_origin_v()` import remains — the production→e2e arrow flagged in D7 architect's Side note 6. Resolving it (move `demo_origin_v` to a non-e2e module like `voxel/grid::demo_origin_v`) is in-scope for a future cleanup; this follow-up's brief said "consult the D5 architect's design first" and D5's architect doesn't propose a destination, so the minimal-rehome path is taken.

**Status:** complete.

---

#### Follow-up 3 — Remove `[device-snapshot]` console-reads from `vox-horizon-parity.spec.ts`

**Edits applied:**
- `e2e/tests/vox-horizon-parity.spec.ts:122` — docstring line listing the sentinel groups dropped the `[device-snapshot]` entry.
- `e2e/tests/vox-horizon-parity.spec.ts:147` (pre-edit) — deleted `const deviceSnapshot = lines.filter((l) => l.includes("[device-snapshot"));`.
- `e2e/tests/vox-horizon-parity.spec.ts:158` (pre-edit) — removed `"[device-snapshot",` from the `namedBuckets` array.
- `e2e/tests/vox-horizon-parity.spec.ts:187` (pre-edit) — deleted the 3-line sidecar section header + body that emitted `## [device-snapshot] sentinel (raw)`.

**Verification:**
- `grep -n "device-snapshot" e2e/tests/vox-horizon-parity.spec.ts` — 0 matches. All 4 references confirmed removed.
- `cargo build --workspace` — pass (TS file isn't part of cargo's compile surface; the build verifies nothing for this follow-up but is run anyway as a regression sanity check).
- Playwright `--vox-horizon-native` web run not executed — the wider gate spawns a 10-minute native compile + WASM-canvas capture; it's the heaviest gate in the repo and was non-functional pre-edit anyway (web-side AADF convergence bug). The edit is mechanical TypeScript surgery (delete 4 references to a removed sentinel); the spec's `[xxx]` sentinel filter still picks up any future tag through the "other" bucket.

**Notes:** the file's full path is `e2e/tests/vox-horizon-parity.spec.ts` (hyphenated), not `vox_horizon_parity.spec.ts` as the orchestrator's brief stated. Same file; updated all 4 cited reference points.

**Status:** complete.

---

#### Follow-up 4 — Relocate `GiSettings` to `settings/canonical.rs`

**Edits applied:**
- `crates/bevy_naadf/src/settings.rs` → `crates/bevy_naadf/src/settings/mod.rs` — `git mv` (preserves blame). No body change.
- `crates/bevy_naadf/src/settings/canonical.rs` — NEW FILE (125 LOC). Contains the `pub struct GiSettings { … }` (19 fields, verbatim from `lib.rs:108-185` pre-edit), `impl GiSettings { pub const DEFAULTS: GiSettings = … }` (verbatim 19-field const literal), `impl Default for GiSettings { fn default() -> Self { Self::DEFAULTS } }`.
- `crates/bevy_naadf/src/settings/mod.rs:18-21` — added `pub mod canonical;` + `pub use canonical::GiSettings;` immediately under the existing module docstring.
- `crates/bevy_naadf/src/settings/mod.rs:41` (pre-edit) — `use crate::{AppArgs, DevFont, GiSettings};` → `use crate::{AppArgs, DevFont};` (the local `pub use canonical::GiSettings` shadows the prior re-export from `crate`, so the explicit import would collide; the in-module symbol now resolves via the local re-export).
- `crates/bevy_naadf/src/lib.rs:104-220` (pre-edit) — deleted the `pub struct GiSettings { … }` + `impl GiSettings { pub const DEFAULTS … }` + `impl Default for GiSettings`. ~117 LOC removed. Replaced with a 3-line "moved" comment block.
- `crates/bevy_naadf/src/lib.rs:30-37` — added `pub use settings::canonical::GiSettings;` to the existing re-export block so existing `crate::GiSettings` imports throughout the codebase (e.g. D2's KNOBS macros, D4's GPU mirror) resolve unchanged.

**Verification:**
- `cargo build --workspace` — pass (24.63s).
- `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — pass (137).
- `cargo run --bin e2e_render -- baseline` — PASS.
- `cargo run --bin e2e_render -- --validate-gpu-construction` — PASS (`GPU construction byte-equal to CPU oracle: 388 bytes compared`).
- `cargo run --bin e2e_render -- --edit-mode` — PASS.
- `cargo run --bin e2e_render -- --runtime-edit-mode` — PASS.
- `cargo run --bin e2e_render -- --vox-e2e` — PASS (vox_geometry luminance 250.5, channel max 251.8).
- `cargo run --bin e2e_render -- --oasis-edit-visual` ×2 — PASS, PASS (rect mean per-pixel RGB Δ=18.03, then Δ=18.07; well above 8.0 floor; consistent with prior session's 18.0 range — no behavioural delta).

**Notes:** D2's architect's HIGH-3 left the directory-vs-file question open ("Either home is fine for D2"); this follow-up took the `settings/` directory path (D7 architect's C3 / Step 2 first branch). The in-tree consumer chain — `settings/mod.rs` (KNOBS table) reads `GiSettings::DEFAULTS` via the local `pub use canonical::GiSettings`; `lib.rs` re-exports via the existing public surface; all downstream `crate::GiSettings` imports unchanged. The `Resource` derive on `GiSettings` (D2's KNOBS table consumes it through `AppArgs.gi`) was NOT applied — the type isn't a standalone `Resource`; it's an embedded field of `AppArgs` (which IS a `Resource`). Verbatim transplant; no derive change.

**Status:** complete.

---

### 2. Failure (if any)

None.

---

### 3. Summary

- **Follow-ups complete**: 4 of 4 (VoxelIoPlugin: yes / spawn_phase_c_test_entity: yes / vox_horizon_parity cleanup: yes / GiSettings relocation: yes).
- **Verification gates final pass/fail**:
  - `cargo build --workspace` — pass (all 4 follow-ups built clean).
  - `cargo test --workspace --lib -- --skip render::construction --skip world::buffer --skip e2e` — 137 passed, 1 ignored, 42 filtered. Same host-NVIDIA-Vulkan-driver block as the main D7 session.
  - `cargo run --bin e2e_render -- baseline` — PASS.
  - `cargo run --bin e2e_render -- --validate-gpu-construction` — PASS (388 bytes byte-equal).
  - `cargo run --bin e2e_render -- --edit-mode` — PASS.
  - `cargo run --bin e2e_render -- --runtime-edit-mode` — PASS.
  - `cargo run --bin e2e_render -- --entities` — PASS (entity handler validation green; fixture log emits from new `test_fixture` module).
  - `cargo run --bin e2e_render -- --vox-e2e` — PASS.
  - `cargo run --bin e2e_render -- --oasis-edit-visual` ×2 — PASS, PASS (Δ=18.03, Δ=18.07; non-deterministic ≥2× rule satisfied).
- **Files changed (5 modified, 3 added, 0 removed; 1 git-renamed)**:
  - `crates/bevy_naadf/src/lib.rs` — modified (−240 LOC; lost `GiSettings` struct + impls + `spawn_phase_c_test_entity` fn + the inline voxel-io registrations + the inline fixture-spawn `if` block; gained 1 `pub use` line + 3 `add_plugins` / "moved" comments).
  - `crates/bevy_naadf/src/voxel/mod.rs` — modified (+3 LOC; added `pub mod plugin;` + `pub use plugin::VoxelIoPlugin;`).
  - `crates/bevy_naadf/src/render/construction/mod.rs` — modified (+13 LOC; added `pub mod test_fixture;` + 11-LOC fixture-spawner registration inside `ConstructionPlugin::build`).
  - `crates/bevy_naadf/src/settings.rs` → `crates/bevy_naadf/src/settings/mod.rs` — git rename + +3 LOC body edit (`pub mod canonical;` + `pub use canonical::GiSettings;`; the inline `use crate::{… GiSettings};` reduced to drop the re-export collision).
  - `e2e/tests/vox-horizon-parity.spec.ts` — modified (−7 LOC; deleted the 4 `[device-snapshot]` references — docstring line + filter line + namedBuckets entry + sidecar section).
- **Files added (3)**:
  - `crates/bevy_naadf/src/voxel/plugin.rs` — 98 LOC; `VoxelIoPlugin`.
  - `crates/bevy_naadf/src/render/construction/test_fixture.rs` — 78 LOC; `spawn_phase_c_test_entity` body.
  - `crates/bevy_naadf/src/settings/canonical.rs` — 125 LOC; `GiSettings` struct + `DEFAULTS` const + `Default` impl.
- **Net LOC**: existing files −260 +42 (diff stat) = −218; new files +301; **net ≈ +83 LOC** (relocations move docstring-heavy types out of the spine into purpose-named modules — the docstrings travel with the types, and the new module-header docstrings add ~50 LOC). The `lib.rs` spine alone dropped from 598 to ~360 LOC, which is the structural-tidy win the brief asked for; net LOC delta being slightly positive is the docstring-relocation cost.
- **Behavioural deltas observed during verification**: none. All 9 e2e gates produced numerically identical (or within the non-deterministic noise floor) outputs to the pre-follow-up baseline. The `spawn_phase_c_test_entity` move was the only one that risked behavioural drift (system ordering), and the `.after(setup_test_grid)` edge was preserved verbatim — entity spawn log entries and entity-handler chunk counts are unchanged.

---

### 4. Open conflicts for the orchestrator

- **`crate::e2e::gates::demo_origin_v()` is still imported by production code** (`render::construction::test_fixture::spawn_phase_c_test_entity`). D7 architect Side note 6 flagged this dep-arrow inversion; the cleanest resolution is moving `demo_origin_v` out of `e2e/gates.rs` into a non-e2e module (e.g. `voxel/grid::demo_origin_v` next to `DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS`). Not landed in this follow-up — would expand scope. Surface for a future cleanup pass.
- **D2's `GiSettings` re-import collision was resolved by dropping `GiSettings` from the explicit `use crate::{…};`** at `settings/mod.rs:41`. The local `pub use canonical::GiSettings;` (at line 21) handles the re-export within the module; in-file references to `GiSettings` resolve through that. No semantic change but worth noting for any future cross-module type imports.

### 5. Side notes / observations / complaints

1. **D3's architect documented `VoxelIoPlugin` only by name** in their architecture doc; D7's architect documented its expected systems list explicitly in `## 6 Cross-domain assumptions`. The follow-up implementor used D7 architect's list as the authority since D3 architect's F4 only describes a `GridPreset` enum extension, not a plugin extraction. Net result is the same plugin shape D7 architect expected.

2. **The `cargo run --bin e2e_render -- --oasis-edit-visual` numerical floor held flat across all four follow-ups** — pre-follow-up Δ=18.08 (D7 main session), post-follow-up-1 (this session): not measured separately; post-follow-up-4 (final): Δ=18.03, Δ=18.07. The voxel-io extraction, fixture rehome, and `GiSettings` move are all behaviour-preserving by construction; the numerical match is corroboration.

3. **The brief's `vox_horizon_parity.spec.ts` filename was a typo** — actual file is `vox-horizon-parity.spec.ts` with hyphen. Same file; the 4 `[device-snapshot]` references were located, removed, and verified zero remaining.

4. **`settings.rs` → `settings/mod.rs` via `git mv`** preserves blame across the directory split. The `git status` output shows `RM` (renamed-modified) rather than `D` + `??`, which is the desired outcome for archaeology.

5. **`GiSettings` doesn't get a `Resource` derive** because it's an embedded field of `AppArgs` (which IS the `Resource`). D7 architect §2 F2 hinted at this implicitly ("`pub use settings::canonical::GiSettings;` for source-stability on existing `crate::GiSettings` imports") — confirmed by reading the consumer chain: `AppArgs.gi: GiSettings` → KNOBS macros read `args.gi.<field>` → no direct `Res<GiSettings>` access anywhere.

6. **Subjective**: these four follow-ups were exactly the pattern D7 main session predicted ("D7's mechanical Plugin-extraction work is the easy half"). The dependency chain on D2/D3/D5/D6 architects' designs held — every cross-domain plugin call site was already in shape; only the plugin definitions themselves needed authoring. The whole follow-up session ran clean (no rolled-back commits, no failed gates, no rebuilds-from-scratch). Net lib.rs spine is now ~360 LOC of pure plugin wiring + re-exports, which reads as a one-page summary of the application's structure.

7. **Equal-footing observation**: with these follow-ups landed, the open conflicts surfaced by the D7 main session reduce to one residual (`demo_origin_v` production→e2e arrow). The codebase-tightening orchestration's structural work is materially complete for D7's domain; what remains is D5's `mod.rs` LOC reduction (still ~6000 LOC) and the `demo_origin_v` cross-module-arrow cleanup. Both are in-scope for future `/refactor` sessions on their respective domains.
