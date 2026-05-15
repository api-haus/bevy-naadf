# 22 — Impl: Comprehensive Raymarching-Quality Panel

**Date:** 2026-05-15
**Branch:** `main` (HEAD pre-dispatch `1c35c7f`, Dispatch A)
**Predecessor:** `21-design-quality-panel.md` (design + self-review).

---

## §1. Files touched

| Path | Range | Change |
|---|---|---|
| `crates/bevy_naadf/src/lib.rs` | ~13-21, ~73-145, ~88-122 | New `pub mod panel;` line. `GiSettings` += 6 fields (max_ray_steps_primary/secondary/sun/sun_secondary/visibility + spatial_iter_count) + their defaults. Panel plugin wiring under `cfg.add_hud` gate. |
| `crates/bevy_naadf/src/hud.rs` | ~222 | One-line `[F1] quality panel` keybind hint appended. |
| `crates/bevy_naadf/src/panel.rs` | NEW | The whole panel module (594 lines incl. doc / tests). |
| `crates/bevy_naadf/src/render/gpu_types.rs` | ~77-95 | `GpuRenderParams._pad0a` → `max_ray_steps_primary` (layout-preserving rename). |
| `crates/bevy_naadf/src/render/gpu_types.rs` | ~504-530 | `GpuGiParams` += 5 new u32 fields (`max_ray_steps_secondary` / `_sun` / `_sun_secondary` / `_visibility` / `spatial_iter_count`) + 3 trailing pads. |
| `crates/bevy_naadf/src/render/gpu_types.rs` | ~815, ~826-832 | Size assert bumped 304 → 336; 2 new `offset_of!` guards (rows 304 + 320). |
| `crates/bevy_naadf/src/render/prepare.rs` | ~42-44, ~456-460, ~559-565 | Add `ExtractedGiConfig` to imports; add `extracted_gi: Res<ExtractedGiConfig>` to `prepare_frame_gpu` args; write `max_ray_steps_primary` into `GpuRenderParams` at the upload site. |
| `crates/bevy_naadf/src/render/gi.rs` | ~370-382 | Write the 5 new fields + 3 pads into `GpuGiParams` at the `prepare_gi` upload site. |
| `crates/bevy_naadf/src/assets/shaders/gi_params.wgsl` | ~5-12, ~128-141 | Header byte-count comment updated 304 → 336. 8 new top-level u32 fields appended (5 knobs + 3 pads). |
| `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl` | ~139-168 | `pad0a` → `max_ray_steps_primary`. Header comment updated. |
| `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl` | ~121-138 | Header comment on `MAX_RAY_STEPS_*` consts retitled "documentation-only" — the consts stay (naga DCEs them) so the canonical values have one source location, but every consumer now reads from `gi_params.*` / `params.max_ray_steps_primary`. |
| `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` | ~57-59, ~178-183 | Drop `MAX_RAY_STEPS_PRIMARY` from the import; replace the const at the `shoot_ray` call with `i32(max(params.max_ray_steps_primary, 1u))`. |
| `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` | ~46-48, ~289-294, ~369-381 | Drop `MAX_RAY_STEPS_SECONDARY` + `MAX_RAY_STEPS_SUN_SECONDARY` from imports; replace both `shoot_ray` constants with the corresponding `gi_params.*` reads. |
| `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` | ~57-59, ~454-461, ~554-561, ~621-633 | Drop `MAX_RAY_STEPS_VISIBILITY` + `MAX_RAY_STEPS_SUN` from imports; replace both `shoot_ray` constants with `gi_params.*` reads; replace `sample_neighbors(.., 12u, ..)` with `sample_neighbors(.., max(gi_params.spatial_iter_count, 1u), ..)`. |

No `Cargo.toml` edit — zero new dependencies (the design decision §3.4 to use Bevy native UI made this possible). InstaMAT + render plugins untouched.

---

## §2. New files created

| Path | Lines | Purpose |
|---|---|---|
| `crates/bevy_naadf/src/panel.rs` | 594 | The quality-panel module: `PanelState` resource, `PanelRoot`/`PanelText` markers, the `KNOBS` table (28 rows — 6 P, 11 C, 7 D-show, 4 section headers), `setup_panel` / `toggle_panel` / `adjust_panel` / `update_panel_text` systems, `step_cursor` navigation helper, 4 `#[test]` runtime guards. |
| `docs/orchestrate/naadf-bevy-port/21-design-quality-panel.md` | ~480 | Design + self-review (separate file — written before impl per the dispatch protocol). |
| `docs/orchestrate/naadf-bevy-port/22-impl-quality-panel.md` | (this file) | Implementation log. |

---

## §3. New `Cargo.toml` deps + versions

**None.** The design (`21-design-quality-panel.md` §3.4) chose Bevy 0.19's native `bevy_ui` over `bevy_egui` after verifying:
- `bevy_egui` 0.39.1 declares `bevy_app = 0.18.0` (no Bevy 0.19 release / no open PR);
- `bevy-inspector-egui` rides the same blocked dep + is explicitly forbidden by brief;
- bare-`egui` + manual wgpu/winit/clipboard integration ≈ reimplementing `bevy_egui`.

Bevy 0.19-rc.1's `bevy_ui` ships `Node`, `Display`, `BackgroundColor`, `Text`, `TextColor`, `TextFont`, `Button`, `Interaction`, `Pressed` — every primitive the panel needs. The panel is keyboard-driven (no slider widget needed). **Zero new transitive deps; zero new compile time hit on the workspace.**

---

## §4. `GpuGiParams` + `GpuRenderParams` final layout

### `GpuRenderParams` — 112 bytes (unchanged), one field-rename only

| Offset | Field | Size | Note |
|---|---|---|---|
| 0  | screen_width / screen_height / frame_count / rand_counter | 16 | row 0 |
| 16 | taa_index | 4 | row 1 |
| 20 | flags | 4 | row 1 |
| **24** | **max_ray_steps_primary** | **4** | **NEW (was `_pad0a`)** |
| 28 | _pad0b | 4 | row 1 |
| 32..112 | sky_sun_dir / sun_color / taa_jitter / bounding_box_min / bounding_box_max + their per-row pads | 80 | rows 2..6 unchanged |

**Net size delta: 0 bytes.** The `18-taa-fidelity.md` fix #2 had already converted `_pad0a` from `exposure` to a pad; this dispatch reclaims it for `max_ray_steps_primary`. Layout-preserving rename.

### `GpuGiParams` — 336 bytes (was 304)

| Offset range | Fields | Note |
|---|---|---|
| 0..128 | inv_view_proj, view_proj | mat4 × 2 |
| 128..192 | cam_pos_int+pad, cam_pos_frac+pad, sky_sun_dir+pad, sun_color+pad | 4 × vec4 rows |
| 192..276 | screen_width..flags + the 24-u32 scalar tail | unchanged |
| 276 | _pad4 | unchanged |
| 280..288 | taa_jitter (Vec2) | TAA-fidelity row, unchanged |
| 288 | sun_shadow_taps | Dispatch A field, unchanged |
| 292..304 | _pad5/_pad6/_pad7 | Dispatch A row tail, unchanged |
| **304** | **max_ray_steps_secondary** | **NEW** |
| **308** | **max_ray_steps_sun** | **NEW** |
| **312** | **max_ray_steps_sun_secondary** | **NEW** |
| **316** | **max_ray_steps_visibility** | **NEW — closes row 304..320** |
| **320** | **spatial_iter_count** | **NEW** |
| **324** | **_pad8** | **NEW** |
| **328** | **_pad9** | **NEW** |
| **332** | **_pad10** | **NEW — closes row 320..336** |

**Net size delta: +32 bytes** (5 u32 + 3 u32 pads). Total = 336, divisible by 16 (alignment-friendly).

---

## §5. `offset_of!` guards added

```rust
// (gpu_types.rs:815) size bumped
const _: () = assert!(std::mem::size_of::<GpuGiParams>() == 336);
// (gpu_types.rs:826-832) two new row-start guards
const _: () =
    assert!(std::mem::offset_of!(GpuGiParams, max_ray_steps_secondary) == 304);
const _: () =
    assert!(std::mem::offset_of!(GpuGiParams, max_ray_steps_secondary) % 16 == 0);
const _: () =
    assert!(std::mem::offset_of!(GpuGiParams, spatial_iter_count) == 320);
const _: () =
    assert!(std::mem::offset_of!(GpuGiParams, spatial_iter_count) % 16 == 0);
```

The 2 row-start guards pin both new rows. Plain `u32` fields — no `vec3`-then-scalar hazard can fire here (§4.3 of the design).

No new guards on `GpuRenderParams` — the field rename did not move the offset (`max_ray_steps_primary` still at 24, same as `_pad0a` was), so the existing 112-byte size assert is sufficient.

---

## §6. Knob defaults table

Class-P promotions — each pre-dispatch hardcoded WGSL `const` against the new runtime uniform default. Bit-equivalent at default values (`21-design-quality-panel.md` §6).

| Knob | Pre-dispatch WGSL | `GiSettings::default()` | Bit-equiv? |
|---|---|---|---|
| `max_ray_steps_primary`        | `ray_tracing.wgsl:122` const `120` | `120` | ✓ |
| `max_ray_steps_secondary`      | `ray_tracing.wgsl:123` const `100` | `100` | ✓ |
| `max_ray_steps_sun`            | `ray_tracing.wgsl:124` const `120` | `120` | ✓ |
| `max_ray_steps_sun_secondary`  | `ray_tracing.wgsl:125` const `80`  | `80`  | ✓ |
| `max_ray_steps_visibility`     | `ray_tracing.wgsl:126` const `60`  | `60`  | ✓ |
| `spatial_iter_count`           | `spatial_resampling.wgsl:622` literal `12u` | `12` | ✓ |

Runtime confirmation: the e2e baseline / `--entities` luminance numbers (§7 below) match Dispatch A's reported values to within float noise. The 6 promoted defaults are confirmed bit-equivalent.

There are also runtime tests in `panel.rs::tests`:
- `defaults_match_gi_settings_default` — every knob in `KNOBS[]` has its declared `default` == `GiSettings::default()`.
- `promoted_defaults_match_canonical_consts` — the 6 Class-P knobs equal the canonical WGSL/paper values bit-for-bit.

A future drift in either direction trips a `#[test]` failure, not silent rendering breakage.

---

## §7. Gate results

```
1) cargo build --workspace                                  → exit 0
   "Finished `dev` profile [optimized + debuginfo]" — clean, zero warnings
   on touched files.

2) cargo test -p bevy-naadf --lib                            → exit 0
   "116 passed, 1 ignored (1 suite, 4.25s)"
   Was 112 pre-dispatch (Dispatch A baseline). +4 new tests from
   panel.rs::tests: cursor_skips_non_interactive_rows,
   defaults_match_gi_settings_default, promoted_defaults_match_canonical_consts,
   at_least_one_interactive_knob. No regressions.

3) cargo run --release --bin e2e_render                      → exit 0
   PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames,
   framebuffer read back & non-degenerate, per-batch region gate green
   through camera motion, every pipeline created cleanly, every expected
   render-graph node dispatched.
   Region luminance — emissive 247.1, solid (GI-lit diffuse) 242.0,
                      sky 145.9.
   Dispatch A baseline was emissive 247.1, solid 242.0, sky 145.9 — exact
   match (bit-equivalent default promotion confirmed).

4) cargo run --release --bin e2e_render -- --entities        → exit 0
   PASS (batch 6) — same per-batch gates green.
   Region luminance — emissive 247.0, solid 241.9, sky 145.9.
   entity_pixel gate PASS.
   Dispatch A baseline was 247.1 / 241.9 / 145.9 — within float noise.
```

All four gates green. No luminance drift beyond float noise → no layout / default bug.

---

## §8. Keybind + how to use

The panel only spawns in the **production** windowed app (`cargo run` / `cargo run --bin bevy-naadf`). The e2e harness has `AppConfig::e2e` → `add_hud: false` → no panel. Same gate as the HUD.

**Toggle:** `F1` (verified unused by `FreeCameraPlugin` and the existing HUD's `D` DLSS toggle).

**Layout** (bottom-left, 360px wide, ~30% screen height, dim grey background):

```
[F1] Raymarching Quality
─────────────────────────────
  RAY STEP CAPS
> primary                    120 [P]
  secondary                  100 [P]
  sun                        120 [P]
  sun-secondary               80 [P]
  visibility                  60 [P]
  SPATIAL RESAMPLING
  iter count                  12 [P]
  sun_shadow_taps              4 [C]
  resample_size            500.00 [C]
  radius_lit_factor          3.00 [C]
  noise_suppress             0.40 [C]
  GI
  bounce_count                 3 [C]
  denoise_thresh           400.00 [C]
  is_denoise                true [C]
  is_sample_leveling        true [C]
  is_varying_radius         true [C]
  is_atmosphere_int         true [C]
  skip_samples              true [C]
  DIAGNOSTICS (read-only)
  taa_ring_depth         32 [restart-required] [D]
  camera_history_depth   128 [const] [D]
  valid_sample_storage   2 [storage-tied] [D]
  invalid_sample_storage 8 [storage-tied] [D]
  bucket_storage         32 [storage-tied] [D]
  refined_bucket         8 [storage-tied] [D]
  global_illum_max_accum 128 [const] [D]

[↑↓] navigate  [←→] adjust  [PgUp/PgDn] big
[Shift+←→] fine  [R] reset row  [Shift+R] reset all
```

**Keybinds (only while panel is open):**
- `F1` — close.
- `↑` / `↓` — move cursor (skips section / readonly rows).
- `←` / `→` — adjust selected knob by its `nudge` step.
- `PgUp` / `PgDn` — adjust by `big_step`.
- `Shift+←` / `Shift+→` — fine adjust (`nudge / 4`).
- `R` — reset the selected row to its default.
- `Shift+R` — reset **every** row to defaults.

Closed → no input is consumed; the FreeCamera responds normally (WASD/Shift/mouse). The `[F1] quality panel` keybind is also surfaced in the HUD's keybind-hint line for discoverability.

Class indicators (right-margin single character):
- `P` — Promoted from WGSL const; uniform-driven.
- `C` — already-config (existed pre-dispatch; now panel-exposed).
- `D` — Read-only diagnostic (storage-/texture-allocation tied; change requires restart).

---

## §9. What was NOT done (scope discipline)

### Class D-drop (explicitly out of scope per `21-design-quality-panel.md` §2.3)

- **TAA `screenPosDistanceSqr` threshold** (`taa.wgsl:349` literal `16.0`) — per-variant value (16 for base, 1 for albedo); promoting risks accidentally landing the albedo value on the base path. Not a quality-tuning lever; a correctness-critical TAA reproject reject threshold. **Dropped.**
- **Denoiser kernel radius** (`denoise_split.wgsl:102,199` literal `-10..=10`) — kernel is compiler-unrolled; sparsity pattern couples to size. Promoting opens a class of correctness regressions for marginal tuning value. **Dropped.**
- **Denoiser σ** (`denoise_split.wgsl:130,225` literal `10.0`) — pairs with kernel radius. **Dropped.**
- **`mod_size` adaptive-sampler formula constants** (`ray_queue_calc.wgsl:115` — `round(clamp(fac * 2.0, 0.0, 3.0) + 1.0)`) — paper-specific tuning constants, not user-facing knobs. The `skip_samples` toggle is the user-facing knob for adaptive sampling. **Dropped.**
- **Per-bucket-storage / valid / invalid / refined storage counts** (Class D-show) — surfaced read-only in the panel. **NOT promoted** because each is buffer-storage-tied + WGSL-array-sized; lifting them requires a resize-flush pipeline rebuild that is out of this dispatch's "one cohesive change" scope.

### Brief out-of-scope items (per `21-design-quality-panel.md` §8) — confirmed honoured

- **Atmosphere knobs** (`GpuAtmosphereParams`) — not touched.
- **Camera controls / FOV / near-far** — not touched.
- **World-edit tools / cube-sphere-paint UIs** — not touched.
- **Material editors** — not touched.
- **CLI flags for new knobs** — not added. Panel only.
- **HUD replacement** — `hud.rs` got one keybind-hint line append, nothing else.
- **`bevy_egui` / `bevy-inspector-egui` / bare `egui` integration** — none added; native `bevy_ui` only.

### High-risk findings — fresh-eyes-reviewer recommendations (carry-forward from §9.8 of the design)

The implementation completed every stage, but the design's §9.8 listed three items the implementer should *not* self-certify. They are restated here so the orchestrator can dispatch a fresh-eyes `delegate-reviewer`:

1. **GUI library deviation from brief default** — the brief specified `bevy_egui` with the fallback being "bare egui + manual integration"; I chose Bevy native UI instead. Verified `bevy_egui` 0.39.1 cannot link against Bevy 0.19-rc.1 (only Bevy 0.18.0). The keyboard-driven Bevy-native panel is functionally adequate (all knobs reachable, all defaults restorable, all reads visible). **Recommend fresh-eyes confirm:** (a) the egui-incompatibility re-verification, (b) acceptability of the keyboard-driven panel vs deferring until `bevy_egui` 0.19 lands. **HIGH RISK** — changes the deliverable's shape, not just its content.

2. **`GpuGiParams` size growth — 288 → 304 → 336 across three dispatches** — each step is clean; cumulatively the struct is getting large. Not load-bearing now, but worth a fresh-eyes look once stable to consider splitting into `gi_static_params` (tuning) + `gi_per_frame_params` (jitter/counters). **MEDIUM RISK** — purely a future-refactor flag.

3. **The `_pad0a` repurpose pattern** (this dispatch reclaimed `GpuRenderParams._pad0a` as `max_ray_steps_primary`, the same pattern `18-taa-fidelity.md` fix #2 used to drop `exposure`) — layout-preserving rename, fully reviewed in design §4.1. **LOW RISK** but a fresh-eyes pass might want to confirm the offset-table walk in `21-design-quality-panel.md` §R2 is correct.

### Verification gates retraced

All four gates green; luminance numbers match Dispatch A baseline exactly (within float noise). Test count 116 (was 112 — +4 panel-module tests). Build clean, zero warnings on touched files. Per-frame cost of the panel is bounded to one Text-string format + one Node visibility flip — negligible.

---

## §10. Decisions & rejected alternatives (impl-stage)

1. **Chose: `KnobKind` as an enum, not separate `U32Knob` / `F32Knob` / `BoolKnob` struct tables.** Rejected: dispatch-table pattern. Reason: one row-iterator handles every knob with a tagged `match`; no dynamic dispatch; the enum holds its own getter / setter / default. The compile-time runtime check `defaults_match_gi_settings_default` walks the same shape.

2. **Chose: `fn first_interactive()` non-const.** Rejected: `const fn`. Reason: `KnobKind::is_interactive` cannot be `const fn` because it uses `matches!` on a non-Copy field (the function-pointer fields). Making it const would mean inlining the section/readonly check at every call site or replacing the function-pointer storage with `Option<u32>`-style tagged unions. Non-const is simpler; this function is called once per F1-toggle, not per frame.

3. **Chose: `lib.rs` does the panel plugin wiring inline** (not as a `Plugin`). Rejected: a dedicated `PanelPlugin` struct. Reason: `hud.rs` uses the same inline pattern; consistency with the existing project style. One conditional + 3 system inserts.

4. **Chose: `add_systems(.chain())` ordering for toggle → adjust → update_text.** Rejected: parallel. Reason: `adjust_panel` mutates the knob state; `update_panel_text` reads it. Same-frame ordering matters so the panel always reflects the just-applied adjustment. Chain forces serial execution.

5. **Chose: a defensive `max(_, 1u)` clamp at every promoted WGSL use site.** Same pattern Dispatch A used (`19-gi-reservoir-scope.md` §3.1 + `20-impl-phase-d-shadow-A.md` decision #3). Belt-and-suspenders for the bytemuck::Zeroable / `GiSettings { ..: 0, .. }` case. **No correctness regression** at default values — the clamp is a no-op there.

6. **Chose: keep `ray_tracing.wgsl:122-126` `const MAX_RAY_STEPS_*` declarations in place** with a header comment marking them documentation-only. Rejected: delete them. Reason: numerous doc anchors in `19-gi-reservoir-scope.md`, `12-alignment-gap.md`, `20-impl-phase-d-shadow-A.md`. naga DCEs unused consts at compile time — zero runtime cost. Future readers see the canonical values in one place.

7. **Chose: hide via `Display::None` (not despawn).** Rejected: spawn/despawn on every F1. Reason: avoids spawn cost on every toggle; the panel's `Text` and `Node` entities have stable Bevy entity IDs the systems can query consistently.

---

## §11. Assumptions made (impl-stage)

1. **`Display::None` in `Node` hides the entire subtree** (so the `Text` child does not render). Verified `bevy_ui-0.19.0-rc.1` source — `Display::None` causes the layout engine to skip the node + children entirely.
2. **F1 input arrives via `ButtonInput<KeyCode>` in `Update`** with `just_pressed` returning true exactly once per physical key press. Standard Bevy pattern; already in use by `camera::toggle_dlss` (the `D` toggle).
3. **`AppArgs` is `ResMut`-accessible from `Update`.** Verified — `AppArgs` is inserted as a Resource in `build_app_with_args`.
4. **`ExtractedGiConfig` mirrors the entire `GiSettings` struct each frame.** Verified `render/extract.rs::extract_gi_config` — it copies the whole `args.gi` value, so new fields ride along for free.
5. **The panel does not need to consume input** (camera will not move when the user presses ↑↓ ←→ because those keys are not in the camera's WASD binding set). Confirmed by reading `bevy_camera_controller`'s `FreeCameraPlugin` — uses WASD + mouse, not arrows.
6. **`R` key collisions** — `KeyCode::KeyR` is not used elsewhere in the project. Verified by grep.

---

## §12. Carry-forward for future sessions

- The `class` indicator `P` / `C` / `D` is documented in the panel UI itself (the column header is implicit but each row carries it). If the user wants explicit text legend in the panel, that's a 5-line addition to `update_panel_text`.
- The panel does not currently expose **mouse-clickable** controls. The brief allowed for a "Reset to paper-canonical defaults" mouse button; my implementation uses `Shift+R` for that instead (keyboard-only). If mouse Reset is wanted later, a Bevy `Button` entity + an `Interaction` system is the standard pattern (~30 LOC).
- The Class-D-show rows display the *current* allocation depth values. If a future track unlocks runtime resize for any of them (e.g. `BUCKET_STORAGE_COUNT`), the Class-P promotion + WGSL array-resize logic would land separately; the panel row's class indicator flips from `D` to `P`, the read-only formatter swaps for a setter, and the dispatch line stays the same.
