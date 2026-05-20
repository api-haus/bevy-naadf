# D7 — app-and-camera — architecture

## refactor-architect findings (2026-05-20)

**Author**: refactor-architect (codebase-tightening, D7 of 8).
**Status**: D7 lands LAST — after D1, D2, D3, D4, D5, D6, D8.
**Anchor**: `01-context.md` (incl. 2026-05-20 addendum); `app-and-camera/02-exploration.md`; `CLAUDE.md`; `00-reuse-audit.md §2 D7 + §3 SSoT-1/2`.
**Domain LOC pre-refactor (verified `wc -l`)**: lib 1146 + main 54 + camera/mod 287 + camera/position_split 177 + diagnostics 711 = **2 375 LOC**.
**Domain LOC post-refactor (target)**: lib ≤ 500 + main 54 + camera/* (mod + position_split + plugin + pose + drift) ≈ 460 + diagnostics 148 + app_mode (relocated) ~95 + new files (app_args/settings_canonical/window_config/dev_font/spawn_test_entity) ≈ 450 ≈ **~1 700 LOC** (excluding files relocated into D2/D3/D5 modules, which take ~310 LOC with them).

---

### 1. Findings addressed

The brief told D7 to cover the 10 findings in `02-exploration.md`, plus the SSoT-1/SSoT-2 design coordinated with D2/D4, the `device_snapshot` deletion (Resolution A), the PBR `AppArgs` field deletion (Resolution C), `WindowConfig` centralisation (F3), camera-pose `Local` cleanup (F6), and the `build_app_with_args` decomposition (F1).

| Finding | Title | Addressed by |
|---|---|---|
| F1 | `build_app_with_args` monolith (336 LOC) | §2 F1 + Migration Steps 4–7 |
| F2 | `GiSettings` triple-canonical (SSoT-1) | §2 F2 + Step 2 |
| F3 | `WindowConfig` ladder + ad-hoc literal | §2 F3 + Step 3 |
| F4 | `AppArgs` mixes prod + 11 e2e flags (+ PBR fields per addendum Res-C) | §2 F4 + Step 1 (PBR delete) + Step 8 (E2eMode split, coord with D6) |
| F5 | SSoT-2 `WORLD_SIZE_*` hand-typed | §2 F5 + Step 2 |
| F6 | camera-pose `Local<Option<Transform>>` drift + magic numbers | §2 F6 + Step 5 |
| F7 | `device_snapshot` full delete-chain (addendum Resolution A) | §2 F7 + Step 1 |
| F8 | conditional `sync_position_split` double-registration | §2 F1 (subsumed) + Step 5 |
| F9 | press-P dump body — formatting + queries tangled | §2 F9 — judgment call: **leave it.** Rationale in §2 F9. |
| F10 | `lib.rs` `pub` surface after extraction | §2 F10 + Step 9 |

Findings deferred / not addressed:
- **F9 deep refactor (Display impl on Diagnostics struct)** — explicit judgement call to **leave the 103-LOC press-P body unchanged**. Single-purpose debug handler; abstraction adds indirection for no real readability win. Rationale tracks F9's own "Or — judgment call — leave it" branch.

---

### 2. Target-state architecture

#### Finding F1: `build_app_with_args` plugin decomposition

**Current shape (verified):** `crates/bevy_naadf/src/lib.rs:638-974` is a 336-line `pub fn` body with ten inline sections — `DlssProjectId` insert, `Window` build, `AppArgs/CameraHistory/DefaultPlugins` insert, the 7-plugin tuple, the `add_free_camera` branch (lines 770-784) with 3 camera systems chained, the `!cfg.add_e2e_systems` branch wiring `DiagnosticsPlugin` (788-790), unconditional `DeviceSnapshotPlugin` (799), `load_dev_font` + `setup_test_grid` + async-vox pump + wasm `web_vox` + native dnd (803-864), the `args.spawn_test_entity` branch (874-876), and finally the 95-LOC `cfg.add_hud` block (lines 900-971) that owns `AppMode` state init + 3 resources + 15 systems across `Startup`/`OnEnter`/`OnExit`/`Update` with hand-written `.after()` ordering.

**Target shape:**

```rust
// crates/bevy_naadf/src/lib.rs — total ~500 LOC after extraction.

pub fn build_app(cfg: AppConfig) -> App {
    build_app_with_args(cfg, AppArgs::default())
}

pub fn build_app_with_args(cfg: AppConfig, args: AppArgs) -> App {
    let mut app = App::new();

    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    app.insert_resource(DlssProjectId(bevy::asset::uuid::uuid!(
        "8f6b1d2e-3c4a-4f5b-9a7c-1e2d3f4a5b6c"
    )));

    app.insert_resource(args.clone())
        .insert_resource(cfg)
        .init_resource::<render::taa::CameraHistory>()
        .add_plugins(default_plugins_for(&cfg));

    // Core engine + render plugins (already extracted in tree, just re-listed).
    app.add_plugins((
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
        world::WorldPlugin,
        render::NaadfRenderPlugin,
        render::construction::ConstructionPlugin,
        baked_material::BakedMaterialPlugin,    // D8 may delete; this stays for now.
        texture_array::TextureArrayPlugin,      // D8 may delete; this stays for now.
    ));

    // D7-owned plugins (NEW seam):
    app.add_plugins((
        DevFontPlugin,                          // ROBOTO_REGULAR_BYTES + load_dev_font
        camera::CameraPlugin,                   // setup_camera + sync + DLSS + pose-changes;
                                                //   reads cfg.add_free_camera from Res<AppConfig>
        diagnostics::DiagnosticsPlugin,         // press-P only (device_snapshot gone, Res-A);
                                                //   self-skips when AppConfig.add_e2e_systems
    ));

    // D3-owned voxel-io plugin (D3 architect lands `VoxelIoPlugin` that wraps
    // setup_test_grid + async pump + wasm web_vox + native dnd; D7 just wires).
    app.add_plugins(voxel::VoxelIoPlugin);

    // D2-owned editor/settings/HUD/AppMode plugins (D2 architect lands them as
    // a PluginGroup so D7 adds one call). Self-gated on `Res<AppConfig>.add_hud`.
    if cfg.add_hud {
        app.add_plugins(editor::EditorUiPlugins);
    }

    // D6-owned e2e plugin (D6 architect lands `E2ePlugin` as a `PluginGroup`).
    if cfg.add_e2e_systems {
        app.add_plugins(e2e::E2ePlugin);
    }

    app
}

// Helper — collects the 3 `.set()`s on DefaultPlugins. ~30 LOC.
fn default_plugins_for(cfg: &AppConfig) -> impl PluginGroup { /* ... */ }
```

**Reuse choices:**

- **`Plugin` per subsystem** is the established idiom in this codebase. Seven plugins are already extracted (`WorldPlugin`, `NaadfRenderPlugin`, `ConstructionPlugin`, `BakedMaterialPlugin`, `TextureArrayPlugin`, `DiagnosticsPlugin`, `DeviceSnapshotPlugin`); we extend the seam — no new pattern.
- **`Res<AppConfig>` insertion before `add_plugins`** is already a Bevy idiom (the resource is inserted at line 679 today; we hoist it slightly so internal plugins can read it).
- **`PluginGroup`** (e.g. `editor::EditorUiPlugins`) is `bevy::app::PluginGroup` — Bevy ships the trait for exactly the "bundle of related plugins" use case (e.g. `DefaultPlugins`).
- **No new types invented for the spine.** `CameraPlugin`, `DevFontPlugin`, `VoxelIoPlugin`, `E2ePlugin`, `EditorUiPlugins` already exist in proposed form across the other architect docs (D2 `02-exploration.md` Side note 11, D3 `02-exploration.md` F4 suggestion, D6 `02-exploration.md` Finding 6 Suggested direction).

**Behavioural delta:**

- **No behaviour change at the `App.update()` level.** Every system that exists today still runs at the same schedule + ordering in `Update`/`Startup`/`OnEnter`/`OnExit`. The plugins wrap, they do not reorder.
- **`DiagnosticsPlugin` becomes self-skipping under e2e** instead of being conditionally registered. Inside `DiagnosticsPlugin::build`, the system is wired with `.run_if(|cfg: Res<AppConfig>| !cfg.add_e2e_systems)`. Net behaviour identical (the press-P dump never fires under e2e); central registry simplified.
- **`spawn_phase_c_test_entity` (`lib.rs:1052-1095`) moves under `render/construction/`** (D5 territory — `MainWorldEntities` lives there). D5 architect proposes their re-home; D7 only deletes the call.
- **`render::taa::update_camera_history` ordering stays.** Today `lib.rs:895-898` registers it `.after(camera::sync_position_split)`. Post-refactor `CameraPlugin::build` registers the same `.after()` edge — the function definition stays in `render/taa.rs` (D4 territory; D4 architect agrees to leave it, D7 only references it from `CameraPlugin`). This avoids a D4↔D7 cross-edit.

---

#### Finding F2: `GiSettings` canonical location + SSoT-1

**Current shape (verified):**

- `GiSettings` struct + `Default` impl at `crates/bevy_naadf/src/lib.rs:108-231` (~120 LOC).
- KNOBS table that exposes the struct to UI at `crates/bevy_naadf/src/settings.rs:172-219` (D2-owned).
- GPU uniform mirror fields at `crates/bevy_naadf/src/render/gpu_types.rs:87` (`max_ray_steps_primary`) and `:539` (`spatial_iter_count`) (D4-owned).
- WGSL `MAX_RAY_STEPS_*` consts at `assets/shaders/ray_tracing.wgsl:122-126` (D4-owned).
- Default values duplicated at 5 sites; only 2 are test-pinned (`settings.rs::defaults_match_gi_settings_default` + `:promoted_defaults_match_canonical_consts`).

**Target shape:**

```rust
// crates/bevy_naadf/src/settings/canonical.rs — NEW FILE (D7-owned).
//
// SSoT-1 canonical home. D2's KNOBS table reads `GI_DEFAULTS.*`; D4's GPU
// uniform mirror also reads `GI_DEFAULTS.*` via `From<&AppArgs>` on the
// GpuRenderParams/GpuGiParams structs (D4 architect proposes the conversion
// shape).

pub struct GiSettings { /* the existing 23 fields, verbatim */ }

impl GiSettings {
    /// The canonical defaults — single source of truth for the C# slider
    /// defaults (`WorldRenderBase.cs:14-25`) + the 5 promoted ray-step caps
    /// + spatial_iter_count. Consumed by:
    ///   - `Default for GiSettings` (just returns this)
    ///   - D2's `settings::KNOBS` table `default:` fields
    ///   - D4's `GpuRenderParams::from(&AppArgs)` / `GpuGiParams::from(&AppArgs)`
    ///   - The press-P dump (`diagnostics.rs`) for "what would defaulting reset to?"
    pub const DEFAULTS: GiSettings = GiSettings {
        bounce_count: 3,
        global_illum_max_accum: 128,
        spatial_resample_size: 500.0,
        spatial_visibility_count: 80,
        denoise_thresh: 400.0,
        radius_lit_factor: 3.0,
        noise_suppression_factor: 0.4,
        skip_samples: true,
        is_denoise: true,
        is_sample_leveling: true,
        is_varying_resampling_radius: true,
        is_atmosphere_interaction: true,
        sun_shadow_taps: 1,
        max_ray_steps_primary: 120,
        max_ray_steps_secondary: 100,
        max_ray_steps_sun: 120,
        max_ray_steps_sun_secondary: 80,
        max_ray_steps_visibility: 60,
        spatial_iter_count: 12,
    };
}

impl Default for GiSettings {
    fn default() -> Self { Self::DEFAULTS }
}
```

The struct **moves out of `lib.rs` into the new `crates/bevy_naadf/src/settings/canonical.rs` module** (with `settings.rs` becoming `settings/mod.rs` — D2's architect already proposes `settings/` as a directory in HIGH-3.q). `lib.rs` adds `pub use settings::canonical::GiSettings;` for source-stability on existing `crate::GiSettings` imports.

**Reuse choices:**

- **`const` for compile-time defaults**: Rust's struct literals in `const` context have worked for primitive fields since Rust 1.31. All `GiSettings` fields are `u32`/`f32`/`bool` — `const GiSettings { ... }` literal compiles today; no `const fn` builder needed. This was confirmed by reading D2's `02-exploration.md` HIGH-4 ("an `impl GiSettings { pub const DEFAULT: GiSettings = GiSettings {...} }` constant would work").
- **`pub use` re-export in `lib.rs`** — Bevy ecosystem idiom for source-stability when moving a type to its proper module.
- **No `Reflect` derivation here**: that's BEV-4 / OA-1 work for D2 architect — out of scope for D7. D7's job is to make the move; D2 chooses Reflect-vs-decl-macro on top of the moved struct.

**Behavioural delta:**

- **No behaviour change.** Defaults identical; `Default for GiSettings` returns the same struct. Tests `default_taa_ring_depth_*` etc. still pass.
- **Tests `defaults_match_gi_settings_default` (`settings.rs:875-892`) becomes trivial** once D2's KNOBS rows read `GiSettings::DEFAULTS.*` instead of literal `120`/`100`/etc. The test is preserved as a sanity check pinning `GiSettings::DEFAULTS` against `GiSettings::default()` — the assertions become tautologies but pin the contract for future readers. D2 architect handles the actual KNOBS edit; D7 only provides the `const DEFAULTS`.

---

#### Finding F3: `WindowConfig` centralisation

**Current shape (verified):**

- 4 named constructors at `crates/bevy_naadf/src/lib.rs:485-568` (`windowed`, `e2e`, `e2e_horizon`, `e2e_resize_test`).
- A 5th window — the small-edit-repro at the user's screen size — inlined as a struct literal at `crates/bevy_naadf/src/lib.rs:1006-1015` (inside `run_e2e_render_with_args`).
- The mode→window-config mapping is scattered: 3 `if args.<mode>` branches at `:999-1022`.

**Target shape:**

```rust
// crates/bevy_naadf/src/window_config.rs — NEW FILE, ~80 LOC.

pub struct WindowConfig {
    pub resolution: Option<(f32, f32)>,
    pub resizable: bool,
    pub title: &'static str,
    pub name: Option<&'static str>,
}

impl WindowConfig {
    pub fn windowed() -> Self { /* prod default */ }
    pub fn e2e() -> Self { /* 256×256 fixed */ }
    pub fn e2e_horizon() -> Self { /* 1280×720 */ }
    pub fn e2e_resize_test() -> Self { /* 800×600, resizable */ }
    pub fn e2e_small_edit_repro() -> Self { /* user's 1920×1080, NEW constructor */ }
}

/// Maps the active e2e mode (or `None` for production) to its `WindowConfig`.
/// Replaces the scattered `if args.<flag>` ladder in `run_e2e_render_with_args`.
pub fn window_for_e2e_args(args: &AppArgs) -> WindowConfig {
    if args.resize_test { WindowConfig::e2e_resize_test() }
    else if args.small_edit_repro_mode { WindowConfig::e2e_small_edit_repro() }
    else if args.vox_horizon_native_phase { WindowConfig::e2e_horizon() }
    else { WindowConfig::e2e() }
}
```

`run_e2e_render_with_args` collapses (`lib.rs:993-1025`) to:

```rust
pub fn run_e2e_render_with_args(args: AppArgs) -> AppExit {
    let mut cfg = AppConfig::e2e();
    cfg.window = window_for_e2e_args(&args);
    let app = build_app_with_args(cfg, args);
    e2e::run_with_app(app)
}
```

**Reuse choices:**

- **No new types** — `WindowConfig` already exists; we add one constructor + one free function.
- **`fn window_for_e2e_args`** is the F3 explorer's suggestion 2, picked over suggestion 3 (push the constant into each `e2e/<mode>.rs`) because the mapping is mode→config — a small function captures it more legibly than 11 scattered consts. **Rejected**: F4's `E2eMode` enum (eventually) would let this become a `match` rather than an if-ladder. **Accepted partial** path: ship the function shape now; once F4's enum lands (D6+D7 coord, Step 8) it becomes `match args.e2e_mode() { … }`.

**Behavioural delta:**

- **No behaviour change.** The 5 window configs produced are byte-identical to today's; only the mapping site moves.
- The inline `WindowConfig { … }` literal at `lib.rs:1006-1015` becomes `WindowConfig::e2e_small_edit_repro()`. Source-stable; same fields.

---

#### Finding F4 + Resolution C: `AppArgs` cleanup — PBR field deletion + production/e2e separation

**Current shape (verified):**

- `AppArgs` struct at `crates/bevy_naadf/src/lib.rs:283-462` with 19 fields. Mix: 5 production-meaningful + 1 fixture (`spawn_test_entity`) + 11 e2e-mode flags.
- **PBR fields per addendum Res-C and D6 Finding 1**: the brief says `AppArgs.pbr_*_mode` fields must be deleted. **Empirical verification (Read of `lib.rs:283-462`)**: no `pbr_*` fields exist in the current `AppArgs` struct. D6 explorer Finding 1 corroborates — the orphan PBR modules at `e2e/pbr_*.rs:82,294,219` reference `args.pbr_*_mode` fields that don't exist (the modules cannot compile if re-included). The fields were already deleted before this orchestration started. **D7's PBR-field cleanup is a no-op on the production side — only D6 deletes the orphan files.** Flag this in §3 Step 1 verification.
- `AppArgs::default()` at `lib.rs:441-462` initialises all 11 e2e flags to `false`.

**Target shape (initial, this orchestration):**

```rust
// crates/bevy_naadf/src/app_args.rs — NEW FILE, ~250 LOC after splitting docs.
// `AppArgs` moves out of `lib.rs` for tidy public-surface (F10).

#[derive(Resource, Clone)]
pub struct AppArgs {
    // === Production-meaningful (5 fields) ===
    pub grid_preset: GridPreset,
    pub taa: bool,
    pub taa_ring_depth: u32,
    pub gi: GiSettings,
    pub construction_config: ConstructionConfig,

    // === Fixture entity (1 field — used in both prod and e2e) ===
    pub spawn_test_entity: bool,

    // === E2E mode/phase markers (11 fields — stay flat for now) ===
    // Migration to enum E2eMode deferred to Step 8 (D6+D7 coord). The flag set
    // matches today's; only the home moves from lib.rs to app_args.rs.
    pub resize_test: bool,
    pub vox_e2e_mode: bool,
    pub oasis_edit_visual_mode: bool,
    pub small_edit_visual_mode: bool,
    pub small_edit_repro_mode: bool,
    pub vox_gpu_construction_mode: bool,
    pub vox_gpu_oracle_cpu_phase: bool,
    pub vox_gpu_oracle_gpu_phase: bool,
    pub vox_web_parity_skybox_phase: bool,
    pub vox_web_parity_loaded_phase: bool,
    pub vox_horizon_native_phase: bool,
}
```

**Reuse choices:**

- **Type stays as one struct**; the production/e2e split-into-`enum E2eMode` is deferred to Step 8 (cross-domain with D6) because changing the surface mid-refactor would break D6's mid-flight implementor session.
- **`taa` docstring fix**: `lib.rs:289` says "always `false` in Phase A" but `Default for AppArgs` (`:445`) sets `taa: true`. Fix the docstring as part of the move — drop the obsolete "Phase A" framing. (Side-note 3 of exploration.)
- **`pub use` from lib.rs**: `crate::AppArgs` stays reachable via re-export.

**Behavioural delta:**

- **No behaviour change.** Fields preserved; defaults preserved; field types preserved.
- Future Step 8 (if accepted by D6+user) splits into `AppArgs` (6 prod) + `enum E2eMode { Standard, ResizeTest, VoxE2e, OasisEditVisual, SmallEditVisual, SmallEditRepro, VoxGpuConstruction, VoxGpuOracleCpu, VoxGpuOracleGpu, VoxWebParitySkybox, VoxWebParityLoaded, VoxHorizonNative }`. **D7 architect proposes the future shape; impl step is staged but not landed in this orchestration** — see §3 Step 8.

---

#### Finding F5: SSoT-2 `WORLD_SIZE_IN_*` `const fn` derivation

**Current shape (verified):**

- `crates/bevy_naadf/src/lib.rs:241-260` — three `pub const UVec3`s hand-typed (`WORLD_SIZE_IN_SEGMENTS = (16,2,16)`, `WORLD_SIZE_IN_CHUNKS = (256,32,256)`, `WORLD_SIZE_IN_VOXELS = (4096,512,4096)`) plus a `u32` (`WORLD_GEN_SEGMENT_SIZE_IN_GROUPS = 4`).
- `lib.rs:1131-1145` — `tests::fixed_world_size_constants_agree` enforces `CHUNKS = SEGMENTS * GROUPS * 4` and `VOXELS = CHUNKS * 16` at test time only.
- Comment at `lib.rs:255-256` claims "Hardcoded rather than computed because `glam`'s `UVec3` ops are not `const`."

**Target shape:**

```rust
// crates/bevy_naadf/src/world_size.rs — NEW FILE, ~60 LOC including docs.

use bevy::math::UVec3;

/// C# `WorldHandler.worldSizeToUseInWorldGenSegments` (`WorldHandler.cs:19`).
pub const WORLD_SIZE_IN_SEGMENTS: UVec3 = UVec3::new(16, 2, 16);

/// C# `WorldHandler.worldGenSegmentSizeInGroups` (`WorldHandler.cs:18`).
pub const WORLD_GEN_SEGMENT_SIZE_IN_GROUPS: u32 = 4;

/// `const fn` component-wise scalar multiply. glam's `UVec3 * u32` is not
/// `const`; this helper is.
const fn mul_uvec3(v: UVec3, k: u32) -> UVec3 {
    UVec3::new(v.x * k, v.y * k, v.z * k)
}

/// Derived: 256/32/256 chunks. `SEGMENTS × GROUPS × 4` (4 chunks per group).
pub const WORLD_SIZE_IN_CHUNKS: UVec3 =
    mul_uvec3(mul_uvec3(WORLD_SIZE_IN_SEGMENTS, WORLD_GEN_SEGMENT_SIZE_IN_GROUPS), 4);

/// Derived: 4096/512/4096 voxels. `CHUNKS × 16`.
pub const WORLD_SIZE_IN_VOXELS: UVec3 =
    mul_uvec3(WORLD_SIZE_IN_CHUNKS, 16);

#[cfg(test)]
mod tests {
    use super::*;
    /// Pin against C# canonical values (`WorldHandler.cs:18-19`) — catches
    /// segment factor edits that would silently change the canonical world.
    #[test]
    fn world_size_matches_csharp() {
        assert_eq!(WORLD_SIZE_IN_CHUNKS, UVec3::new(256, 32, 256));
        assert_eq!(WORLD_SIZE_IN_VOXELS, UVec3::new(4096, 512, 4096));
    }
}
```

**Reuse choices:**

- **`const fn` helper** over a macro: simpler, type-checked, IDE-friendly.
- **C# canonical pin retained**: the `fixed_world_size_constants_agree` test's "drift from segments × groups × 4" assertion becomes redundant by construction; only the "matches C# canonical" assertion remains. Saves the test from being a tautology while keeping the C#-faithful pin.
- **`pub use` from `lib.rs`**: preserves `crate::WORLD_SIZE_IN_*` import paths.

**Behavioural delta:**

- **No behaviour change.** Same 4 constants, same values, same `pub const UVec3` type.
- **Test pruning**: the `fixed_world_size_constants_agree` derivation half is by construction; only the C# canonical pin half stays. Net: test is shorter, equally strong.

---

#### Finding F6: camera-pose `Local` drift + magic numbers

**Current shape (verified):**

- `crates/bevy_naadf/src/camera/mod.rs:179-213` — `apply_initial_camera_pose_changes` holds `Local<Option<Transform>>` tracking last-applied pose, compares live `Transform.translation`/`rotation` against it.
- Thresholds: `translation_drift > 1.0` (squared length, line `:196`), `rotation_drift > 0.01` rad. Both bare literals.

**Target shape:**

```rust
// crates/bevy_naadf/src/camera/mod.rs — named consts at the head, drift logic
// otherwise unchanged.

/// User-movement drift threshold (squared length). If the camera's current
/// translation differs from the last applied `InitialCameraPose` by more
/// than this squared distance, treat it as "user has flown" and don't snap
/// back. Squared because we test `length_squared()`; `1.0` square == 1.0
/// world-units distance (a single voxel).
const DRIFT_TRANSLATION_THRESHOLD_SQ: f32 = 1.0;

/// User-movement drift threshold (radians). Same intent as
/// `DRIFT_TRANSLATION_THRESHOLD_SQ` but for `rotation.angle_between(last)`.
/// `0.01` rad ≈ 0.57°: small enough that float-rounding doesn't trigger,
/// large enough that any deliberate look-around exceeds it.
const DRIFT_ROTATION_THRESHOLD_RAD: f32 = 0.01;

pub fn apply_initial_camera_pose_changes(
    initial_pose: Option<Res<InitialCameraPose>>,
    mut camera: Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>,
    mut last_applied: Local<Option<Transform>>,
) {
    let Some(initial_pose) = initial_pose else { return; };
    if !initial_pose.is_changed() { return; }
    let new_pose = initial_pose.0;
    let (cam_transform, cam_position_split) = &mut *camera;
    if let Some(last) = *last_applied {
        let t_drift_sq = (cam_transform.translation - last.translation).length_squared();
        let r_drift = cam_transform.rotation.angle_between(last.rotation);
        if t_drift_sq > DRIFT_TRANSLATION_THRESHOLD_SQ
            || r_drift > DRIFT_ROTATION_THRESHOLD_RAD
        {
            return;
        }
    }
    **cam_transform = new_pose;
    **cam_position_split = PositionSplit::from_world(new_pose.translation);
    *last_applied = Some(new_pose);
    info!(/* … unchanged */);
}
```

**Reuse choices:**

- **Bare consts at module head**, not a struct or builder. This is one system with two thresholds; over-abstracting (event-driven `UserMovedCamera` marker, etc.) would cost more than it saves and would cross into `FreeCameraPlugin` (third-party) territory.
- **No `Changed<FreeCamera>` event redesign**: explorer's F6 "Suggested direction" pointed at the event-driven alternative but called it "architect's judgement call". Judgement: **leave the drift detector**, fix the magic numbers. The current behaviour is empirically correct; the abstraction win is theoretical.

**Behavioural delta:**

- **No behaviour change.** Same thresholds, same comparison logic. Only the literal `1.0`/`0.01` get named.

---

#### Finding F7 + Resolution A: `device_snapshot` full delete-chain

**Current shape (verified):**

- `crates/bevy_naadf/src/diagnostics.rs:155-711` — the `device_snapshot` submodule (~557 LOC: schema types `DeviceSnapshot`, `SnapshotAdapterInfo`, `SnapshotLimits`, `SnapshotDownlevel`, `LimitDelta`, `SnapshotBuild`, `PendingRenderSnapshot`, `PendingMainSnapshot`; capture/extract/emit systems; helpers `snapshot_adapter_info`, `features_to_names`, `features_to_bits`, `limits_to_snapshot`, `snapshot_downlevel`, `compute_limit_deltas`, `build_facts`; `DeviceSnapshotPlugin`).
- `crates/bevy_naadf/src/diagnostics.rs:155-174` — section divider + comment block + module docstring lines 12-24 (the "Device snapshot" half).
- `crates/bevy_naadf/src/lib.rs:792-799` — `DeviceSnapshotPlugin` registration + 7-line comment block above it.
- `crates/bevy_naadf/src/diagnostics.rs:1-24` module docstring — the press-P half stays, the device-snapshot half goes.

**Cross-domain (logged for coordination, NOT D7 territory):**

- `crates/bevy_naadf/src/bin/e2e_render.rs:137-143` (`device_snapshot_native_mode` flag) — D6 deletes.
- `crates/bevy_naadf/src/bin/e2e_render.rs:364-375` (`--device-snapshot-native` dispatch branch) — D6 deletes.
- `crates/bevy_naadf/src/bin/diag_compare.rs` (whole 314-LOC file) — D6 deletes (D6's Finding 7).
- `crates/bevy_naadf/Cargo.toml` — `[[bin]] name = "diag_compare"` entry — D6 deletes.
- `e2e/tests/device-snapshot.spec.ts` — D6 deletes.
- `justfile:194-204` — `diag-snapshot-native` / `diag-snapshot-web` recipes — D6 deletes (D6's Finding 7 enumerates).
- `crates/bevy_naadf/src/e2e/vox_horizon_parity.spec.ts:122,147,158,187` — these references **consume the `[device-snapshot]` console sentinel for diagnostic output only — not load-bearing** per D6 Finding 7 verification. Architect call: D6 may delete or leave these console-pass-through reads. **Flag in §5 Open conflicts.**

**Target shape:**

```rust
// crates/bevy_naadf/src/diagnostics.rs — total ~148 LOC after delete.

//! Press-`P` runtime diagnostics dump.
//!
//! One read-only `Update` system that, on `KeyCode::KeyP` just_pressed,
//! formats a single multi-line block and emits it via `info!` (on wasm32
//! this routes through Bevy's `LogPlugin` to `console.log`, so the same
//! dump appears in browser DevTools). Mutates nothing.

use std::fmt::Write;
use bevy::camera::Camera;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::AppArgs;
use crate::AppConfig;
use crate::camera::position_split::PositionSplit;
use crate::editor::ray::screen_to_ray;
use crate::world::data::{VoxelTypes, WorldData};

pub fn dump_diagnostics_on_p(/* … unchanged body … */) { /* … */ }

pub struct DiagnosticsPlugin;

impl Plugin for DiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        // Self-gate on AppConfig — under e2e, the dump's required resources
        // may be absent + the harness is non-interactive. Replaces today's
        // conditional registration at lib.rs:788-790.
        app.add_systems(
            Update,
            dump_diagnostics_on_p
                .run_if(|cfg: Res<AppConfig>| !cfg.add_e2e_systems),
        );
    }
}
```

**Reuse choices:**

- **`.run_if(|cfg: Res<AppConfig>| !cfg.add_e2e_systems)`** — Bevy idiom for conditional system execution. Replaces the `if !cfg.add_e2e_systems { app.add_plugins(DiagnosticsPlugin); }` ladder at `lib.rs:788-790`. Internalises the gating to the plugin (F1 idiom).
- **Delete everything else.** Module docstring shrinks to the press-P-only half (lines 1-11 today are the joint header — collapse to a focused 4-line block).

**Behavioural delta:**

- **No device-snapshot capture** in either native or web. The JSON dump to `target/diagnostics/device-snapshot-*.json` stops; the `[device-snapshot] …` console sentinel stops.
- **No regression** on the press-P press-P diagnostics — body unchanged.
- **Cargo dep audit**: the `serde` + `serde_json` deps may have been pulled in for `device_snapshot`. Verify `Cargo.toml` whether they're still needed by `diag_compare.rs` only (going away) or by other code. **Investigate at impl time** — likely safe to keep; this is not a `cargo nuke unused deps` pass.

---

#### Finding F8: conditional `sync_position_split` double-registration

**Current shape (verified):**

- `crates/bevy_naadf/src/lib.rs:770-784` — `if cfg.add_free_camera { ... } else { ... }` registers `sync_position_split` in both arms (chained in first arm, standalone in second).

**Target shape (subsumed by F1 — `CameraPlugin::build`):**

```rust
// crates/bevy_naadf/src/camera/mod.rs — CameraPlugin lands here.

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        // Single, unconditional registration. `sync_position_split` is a pure
        // function of Transform — always safe to run.
        app.add_systems(Update, sync_position_split);

        // The camera-history ring update must run after sync_position_split
        // so the ring stores this frame's current camera state.
        app.add_systems(
            Update,
            render::taa::update_camera_history.after(sync_position_split),
        );

        // setup_camera runs after voxel/grid::setup_test_grid (so the
        // GridPreset::Vox arm has had a chance to insert InitialCameraPose).
        // The Startup ordering remains unchanged from today's lib.rs:886-890.
        app.add_systems(
            Startup,
            setup_camera.after(crate::voxel::grid::setup_test_grid),
        );

        // FreeCamera + DLSS + initial-pose-changes — only when the production
        // config asks for it.
        app.add_systems(
            Update,
            (toggle_dlss, apply_initial_camera_pose_changes)
                .before(sync_position_split)
                .run_if(|cfg: Res<AppConfig>| cfg.add_free_camera),
        );

        // FreeCameraPlugin itself is third-party; only add it when wanted.
        // The `.run_if` doesn't help with plugin registration — we still
        // gate plugin add at the lib.rs site for the FreeCameraPlugin add.
        // (Alternative: add the plugin unconditionally and let cfg.add_free_camera
        //  drive its systems' `.run_if`. We choose to keep it conditional
        //  because FreeCameraPlugin owns its own resources we don't want to
        //  pay for under e2e.)
    }
}
```

**Reuse choices:**

- **One `sync_position_split` registration site.** Resolves F8.
- **`.run_if` filters the free-camera-coupled systems** instead of `if` branching the registration. Single source of truth.
- **FreeCameraPlugin add stays conditional** at the `build_app_with_args` site (or inside `CameraPlugin::build`); this is a third-party plugin and pulling its resources under e2e wastes memory + may interact with the harness. Final placement: inside `CameraPlugin::build`, `if app.world().get_resource::<AppConfig>().is_some_and(|c| c.add_free_camera) { app.add_plugins(FreeCameraPlugin); }`. **Note**: reading `Res<AppConfig>` inside `Plugin::build` requires the resource be inserted before `add_plugins(CameraPlugin)` — F1's plan does this (insert resources at the top of `build_app_with_args`).

**Behavioural delta:**

- **No behaviour change**: `sync_position_split` runs every frame in both configs; `update_camera_history` runs `.after(sync_position_split)`; `toggle_dlss` + `apply_initial_camera_pose_changes` run only in production (today's `cfg.add_free_camera` true case).
- **Ordering preserved**: today's `(toggle_dlss, apply_initial_camera_pose_changes, sync_position_split).chain()` becomes `(toggle_dlss, apply_initial_camera_pose_changes).before(sync_position_split)` — semantically identical for these three (toggle_dlss + apply_pose can run in any order between themselves; both must precede sync). If a strict ordering between `toggle_dlss` and `apply_initial_camera_pose_changes` is needed, add `.chain()` to that pair — empirically there is no such constraint (toggle_dlss reads `KeyCode::KeyD`, apply_pose reads `InitialCameraPose`; they touch disjoint state).

---

#### Finding F9: press-P diagnostics dump — *no refactor*

**Current shape (verified):** `diagnostics.rs:40-143`, 103-LOC `dump_diagnostics_on_p` system body, mixes `Query`/`Res` data access with `writeln!`/`push_str` formatting.

**Target shape:** **unchanged.**

**Reuse choices:** N/A — explicit no-op.

**Behavioural delta:** none — explicit no-op.

**Rationale**: The explorer's F9 "Suggested direction" itself flagged "Or — judgment call — leave it. 103 LOC of straight-line dump code is fine for a single-purpose debug-key handler; the abstraction adds indirection for no real readability win." Adopt that branch. The function is read once when a developer presses P; the formatting tangle is not a tax anyone pays at edit time except when adding a field, and the cost of "find the writeln + add another" is lower than the cost of "decode the `Display`/`Snapshot` indirection."

---

#### Finding F10: `lib.rs` public-surface tidy

**Current shape (verified):** `crates/bevy_naadf/src/lib.rs:13-25` declares 13 `pub mod`s; lib root carries `pub struct AppArgs` + `pub struct AppConfig` + `pub struct WindowConfig` + `pub struct GiSettings` + `pub struct DevFont` + `pub enum GridPreset` + 4 `pub const`s + `pub fn build_app*` + `pub fn run_e2e_render*`.

**Target shape:**

```rust
// crates/bevy_naadf/src/lib.rs — final shape, ~500 LOC.

//! bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer (library surface).

pub mod aadf;
pub mod app_args;          // NEW — AppArgs lives here
pub mod app_config;        // NEW — AppConfig lives here (split from lib.rs)
pub mod app_mode;
pub mod baked_material;
pub mod camera;            // CameraPlugin, InitialCameraPose, setup_camera, …
pub mod dev_font;          // NEW — DevFontPlugin, DevFont, load_dev_font
pub mod diagnostics;       // DiagnosticsPlugin (press-P only)
pub mod e2e;
pub mod editor;
pub mod hud;
pub mod render;
pub mod settings;          // contains canonical::GiSettings + KNOBS (D2)
pub mod texture_array;
pub mod voxel;             // grid::GridPreset re-exported below
pub mod window_config;     // NEW — WindowConfig + window_for_e2e_args
pub mod world;
pub mod world_size;        // NEW — WORLD_SIZE_IN_* + WORLD_GEN_SEGMENT_SIZE_IN_GROUPS

// Source-stability re-exports — keep old `crate::X` paths working.
pub use app_args::AppArgs;
pub use app_config::AppConfig;
pub use dev_font::DevFont;
pub use settings::canonical::GiSettings;
pub use voxel::grid::GridPreset;
pub use window_config::WindowConfig;
pub use world_size::{
    WORLD_GEN_SEGMENT_SIZE_IN_GROUPS, WORLD_SIZE_IN_CHUNKS,
    WORLD_SIZE_IN_SEGMENTS, WORLD_SIZE_IN_VOXELS,
};

pub const DEFAULT_TAA_RING_DEPTH: u32 = 32;

pub fn build_app(cfg: AppConfig) -> App { /* … as in F1 … */ }
pub fn build_app_with_args(cfg: AppConfig, args: AppArgs) -> App { /* … */ }
pub fn run_e2e_render() -> AppExit { e2e::run_e2e_render() }
pub fn run_e2e_render_with_args(args: AppArgs) -> AppExit { /* … */ }
```

**Reuse choices:**

- **`pub use` re-exports preserve every existing `crate::X` import path** — the ~20 sites that today say `use crate::AppArgs;`, `use crate::GiSettings;`, `use crate::WORLD_SIZE_IN_*;`, etc. continue to compile without edit. The new module homes are reachable via `crate::app_args::AppArgs` for callers that want to be explicit.
- **`GridPreset` re-exports from `voxel::grid`** because D3's architect places it inside `VoxelIoPlugin`'s territory (D3 Finding 4 — `GridPreset` as the source-of-truth for install-path dispatch).
- **`DEFAULT_TAA_RING_DEPTH` stays in `lib.rs`** — it's a single `u32`, used by both `AppArgs::default` (which lives in `app_args.rs` after the move — re-export OK) and `render::pipelines` (shader-def source). Easiest to keep at lib root.

**Behavioural delta:**

- **No behaviour change.** Pure structural reorg.

---

### 3. Migration steps

Each step is atomic (`cargo build --workspace` + `cargo test --workspace --lib` + named e2e gates green afterward). D7 lands LAST so steps assume D1–D6, D8 implementors have shipped their domains' architecture proposals. Cross-domain calls that block D7 are flagged.

#### Step 1 — Delete `device_snapshot` chain (D7 production side only)

**Edits:**

- `crates/bevy_naadf/src/diagnostics.rs:155-711` — delete the entire `device_snapshot` submodule (557 LOC).
- `crates/bevy_naadf/src/diagnostics.rs:1-24` — collapse module docstring to the press-P-only half (~6 LOC).
- `crates/bevy_naadf/src/lib.rs:792-799` — delete `DeviceSnapshotPlugin` registration line + the 7-line comment block above it (8 LOC total).
- `crates/bevy_naadf/Cargo.toml` — if `serde`/`serde_json` were pulled in only by `device_snapshot`, drop them. Audit `grep -rn "serde::Serialize\|serde_json" crates/bevy_naadf/src/` post-delete; keep if anyone else consumes. **Investigation at impl time.** PBR `AppArgs` fields: **verify there are no `pbr_*` fields** in `AppArgs` (per F4 finding). Per current Read of `lib.rs:283-462` there are zero — Step 1's PBR-field cleanup is a no-op.

**Rationale**: Resolution A (addendum 2026-05-20). Lands first because it's pure deletion (no callers in D7's domain post-delete — D6 has already landed the `bin/e2e_render.rs` + spec deletions per the implementor sequence).

**Post-step state**: `diagnostics.rs` is ~148 LOC; `lib.rs` is ~1 130 LOC. `bin/e2e_render.rs --device-snapshot-native` is gone (D6 work). No `serde_json` dep if D8's audit confirms.

**Verification**: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo run --bin e2e_render -- baseline`, `cargo run --bin e2e_render -- --validate-gpu-construction`, `cargo run --bin e2e_render -- --oasis-edit-visual` (any of these gates would fail-to-link if a stray reference to `device_snapshot::*` survived).

**Coordination**: this Step depends on D6's deletions already being in tree. If D6's implementor hasn't landed `bin/e2e_render.rs:137-143,364-375` deletion + `Cargo.toml [[bin]] diag_compare` removal, **D7's implementor stops at Step 1 and dispatches a coordination request to the orchestrator.** Per Q3 sequencing (D7 last), this should not occur.

---

#### Step 2 — Extract SSoT-1 + SSoT-2 to new modules

**Edits:**

- `crates/bevy_naadf/src/world_size.rs` — NEW FILE per §2 F5. Contains `WORLD_SIZE_IN_SEGMENTS`, `WORLD_GEN_SEGMENT_SIZE_IN_GROUPS`, `mul_uvec3`, `WORLD_SIZE_IN_CHUNKS`, `WORLD_SIZE_IN_VOXELS`, `#[cfg(test)] world_size_matches_csharp`.
- `crates/bevy_naadf/src/lib.rs:241-260` — delete the 4 `pub const`s + their docstrings (move to `world_size.rs`).
- `crates/bevy_naadf/src/lib.rs:1131-1145` — delete `fixed_world_size_constants_agree` (replaced by the smaller test in `world_size.rs`).
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub mod world_size;` + `pub use world_size::{WORLD_SIZE_IN_*, WORLD_GEN_SEGMENT_SIZE_IN_GROUPS};`.
- `crates/bevy_naadf/src/settings/mod.rs` — NEW DIR (assumes D2 already lands `settings/` directory split; D2 architect's HIGH-3.q). If not, create it: rename `settings.rs` → `settings/mod.rs` (no body change). **If D2 lands `settings/mod.rs` shape first, D7 only adds the `canonical` submodule.**
- `crates/bevy_naadf/src/settings/canonical.rs` — NEW FILE per §2 F2. Contains `pub struct GiSettings { … }`, `impl GiSettings { pub const DEFAULTS: GiSettings = …; }`, `impl Default for GiSettings`.
- `crates/bevy_naadf/src/lib.rs:108-231` — delete `GiSettings` struct + `Default` impl (move to `settings/canonical.rs`).
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub use settings::canonical::GiSettings;` (the `pub mod settings;` declaration is already there).

**Rationale**: SSoT-1 + SSoT-2 closure. D4's architect proposes the GPU-side consumer (`GpuRenderParams`/`GpuGiParams` read from `GiSettings::DEFAULTS` or `AppArgs.gi.*`); D2's architect proposes the KNOBS-side consumer. D7 lands the canonical move.

**Post-step state**: `lib.rs` is ~990 LOC (down ~140 from world_size + GiSettings move). `settings/canonical.rs` is ~120 LOC. `world_size.rs` is ~60 LOC. All `use crate::{GiSettings, WORLD_SIZE_*}` imports across the codebase continue to compile (re-exports).

**Verification**: same gates as Step 1. Plus `cargo test --workspace --lib` (the `world_size_matches_csharp` test must pass).

---

#### Step 3 — Move `WindowConfig` + add `window_for_e2e_args`

**Edits:**

- `crates/bevy_naadf/src/window_config.rs` — NEW FILE per §2 F3. Contains `pub struct WindowConfig` + 5 named constructors (`windowed`, `e2e`, `e2e_horizon`, `e2e_resize_test`, `e2e_small_edit_repro`) + `pub fn window_for_e2e_args(args: &AppArgs) -> WindowConfig`.
- `crates/bevy_naadf/src/lib.rs:465-568` — delete `WindowConfig` struct + 4 constructors (move to `window_config.rs`). The new 5th constructor absorbs the inline literal from `lib.rs:1006-1015`.
- `crates/bevy_naadf/src/lib.rs:993-1025` — `run_e2e_render_with_args` collapses to 4 LOC per §2 F3.
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub mod window_config;` + `pub use window_config::WindowConfig;`.

**Rationale**: F3. Mechanically extractable; no behaviour change.

**Post-step state**: `lib.rs` ~890 LOC. `window_config.rs` ~80 LOC.

**Verification**: gates as Step 1, plus specifically `cargo run --bin e2e_render -- --resize-test`, `cargo run --bin e2e_render -- --vox-horizon-native`, `cargo run --bin e2e_render -- --small-edit-repro <path>` — these exercise the three e2e-only window configs.

---

#### Step 4 — Extract `DevFontPlugin`

**Edits:**

- `crates/bevy_naadf/src/dev_font.rs` — NEW FILE, ~30 LOC. Contains `ROBOTO_REGULAR_BYTES`, `pub struct DevFont(pub FontSource)`, `pub fn load_dev_font(…)`, `pub struct DevFontPlugin` with `Plugin::build` registering `load_dev_font` in `Startup`.
- `crates/bevy_naadf/src/lib.rs:27-39` — delete `ROBOTO_REGULAR_BYTES` static + `DevFont` struct (move).
- `crates/bevy_naadf/src/lib.rs:1027-1036` — delete `load_dev_font` fn (move).
- `crates/bevy_naadf/src/lib.rs:803` — delete `app.add_systems(Startup, load_dev_font);` (subsumed by `DevFontPlugin`).
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub mod dev_font;` + `pub use dev_font::DevFont;`.

**Rationale**: F1 piece — small, self-contained, doesn't touch other domains.

**Post-step state**: `lib.rs` ~870 LOC. `dev_font.rs` ~30 LOC. `crate::DevFont` import paths still work.

**Verification**: gates as Step 1. **HUD must still render text** — `cargo run --bin e2e_render -- baseline` produces a framebuffer with the HUD (e2e config has `add_hud=false`, so this gate exercises the no-HUD path; need a manual visual check for HUD-on or a dedicated gate). The press-P diagnostics and FPS HUD both query `DevFont` — if either is silent (no font), the bug surfaces in the next dev run. **Architect-flag**: add a unit test that boots a minimal app with `DevFontPlugin` and asserts `Res<DevFont>` is present after one `Startup` tick.

---

#### Step 5 — Extract `CameraPlugin`

**Edits:**

- `crates/bevy_naadf/src/camera/mod.rs` — add `pub struct CameraPlugin;` + `impl Plugin for CameraPlugin { build }` per §2 F8 target shape.
- `crates/bevy_naadf/src/camera/mod.rs:179-213` — add named consts (per §2 F6) at the head; `apply_initial_camera_pose_changes` body updated to reference them.
- `crates/bevy_naadf/src/lib.rs:770-784` — delete the `if cfg.add_free_camera { … } else { … }` branch (subsumed by `CameraPlugin`).
- `crates/bevy_naadf/src/lib.rs:886-898` — delete the inline `setup_camera.after(setup_test_grid)` + `update_camera_history.after(sync_position_split)` registrations.
- `crates/bevy_naadf/src/lib.rs:737-764` — add `camera::CameraPlugin` to the plugin tuple (the e2e branch + the production branch both get it; the `add_free_camera` distinction is internal to `CameraPlugin`).

**Rationale**: F1 + F8 + F6. `CameraPlugin` owns its systems; the central registry shrinks by ~40 LOC.

**Post-step state**: `lib.rs` ~830 LOC. `camera/mod.rs` ~330 LOC (was 287 + CameraPlugin's ~40 LOC). `crate::camera::setup_camera` import path still works.

**Verification**: gates as Step 1, plus specifically `cargo run --bin e2e_render -- --oasis-edit-visual` (exercises the production setup_camera + InitialCameraPose path) and `cargo run --bin e2e_render -- --vox-horizon-native` (exercises the apply_initial_camera_pose_changes drift logic on a loaded vox). **The drift threshold consts must not change the behavioural floor** — `cargo run --bin e2e_render -- --vox-horizon-native` reproduces a deterministic camera-on-spawn, so any threshold-induced drift will surface as an SSIM diff.

---

#### Step 6 — Move `AppArgs` to `app_args.rs`

**Edits:**

- `crates/bevy_naadf/src/app_args.rs` — NEW FILE, ~250 LOC. Contains `pub struct AppArgs` + `Default for AppArgs`. Doc comments copied verbatim (the 11 e2e flag doc-comments are load-bearing). Fix the `taa` field docstring per §2 F4 ("always `false` in Phase A" obsolete → drop the Phase A framing).
- `crates/bevy_naadf/src/lib.rs:283-462` — delete `AppArgs` struct + `Default` impl (move).
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub mod app_args;` + `pub use app_args::AppArgs;`.

**Rationale**: F1 + F4 + F10. `AppArgs` is the biggest single struct in `lib.rs`; moving it is the LOC win.

**Post-step state**: `lib.rs` ~650 LOC. `app_args.rs` ~250 LOC. All `use crate::AppArgs;` imports still work.

**Verification**: gates as Step 1. No behaviour delta; only file home moves.

---

#### Step 7 — Wire D2's `EditorUiPlugins` + D3's `VoxelIoPlugin`

**Edits:**

- `crates/bevy_naadf/src/lib.rs:874-876` — delete the `if args.spawn_test_entity { app.add_systems(Startup, spawn_phase_c_test_entity.after(voxel::grid::setup_test_grid)); }` (per D5's architecture, `spawn_phase_c_test_entity` moves into `render/construction/` with its own self-gating — D5 already lands this).
- `crates/bevy_naadf/src/lib.rs:1038-1095` — delete `spawn_phase_c_test_entity` (D5 owns its new home).
- `crates/bevy_naadf/src/lib.rs:803-864` — delete the inline `setup_test_grid`, `PendingVoxParse` init + `poll_pending_vox_parse`, wasm `web_vox` registrations, native dnd registrations (replaced by `app.add_plugins(voxel::VoxelIoPlugin);` — assumes D3 architect's F4 voxel-io plugin landed).
- `crates/bevy_naadf/src/lib.rs:900-971` — delete the entire `if cfg.add_hud { … }` block (replaced by `if cfg.add_hud { app.add_plugins(editor::EditorUiPlugins); }` — assumes D2 architect's HIGH-3 Plugin extraction landed).
- `crates/bevy_naadf/src/lib.rs:877-879` — `if cfg.add_e2e_systems { e2e::add_e2e_systems(&mut app); }` becomes `if cfg.add_e2e_systems { app.add_plugins(e2e::E2ePlugin); }` (assumes D6 architect's Finding 6 PluginGroup landed). If D6 keeps `add_e2e_systems`, leave this line.

**Rationale**: F1 core. Once D2 + D3 + D5 + D6 ship their plugins, D7 deletes 200+ LOC from `lib.rs` and replaces them with `add_plugins` calls.

**Post-step state**: `lib.rs` ~500 LOC.

**Verification**: gates as Step 1. **Plus full e2e battery**: `--baseline`, `--validate-gpu-construction`, `--edit-mode`, `--entities`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`. **Manual visual check from user** for HUD-on production behaviour (this Step touches HUD + Editor + Settings registration).

**Coordination**: this is the most heavily-coupled step. If D2/D3/D5/D6 implementors haven't landed their plugins, **D7's implementor pauses at Step 6's completion and dispatches a coordination request.** Per Q3 sequencing, this should not occur.

---

#### Step 8 — `AppArgs` → `enum E2eMode` split (DEFERRED; cross-domain with D6)

**Edits:** (proposed shape, not for this orchestration's impl unless user explicitly approves)

- `crates/bevy_naadf/src/app_args.rs` — split into:
  ```rust
  pub struct AppArgs {
      pub grid_preset: GridPreset,
      pub taa: bool,
      pub taa_ring_depth: u32,
      pub gi: GiSettings,
      pub construction_config: ConstructionConfig,
      pub spawn_test_entity: bool,
      pub e2e_mode: Option<E2eMode>,
  }

  pub enum E2eMode {
      ResizeTest,
      VoxE2e,
      OasisEditVisual,
      SmallEditVisual,
      SmallEditRepro,
      VoxGpuConstruction,
      VoxGpuOracleCpu,
      VoxGpuOracleGpu,
      VoxWebParitySkybox,
      VoxWebParityLoaded,
      VoxHorizonNative,
  }
  ```
- `crates/bevy_naadf/src/bin/e2e_render.rs:71-208` — D6 territory. Dispatch ladder becomes `match parse_e2e_mode(args) { Some(mode) => run_e2e_render_with_args(AppArgs { e2e_mode: Some(mode), .. AppArgs::default() }), None => … }`.
- `crates/bevy_naadf/src/e2e/<11 mode files>.rs` — each reads `args.e2e_mode == Some(E2eMode::OasisEditVisual)` instead of `args.oasis_edit_visual_mode`.

**Rationale**: F4. The 11 mutually-exclusive flags are an enum-in-disguise. Type-level honesty + collapses `bin/e2e_render.rs`'s 18-flag ladder.

**Post-step state**: `app_args.rs` ~120 LOC (defers 11 flag's worth of docstrings to the enum). `bin/e2e_render.rs` dispatch is shorter.

**Verification**: full e2e battery + all 11 mode flags. High-risk: every `e2e/<mode>.rs` consumer needs to read off the enum instead of off the boolean.

**Coordination**: D6+D7 paired implementor session, OR user defers the split to a follow-on /refactor. **Recommendation**: defer. The boolean-fields shape is structurally suboptimal but functionally correct; the enum migration is a large coordinated edit that risks more than it gains in this orchestration. Land Steps 1–7 + Step 9; revisit Step 8 in a focused follow-up.

---

#### Step 9 — `AppConfig` extraction + final lib.rs polish

**Edits:**

- `crates/bevy_naadf/src/app_config.rs` — NEW FILE, ~80 LOC. Contains `pub struct AppConfig { … }` + `impl AppConfig { pub fn windowed() / pub fn e2e() }`.
- `crates/bevy_naadf/src/lib.rs:570-619` — delete `AppConfig` struct + impl (move).
- `crates/bevy_naadf/src/lib.rs:13-25` — add `pub mod app_config;` + `pub use app_config::AppConfig;`.
- `crates/bevy_naadf/src/lib.rs` — audit `pub` surface per F10. Trim what's no longer used externally.

**Rationale**: F10. Polish step. `AppConfig` is small (~50 LOC) but moving it tidies the lib root.

**Post-step state**: `lib.rs` ≤ 500 LOC; mostly `build_app_with_args` + `default_plugins_for` + `run_e2e_render*` + module declarations + re-exports.

**Verification**: gates as Step 1.

---

### 4. What stays / what changes / what's removed

**Stays unchanged in D7 paths:**

- `crates/bevy_naadf/src/main.rs` — already a 54-LOC shim; nothing to do.
- `crates/bevy_naadf/src/camera/position_split.rs` — `PositionSplit` type + `sync_position_split` system + tests; the int+frac type is the C# faithful port and is solid.
- `crates/bevy_naadf/src/camera/mod.rs` `setup_camera`, `toggle_dlss`, `default_free_camera`, `InitialCameraPose::from_world_voxels` + tests — all behaviour preserved; only the wiring is plugin-ised.
- `crates/bevy_naadf/src/diagnostics.rs` press-P body (`dump_diagnostics_on_p`, lines 40-143) — explicit no-op per F9.
- `crates/bevy_naadf/src/app_mode.rs` — D2's territory per audit (lives in editor-and-settings-ui domain LOC table). D7 leaves it alone except its `Plugin` registration in `lib.rs` migrates to D2's `EditorUiPlugins` group.

**Changes (D7's edit list):**

- `crates/bevy_naadf/src/lib.rs` — drops from 1 146 to ~500 LOC. Becomes the spine: module decls + re-exports + `build_app*` + `run_e2e_render*` + `default_plugins_for`.
- `crates/bevy_naadf/src/diagnostics.rs` — drops from 711 to ~148 LOC. `DiagnosticsPlugin` gains `.run_if(|cfg: Res<AppConfig>| !cfg.add_e2e_systems)`.
- `crates/bevy_naadf/src/camera/mod.rs` — gains `CameraPlugin`; `apply_initial_camera_pose_changes` gains named consts.

**Adds (NEW files):**

- `crates/bevy_naadf/src/world_size.rs` — `WORLD_SIZE_IN_*` derived via `const fn mul_uvec3`.
- `crates/bevy_naadf/src/window_config.rs` — `WindowConfig` + 5 constructors + `window_for_e2e_args`.
- `crates/bevy_naadf/src/dev_font.rs` — `DevFont` + `DevFontPlugin` + `ROBOTO_REGULAR_BYTES`.
- `crates/bevy_naadf/src/app_args.rs` — `AppArgs` + `Default`.
- `crates/bevy_naadf/src/app_config.rs` — `AppConfig` + `windowed/e2e`.
- `crates/bevy_naadf/src/settings/canonical.rs` — `GiSettings` + `pub const DEFAULTS`. (Inside D2's `settings/` dir; co-owned but D7 lands the file.)

**Removes:**

- `crates/bevy_naadf/src/diagnostics.rs:155-711` — `device_snapshot` submodule (~557 LOC). Callers: deleted by D6 (bin/e2e_render dispatch + bin/diag_compare + Playwright spec + justfile recipes).
- `crates/bevy_naadf/src/lib.rs:792-799` — `DeviceSnapshotPlugin` registration + comment block.
- `crates/bevy_naadf/src/lib.rs:1038-1095` — `spawn_phase_c_test_entity` moves to `render/construction/` (D5's new home).
- `crates/bevy_naadf/src/lib.rs:1131-1145` — `fixed_world_size_constants_agree` test (replaced by smaller test in `world_size.rs`).
- `crates/bevy_naadf/src/lib.rs:108-231` — `GiSettings` struct (moves to `settings/canonical.rs`).
- `crates/bevy_naadf/src/lib.rs:241-260` — `WORLD_SIZE_IN_*` consts (move to `world_size.rs`).
- `crates/bevy_naadf/src/lib.rs:283-462` — `AppArgs` (moves to `app_args.rs`).
- `crates/bevy_naadf/src/lib.rs:465-568` — `WindowConfig` (moves to `window_config.rs`).
- `crates/bevy_naadf/src/lib.rs:570-619` — `AppConfig` (moves to `app_config.rs`).
- `crates/bevy_naadf/src/lib.rs:27-39, 1027-1036` — `ROBOTO_REGULAR_BYTES` + `DevFont` + `load_dev_font` (move to `dev_font.rs`).
- `crates/bevy_naadf/src/lib.rs:770-784` — conditional `sync_position_split` double-register (`CameraPlugin` absorbs).
- `crates/bevy_naadf/src/lib.rs:803-864` — inline voxel-io system registrations (D3's `VoxelIoPlugin` absorbs).
- `crates/bevy_naadf/src/lib.rs:874-876` — `spawn_phase_c_test_entity` registration (D5 absorbs).
- `crates/bevy_naadf/src/lib.rs:900-971` — entire `if cfg.add_hud { … }` block (D2's `EditorUiPlugins` absorbs).

---

### 5. Open conflicts

**C1 — D6 may want to keep some `device_snapshot` console reads in `vox_horizon_parity.spec.ts`.**

D6 Finding 7 notes that `vox-horizon-parity.spec.ts:122,147,158,187` references the `[device-snapshot]` console sentinel for diagnostic output only — "not load-bearing." After D7's deletion of the `device_snapshot` submodule, the sentinel will never be emitted, so these reads will silently produce empty/missing data. **No conflict for D7's deletions** (D7 still deletes the submodule per Resolution A), but D6's architect should either (a) delete the sentinel reads as dead, or (b) accept silent no-ops. **Flag for orchestrator to forward to D6's architect.**

**C2 — `spawn_phase_c_test_entity` relocation depends on D5 landing first.**

D5's architect must have moved `MainWorldEntities` ownership inside `render/construction/` (it already lives there per `crates/bevy_naadf/src/lib.rs:1053`) and provided a self-gating mechanism for spawning the fixture entity (today's `if args.spawn_test_entity { add_systems(Startup, spawn_phase_c_test_entity) }`). D5's architect must propose: either (a) `ConstructionPlugin` reads `Res<AppArgs>.spawn_test_entity` internally and adds the system with `.run_if(|args| args.spawn_test_entity)`, or (b) a new `ConstructionFixturePlugin` that's added conditionally. **D7's Step 7 assumes one of these landed.**

**C3 — D2's `settings/` directory split.**

D2's HIGH-3.q poses the open question of whether `settings.rs` becomes `settings/mod.rs`. D7's Step 2 lands `settings/canonical.rs` — which requires `settings/` to be a directory. If D2 lands first with a `settings/` dir, D7 only adds the submodule. If D2 keeps `settings.rs` as a single file, D7 creates `settings/` (renaming `settings.rs` → `settings/mod.rs` first, no body change). **Either path works**; D7's Step 2 includes both branches in its edit list.

**C4 — `update_camera_history` lives in `render/taa.rs` (D4 territory).**

`CameraPlugin::build` references `render::taa::update_camera_history`. The function definition stays in `render/taa.rs` — D4 architect doesn't have to move it. D7 only adds a system registration referencing it. **No conflict**, flagged for clarity.

**C5 — D6's `E2ePlugin` shape.**

D6's Finding 6 proposes "Plugin-per-gate" — but the call site D7 needs is "one thing I add". D6 architect should land either (a) a `PluginGroup` umbrella `E2ePlugin` that gathers per-gate plugins (D7 prefers this — matches the F1 sketch's `app.add_plugins(e2e::E2ePlugin)`), OR (b) keep `pub fn add_e2e_systems(&mut App)` as a coordinator that internally `add_plugins` per-gate (D7 leaves Step 7's call as `if cfg.add_e2e_systems { e2e::add_e2e_systems(&mut app); }`). **Both work.** D6 architect picks.

**C6 — `default_plugins_for(&cfg)` placement.**

The helper that wraps `DefaultPlugins.set(...).set(...).set(...)` is currently inline at `lib.rs:691-735`. Target shape per F1 puts it as a free fn in `lib.rs`. **No conflict**; flagged for transparency. If the LogPlugin custom layer (`e2e::tracing_error_counter::vox_web_parity_log_layer`) is the only e2e-specific knob, it stays inline; otherwise the helper signature might end up bigger than worthwhile.

---

### 6. Cross-domain assumptions

This section enumerates what D7 assumes other architects have landed by the time D7's implementor runs. If any of these don't materialise, D7's Step 7 is the affected step and the implementor escalates.

- **D2 lands `editor::EditorUiPlugins` (or equivalent PluginGroup)** that bundles `HudPlugin` + `AppModePlugin` + `EditorPlugin` + `SettingsPlugin`. D7's Step 7 calls `if cfg.add_hud { app.add_plugins(editor::EditorUiPlugins); }`. D2's architect's HIGH-3 Side-note 11 acknowledges this is their move.
- **D3 lands `voxel::VoxelIoPlugin`** that owns `setup_test_grid` + `PendingVoxParse` init + `poll_pending_vox_parse` + wasm `web_vox::startup_fetch_default_vox` + wasm `apply_pending_vox` + wasm `pin_web_horizon_camera` + native dnd. D7's Step 7 calls `app.add_plugins(voxel::VoxelIoPlugin);`. D3's architect F4 proposes this.
- **D3 moves `HORIZON_CAMERA_POS`/`HORIZON_CAMERA_ROT` out of `e2e/vox_horizon_parity.rs`** (D3 F6). D7 doesn't directly consume them but the dependency-arrow inversion is required for `pin_web_horizon_camera` to not import from `e2e/`. Confirmed in 01-context addendum (D3 F6 "Approved").
- **D4 leaves `render::taa::update_camera_history` in place** (function definition stays; D7 only adds a system-registration edge). D4 architect's response to C4 should be a no-op acknowledgement.
- **D4 produces `GpuRenderParams`/`GpuGiParams` from `&AppArgs`** in a way that reads `args.gi` field-by-field (not `GiSettings::DEFAULTS`). The relationship between `GiSettings` and the GPU mirror is "uniform fields, runtime upload" per `01-context.md ¶453` — D4 handles the conversion shape.
- **D5 absorbs `spawn_phase_c_test_entity`** into `render/construction/` (C2).
- **D6 lands an `E2ePlugin` / `add_e2e_systems` shape D7 can call** (C5).
- **D6 has already deleted `bin/e2e_render.rs --device-snapshot-native` + `bin/diag_compare.rs` + `e2e/tests/device-snapshot.spec.ts`** before D7's Step 1 runs (per implementor sequence: D6 before D7).

---

### Decisions & rejected alternatives

| Decision | Alternative considered | Rationale |
|---|---|---|
| Move `GiSettings` to `settings/canonical.rs` | Move to new top-level `gi_settings.rs` | D2's `KNOBS` table is the heaviest consumer; co-locating with the table reduces cross-module noise. D2 HIGH-3 already proposes a `settings/` dir. |
| Keep `AppArgs` flat (defer enum split) | Split into `AppArgs` + `enum E2eMode` in this orchestration | Step 8 enum split crosses 12+ files (11 e2e modules + bin/e2e_render). High-coordination edit; better as a follow-up. Today's flat shape is structurally suboptimal but functionally fine. |
| Press-P dump body unchanged | Extract `DiagnosticsDumpSnapshot` + `impl Display` | Single-purpose debug handler; abstraction adds indirection. Explorer's F9 itself flagged the leave-it branch. |
| `WindowConfig` in own module + `window_for_e2e_args` fn | Push per-mode `WindowConfig` constants into each `e2e/<mode>.rs` | Module-bundled fn captures the mapping in one legible place. Per-mode push fragments the lookup. |
| `const GiSettings::DEFAULTS` over `Reflect`-driven defaults | Derive `Reflect` on `GiSettings` and expose defaults via reflection | Reflect is D2 architect's BEV-4 work. D7's role is to provide the `const` SSoT; D2 layers Reflect on top. |
| Plugin per subsystem (Bevy idiom) | Function-pointer registry | Established pattern: 7 plugins already extracted. Continuing the idiom. |
| `apply_initial_camera_pose_changes` keeps `Local` + named thresholds | Replace with `Changed<FreeCamera>` event chain | Third-party `FreeCameraPlugin` ownership; threshold approach is simpler and empirically correct. |
| `CameraPlugin` adds `FreeCameraPlugin` internally when `cfg.add_free_camera` | Always add `FreeCameraPlugin`, gate its systems with `.run_if` | `FreeCameraPlugin` owns resources we don't want under e2e; conditional add saves memory + avoids harness interaction. |

---

### Assumptions made

- **Bevy 0.19's `.run_if(|cfg: Res<X>| …)` closure-condition syntax is in use elsewhere in the codebase.** Verified: `crates/bevy_naadf/src/lib.rs:953-957` uses `.run_if(in_state(AppMode::Playing))` and `.run_if(in_state(AppMode::Settings))`. The closure form is also Bevy 0.19 stable.
- **`PluginGroup` is the right vehicle for the D2 / D6 plugin bundles.** Bevy 0.19's `bevy::app::PluginGroup` is the documented "bundle of related plugins" trait. If D2 / D6 instead lands single `Plugin`s that internally `add_plugins`, that also works; D7's call site adapts (one-line change).
- **No Cargo deps need to be added or removed by D7's work in Step 1-7.** Step 1's `serde_json` removal is best-effort (audit at impl time).
- **`const GiSettings { … }` literal syntax works at the const item position.** Verified: all fields are `u32`/`f32`/`bool` (primitive). Rust allows this.

---

## Side notes / observations / complaints

1. **D7's job is mechanical extraction more than design.** The codebase is already well-structured under the surface; every system has a clear owner, every resource has a clear scope. The 1 146-LOC `lib.rs` is the staging area where each phase landed a new chunk that should have been extracted at the time. Re-confirming the explorer's Side note 10.

2. **The biggest D7 risk is Step 7's cross-domain dependency on D2/D3/D5/D6 plugins.** If any of those architects' proposals don't land in implementor form before D7 runs, D7 either stalls or has to fall back to "leave the inline registrations in place." This is exactly the Chesterton's-fence problem flagged in the explorer's Side note 9. **Mitigation**: orchestrator should verify D2/D3/D5/D6 implementor commits are on `main` before dispatching D7's implementor.

3. **The `AppConfig::e2e()` IS still consumed by the e2e harness** (per `feedback-e2e-must-drive-actual-main`). All Step 7 wiring preserves this — `e2e_render` boots `build_app_with_args(AppConfig::e2e(), args)` exactly as today; only the plugin set under that `App` changes.

4. **`AppArgs::default().taa = true` despite docstring "Phase A: always false"** is genuine doc-rot from the orchestration history (Phase A → A-2 → B turned TAA on). Step 6 fixes the docstring as a drive-by. Side-note 3 of the explorer.

5. **Subjective complaint**: the brief's "D7 last" sequencing is correct but uncomfortable. Every other architect is designing in parallel and may shift their proposal between when D7's architect writes and when D7's implementor runs. D7's design is structured around "the call sites D7 will write" — if D2's `EditorUiPlugins` ends up being `EditorPlugin + SettingsPlugin + HudPlugin + AppModePlugin` (4 separate adds), D7's Step 7 site changes from `add_plugins(EditorUiPlugins)` to `add_plugins((EditorPlugin, …))`. Mechanical, but it's a coordination tax. **Recommend orchestrator hold a sync window where all 8 architect docs are visible to each other before any implementor runs.**

6. **`spawn_phase_c_test_entity` reaches into `crate::e2e::gates::demo_origin_v()`** at `lib.rs:1078`. After Step 7 relocates the function to `render/construction/`, that import becomes `render::construction → e2e::gates::demo_origin_v`. The production crate would import from the e2e module — same dependency-arrow inversion D3 F6 flagged. **Architect-flag to D5 + D6**: ideally `demo_origin_v` moves out of `e2e/gates.rs` into a non-e2e module (`render/construction/test_fixture.rs` or similar) so the production-into-e2e import goes away. D5 architect picks the destination.

7. **`AppArgs` carries `construction_config: ConstructionConfig` (D5 type) as a field.** This is a D7→D5 type dependency at the lib root. After Step 6 moves `AppArgs` to `app_args.rs`, the import becomes `use crate::render::construction::ConstructionConfig;`. Verified `ConstructionConfig` is `Clone` (file `render/construction/config.rs:36`).

8. **`GridPreset` after Step 6 lives behind `crate::voxel::grid::GridPreset`.** D3's architect F4 may move it further (proposes extending the enum). D7 just re-exports — whichever shape D3 lands.

9. **The `AppConfig::e2e()` `add_e2e_systems` flag does double duty** (explorer Side note 5): gates LogPlugin custom layer (`lib.rs:701-708`), gates `DiagnosticsPlugin` registration (today, post-refactor it's a `.run_if`), gates native dnd, gates the e2e driver. After F1's decomposition, each consumer plugin self-checks `Res<AppConfig>`. The flag's coupling is internal to each plugin, not central.

10. **Equal-footing observation**: this codebase is genuinely well-structured under the bloat. The hard work is at D5 (the 11k mod.rs) and D2 (the reflect-from-scratch settings panel). D7's mechanical Plugin-extraction work is the easy half. The user's "tight idiomatic Bevy" goal lands most clearly in D5 + D2; D7's win is "smaller spine that's easier to read."

11. **Verification of every cited line/path**: every `crates/...:lines` reference in §2 + §3 was Read in this architect session. No fabricated line numbers.

12. **`AppMode` lives in `app_mode.rs` (D2 territory per 00-reuse-audit §2 D2 row).** The plugin that registers `AppMode` state + Escape toggle + suspend/restore camera input belongs to D2's `EditorUiPlugins` group, not D7. The explorer's F1 sketch placed `AppModePlugin` in D7's column; the audit's domain LOC table assigns it to D2. **Adopt the audit's assignment** — D2 architect owns `AppModePlugin`. D7 does not own it. Step 7's `if cfg.add_hud { app.add_plugins(editor::EditorUiPlugins); }` covers it.

13. **`hud.rs` (the FPS/timing overlay) lives at the lib root**, sibling to `editor::hud`. Audit assigns root `hud.rs` to D2; D2's architect's HIGH-2 confirms. D7 doesn't own `hud.rs` either; D2's `EditorUiPlugins` includes `HudPlugin` (or similar) and Step 7 adds it via the group.

14. **C6 + Side note 9 give the same answer twice**: `default_plugins_for` stays as an inline helper in `lib.rs` to avoid yet-another-file. If it grows beyond ~50 LOC across iterations, revisit.
