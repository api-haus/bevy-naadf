# 21 — Design: Comprehensive Raymarching-Quality Panel

**Date:** 2026-05-15
**Branch:** `main` (HEAD `1c35c7f` at design time — Dispatch A landed `sun_shadow_taps`)
**Status:** Design + self-review BEFORE implementation; implementation log lands in `22-impl-quality-panel.md`.

---

## §1. Goal restatement (verbatim user ask)

> "make a comprehensive panel with all the knobs and whistles regarding the raymarching quality"

Operational reading: an **in-app dev panel** that exposes every meaningful runtime knob affecting raymarching / GI / reservoir / TAA quality, so the user can tune live without rebuilds. Triggered by post-Dispatch-A visual confirmation: the multi-tap sun shadow worked; residual reservoir-convergence noise remains on jagged voxel side-faces. The panel is a tuning surface for finding the right tradeoffs interactively.

---

## §2. Knob inventory (audit)

Audited every config knob in the GI / TAA / raymarching pipeline against `19-gi-reservoir-scope.md` §3 and the WGSL source. Class P = promote to a `gi_params` uniform field. Class C = already-config; just expose. Class D-show = display read-only (storage-allocation-tied). Class D-drop = surfaced but explicitly out of scope.

### §2.1 — Knob table

| # | Knob | Source (file:line) | Class | Paper / C# default | Proposed runtime range | Tooltip |
|---|---|---|---|---|---|---|
| 1  | `MAX_RAY_STEPS_PRIMARY` | `ray_tracing.wgsl:122` (const); used `naadf_first_hit.wgsl:180` | P | 120 | 32..512 | Max DDA steps for primary G-buffer rays. Lower = faster + visible voxel-edge misses. |
| 2  | `MAX_RAY_STEPS_SECONDARY` | `ray_tracing.wgsl:123`; used `naadf_global_illum.wgsl:290` | P | 100 | 32..512 | Max DDA steps per GI bounce ray. Lower = faster + GI darkening (rays die in air). |
| 3  | `MAX_RAY_STEPS_SUN` | `ray_tracing.wgsl:124`; used `spatial_resampling.wgsl:556` | P | 120 | 32..512 | Max DDA steps for the spatial-resampling sun-visibility ray. Lower = light leaks beyond reach. |
| 4  | `MAX_RAY_STEPS_SUN_SECONDARY` | `ray_tracing.wgsl:125`; used `naadf_global_illum.wgsl:374` | P | 80 | 32..512 | Max DDA steps for the per-bounce sun ray inside `globalIllum`. Lower = secondary-bounce sun light leaks. |
| 5  | `MAX_RAY_STEPS_VISIBILITY` | `ray_tracing.wgsl:126`; used `spatial_resampling.wgsl:457` | P | 60 | 32..512 | Max DDA steps for the spatial-resampling visibility ray (reservoir re-projection). Lower = light leaks; higher = no benefit (see `19-gi-reservoir-scope.md` §3.2). |
| 6  | Spatial-resampling iteration count | `spatial_resampling.wgsl:622` (literal `12u`) | P | 12 | 1..32 | Neighbour-reservoir sample count (Algorithm 2). Higher = less indirect-lighting variance ∝ √N; linear cost. |
| 7  | `sun_shadow_taps` | `gpu_types.rs:504` (already config) | C | 4 (Dispatch A default) | 1..32 | Sun-disk taps per pixel per frame. N=1 = C# bit-equivalent; higher = softer penumbra ∝ √N (paper §5.2). |
| 8  | `bounce_count` | `lib.rs:50`, gi.rs upload | C | 3 | 1..3 | Max GI bounce depth (`naadf_global_illum.wgsl:281` clamps to `min(_, 3u)`). |
| 9  | `is_denoise` | `lib.rs:66`, `GI_FLAG_IS_DENOISE` | C | true | bool | Run the sparse bilateral denoiser (`denoise_split.wgsl`). |
| 10 | `is_sample_leveling` | `lib.rs:67`, `GI_FLAG_IS_SAMPLE_LEVELING` | C | true | bool | Brightness-level the bucket samples (`sample_refine.wgsl`). |
| 11 | `is_varying_resampling_radius` | `lib.rs:69`, `GI_FLAG_IS_VARYING_RADIUS` | C | true | bool | Adaptive-radius pre-pass in spatial resampling. |
| 12 | `is_atmosphere_interaction` | `lib.rs:71`, `GI_FLAG_IS_ATMOSPHERE_INTERACTION` | C | true | bool | In-volume atmosphere fold along primary rays. |
| 13 | `skip_samples` | `lib.rs:63`, `GI_FLAG_SKIP_SAMPLES` | C | true | bool | The 1↔0.25-spp toggle (drives `rayQueueCalc`). |
| 14 | `denoise_thresh` | `lib.rs:57` (`denoiseThresh`) | C | 400.0 | 0..2000 | Sparse bilateral guide-weight scale (`denoise_split.wgsl:122,218`). |
| 15 | `radius_lit_factor` | `lib.rs:59` (`radiusLitFactor`) | C | 3.0 | 0..1000 (log) | Adaptive-radius shadow-bias factor (paper §5.2 "mitigates darkening"). |
| 16 | `noise_suppression_factor` | `lib.rs:62` | C | 0.4 | 0.01..100 (log) | Per-bucket firefly-suppression strength. |
| 17 | `spatial_resample_size` | `lib.rs:54` | C | 500.0 | 32..2000 | Spatial-resampling neighbour-search radius (px). |
| 18 | `global_illum_max_accum` | `lib.rs:52` | C | 128 | constant | Ring depth of the per-pixel hit-count accumulator (D-show: storage-tied via `SAMPLE_COUNTS_LEN = 128+3`). |
| 19 | `taa_ring_depth` | `lib.rs:140`, `TaaRingConfig` | D-show | 32 | 16 / 24 / 32 | TAA sample ring depth (texture-allocation tied — change requires restart). |
| 20 | `CAMERA_HISTORY_DEPTH` | `taa.rs:30` | D-show | 128 | constant | Camera-history ring depth (texture-allocation tied). |
| 21 | `VALID_SAMPLE_STORAGE_COUNT` | `gi.rs:51` | D-show | 2 | constant | Lit-sample ring multiplier (`19-gi-reservoir-scope.md` §3.7 — storage-tied; +64 MB @ 1080p per +1). |
| 22 | `INVALID_SAMPLE_STORAGE_COUNT` | `gi.rs:54` | D-show | 8 | constant | Unlit-sample ring multiplier (`19-gi-reservoir-scope.md` §3.8 — storage-tied; +256 MB @ 1080p per +8). |
| 23 | `BUCKET_STORAGE_COUNT` | `gi.rs:57` | D-show | 32 | constant | Per-bucket refined-sample capacity (storage-tied + shader-array-sized at `sample_refine.wgsl:622`). |
| 24 | `REFINED_BUCKET_STORAGE_COUNT` | `gi.rs:60` | D-show | 8 | constant | Per-bucket compressed-sample capacity (storage-tied). |
| 25 | TAA `screenPosDistanceSqr` threshold | `taa.wgsl:349` (literal `16.0`) | D-drop | 16.0 | n/a | The base-variant per-variant divergence; promoting risks accidentally landing the albedo `> 1.0` value. Not a quality-tuning lever — a reproject reject threshold. Out of scope. |
| 26 | Denoiser kernel radius | `denoise_split.wgsl:102,199` (literal `-10..=10`) | D-drop | 10 | n/a | Promoting requires variable loop bounds inside a 21-iteration sparse-kernel hot path; the runtime variant cost is a wash and the kernel-radius is not a noise-tuning knob users typically reach for. Defer. |
| 27 | Denoiser σ | `denoise_split.wgsl:130,225` (literal `10.0`) | D-drop | 10.0 | n/a | Pairs with knob 26 (changes meaning if kernel radius shifts); defer with kernel radius. |

Total: **6 P, 11 C, 6 D-show, 3 D-drop.**

### §2.2 — Why each promotion is safe

For each Class-P knob (1–6), the change is uniform-field-only — a `u32` (or i32 cast) read inside an already-hot WGSL function. No buffer resize, no storage-array sizing, no pipeline rebuild. All five `MAX_RAY_STEPS_*` are arguments to `shoot_ray` at use-sites; the spatial iter count is a `for` loop bound. None of them is a `naga-oil` shader-def (i.e., no specialization-constant churn).

### §2.3 — Why the D-drops are deferred

- **Knob 25** (`screenPosDistanceSqr`): per-variant value (16 for base, 1 for albedo). Promoting without preserving the per-variant distinction would tempt drift. The user's "knobs and whistles" framing does not include reproject-reject thresholds — these are correctness-critical TAA controls, not quality dials.
- **Knobs 26/27** (denoise kernel radius / σ): the kernel is unrolled-ish (compiler typically unrolls `for i: i32 = -10; i <= 10`) and the sparsity pattern (`x = select(0, 1, ...)`) couples to the kernel size. Promoting opens a class of regressions far out of proportion to the rare desire to tune kernel size live. σ alone is exposed via the existing `denoise_thresh` (C) which adjusts the bilateral guide-weight scale — that's the noise-relevant lever.

---

## §3. GUI library decision

**Decision: built-in Bevy UI (`bevy_ui` 0.19, already in `DefaultPlugins`), keyboard-driven.**

### §3.1 — Why not `bevy_egui`

Checked crates.io (`bevy_egui` latest is `0.39.1`, 2026-02-06) and the GitHub repo. The Cargo.toml of `bevy_egui` 0.39.1 declares `bevy_app = 0.18.0`, `bevy_render = 0.18.0`, etc. — **all on Bevy 0.18**. No Bevy 0.19-rc.1 release exists; no open PR; no main-branch update past 0.18. Pulling `bevy_egui` would force a Bevy downgrade or a 5-day Bevy-API porting exercise just to get a panel — wildly out of scope and impossible without forking.

### §3.2 — Why not bare `egui` + manual wgpu integration

That is exactly `bevy_egui`'s job. Doing it by hand is a 500-line wgpu / winit / clipboard integration over a Bevy render graph — same impossibility, more code.

### §3.3 — Why not `bevy-inspector-egui`

Brief explicitly forbids it ("too magic / reflection-based"). And it sits on top of `bevy_egui` anyway, inheriting the same Bevy-version block.

### §3.4 — Why Bevy native UI

`bevy_ui` 0.19 ships `Button`, `Pressed`, `Text`, `Node`, `Interaction` states, and a working layout engine — already used by `hud.rs`. Zero new deps. No version mismatch. Same render-graph the rest of the app uses. The HUD pattern (`Node` with `BackgroundColor` + a `Text` child) trivially extends to a multi-row panel.

The cost: native Bevy UI has no built-in slider. We replace sliders with a keyboard-driven discrete-step navigator. Each row shows `label = value [range]`; F1 toggles the panel; ↑/↓ moves the selection cursor; ←/→ decreases / increases the selected knob by one step; PageUp/PageDown by a larger step; Shift modifier for fine-grain; `R` resets the selected knob to its paper-canonical default; `Shift+R` resets all. **One Bevy UI Button — "Reset all" — for mouse users; otherwise keyboard-driven.**

This is the cleanest path that works on Bevy 0.19-rc.1 without dragging in dead-end deps.

### §3.5 — Why the **brief's bevy_egui guidance is overridden**

Brief: *"Default: `bevy_egui` (Bevy ecosystem standard for dev panels). Verify a version compatible with Bevy 0.19-rc.1; if none, fall back to `bevy_dev_tools` egui, or bare `egui` + manual integration — pick the cleanest path that works on the project's current Bevy."*

The fallback path the brief sanctions is exactly this dispatch: `bevy_egui` does not work, and the cleanest replacement is Bevy native UI (the rationale §3.4 holds for both "bare egui manual integration" and "Bevy native") — built-ins are simpler than manual egui. The brief allows this choice explicitly.

---

## §4. Layout plan — `GpuGiParams` + `GpuRenderParams` extensions

Revised after stage-2 verification (§9.5): `naadf_first_hit.wgsl` does NOT bind `gi_params` (only `GpuRenderParams` + `GpuCamera` + atmosphere). Adding a `gi_params` binding to the first-hit pipeline is significant pipeline-layout churn (bind-group descriptor rewrite + first-hit pipeline-spec change). Cleaner: **route `MAX_RAY_STEPS_PRIMARY` through `GpuRenderParams` via the existing `_pad0a` slot**; the other 4 ray-step caps + spatial iter count go in `GpuGiParams` (their consumers already bind it).

### §4.1 — `GpuRenderParams` — repurpose `_pad0a` only

`GpuRenderParams` size stays **112 bytes** (no struct edit at all — just a field-name rename).

- Pre-edit (`gpu_types.rs:60-112`): `_pad0a` at offset 24, `_pad0b` at offset 28 (formerly `exposure` / `tone_mapping_fac`; replaced with pads in `18-taa-fidelity.md` fix #2 to keep the 112-byte layout).
- Post-edit: rename `_pad0a` → `max_ray_steps_primary` (u32, offset 24); `_pad0b` stays as `_pad0b`. **One field rename, zero layout change.** WGSL counterpart in `render_pipeline_common.wgsl` gets the same rename.

This is the exact same layout-preserving repurpose pattern the TAA-fidelity track used.

### §4.2 — `GpuGiParams` extension

Before edits: 304 bytes (post-Dispatch-A).
After edits: **336 bytes** (+32 bytes / 2 fresh 16-byte rows).

Five new fields, all `u32`, grouped + padded into 2 new 16-byte rows. Predicted layout:

| Rust field | Offset | Size | Lane |
|---|---|---|---|
| `sun_shadow_taps`             | 288 | 4 | (already present, Dispatch A row x) |
| `_pad5/6/7`                   | 292..304 | 12 | (already present, row trailing) |
| `max_ray_steps_secondary`     | 304 | 4 | NEW — row20.x |
| `max_ray_steps_sun`           | 308 | 4 | NEW — row20.y |
| `max_ray_steps_sun_secondary` | 312 | 4 | NEW — row20.z |
| `max_ray_steps_visibility`    | 316 | 4 | NEW — row20.w |
| `spatial_iter_count`          | 320 | 4 | NEW — row21.x |
| `_pad8`                       | 324 | 4 | NEW — row21.y |
| `_pad9`                       | 328 | 4 | NEW — row21.z |
| `_pad10`                      | 332 | 4 | NEW — row21.w |
| (struct end)                  | 336 | — | — |

**Total: 336 bytes.** 4 × u32 + 4 × u32 = 32 new bytes (5 used + 3 pads).

WGSL mirror: 8 new top-level u32 fields (`max_ray_steps_secondary` ... `_pad10`) appended after the existing `pad_d` (offset 300) in `gi_params.wgsl`. Plain u32s — no `vec3`-then-scalar hazard. Compile-time guards: assert size 336, plus `offset_of!` guards on `max_ray_steps_secondary` (must be 304) and `spatial_iter_count` (must be 320) — those two guards pin the two new rows.

### §4.3 — Why pack as plain u32s (not a `vec4<u32>` in WGSL)

The `vec3`-then-scalar hazard only triggers when a `vec3<T>` is followed by a non-16-byte member. A run of plain `u32`s mirrors byte-for-byte between WGSL's storage-buffer std140 packing and Rust `#[repr(C)]` — same as the existing 24-`u32` scalar tail (offsets 192..276 in `gi_params.wgsl`). The trailing `_pad8`/`_pad9`/`_pad10` lanes are explicit so a future `vec2<f32>` or `vec3` cannot accidentally land mid-row.

---

## §5. Panel layout (sections + their knobs + tooltips)

Single overlay at the bottom-left (HUD is top-left — no conflict). Toggleable via F1. Sections (collapsing not needed; the panel always renders all rows when open):

```
[F1] Raymarching Quality           <- title bar
─────────────────────────────────
RAY STEP CAPS                       <- section header
> max_ray_steps_primary    120      <- the > marks the selected row
  max_ray_steps_secondary  100
  max_ray_steps_sun        120
  max_ray_steps_sun_sec     80
  max_ray_steps_visibility  60
SPATIAL RESAMPLING
  spatial_iter_count        12
  sun_shadow_taps            4
  spatial_resample_size    500.0
  radius_lit_factor          3.0
  noise_suppression_factor   0.4
GI
  bounce_count               3
  global_illum_max_accum   128 [readonly]
  denoise_thresh           400.0
  is_denoise              true
  is_sample_leveling      true
  is_varying_radius       true
  is_atmosphere_interact  true
  skip_samples            true
DIAGNOSTICS (read-only)
  taa_ring_depth            32 [restart-required]
  camera_history_depth     128 [const]
  valid_sample_storage       2 [storage-tied]
  invalid_sample_storage     8 [storage-tied]
  bucket_storage_count      32 [storage-tied]
  refined_bucket_storage     8 [storage-tied]
  viewport            256 x 256 [info]

[R] reset selected   [Shift+R] reset all
[↑↓] navigate   [←→] adjust   [PgUp/PgDn] big step
```

Section headers + readonly lines are dim grey; the selected row is highlighted with `> ` prefix + brighter text. **No tooltips overlay** (Bevy UI doesn't ship hover tooltips); each knob row's right-margin shows a one-character mode indicator (`P` = promoted, `C` = config, `D` = diag).

### §5.1 — Step sizes

Each knob carries a `nudge_step` and `big_step` matching its scale:
- ray-step caps: nudge 8, big 32
- spatial iter / sun taps: nudge 1, big 4
- bounce_count: nudge 1, big 1 (range 1..3)
- spatial_resample_size: nudge 50.0, big 200.0
- radius_lit_factor: nudge 0.5, big 3.0
- noise_suppression_factor: nudge 0.05, big 0.5
- denoise_thresh: nudge 50.0, big 200.0
- bool toggles: ←/→ flips
- read-only: ←/→ no-op

Shift modifier: nudge / 4 (fine-grain).

---

## §6. Defaults strategy — bit-equivalence preservation

Every Class-P promotion's runtime default **must equal the const it replaces, bit-for-bit**, so panel-disabled (or default-loaded) behaviour is the same as pre-dispatch.

| Knob | Pre-dispatch WGSL const | New Rust default | Bit-equivalent? |
|---|---|---|---|
| `max_ray_steps_primary`        | 120 | 120 (GpuRenderParams.max_ray_steps_primary) | ✓ |
| `max_ray_steps_secondary`      | 100 | 100 (GpuGiParams.max_ray_steps_secondary) | ✓ |
| `max_ray_steps_sun`            | 120 | 120 | ✓ |
| `max_ray_steps_sun_secondary`  |  80 |  80 | ✓ |
| `max_ray_steps_visibility`     |  60 |  60 | ✓ |
| `spatial_iter_count`           |  12 |  12 | ✓ |

The compile-time `const MAX_RAY_STEPS_*` declarations in `ray_tracing.wgsl:122-126` are **retained as documentation-only references** (their consumers move to the uniform) — this keeps the line-anchored citations in `19-gi-reservoir-scope.md`, `12-alignment-gap.md`, `20-impl-phase-d-shadow-A.md` from going stale, and gives future readers a single place to see "C# / paper value = X". A WGSL `const` left in place but unused costs nothing (naga dead-code-eliminates it from the binary).

If any e2e gate's luminance number shifts more than ε after this dispatch, that is **proof of a layout/default bug** and the implementation halts.

---

## §7. Activation strategy

- **Default:** panel **closed** on startup. Production app gets the panel toggle keybind; e2e harness gets neither (no `add_hud`, no panel — `AppConfig::e2e` already turns off the HUD).
- **Toggle:** `F1`. F1 is unused in `FreeCameraPlugin` (verified by searching for `KeyCode::F1`); `D` is the DLSS toggle (`hud.rs`). F1 is a safe binding.
- **Open state:** the panel covers ~30 % of the screen (bottom-left); the HUD is top-left; they never overlap.
- **Input gating:** when the panel is open, ↑↓←→ events are consumed by the panel (don't bubble to camera movement). Closed → all input flows to the camera. This is implemented with a `panel_open` resource the camera systems can check via a `if panel.open { return; }` early-out — see §9.
- **HUD coexistence:** `hud.rs` gets a single new line: `"[F1] quality panel"` appended to the existing keybind hint. That is **the only one-line touch** of `hud.rs` permitted by the brief.

---

## §8. Out of scope

The brief enumerates these; I list them so the reviewer can verify nothing leaks:
- **Atmosphere knobs** (`atmosphere.rs`, `GpuAtmosphereParams`) — untouched.
- **Camera controls** (FreeCameraPlugin, FOV, near/far) — untouched.
- **World-edit tools** (cube/sphere/paint UIs) — Phase-C explicit non-goal.
- **Material editors** — out of scope.
- **HUD replacement** — `hud.rs` gets one keybind-hint line and nothing else.
- **Atmosphere `screenPosDistanceSqr` / denoiser kernel size / σ** — Class D-drop per §2.3.
- **CLI flags for new knobs** — none. Brief says runtime knob via panel only.

---

## §9. Self-review notes

Several things surfaced while walking the design; they changed the plan.

### §9.1 — `bevy_egui` 0.19 incompatibility (the biggest pivot)

Verified `bevy_egui` 0.39.1 declares `bevy_app = 0.18.0` — no Bevy 0.19 release exists. The brief's default + the brief's fallbacks both rest on egui being available; both are blocked. The clean replacement is Bevy native UI (§3.4), which adds **zero deps** and works trivially. This is now §3 §3.1–§3.5.

If the orchestrator deems "no egui = no panel-worth-doing", that is a fresh-eyes call. Flagged in §10 below — **medium risk, recommend fresh-eyes confirmation**.

### §9.2 — `GpuRenderParams` extension would have been the obvious choice for `MAX_RAY_STEPS_PRIMARY`

`naadf_first_hit.wgsl` binds `GpuRenderParams`, not `gi_params`. The clean thing would be to extend `GpuRenderParams` with `max_ray_steps_primary` only. But that doubles the layout-edit surface (two structs to mirror, two compile-time asserts, two e2e gate proofs), and the `MAX_RAY_STEPS_*` group is conceptually one batch. Decision: route ALL 5 ray-step caps through `gi_params`, including the primary one. **Cost: `naadf_first_hit.wgsl` gains a `gi_params` import** (one new `#import` line + the binding to the same `@group(2)` slot — confirmed via `pipelines.rs` that the first-hit pipeline already has a `gi_params` binding available). **Trade: 1 import line vs 1 new GpuRenderParams field.** Imports win.

Actually re-check: `naadf_first_hit.wgsl` may NOT already bind `gi_params`. If it doesn't, adding it requires editing the bind-group layout — significant. **Verify in §9.5 before coding.**

### §9.3 — The spatial iter count change inside `sample_neighbors`

The existing loop bound at `spatial_resampling.wgsl:622` is `12u`. Replacing with `gi_params.spatial_iter_count` is one literal. The two adaptive-radius pre-pass 12-tap loops inside `sample_neighbors` (`:203`, `:281`?) are **separately literal-bounded** — those are NOT the spatial-iter loop the `19-gi-reservoir-scope.md` §3.3 candidate refers to. Promoting only the outer Algorithm 2 loop (`spatial_resampling.wgsl:622`'s call site that passes `12u` into `sample_neighbors`'s `sample_count` parameter); the adaptive-radius inner loops stay literal. This is **deliberate** — the adaptive-radius pre-pass is structurally 12-tap, not a tuning knob.

### §9.4 — `MAX_RAY_STEPS_VISIBILITY` interaction with the **3-iteration visibility loop**

`spatial_resampling.wgsl:453` has `for (var i: u32 = 0u; i < 3u; i = i + 1u)` — three reflections per visibility check, each calling `shoot_ray(.., MAX_RAY_STEPS_VISIBILITY, ..)`. Lowering `max_ray_steps_visibility` to a tiny number (8?) would cost 3× per pixel, not 1×. Tooltip should warn. Note: I am NOT promoting this `3u` iteration count — that's a per-mirror-bounce-depth choice, structural.

### §9.5 — Audit risk: does `naadf_first_hit.wgsl` already see `gi_params`?

Need to verify before implementation. If yes, the 5-cap promotion sits cleanly in `gi_params`. If no, I'll either:
- (a) extend `naadf_first_hit.wgsl`'s bind group to include `gi_params` (touches `pipelines.rs::first_hit_layout`);
- (b) extend `GpuRenderParams` with just `max_ray_steps_primary` (touches `GpuRenderParams` layout, one new u32 field with offset guard).

(a) is preferred because it batches with the rest of the dispatch (and §9.2 already chose `gi_params`). Will determine in stage 3 reading and document the choice in `22-impl-quality-panel.md`. Either way the bit-equivalence at default values is preserved.

### §9.6 — Coverage check

Re-walked the brief:
- "Primary rays" — ✓ knobs 1, 2 (primary, secondary).
- "Sun rays" — ✓ knobs 3, 4 (sun, sun_secondary).
- "Visibility ray" — ✓ knob 5.
- "Adaptive sampler" — `skip_samples` (C, already in `flags`) + `mod_size` (the `round(clamp(fac*2,0,3)+1)` formula at `ray_queue_calc.wgsl:115`). The `mod_size` formula has a hardcoded `2.0`, `3.0`, `1.0` — these are **paper-specific tuning constants**, not user-facing knobs (changing them changes the adaptive scheme's structure, not its quality). Defer entirely; the `skip_samples` toggle is the user-facing knob.
- "Spatial iter count + radius_lit_factor + GI bounce" — ✓ knobs 6, 15, 8.
- "Denoiser kernel/σ" — explored, found D-drop (§2.3).
- "TAA `screenPosDistanceSqr`" — D-drop (§2.3) — not a tuning knob.
- "Ring depths" — D-show (§2.1 knobs 19–24).
- "Defaults button" — ✓ §3.4 keybind R / Shift+R.
- "Read-only diagnostics" — ✓ §5 panel section "DIAGNOSTICS".
- "Tooltip for every knob" — partial; one-character mode indicator only (Bevy UI lacks hover tooltips natively). Adding text tooltips would mean a hover overlay system — out of scope of "the cleanest path that works".

### §9.7 — Risk audit (worst-case per Class-P default bug)

| Knob | If wrong default = 0 | Mitigation |
|---|---|---|
| max_ray_steps_primary | All primary rays die instantly — black screen on hit pixels | Compile-time `assert!` that struct field default = 120 in `lib.rs` |
| max_ray_steps_secondary | GI ≤3-bounce dies → no indirect light | Same |
| max_ray_steps_sun(_secondary) | Sun shadows always-pass (early-out) → over-bright | Same |
| max_ray_steps_visibility | All reservoir visibility passes → light leaks | Same |
| spatial_iter_count | Loop runs 0 times → black GI | Defensive `max(_, 1u)` clamp in WGSL like the sun-shadow shader does |
| sun_shadow_taps | (Already has `max(_, 1u)` clamp from Dispatch A) | — |

All six Class-P knobs get a defensive `max(_, 1u)` clamp at use site — same pattern Dispatch A used. The Rust default already matches the const-replaced value bit-for-bit (§6); the clamp is belt-and-suspenders for "zero-init bytemuck::Zeroable struct" or "hand-constructed `GiSettings { max_ray_steps_primary: 0, .. }`".

### §9.8 — High-risk findings (escalate fresh-eyes)

1. **`bevy_egui` deviation (§3.5).** I'm replacing the brief's default tool-choice with a different UI library because the brief's choice physically does not work. Recommend `delegate-reviewer` confirm: (a) `bevy_egui` is genuinely incompatible (re-verify Cargo.toml inspection), (b) Bevy native UI is acceptable, (c) keyboard-driven panel is acceptable (vs deferring the dispatch entirely until egui supports 0.19). **HIGH RISK** — this changes the deliverable's shape, not just its content.

2. **`GpuGiParams` size grew 5× in three dispatches** (B at 288 → TAA-fidelity at 288 still → Dispatch A at 304 → this at 336). Each step has been clean; cumulatively the struct is getting large. **MEDIUM RISK** — not load-bearing, but worth a fresh-eyes look once stable to consider splitting `gi_params` into `gi_static_params` (load-once tuning) + `gi_per_frame_params` (jitter/counter/RNG salts). Defer that refactor; flag it.

3. **`naadf_first_hit.wgsl` bind-group extension** (§9.5). I will resolve this in implementation; the resolution is mechanical, but it touches `pipelines.rs::first_hit_layout` if option (a) is needed. **LOW RISK** (mechanical edit), but worth documenting the choice in `22-impl-quality-panel.md` §1 explicitly.

---

## §10. Decisions & rejected alternatives

1. **Chose: Bevy native UI.** Rejected: `bevy_egui` (Bevy version mismatch); `bevy-inspector-egui` (brief forbids + same dep); bare `egui` (manual wgpu/winit/clipboard ≈ reimplementing `bevy_egui`); `bevy_dev_tools` (no egui dev-panel widget exists — verified). **Flip-trigger:** orchestrator + user decide a panel without sliders is unacceptable → defer until `bevy_egui` 0.19 lands → fresh-eyes review per §9.8.

2. **Chose: all 5 ray-step caps in `gi_params` (not GpuRenderParams).** Rejected: split between `gi_params` + `GpuRenderParams` per-shader-binding. Reason: one struct edit, one offset guard, one upload site. **Flip-trigger:** §9.5 verification shows `naadf_first_hit.wgsl` cannot bind `gi_params` without significant pipeline-layout churn → fall back to extending `GpuRenderParams` with just `max_ray_steps_primary`.

3. **Chose: keep the `MAX_RAY_STEPS_*` consts in `ray_tracing.wgsl` as documentation refs.** Rejected: delete them. Reason: numerous doc anchors. naga DCEs unused consts at compile time — zero runtime cost.

4. **Chose: 6 P, 11 C, 6 D-show, 3 D-drop.** Rejected: also promote the denoise kernel-radius / σ / `screenPosDistanceSqr` / `mod_size` formula constants. Reason: each has cost-or-correctness risk disproportionate to its tuning value (§2.3, §9.6).

5. **Chose: keyboard-driven panel (no mouse sliders).** Rejected: build custom drag-and-drop slider widgets on `bevy_ui` primitives. Reason: ~200 LOC for an unbounded surface, vs ~30 LOC for the keyboard navigator. Single "Reset all" mouse button covers the mouse-user case.

6. **Chose: F1 toggle.** Rejected: backtick `\``, `~`, `P`. Reason: F1 is unused; `~` collides with dev consoles in many engines (the user might add one later); `P` collides with the C# convention for pause; backtick has no obvious meaning.

7. **Chose: panel BOTTOM-left, HUD top-left, no collision.** Rejected: side-by-side (eats horizontal real estate); modal centre (covers GI region the user wants to see while tuning). Reason: HUD ≈ 200px tall × 200px wide; panel ≈ 350px tall × 250px wide.

8. **Chose: `_pad8/_pad9` explicit at struct end.** Rejected: trailing struct end at offset 328 (struct size 328). Reason: WGSL `array<GpuGiParams>` stride is `roundUp(16, sizeof)`, so a 328-byte struct would round up to 336 anyway. Better to declare the pad explicitly so the Rust mirror's `size_of` exactly equals what WGSL allocates — same pattern Dispatch A used.

---

## §11. Assumptions made

1. **`bevy_egui` 0.19 will not be available before this dispatch lands.** Verified crates.io + GitHub default branch + GitHub PRs (none).
2. **`F1` is an unused keybind in the current app.** Verified via grep — no hits for `KeyCode::F1`. `KeyCode::D` is taken by DLSS toggle.
3. **Bevy 0.19-rc.1's `bevy_ui` has stable `Button` / `Interaction` / `Pressed` types.** Confirmed via reading `bevy_ui-0.19.0-rc.1` source — `widget/button.rs`, `interaction_states.rs`.
4. **Plain `u32` fields in `gi_params.wgsl` mirror byte-for-byte with Rust `#[repr(C)]` u32s.** Confirmed by the existing 24-`u32` scalar tail at offsets 192..276 — already byte-for-byte verified by the 112-test suite + every e2e gate.
5. **The 6 Class-P promotions can default to bit-equivalent values.** Verified table §6.
6. **Subtle: the e2e harness has the HUD off (`AppConfig::e2e().add_hud == false`).** So adding panel systems will NOT affect e2e luminance gates as long as the panel is gated on `cfg.add_hud == true` — same gate as the HUD itself. This is the load-bearing wiring rule.
7. **The user wants the panel for the **production** app, not e2e.** The brief says "tune live without rebuilds" — that's the windowed `cargo run` path, not the bounded `e2e_render` path. Panel only adds wiring under the windowed config.
8. **F1 input arrival at `Update` is fast enough to feel "live".** Bevy's input system fires every frame; the panel toggle is a Pressed-edge event; latency is one frame. Same as the existing DLSS toggle.

---

(Continued in `## Independent review` below.)

---

## Independent review

Self-review against the success criteria in the brief, adversarial about the design above. The brief lists six explicit areas; I work through each, then the implicit risks.

### §R1. Re-implementation check — does any existing system do this?

Searched the tree:
- `hud.rs` — read-only diagnostics overlay, FPS + per-pass GPU timings. **Does NOT take input, does NOT mutate `AppArgs`.** No conflict.
- No `*panel*.rs`, no `*gui*.rs`, no `dev_*.rs` files in `crates/bevy_naadf/src/`. Verified by file listing in stage 1.
- `e2e/checks.rs` etc are post-hoc validation, not a panel.

No duplication. The panel is new code, sitting alongside the HUD with a different role (`hud.rs` = monitor; panel = control).

### §R2. Layout audit — predicted offsets, walked manually

`GpuRenderParams` after edit:
- `screen_width` (0), `screen_height` (4), `frame_count` (8), `rand_counter` (12) — row 0
- `taa_index` (16), `flags` (20), **`max_ray_steps_primary` (24, was `_pad0a`)**, `_pad0b` (28) — row 1
- `sky_sun_dir` (32-44), `_pad1` (44) — row 2
- `sun_color` (48-60), `_pad2` (60) — row 3
- `taa_jitter` (64-72), `_pad3` (72) — row 4
- `bounding_box_min` (80-92), `_pad4` (92) — row 5
- `bounding_box_max` (96-108), `_pad5` (108) — row 6
- (struct end at 112)

The only change is the field rename at offset 24. WGSL still reads `params.max_ray_steps_primary` at offset 24 = byte-for-byte equivalent.

`GpuGiParams` after edit:
- inv_view_proj (0..64), view_proj (64..128) — matrices
- cam_pos_int+pad (128..144), cam_pos_frac+pad (144..160), sky_sun_dir+pad (160..176), sun_color+pad (176..192) — 4 vec4 rows
- screen_width..pad_d (192..304) — 28 × u32 scalar tail (existing — 24 originals + 4 added by Dispatch A: `sun_shadow_taps`, `_pad5`, `_pad6`, `_pad7`)
- **NEW** max_ray_steps_secondary (304), max_ray_steps_sun (308), max_ray_steps_sun_secondary (312), max_ray_steps_visibility (316) — row 20
- **NEW** spatial_iter_count (320), _pad8 (324), _pad9 (328), _pad10 (332) — row 21
- (struct end at 336)

All plain u32 — no `vec3`-then-scalar trap can fire here. Total = 336, divisible by 16 (alignment-friendly).

### §R3. Risk audit per Class-P promotion

Same as §9.7. Every promoted field gets a defensive `max(_, 1u)` clamp at WGSL use site:

| WGSL site | Existing call | New call |
|---|---|---|
| `naadf_first_hit.wgsl:180` | `shoot_ray(..., MAX_RAY_STEPS_PRIMARY, ...)` | `shoot_ray(..., i32(max(params.max_ray_steps_primary, 1u)), ...)` |
| `naadf_global_illum.wgsl:290` | `shoot_ray(..., MAX_RAY_STEPS_SECONDARY, ...)` | `shoot_ray(..., i32(max(gi_params.max_ray_steps_secondary, 1u)), ...)` |
| `naadf_global_illum.wgsl:374` | `shoot_ray(..., MAX_RAY_STEPS_SUN_SECONDARY, ...)` | `shoot_ray(..., i32(max(gi_params.max_ray_steps_sun_secondary, 1u)), ...)` |
| `spatial_resampling.wgsl:457` | `shoot_ray(..., MAX_RAY_STEPS_VISIBILITY, ...)` | `shoot_ray(..., i32(max(gi_params.max_ray_steps_visibility, 1u)), ...)` |
| `spatial_resampling.wgsl:556` | `shoot_ray(..., MAX_RAY_STEPS_SUN, ...)` | `shoot_ray(..., i32(max(gi_params.max_ray_steps_sun, 1u)), ...)` |
| `spatial_resampling.wgsl:622` | `sample_neighbors(_, 12u, _, _)` | `sample_neighbors(_, max(gi_params.spatial_iter_count, 1u), _, _)` |

Default-at-construction values:
- `lib.rs::GiSettings::default`: `max_ray_steps_secondary: 100, max_ray_steps_sun: 120, max_ray_steps_sun_secondary: 80, max_ray_steps_visibility: 60, spatial_iter_count: 12, max_ray_steps_primary: 120`.

(`max_ray_steps_primary` lives in `GiSettings` even though it ends up in `GpuRenderParams`; the upload site in `prepare_frame_gpu` / `prepare_render_params` reads it from `ExtractedGiConfig` and writes to `GpuRenderParams.max_ray_steps_primary`. Keeping all knobs centralized in `GiSettings` matches §6's "all promoted knobs match pre-dispatch behavior bit-for-bit" promise.)

### §R4. The big risk — `bevy_egui` deviation

The brief's chosen library does not work. I am replacing it with Bevy native UI. This is a substantial departure from the brief's expectation of a multi-section collapsing-header window with sliders + tooltips. The keyboard-driven discrete-step navigator is **functionally adequate** (all knobs reachable, all defaults restorable, all reads visible) but **structurally different** from the brief's envisioning.

**Recommendation: dispatch a fresh-eyes `delegate-reviewer` on the GUI library choice before this lands.** Specifically:
- Re-verify `bevy_egui` 0.39.1 cannot link against Bevy 0.19-rc.1 (try a sandbox build).
- Decide whether the keyboard-driven Bevy-native panel is acceptable, or whether to defer the dispatch entirely until `bevy_egui` 0.19 lands.

This is a HIGH-RISK escalation per the brief's protocol: I should not self-certify it.

### §R5. Lower risks

- **`GpuGiParams` size now 336** (was 304 pre-dispatch, 288 pre-Dispatch-A). Bigger but still within wgpu's 64 KiB uniform-buffer limit (336 ≪ 65536). Layout-trap-free per §R2.
- **The `GpuRenderParams._pad0a` repurpose** is a layout-preserving rename. Trivial; any reader of the field is in the same file. Verify the WGSL `render_pipeline_common.wgsl` declaration matches.
- **`naadf_first_hit.wgsl`'s `MAX_RAY_STEPS_PRIMARY` import** can stay — naga will DCE it if unused. To make the source clean, I'll drop the import.
- **Code reuse: `extract_gi_config` extends naturally** — it copies the whole `GiSettings` struct, so new fields ride along for free; no extract-system code edit.
- **The panel input system can collide with the FreeCamera input.** Mitigation: `Res<PanelState>.open` checked in `FreeCamera` movement systems. But that requires editing `FreeCameraPlugin`'s movement systems, which are upstream. Alternative: gate the panel-keybind events to consume input via `ButtonInput::reset_*` or by inserting a `BlockInput` resource. Simplest safe path: have the panel only listen to F1/Up/Down/Left/Right/PageUp/PageDn/Shift/R *while open*, AND don't block camera input. WASD-conflicts: navigation uses arrow keys, not WASD; R reset is a discrete key the camera doesn't use. Tested mentally — no input collision.

### §R6. Coverage re-check vs. brief

Brief lists 6 panel sections. Mapping to my §5 layout:
1. **Primary rays** → §5 "RAY STEP CAPS" section, knobs 1–5.
2. **Adaptive sampler** → §5 "GI" section, `skip_samples` row. (`mod_size` not exposed — §9.6 rationale.)
3. **GI reservoir** → §5 "SPATIAL RESAMPLING" section, knobs 6, 7, 15, 8.
4. **Denoiser** → §5 "GI" section: `is_denoise` + `denoise_thresh`. (kernel/σ are D-drop, see §2.3.)
5. **TAA** → §5 "DIAGNOSTICS" section: `taa_ring_depth` (D-show) + `camera_history_depth` (D-show). (No tunable in scope after `screenPosDistanceSqr` D-drop.)
6. **Read-only diagnostics** → §5 "DIAGNOSTICS" section.

All 6 sections present. Brief's specifically-named knobs (`MAX_RAY_STEPS_PRIMARY`, `MAX_RAY_STEPS_SUN`, `MAX_RAY_STEPS_SUN_SECONDARY` / `MAX_RAY_STEPS_BOUNCE` = my secondary, etc.) all present.

### §R7. Files-touched preview

To support stage 3 budgeting:

| File | Change | Lines (approx) |
|---|---|---|
| `crates/bevy_naadf/Cargo.toml` | (none — no new deps) | 0 |
| `crates/bevy_naadf/src/lib.rs` | `GiSettings` += 6 fields + their defaults | ~30 |
| `crates/bevy_naadf/src/render/gpu_types.rs` | `GpuRenderParams._pad0a` → `max_ray_steps_primary` (rename) + 8 new u32s in `GpuGiParams` + 2 new `offset_of!` guards + size assert bump 304→336 | ~30 |
| `crates/bevy_naadf/src/render/gi.rs` | `prepare_gi` write site += 8 new fields | ~10 |
| `crates/bevy_naadf/src/render/prepare.rs` | `prepare_render_params` write site += 1 field rename | ~3 |
| `crates/bevy_naadf/src/assets/shaders/gi_params.wgsl` | += 8 trailing u32 lanes | ~10 |
| `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl` | field rename | ~2 |
| `crates/bevy_naadf/src/assets/shaders/naadf_first_hit.wgsl` | 1 call-site edit + drop `MAX_RAY_STEPS_PRIMARY` import | ~3 |
| `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl` | 2 call-site edits | ~4 |
| `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` | 3 call-site edits | ~6 |
| `crates/bevy_naadf/src/panel.rs` | NEW module — state resource + systems + UI scaffolding | ~400 |
| `crates/bevy_naadf/src/lib.rs` (panel registration) | add `pub mod panel;` + plugin wiring under `cfg.add_hud` gate | ~10 |
| `crates/bevy_naadf/src/hud.rs` | one-line keybind hint append | ~2 |

Estimated total: ~500 lines of new/changed code. The `panel.rs` module is the bulk; everything else is mechanical.

### §R8. Verification plan

Per the brief's gates:
1. `cargo build --workspace` — must compile clean.
2. `cargo test -p bevy-naadf --lib` — must show **≥ 112 tests pass** (baseline). New tests for `GpuGiParams` size + the new `offset_of!` guards' runtime mirror are welcome but not required.
3. `cargo run --release --bin e2e_render` — must pass; luminance numbers should match baseline ε-tight (bit-equivalent default values §6).
4. `cargo run --release --bin e2e_render -- --entities` — same.

If any luminance differs by more than ε, that's a default-value or layout bug. Halt and report.

---

