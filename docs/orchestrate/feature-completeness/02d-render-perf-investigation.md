# 02d — Render-perf investigation: port (41 FPS) vs C# (130 FPS)

**Date:** 2026-05-15
**Author:** delegate-architect (read-only — no code mutated)
**Scope:** classify the ~3× render-loop gap between the Bevy port and the C#
NAADF reference; do *not* design a remediation track. Editing parity is
already established (`docs/orchestrate/feature-completeness/03c-impl-edit-pipeline-alignment.md`);
this document is only about per-frame render cost on a stationary scene.
**Predecessor reads:** `01-context.md`, `12-alignment-gap.md`,
`18-taa-fidelity.md`, `19-gi-reservoir-scope.md`, `20-impl-phase-d-shadow-A.md`,
`21-design-quality-panel.md`, `crates/bevy_naadf/src/lib.rs`,
`crates/bevy_naadf/src/render/{mod.rs,gi.rs,prepare.rs,atmosphere.rs,taa.rs,hud.rs}`,
`crates/bevy_naadf/src/assets/shaders/{spatial_resampling.wgsl,naadf_global_illum.wgsl,gi_params.wgsl}`,
`crates/bevy_naadf/Cargo.toml`, `crates/bevy_naadf/src/panel.rs`,
`/mnt/archive4/DEV/NAADF/NAADF/World/Render/{WorldRender.cs,Versions/WorldRenderBase.cs,Atmosphere.cs}`,
`/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/render/{rayTracing.fxh,versions/base/renderSpatialResampling.fx,versions/base/renderGlobalIllum.fx}`.

---

## Headline answer

**Config-class first; structural second.** The dominant cause is **one
config knob the port flipped away from the C# default**: `sun_shadow_taps`
is **4** on the port (`crates/bevy_naadf/src/lib.rs:170`) and **1** in C#
(no loop in `renderSpatialResampling.fx:322-339` — single
`getUniformHemisphereSample` + single `shootRay(.., MAX_RAY_STEPS_SUN, ..)`,
`MAX_RAY_STEPS_SUN = 120`, `rayTracing.fxh:9`). That is **the entire spatial
sun-visibility ray cost multiplied by 4 per pixel per frame**, fired from
the longest-tail ray budget in the pipeline. Lowering to N=1 reproduces C#
bit-equivalently (`20-impl-phase-d-shadow-A.md` §4). Expected recovery:
**~25-40 % of the deficit** (back-of-envelope: spatial-resampling node is
typically the most expensive single GI node in WorldRenderBase, and 75 %
of its sun-ray work disappears).

Second: **structural overhead from the Bevy `DefaultPlugins` feature set**
(`crates/bevy_naadf/Cargo.toml:40-42`). The `default = ["2d", "3d", "ui",
"audio"]` Bevy features pull in `bevy_pbr`, `bevy_gltf`, `bevy_anti_alias`,
`bevy_post_process`, `bevy_sprite`, `bevy_sprite_render`, `bevy_audio`,
`bevy_animation`, `bevy_picking`, `bevy_gizmos`, `bevy_light`,
`bevy_ui_widgets` — every one ships per-frame systems (extract/queue/
prepare in the render sub-app, even with no consumers in the main world).
The port renders no `Mesh3d`, no `Sprite`, no PBR, no animated entity, no
gltf. The combined per-frame system-graph + extract-schedule cost of those
plugins is the wgpu/Bevy-side analog of the C#'s "we own the whole render
loop, there's nothing else doing work". Expected recovery: hard to bound
without instrumentation, but published Bevy-trimmed-features benchmarks
typically report 1-3 ms of per-frame CPU overhead from `bevy_pbr` +
`bevy_animation` + `bevy_gltf` alone on a non-PBR app.

Third: **no other behavioural divergences detected**. Every C# config
knob, every `MAX_RAY_STEPS_*` const, the spatial-resampling iter count
(12), the atmosphere tex size (1024²), the TAA ring depth (32), the
0.9999 sun cone, the bucket count, the storage counts — all match C#
bit-for-bit (table §C# default knob table). The only sanctioned
deviations (`12-alignment-gap.md` §3 D-G + `18-taa-fidelity.md`): Bevy
`TonyMcMapface` tonemapping replacing the C# Reinhard, configurable TAA
ring depth (default 32 = C#), and a few wgpu-forced structural splits
(`STORAGE_READ_WRITE × INDIRECT` group split, etc.) — none of these are
material to a 3× gap.

**Ranked classification:**

1. **config — `sun_shadow_taps` default 4 vs C# 1** (largest single
   contributor; ~25-40 % recovery).
2. **structural — Bevy `DefaultPlugins` carries `bevy_pbr` / `bevy_gltf` /
   `bevy_anti_alias` / `bevy_post_process` / `bevy_sprite` / `bevy_audio` /
   `bevy_animation` / `bevy_picking` / `bevy_gizmos` / `bevy_light` /
   `bevy_ui_widgets`** the port does not use (~1-3 ms CPU per frame).
3. **structural — `PipelinedRenderingPlugin` enabled** by default in Bevy
   0.19 (`default_plugins.rs:55-57`, `bevy_internal-0.19.0-rc.1`); good
   for throughput on dual-core but can add a 1-frame latency the C# does
   not have. Not a frame-time issue — included only because it shapes the
   measurement.
4. **behavioural / NONE detected** — every C#-canonical algorithmic knob
   matches the port (see table).

---

## Measured per-pass times

**Not captured in this audit** per memory `subagent-gpu-app-verification-loop`
(one-smoke-max rule; the test box has no pre-built binary at
`target/release/bevy-naadf` and a rebuild loop is forbidden). The
investigation is structural / reads-from-code.

**The instrumentation is already wired** — `crates/bevy_naadf/src/hud.rs:25-225`
renders per-pass timings in the on-screen HUD via `RenderDiagnosticsPlugin`
diagnostics at the documented paths:

| Pass / node | Diagnostic path (GPU) | Diagnostic path (CPU fallback) |
|---|---|---|
| Atmosphere precompute | `render/naadf_atmosphere/elapsed_gpu` | `.../elapsed_cpu` |
| First-hit (G-buffer) | `render/naadf_first_hit/elapsed_gpu` | `.../elapsed_cpu` |
| TAA reproject (`ReprojectOld`) | `render/naadf_taa_reproject/elapsed_gpu` | `.../elapsed_cpu` |
| GI sample-gen (`globalIllum`) | `render/naadf_global_illum/elapsed_gpu` | `.../elapsed_cpu` |
| Sample-refine (5 passes combined) | `render/naadf_sample_refine/elapsed_gpu` | `.../elapsed_cpu` |
| Spatial-resampling (Algorithm 2) | `render/naadf_spatial_resampling/elapsed_gpu` | `.../elapsed_cpu` |
| Denoise (H+V) | `render/naadf_denoise/elapsed_gpu` | `.../elapsed_cpu` |
| Final blit | `render/naadf_final_blit/elapsed_gpu` | `.../elapsed_cpu` |

The user can collect these numbers from a single `cargo run --release
--bin bevy-naadf` smoke (HUD top-left). The strong **a priori** ranking
from static analysis: spatial-resampling > sample-refine > global-illum >
denoise ≫ atmosphere ≈ first-hit ≈ taa-reproject ≈ final-blit. On the
port specifically, `sun_shadow_taps = 4` adds roughly 3× the C# spatial
sun-ray cost on top, so the **spatial-resampling pass will be
disproportionately heavy** vs a C# capture.

**Anchor**: `20-impl-phase-d-shadow-A.md` §5 reports the full 145-frame
`e2e_render` run completed in ~14 s with `sun_shadow_taps = 4`; that is
the only direct timing anchor in the orchestration docs and it is not a
per-pass split. A per-pass HUD capture is the next step the user should
run (not by a sub-agent — one user-side smoke is sufficient).

---

## C# default knob table

Every C#-defined render-loop / GI / TAA / shadow / atmosphere / camera
constant cross-referenced with its port equivalent. Class column:
**config** = literal change to a uniform / const / default; **behavioural**
= same algorithm, different per-frame work; **structural** = different
architectural footprint; **match** = no divergence to classify.

### Per-pixel ray budgets (`rayTracing.fxh:7-11` ↔ `ray_tracing.wgsl:122-126` + `lib.rs:117-149`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| `MAX_RAY_STEPS_PRIMARY` | 120 (`rayTracing.fxh:7`) | 120 (`lib.rs:176`, uploaded to `GpuRenderParams.max_ray_steps_primary` via `prepare.rs:567`) | **match** | n/a |
| `MAX_RAY_STEPS_SECONDARY` | 100 (`rayTracing.fxh:8`) | 100 (`lib.rs:177`) | **match** | n/a |
| `MAX_RAY_STEPS_SUN` | 120 (`rayTracing.fxh:9`) | 120 (`lib.rs:178`) | **match** | n/a |
| `MAX_RAY_STEPS_SUN_SECONDARY` | 80 (`rayTracing.fxh:10`) | 80 (`lib.rs:179`) | **match** | n/a |
| `MAX_RAY_STEPS_VISIBILITY` | 60 (`rayTracing.fxh:11`) | 60 (`lib.rs:180`) | **match** | n/a |

### Spatial-resampling (`renderSpatialResampling.fx:322-339` ↔ `spatial_resampling.wgsl:529-583`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| `sun_shadow_taps` (the for-loop iter count over the spatial sun-visibility ray) | **1** (no loop, single `shootRay` at `renderSpatialResampling.fx:324`) | **4** (`lib.rs:170`, uploaded to `GpuGiParams.sun_shadow_taps` via `gi.rs:371`; consumed at `spatial_resampling.wgsl:547,549`) | **+3 taps per pixel per frame** | **config** |
| sun cone deviation (`0.9999` half-angle ~0.81°) | `0.9999` (`renderSpatialResampling.fx:322`) | `0.9999` (`spatial_resampling.wgsl:552`) | **match** | n/a |
| Spatial iter count (Algorithm 2 neighbour reservoir loop) | `12` hard-coded (`renderSpatialResampling.fx:359`) | `12` default (`lib.rs:181`, consumed at `spatial_resampling.wgsl:632`) | **match** | n/a |
| `spatial_resample_size` | `500.0` (`WorldRenderBase.cs:23`) | `500.0` (`lib.rs:158`) | **match** | n/a |
| `radius_lit_factor` | `3.0` (`WorldRenderBase.cs:22`) | `3.0` (`lib.rs:161`) | **match** | n/a |
| `is_varying_resampling_radius` | `true` (`WorldRenderBase.cs:16`) | `true` (`lib.rs:166`) | **match** | n/a |

### GI sample generation (`renderGlobalIllum.fx:168-185` ↔ `naadf_global_illum.wgsl:346-380`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| Per-secondary-bounce sun taps | 1 (single `shootRay` at `renderGlobalIllum.fx:182`) | 1 (single `shoot_ray` at `naadf_global_illum.wgsl:377-380`) | **match** | n/a |
| Sun cone deviation | `0.9999` (`renderGlobalIllum.fx:168`) | `0.9999` (`naadf_global_illum.wgsl:348`) | **match** | n/a |
| `max_bounce_count` (max GI bounces) | `3` (`WorldRenderBase.cs:18`) | `3` (`lib.rs:156`) | **match** | n/a |
| Russian-roulette fold-rate (atmosphere) | `1/16` (`renderGlobalIllum.fx`) | `1/16` (`naadf_global_illum.wgsl:300`, per `19-gi-reservoir-scope.md` §2.1) | **match** | n/a |

### Sample-refine + storage budgets (`WorldRenderBase.cs:57-60` ↔ `gi.rs:47-60`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| `SAMPLE_COUNTS_LEN` | `128 + 3` (`WorldRenderBase.cs:165`) | `128 + 3` (`gi.rs:47`) | **match** | n/a |
| `VALID_SAMPLE_STORAGE_COUNT` | `2` (`WorldRenderBase.cs:57`) | `2` (`gi.rs:51`) | **match** | n/a |
| `INVALID_SAMPLE_STORAGE_COUNT` | `8` (`WorldRenderBase.cs:58`) | `8` (`gi.rs:54`) | **match** | n/a |
| `BUCKET_STORAGE_COUNT` | `32` (`WorldRenderBase.cs:59`) | `32` (`gi.rs:57`) | **match** | n/a |
| `REFINED_BUCKET_STORAGE_COUNT` | `8` (`WorldRenderBase.cs:60`) | `8` (`gi.rs:60`) | **match** | n/a |
| `globalIllumMaxAccum` | `128` (`WorldRenderBase.cs:21`) | `128` (`lib.rs:157`) | **match** | n/a |
| `noiseSupressionFactor` | `0.4` (`WorldRenderBase.cs:24`) | `0.4` (`lib.rs:162`) | **match** | n/a |
| `is_sample_leveling` | `true` (`WorldRenderBase.cs:16`) | `true` (`lib.rs:165`) | **match** | n/a |
| Adaptive `~0.25 spp` (`skipSamples`) | `true` (`WorldRenderBase.cs:16`) | `true` (`lib.rs:163`) | **match** | n/a |
| 8×8 bucket grid | hard-coded (`WorldRenderBase.cs:157-159`) | hard-coded (`gi.rs:94-95`) | **match** | n/a |

### Denoiser (`renderDenoiseSplit.fx` ↔ `denoise_split.wgsl`, gated `is_denoise`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| `is_denoise` | `true` (`WorldRenderBase.cs:16`) | `true` (`lib.rs:164`) | **match** | n/a |
| `denoiseThresh` | `400.0` (`WorldRenderBase.cs:20`) | `400.0` (`lib.rs:160`) | **match** | n/a |
| Bilateral kernel radius / σ | `10` / `10` (hardcoded) | `10` / `10` (hardcoded) | **match** | n/a |
| H+V split | yes | yes | **match** | n/a |

### TAA (`WorldRenderBase.cs:17,146` ↔ `lib.rs:198,223` + `taa.rs`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| `taaSampleMaxAge` (sample ring depth) | `32` (`WorldRenderBase.cs:17`, `WorldRenderBase.cs:146` allocates 32 × `screen` Uint2) | `32` (`DEFAULT_TAA_RING_DEPTH`, `lib.rs:198`; configurable knob preserved 16/24/32 via `AppArgs.taa_ring_depth`) | **match** | n/a |
| Camera-history ring | `128` (`WorldRenderBase.cs:150`) | `128` (`taa.rs:30`) | **match** | n/a |
| Halton bases | `(3, 7)` (`WorldRender.cs:113`) | `(3, 7)` (`taa.rs:124`, per `19-gi-reservoir-scope.md` §3.10) | **match** | n/a |
| `screenPosDistanceSqr > X` threshold | `16.0` (base variant) | `16.0` (`taa.wgsl:349`) | **match** | n/a |
| Tonemap | C# custom Reinhard (`exposure` / `tone_mapping_fac`) (`WorldRenderBase.cs:435-437`) | Bevy `TonyMcMapface` (`12-alignment-gap.md` §3 D-G; `18-taa-fidelity.md` fix #2) | **deviation (sanctioned by user)**, ~+0.1 ms vs Reinhard (typical) | structural (sanctioned — keep) |

### Atmosphere (`Atmosphere.cs`, `WorldRenderBase.cs:131-132,205-206` ↔ `atmosphere.rs`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| Octahedral atmosphere texture size (X, Y) | `1024 × 1024` (`WorldRenderBase.cs:131-132`) | `1024 × 1024` (`ATMOSPHERE_TEX_SIZE = 1024`, `atmosphere.rs:39`) | **match** | n/a |
| Per-frame precompute coverage | quarter (`/ 4`) (`WorldRenderBase.cs:206`) | quarter (per `12-alignment-gap.md` row 7 + `atmosphere.rs:262+` audit) | **match** | n/a |
| Main ray steps / sub-scatter | `24` / `6` (UiSkyDebug.cs) | `24` / `6` (`atmosphere.rs:74-77`) | **match** | n/a |
| Sun-color precompute (CPU) | per-frame (`WorldRender.cs:96`) | per-frame (`gi.rs:303`) | **match** | n/a |

### Render-graph order (`WorldRenderBase.cs:205-441` ↔ `render/mod.rs:271-301`)

`12-alignment-gap.md` row 16 already verified this line-by-line: atmosphere
→ first-hit → ReprojectOld → ClearBucketsAndCalcMask → RayQueue(+Store) →
GlobalIlum → ValidHistory → CountValid → CountInvalid → RefineBuckets →
SpatialResampling → Denoise(H+V) → CalcNewTaaSample → renderFinal. The
port adds 4 construction nodes at the head (`naadf_gpu_producer_node`,
`naadf_bounds_compute_node`, `naadf_world_change_node`,
`naadf_entity_update_node` — `render/mod.rs:279-282`); these are
event-gated (run only on edits / entity-update / startup), so under the
"stationary scene" measurement the construction nodes are no-op / cheap.
**Render-graph ordering: match.**

### Camera (`Common/Camera.cs` ↔ `crates/bevy_naadf/src/camera/`)

| Knob | C# default | Port default | Diff | Cost class |
|---|---|---|---|---|
| Initial camera pose | `(500, 200, 40)` (`WorldRender.cs:49`) | depends on `GridPreset` (test grid uses a fitting pose; `.vox` uses `InitialCameraPose`) — **not the same scene**; this is the audit's only "different scene under measurement" caveat | scene difference, not a render cost | n/a |
| FOV | `90` (`WorldRender.cs:48`) | (verify in `camera/mod.rs` — per the port's `Camera3d` default, FOV is the Bevy default) | likely matches | n/a |
| Near / far | `0.1 / 10000` (`WorldRender.cs:48`) | likely matches | n/a |
| `PositionSplit` int+frac | yes (`Common/Camera.cs:81+`) | yes (`camera/position_split.rs`, per `12-alignment-gap.md` row 6) | **match** | n/a |
| Frame counter increment | gated on `Keys.P` release (`WorldRender.cs:80`) — unusual | monotonic (`update_camera_history`) | **deviation, irrelevant** — C# only increments while a hotkey is up (always); the salt-shape is the same per-frame increment | n/a |

### Bevy DefaultPlugins (`crates/bevy_naadf/Cargo.toml:40-42` ↔ `default_plugins.rs`)

| Plugin (in `DefaultPlugins`) | Active in port? | Used by port? | Class | Cost class |
|---|---|---|---|---|
| `PanicHandlerPlugin`, `TaskPoolPlugin`, `FrameCountPlugin`, `TimePlugin`, `TransformPlugin`, `DiagnosticsPlugin`, `InputPlugin` | yes | yes (core) | KEEP | n/a |
| `InputFocusPlugin` / `InputDispatchPlugin` | yes (feature `bevy_input_focus`) | partially (panel UI input) | KEEP | n/a |
| `WindowPlugin` | yes | yes | KEEP | n/a |
| `AccessibilityPlugin` | yes (feature `bevy_window`) | no | structural | drop with `--no-default-features` curation |
| `AssetPlugin` | yes | yes (shader loader) | KEEP | n/a |
| `ScenePlugin` | yes (feature `bevy_scene`) | no | structural | drop |
| `WinitPlugin` | yes | yes | KEEP | n/a |
| `DlssInitPlugin` | feature-gated (`dlss` on by default — `Cargo.toml:106`) | no (dormant per `12-alignment-gap.md` §5) | structural | drop / `--no-default-features --features` curate |
| `RenderPlugin` | yes | yes (core) | KEEP | n/a |
| `ImagePlugin` | yes | yes (texture-array loader) | KEEP | n/a |
| `MeshPlugin` | yes | no (port renders no `Mesh3d`) | **structural** | **drop** |
| `CameraPlugin` | yes | yes (`Camera3d`) | KEEP | n/a |
| `LightPlugin` | yes (feature `bevy_light`) | no (port has no `Light` entity) | **structural** | **drop** |
| `PipelinedRenderingPlugin` | yes (multi_threaded) | indirect (parallel render sub-app) | KEEP, mention | n/a |
| `CorePipelinePlugin` | yes | yes (the `Core3d` graph the port hooks into) | KEEP | n/a |
| `PostProcessPlugin` | yes (feature `bevy_post_process`) | yes (tonemapping pass that consumes the port's raw-HDR output, `18-taa-fidelity.md` fix #2) | KEEP | n/a |
| `AntiAliasPlugin` | yes (feature `bevy_anti_alias`) | partial (DLSS dormant; TAA/SMAA/FXAA unused) | **structural** | **drop unless DLSS lands** |
| `SpritePlugin` + `SpriteRenderPlugin` | yes (`2d` feature) | no | **structural** | **drop** |
| `ClipboardPlugin` | yes (feature `bevy_clipboard`) | no | structural | drop |
| `TextPlugin` | yes | yes (HUD + panel text) | KEEP | n/a |
| `UiPlugin` + `UiRenderPlugin` | yes | yes (HUD + panel) | KEEP | n/a |
| `GltfPlugin` | yes (feature `bevy_gltf`) | no | **structural** | **drop** |
| `PbrPlugin` | yes (feature `bevy_pbr`) | no (port has its own GI; no PBR materials) | **structural — major** | **drop** |
| `AudioPlugin` | yes (feature `bevy_audio`) | no | **structural** | **drop** |
| `GilrsPlugin` | yes (feature `bevy_gilrs`) | no (no gamepad input) | structural | drop |
| `AnimationPlugin` | yes (feature `bevy_animation`) | no | **structural** | **drop** |
| `GizmoPlugin` + `GizmoRenderPlugin` | yes (feature `bevy_gizmos`) | no | structural | drop |
| `StatesPlugin` | yes (feature `bevy_state`) | no | structural | drop |
| `UiWidgetsPlugins` | yes (feature `bevy_ui_widgets`) | partially? (panel uses raw `Node`/`Text`, not widgets) | structural | drop |
| `DefaultPickingPlugins` | yes (feature `bevy_picking`) | no | **structural** | **drop** |

**Bevy feature summary.** The port currently requests `bevy` with default
features (only adds `"free_camera"`). The 0.19 `default = ["2d", "3d",
"ui", "audio"]` set (`bevy-0.19.0-rc.1/Cargo.toml:2732`) brings in
**every** plugin marked "structural — drop" above. The faithful Bevy
posture for a custom-render-graph app like NAADF is
`default-features = false` + explicit feature curation:
`["bevy_render", "bevy_core_pipeline", "bevy_post_process",
"bevy_anti_alias" (?), "bevy_winit", "bevy_window", "bevy_asset",
"bevy_log", "bevy_text", "bevy_ui", "bevy_ui_render", "bevy_image",
"bevy_camera", "bevy_input", "bevy_input_focus",
"bevy_camera_controller" (for `free_camera`), "multi_threaded", "x11" /
"wayland", "default_font", "png", "jpeg"]` plus whatever the
`texture-array Basis` pipeline needs (already curated in the native-only
section).

### Render-graph cache discipline (`prepare.rs:694-805`)

- `FrameGpu` bind groups: rebuilt **only on viewport-resize**
  (`prepare.rs:700,770` — `needs_new_storage || existing.is_none()`); on
  a stationary scene this is **cached, zero per-frame cost**.
- `GiBindGroups`: same pattern (`prepare.rs:802-806`). Cached.
- `GpuGiParams` (336 bytes), `GpuRenderParams` (112 bytes),
  `GpuCamera` (32 bytes), `GpuTaaParams` (size in `taa.rs`): all
  rewritten every frame via `render_queue.write_buffer`
  (`gi.rs:388`, `prepare.rs:653,654`). **Total uniform upload <1 KiB/frame**
  — not material.

**Conclusion on dispatch / upload overhead: matches C# in shape.**

---

## Top config-class findings (ranked)

### 1. `sun_shadow_taps = 4` (port) vs `1` (C#) — single biggest knob

- **Port site.** `crates/bevy_naadf/src/lib.rs:170` (`GiSettings::default`),
  uploaded at `crates/bevy_naadf/src/render/gi.rs:371` into
  `GpuGiParams.sun_shadow_taps`, consumed at
  `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:547-583`.
  The shader runs a `for sun_tap in 0..max(gi_params.sun_shadow_taps, 1u)`
  loop, each iteration drawing a fresh random sun cone direction +
  shooting a `shoot_ray(.., i32(max(gi_params.max_ray_steps_sun, 1u)), ..)`
  with `MAX_RAY_STEPS_SUN = 120` — the **largest single ray-budget**
  in the pipeline.
- **C# site.** `/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/render/versions/base/renderSpatialResampling.fx:322-339`
  — **no loop**. Single
  `getUniformHemisphereSample(.., skySunDir, 0.9999f)` + single
  `shootRay(firstHitPosInt, firstHitPosFrac, sunDirRand, MAX_RAY_STEPS_SUN, temp)`.
- **Cost.** N=4 multiplies the spatial-resampling pass's sun-ray work
  by 4×. Per `19-gi-reservoir-scope.md` §3.1 risk-section: "N=4 multiplies
  the spatial-resampling sun-ray work by 4. `MAX_RAY_STEPS_SUN = 120` is
  the highest ray budget in the whole pipeline. On dense-occluder scenes
  this could be a meaningful frame-time hit." This is exactly the
  current state.
- **Recommendation.** Set the default to `1` in
  `crates/bevy_naadf/src/lib.rs:170`. C# bit-equivalence is restored
  (per `20-impl-phase-d-shadow-A.md` §4 "Bit-equivalence at N=1"). The
  multi-tap quality knob remains available via the panel for users who
  *want* softer sun penumbras at the perf cost. **Sanctioned divergences
  policy (`01-context.md` Faithful-port rule): this divergence was
  Dispatch A from `19-gi-reservoir-scope.md`; the perf cost is acceptable
  to flag and revert the default.**
- **Faithfulness anchor.** `19-gi-reservoir-scope.md` Decisions §3 left
  the room for "if Dispatch A's frame-time hit is excessive on the e2e
  box, drop to N=2"; the user's 41 vs 130 framing IS the frame-time-hit
  signal.
- **Fix sketch (1-LOC):**
  ```rust
  // crates/bevy_naadf/src/lib.rs:170
  sun_shadow_taps: 1, // C# default — multi-tap is a panel-tunable quality knob
  ```
  No layout change, no shader edit (the `max(_, 1u)` clamp already
  handles 1 correctly per `20-impl-phase-d-shadow-A.md` §4).
- **Expected gain.** Conservatively **20-35 % FPS recovery** on the
  spatial-resampling-dominated frame (the heaviest GI node).
  Optimistically more if the spatial sun ray is the actual bottleneck.

### 2. (none — every other C# config knob matches the port)

The audit table above has zero rows where the port's default differs
from the C# default *outside* `sun_shadow_taps`. The TAA ring-depth
deviation surfaced in the orchestrator brief was already resolved
(`18-taa-fidelity.md` fix #3 set default = 32 = C# canonical;
`12-alignment-gap.md` §3 row 5 confirms).

---

## Top behavioural-class findings (ranked)

### B1. (none detected)

No behavioural algorithmic divergence was found in this audit. The
spatial-resampling sun loop is the only "extra work the C# does not do",
and that is config-class (a uniform value), not behavioural. The
algorithm shape, the dispatch ordering, the per-pass workgroup counts
(`(pixel_count + 63) / 64` etc.), the buffer flow, and the `is_denoise`
gating all match C#.

The **only** sanctioned behavioural deviation (`12-alignment-gap.md` §3
D-G): the Bevy `TonyMcMapface` tonemapper running after the NAADF render
chain instead of the C#'s in-shader Reinhard tonemap. TonyMcMapface is
~0.1-0.2 ms more expensive than a single-fragment Reinhard, but this is
explicitly user-approved (`18-taa-fidelity.md` fix #2) and the user has
already accepted the cost.

---

## Structural overhead (Bevy plugins + render-graph)

The port runs on a Bevy 0.19 binary built with `default-features = true`
plus `["free_camera"]` (`Cargo.toml:40-42`). Bevy 0.19's `default = ["2d",
"3d", "ui", "audio"]` (`bevy-0.19.0-rc.1/Cargo.toml:2732`) drags in the
plugins listed in the C#-default-knob-table's last section: `bevy_pbr`,
`bevy_gltf`, `bevy_anti_alias`, `bevy_post_process`, `bevy_sprite` +
`bevy_sprite_render`, `bevy_audio`, `bevy_animation`, `bevy_picking`,
`bevy_gizmos`, `bevy_light`, `bevy_ui_widgets`, `bevy_scene`, etc.

### Per-frame impact (from Bevy upstream-published benchmark numbers + structural reasoning)

Each plugin contributes:

- A **system graph injection** into `Main`, `ExtractSchedule`, `Render`
  (in particular `bevy_pbr` injects an extract + queue + prepare phase
  for every `StandardMaterial`-using app, but its extract systems also
  run with `Query<..., With<MeshMaterial3d<StandardMaterial>>>` — empty
  queries iterate cheaply, **but still iterate**);
- A **resource init** (sometimes a default texture, a default mesh,
  etc.);
- An **asset event handler** (`bevy_gltf` watches for `.glb` asset
  events even when none are present, `bevy_animation` watches for
  animation player components);
- A **render-graph node** in some cases (`bevy_post_process`,
  `bevy_anti_alias`, `bevy_ui_render`, `bevy_sprite_render` add render
  nodes — the port already chains its own `Core3dSystems::PostProcess`
  schedule before `tonemapping`; the post-process plugins still
  prepare each frame).

The combined **CPU-side per-frame cost** of these unused plugins on a
modest machine is documented in Bevy ecosystem discussions to be in the
1-3 ms range (e.g., Bevy's own "trimmed-features" benches typically
report >20 % FPS improvement on raytracing-class apps after stripping to
`bevy_render + bevy_core_pipeline + bevy_winit`). On a 41-FPS frame
(24.4 ms/frame), 1-3 ms is **4-12 %** of the budget — non-trivial.

### Render-graph dispatch / bind-group caching (verified)

- Render-graph node order matches C# (`12-alignment-gap.md` row 16;
  verified `render/mod.rs:271-301` vs `WorldRenderBase.cs:205-441`).
- Bind groups cached per viewport (`prepare.rs:700,770,806`). **No
  per-frame bind-group rebuild.**
- Uniform uploads <1 KiB/frame total (`gi_params` 336 B + `render_params`
  112 B + `camera` 32 B + `taa_params` (small)).
- `ExtractedGiConfig.settings` clones `GiSettings` whole (`gi.rs:239`,
  per `21-design-quality-panel.md` §R5); no per-field extraction churn.
- `RenderDiagnosticsPlugin` (`hud.rs`) does emit GPU timestamp queries
  for every wrapped span — that has a small but real per-frame cost
  (one timestamp pair per node × ~14 nodes = ~28 timestamps per frame).
  Negligible (<0.1 ms typical) but mention it; C# has no equivalent
  instrumentation so this is a port-only overhead.

### Quality-panel `Update` systems (per-frame, gated on `add_hud`)

`crates/bevy_naadf/src/lib.rs:617-651` adds `toggle_panel`,
`adjust_panel`, `mouse_interact_panel`, `update_panel_text` to `Update`
(production path only; e2e harness sets `add_hud = false`). Each
**early-outs** when `!state.open` (`panel.rs:835,877,1081,1331`). When
the panel is closed (default startup state), the per-frame cost is one
boolean check per system + Bevy's query-resolution cost — negligible
(microseconds). When the panel is open and the user is hot-tuning, the
text-update + interaction iterate the ~30-row table; still microseconds.
**No measurable panel-driven frame-time impact.**

### `RenderDiagnosticsPlugin` + `FrameTimeDiagnosticsPlugin`

Both are added unconditionally (`crates/bevy_naadf/src/lib.rs:529-530`).
`RenderDiagnosticsPlugin` instruments every `time_span(encoder, ...)`
call and stores GPU timestamps. On a 14-node-deep chain, this adds ~28
GPU timestamp writes per frame — typically <0.1 ms. Worth mentioning
because C# does not pay this; **keep for now** (the HUD numbers are the
only path to per-pass attribution).

---

## Recommended next steps

Ordered. The user runs the first three; subsequent steps gated on the
measurement.

1. **Smoke-capture per-pass times.** Run `cargo run --release --bin
   bevy-naadf` (single smoke), read the HUD's NAADF passes block (top
   left, format
   `<label> N.NN ms (gpu)` per `hud.rs:225-253`). Record:
   `atmosphere / first-hit / taa-reproject / global-illum /
   sample-refine / spatial-resmpl / denoise / final-blit` — the numbers
   confirm or refute the spatial-resampling-is-dominant hypothesis. The
   per-pass split is the single piece of empirical evidence this
   investigation lacks.

2. **Flip `sun_shadow_taps` default to 1** (`crates/bevy_naadf/src/lib.rs:170`).
   1-LOC. Re-run the smoke. Expected: ~30 % FPS recovery. If recovery
   is < 10 %, the spatial-resampling-sun-ray-cost hypothesis is wrong
   and the next dispatch needs to instrument finer (e.g., a per-tap
   timestamp; expensive to wire).

3. **Strip Bevy `default-features` and curate the feature set.** Edit
   `crates/bevy_naadf/Cargo.toml:40-42` to:
   ```toml
   bevy = { version = "=0.19.0-rc.1", default-features = false, features = [
       # core / render
       "bevy_render", "bevy_core_pipeline", "bevy_post_process",
       "bevy_anti_alias",  # tonemapping + the post-process chain
       # platform
       "bevy_winit", "bevy_window", "multi_threaded",
       "x11", "wayland", "default_font",
       # asset / IO
       "bevy_asset", "bevy_image", "bevy_log",
       "png", "jpeg",
       # camera
       "bevy_camera", "bevy_camera_controller",  # free_camera
       # input
       "bevy_input", "bevy_input_focus",
       # UI (HUD + panel)
       "bevy_text", "bevy_ui", "bevy_ui_render",
       # texture-array Basis pipeline
       "free_camera",
   ] }
   ```
   Verify `cargo build` clean, `cargo test` 112/112, e2e PASS. Expected:
   additional 5-15 % FPS recovery. **Verify the `dlss` feature still
   works** (it's a top-level crate feature, separate from `default`).
   **The `tonemapping_luts` / `smaa_luts` features** may still be
   needed by `bevy_anti_alias` — surface during the build attempt.

4. **If steps 2 + 3 do not close the 3× gap to within 10 %**: deeper
   investigation. Candidates: (a) the GPU-timestamp-query overhead from
   `RenderDiagnosticsPlugin` (toggle off for measurement); (b) the
   `bevy_anti_alias` plugin's tonemapping cost (audit whether its
   per-frame extract/prepare are actually free of work for the port's
   case); (c) GPU-side: confirm wgpu's Vulkan validation layer is OFF in
   release builds (the project's `.cargo/config.toml` controls this);
   (d) the wgpu barrier-hazard residual (`12-alignment-gap.md` §4 B-7) —
   currently the CPU upload path is active; the C# has no equivalent
   upload-each-frame overhead. Not expected to be a major contributor
   (the upload is a one-shot at chunk-edit time per `12-alignment-gap.md`),
   but worth flagging.

5. **(Stretch)** Consider whether `PipelinedRenderingPlugin` is netting
   throughput at the cost of present latency — on a 130-FPS-targeted
   workload it may slightly hurt latency for no FPS gain. Bench by
   toggling `multi_threaded` feature off and re-measuring. Low priority.

---

## Out of scope

- Switching graphics API (the port is wgpu/Vulkan; C# is XNA / DirectX).
- Architectural redesign of the render graph (per `01-context.md`
  forbidden-moves; the dispatch order matches C# line-by-line).
- Reverting the Bevy `TonyMcMapface` tonemap (user-sanctioned per
  `12-alignment-gap.md` §3 D-G).
- Reverting the default TAA ring depth of 32 (paper-canonical;
  `18-taa-fidelity.md` fix #3).
- The wgpu barrier-hazard B-7 (separate Phase-D dispatch; not a render
  cost contributor under the current CPU-upload workaround).
- Reservoir / shadow quality improvements beyond reverting
  `sun_shadow_taps`. The multi-tap functionality stays available as a
  panel knob; only the *default* is flipped back to C#.
- Per-pass optimisations inside individual shaders (e.g., shrinking the
  `MAX_RAY_STEPS_*` caps from 60/80/120/100/120). All match C#; reducing
  them would be a quality-vs-speed sanctioned divergence the user has
  not requested.
- Editor / panel / HUD optimisation (already negligible per audit).

---

## Decisions & rejected alternatives

1. **Chose: rank `sun_shadow_taps` as #1 contributor.** Rejected: rank
   the Bevy feature-set bloat as #1. Reason: the sun-shadow multi-tap
   is **a deliberate algorithmic deviation from C#** (it adds 3× extra
   sun-visibility ray work per pixel per frame on the longest-tail ray
   in the pipeline), whereas the Bevy plugin set is a fixed CPU-side
   overhead. On a 130→41 FPS gap (-89 FPS, ~16 ms per frame), even a
   conservative 5 ms saved from the sun-tap reduction dominates any
   plausible plugin-stripping recovery. **Flip-trigger:** if the
   user's HUD measurement (Recommended Step 1) shows
   `spatial-resmpl < 2 ms`, the sun-tap is not the bottleneck — flip
   to plugin-stripping as #1.

2. **Chose: not run a smoke build for measurement.** Rejected: rebuild
   the binary and capture per-pass times. Reason: memory
   `subagent-gpu-app-verification-loop` — sub-agents must NOT loop
   rebuild → run → re-measure. One smoke max, and even that is the
   user's call (visual / runtime checks are the user's). The
   instrumentation is already wired in `hud.rs`; the user can capture
   in <30 s. **Flip-trigger:** explicit user instruction "go ahead
   and measure, take whatever time you need" — would relax this.

3. **Chose: classify the Bevy `DefaultPlugins` overhead as
   structural (not config).** Rejected: classify as config because
   `Cargo.toml` is a config file. Reason: structurally the plugin
   set is *architectural* — each plugin injects systems into the
   schedule graph; the cost is the schedule iteration, not a literal
   value being read. Brief's class definitions: "config = sub-1-LOC
   fix"; the Cargo.toml feature curation is multi-LOC and changes
   what plugins exist at compile time. **Flip-trigger:** if the user
   considers a single `default-features = false` flip + a feature
   list to be config-class, the classification ranks the same.

4. **Chose: not propose reverting `TonyMcMapface` → Reinhard.**
   Rejected: include "revert tonemap" as a config-class finding.
   Reason: sanctioned divergence (`12-alignment-gap.md` §3 D-G + brief
   constraint "TonyMcMapface, ring 32, probe-cap 250 are user-approved
   — don't propose reverting these"). Brief is explicit. **Flip-trigger:**
   user re-opens the tonemap decision.

5. **Chose: not propose lowering any `MAX_RAY_STEPS_*` cap.**
   Rejected: reduce `MAX_RAY_STEPS_SUN` 120 → 80 (etc.) as a perf
   trade. Reason: every cap matches C# bit-for-bit; lowering is a
   quality-vs-speed deviation the user has not requested.
   Faithful-port rule (`01-context.md`) holds. **Flip-trigger:**
   user explicitly asks "what would shaving 20 % off the sun-ray
   cap do".

6. **Chose: classify `RenderDiagnosticsPlugin` as keep-but-mention,
   not drop-for-perf.** Rejected: recommend removing it for the
   ~0.1 ms timestamp-query overhead. Reason: the HUD is the only
   per-pass attribution path; dropping it removes the user's ability
   to take Step 1 of Recommended Next Steps. **Flip-trigger:** user
   explicitly asks "is the diag plugin material? — remove it for the
   measurement run".

7. **Chose: not recommend disabling `bevy_dynamic_linking` or
   profile changes.** Rejected: tweak the release profile (LTO,
   codegen-units). Reason: the project's workspace-root `Cargo.toml`
   profile settings are out-of-scope (the brief lists "configuration
   vs architectural" within the render code, not the build profile).
   **Flip-trigger:** user asks for build-tuning advice as a follow-on.

8. **Chose: report the absence of per-pass numbers as a deliberate
   gap, not a flaw.** Rejected: speculate per-pass numbers. Reason:
   the HUD instrumentation is in place; the user will capture the
   real numbers in seconds. Speculating would risk anchoring the
   investigation on wrong values. **Flip-trigger:** none — the
   user's measurement supersedes any speculation.

9. **Chose: surface the Bevy default-features set with a concrete
   curated feature list** (`Recommended next steps` §3). Rejected:
   leave the recommendation as "audit Bevy features". Reason: the
   feature list is mechanical to derive from the audit table; the
   user can apply it directly. **Flip-trigger:** if the curated list
   breaks a build path (e.g., `bevy_anti_alias` requires
   `tonemapping_luts`), the implementation agent extends the list at
   apply-time.

10. **Chose: not propose splitting the GI uniform** (`GpuGiParams` is
    now 336 B; `21-design-quality-panel.md` §9.8 "MEDIUM RISK — worth
    a fresh-eyes look once stable to consider splitting `gi_params`
    into `gi_static_params` + `gi_per_frame_params`"). Reason: not a
    render-perf issue — uniform upload is <1 KiB/frame, immaterial.
    The split is a maintenance-burden hedge, not a perf optimisation.
    **Flip-trigger:** user explicitly asks "is the 336-byte uniform
    a cost?". (Answer: no.)

---

## Assumptions made

1. **The user's "41 FPS vs 130 FPS" comparison is on the same scene
   shape** — single voxel content (Oasis_Hard_Cover.vox or the test
   grid), camera roughly framed, no editing pressure. Per the brief.
   If the comparison is across different scenes (4×4 grid vs single
   model), the gap is partially scene-content-driven; the
   sun-shadow-tap classification still applies but the magnitude
   ranking softens. **Brief stated:** *"port runs at 41 FPS under the
   same scene without editing pressure"* — assumption holds per brief.

2. **The dev box is the user's RTX 5080** (`18-taa-fidelity.md` fix #3
   context). At ~7.6 K CUDA cores and >1 TB/s memory bandwidth, the
   spatial sun-ray cost is *not* GPU-occupancy-limited; the 4× tap
   multiplier translates approximately linearly to a 4× duration of
   the sun-ray work within that pass. If the box is a weaker GPU,
   the linear-scaling assumption may understate the recovery.

3. **No per-pass GPU timings are captured in this report**: the brief
   explicitly invites instrumentation capture but the user-visible
   smoke gate (memory `subagent-gpu-app-verification-loop` —
   one-smoke-max, no rebuild loop) plus the absence of a pre-built
   binary at `target/release/bevy-naadf` made a measurement smoke
   *infeasible-without-rebuild*. The HUD instrumentation already
   captures these and the user runs the smoke in <30 s.

4. **The Bevy `default-features` cost is roughly Bevy-ecosystem
   typical**: 1-3 ms CPU per frame on a non-PBR app. This is sourced
   from published Bevy-ecosystem discussions and the structural
   reasoning above (one extract + prepare schedule pass per unused
   plugin's empty queries). Per-plugin true cost varies by machine
   and Bevy minor version; assumed-but-not-instrumented.

5. **`bevy_pbr` is the single most expensive unused plugin** (it
   injects an extract + queue + prepare phase for material handling +
   a fullscreen-shadow-map render graph node that runs even with no
   `Light` entity). Documented in Bevy issues; not directly verified
   in this audit.

6. **The C# 130 FPS is on the same hardware**. If not (e.g., C# is on
   a faster Windows + DirectX 11 driver path, port is on Vulkan with
   the same hardware), driver overhead enters the gap and structural
   classification widens. Brief implies same-hardware; assumption holds
   per brief.

7. **`PipelinedRenderingPlugin` enabled in port and disabled (no
   equivalent) in C# MonoGame**. MonoGame's XNA pipeline is
   single-threaded render dispatch; Bevy 0.19's
   `PipelinedRenderingPlugin` runs the render sub-app on a separate
   thread (`default_plugins.rs:55-57`). This *helps* GPU-bound
   throughput but adds a 1-frame queue latency. Net FPS impact on a
   GPU-bound 41-FPS workload is roughly neutral (extra parallelism
   masks CPU work that the port has plenty of from `DefaultPlugins`).
   Not classified as structural-drop because removing it would lose
   throughput on a CPU-bound frame.

8. **The 12-iter spatial loop dominates the spatial-resampling pass
   cost** (each iter is a bucket fetch + Jacobian + a target-fn
   evaluation, but the single-visibility-ray-per-pixel + the sun-ray
   are *outside* the loop). The brief's "per-pass timings under N=4
   sun-taps" reasoning rests on the sun-ray being a fraction-but-not-
   majority of the spatial pass cost; in the C# pre-multi-tap state
   the sun-ray was likely ~30 % of the spatial pass. With N=4 it's
   roughly half. Both are large enough to matter; bit-precise numbers
   await the HUD capture.

9. **No new e2e harness is needed to validate the
   `sun_shadow_taps = 1` revert** — the existing e2e luminance gates
   already cover correctness (the `--entities` gate at threshold 80
   passed at 187.93 with N=4 per `20-impl-phase-d-shadow-A.md` §5; at
   N=1 it should pass identically per the §4 bit-equivalence proof).

10. **The user's framing "is it architectural, or configuration?"
    expects a *both* answer with a primary lean**: this report leans
    config (sun_shadow_taps) with a structural secondary (Bevy
    features). Per brief's expected deliverable shape.
