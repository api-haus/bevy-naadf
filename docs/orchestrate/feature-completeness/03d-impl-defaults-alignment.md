# 03d — Implementation log — Align all defaults to C#

**Date:** 2026-05-15
**Branch:** `main`
**Predecessor scope:** `02d-render-perf-investigation.md` (HEAD `7bb5e91`)
**Dispatch:** parent `/delegate` brief — *"align ALL with C# defaults first"*
**Source-of-truth tables:** `02d` §C# default knob table (`02d:117-218`) +
`02d` §Top config-class findings (`02d:287-340`) + `19-gi-reservoir-scope.md`
§3.11 audit table (`19:611-639`).

---

## Summary

A single Rust default knob diverged from C#: `GiSettings::sun_shadow_taps`
was **4** (Dispatch A's deliberate paper-§5.2-mitigation upgrade landed
2026-05-15 in `1c35c7f`); the user is now retracting the "by-default" part
of that divergence and pulling the runtime default back to C#'s **1** while
keeping the panel knob tunable up to 32 so the soft-shadow path remains
opt-in. **One Rust field default reverted (`lib.rs:170` 4→1), one panel-
table `default` aligned (`panel.rs:478` 4→1).** Per `02d` §1 Headline
Answer, this is the single C#-default divergence the port carried, and the
expected FPS recovery is **~25-40 %** of the 41 vs 130 FPS gap (a 4×→1×
reduction in spatial-resampling sun-ray work, the longest-tail ray budget
in the pipeline). No shader edits. No `Cargo.toml` feature-set edits (that
is the separate structural-fix dispatch the user has not yet authorized).
Every other C# config knob audited matches the port bit-for-bit per `02d`
§C# default knob table.

---

## Divergent-defaults table

Built from `02d` §C# default knob table + `19-gi-reservoir-scope.md` §3.11
(separate audits, cross-checked). One row per `02d`-classified knob.

| # | Knob | Port default site (file:line) | Port current | C# default (file:line) | Action |
|---|---|---|---|---|---|
| 1 | `MAX_RAY_STEPS_PRIMARY` | `crates/bevy_naadf/src/lib.rs:176` (`GiSettings::default`) | 120 | 120 (`rayTracing.fxh:7`) | **Match — no edit** |
| 2 | `MAX_RAY_STEPS_SECONDARY` | `crates/bevy_naadf/src/lib.rs:177` | 100 | 100 (`rayTracing.fxh:8`) | **Match — no edit** |
| 3 | `MAX_RAY_STEPS_SUN` | `crates/bevy_naadf/src/lib.rs:178` | 120 | 120 (`rayTracing.fxh:9`) | **Match — no edit** |
| 4 | `MAX_RAY_STEPS_SUN_SECONDARY` | `crates/bevy_naadf/src/lib.rs:179` | 80 | 80 (`rayTracing.fxh:10`) | **Match — no edit** |
| 5 | `MAX_RAY_STEPS_VISIBILITY` | `crates/bevy_naadf/src/lib.rs:180` | 60 | 60 (`rayTracing.fxh:11`) | **Match — no edit** |
| 6 | Spatial-resampling iter count | `crates/bevy_naadf/src/lib.rs:181` | 12 | 12 (`renderSpatialResampling.fx:359`) | **Match — no edit** |
| 7 | **`sun_shadow_taps`** | `crates/bevy_naadf/src/lib.rs:170` (`GiSettings::default`) | **4** | **1** (no loop — single `shootRay` at `renderSpatialResampling.fx:324`) | **REVERT 4 → 1** |
| 7b | `sun_shadow_taps` panel default | `crates/bevy_naadf/src/panel.rs:478` (`KNOBS` row) | **4** | n/a (panel is port-only — must match `GiSettings::default()` per `defaults_match_gi_settings_default` test at `panel.rs:1509-1546`) | **REVERT 4 → 1** |
| 8 | `bounce_count` | `crates/bevy_naadf/src/lib.rs:156` | 3 | 3 (`WorldRenderBase.cs:18`) | **Match — no edit** |
| 9 | `global_illum_max_accum` | `crates/bevy_naadf/src/lib.rs:157` | 128 | 128 (`WorldRenderBase.cs:21`) | **Match — no edit** |
| 10 | `spatial_resample_size` | `crates/bevy_naadf/src/lib.rs:158` | 500.0 | 500.0 (`WorldRenderBase.cs:23`) | **Match — no edit** |
| 11 | `spatial_visibility_count` | `crates/bevy_naadf/src/lib.rs:159` | 80 | 80 (`WorldRenderBase.cs:19` — C# slider default; **dead in both** per `12-alignment-gap.md` §3 D-C) | **Match — no edit** |
| 12 | `denoise_thresh` | `crates/bevy_naadf/src/lib.rs:160` | 400.0 | 400.0 (`WorldRenderBase.cs:20`) | **Match — no edit** |
| 13 | `radius_lit_factor` | `crates/bevy_naadf/src/lib.rs:161` | 3.0 | 3.0 (`WorldRenderBase.cs:22`) | **Match — no edit** |
| 14 | `noise_suppression_factor` | `crates/bevy_naadf/src/lib.rs:162` | 0.4 | 0.4 (`WorldRenderBase.cs:24`) | **Match — no edit** |
| 15 | `skip_samples` | `crates/bevy_naadf/src/lib.rs:163` | true | true (`WorldRenderBase.cs:16`) | **Match — no edit** |
| 16 | `is_denoise` | `crates/bevy_naadf/src/lib.rs:164` | true | true (`WorldRenderBase.cs:16`) | **Match — no edit** |
| 17 | `is_sample_leveling` | `crates/bevy_naadf/src/lib.rs:165` | true | true (`WorldRenderBase.cs:16`) | **Match — no edit** |
| 18 | `is_varying_resampling_radius` | `crates/bevy_naadf/src/lib.rs:166` | true | true (`WorldRenderBase.cs:16`) | **Match — no edit** |
| 19 | `is_atmosphere_interaction` | `crates/bevy_naadf/src/lib.rs:167` | true | true (`WorldRenderBase.cs:16`) | **Match — no edit** |
| 20 | `taa_ring_depth` (`taaSampleMaxAge`) | `crates/bevy_naadf/src/lib.rs:198,281` (`DEFAULT_TAA_RING_DEPTH = 32`) | 32 | 32 (`WorldRenderBase.cs:17`) | **Match — no edit** |
| 21 | Camera-history ring depth | `crates/bevy_naadf/src/render/taa.rs:30` (`CAMERA_HISTORY_DEPTH = 128`) | 128 | 128 (`WorldRender.cs:88`, `WorldRenderAlbedo.cs:36-40`) | **Match — no edit** |
| 22 | `VALID_SAMPLE_STORAGE_COUNT` | `crates/bevy_naadf/src/render/gi.rs:51` | 2 | 2 (`WorldRenderBase.cs:57`) | **Match — no edit** |
| 23 | `INVALID_SAMPLE_STORAGE_COUNT` | `crates/bevy_naadf/src/render/gi.rs:54` | 8 | 8 (`WorldRenderBase.cs:58`) | **Match — no edit** |
| 24 | `BUCKET_STORAGE_COUNT` | `crates/bevy_naadf/src/render/gi.rs:57` | 32 | 32 (`WorldRenderBase.cs:59`) | **Match — no edit** |
| 25 | `REFINED_BUCKET_STORAGE_COUNT` | `crates/bevy_naadf/src/render/gi.rs:60` | 8 | 8 (`WorldRenderBase.cs:60`) | **Match — no edit** |
| 26 | Atmosphere octahedral tex size | `crates/bevy_naadf/src/render/atmosphere.rs:39` (`ATMOSPHERE_TEX_SIZE`) | 1024 | 1024 (`WorldRenderBase.cs:131-132`) | **Match — no edit** |
| 27 | Atmosphere main ray steps / sub-scatter | `crates/bevy_naadf/src/render/atmosphere.rs:74-77` | 24 / 6 | 24 / 6 (UiSkyDebug.cs) | **Match — no edit** |
| 28 | `ConstructionConfig::initial_hash_map_size` | `construction/config.rs:145` | `1 << 18` = 262144 | `1 << 18` (`BlockHashingHandler.cs:32`) | **Match — no edit** |
| 29 | `ConstructionConfig::wanted_empty_ratio` | `construction/config.rs:147` | 0.5 | 0.5 (`BlockHashingHandler.cs`) | **Match — no edit** |
| 30 | `ConstructionConfig::probe_cap` | `construction/config.rs:149` | 250 | 250 (`BlockHashingHandler.cs`) | **Match — no edit** |
| 31 | `ConstructionConfig::max_group_bound_dispatch` | `construction/config.rs:151` | `512 * 64` | `512 * 64` (`WorldBoundHandler.cs:25`) | **Match — no edit** |
| 32 | `ConstructionConfig::n_bounds_rounds` | `construction/config.rs:157` | 5 | 5 (`WorldBoundHandler.cs:113`) | **Match — no edit** |
| 33 | `ConstructionConfig::max_entity_instances` | `construction/config.rs:182` (`DEFAULT_MAX_ENTITY_INSTANCES`) | 16384 | 16384 (`WorldRender.cs:88`) | **Match — no edit** |
| 34 | Tonemap | Bevy `TonyMcMapface` (post-process, `12-alignment-gap.md` §3 D-G + `18-taa-fidelity.md` fix #2) | TonyMcMapface | C# Reinhard (`WorldRenderBase.cs:435-437`) | **Keep (sanctioned)** — see §Sanctioned divergences |
| 35 | Camera-init formula | `crates/bevy_naadf/src/camera/mod.rs::InitialCameraPose` (`03a-v2` camera-init addendum) | derived from world-size | `(500, 200, 40)` (`WorldRender.cs:49`) for the C# test scene | **Keep (sanctioned)** — user-directed `--vox`-loader pose; non-render perf knob |

**Tally:** 33 matches + 2 reverts (the same knob in two places — runtime
default and panel default; the panel test `defaults_match_gi_settings_default`
welds them) + 2 sanctioned-keep + 0 out-of-scope-render-perf-knobs.

---

## Reverts applied

### Revert #1 — `crates/bevy_naadf/src/lib.rs:170`

```diff
-            // Multi-tap sun shadow — paper §5.2 soft-shadow noise mitigation
-            // (Dispatch A — `19-gi-reservoir-scope.md` §3.1). Default 4.
-            sun_shadow_taps: 4,
+            // Sun-shadow tap count — C# default 1 (no loop;
+            // `renderSpatialResampling.fx:322-339` is a single
+            // `getUniformHemisphereSample` + single `shootRay`). The Phase-D-
+            // shadow Dispatch A (`1c35c7f`, 2026-05-15) shipped N=4 as the
+            // paper-§5.2 soft-shadow mitigation; per `02d-render-perf-
+            // investigation.md` §1 + user directive 2026-05-15, the default
+            // is reverted to the C# canonical 1 — the multi-tap path stays
+            // available via the quality panel's `sun_shadow_taps` knob
+            // (range 1..32) for users who want softer penumbras at the perf
+            // cost. The shader's `max(_, 1u)` clamp at
+            // `spatial_resampling.wgsl:547` handles the default safely.
+            sun_shadow_taps: 1,
```

C# canonical reference: `renderSpatialResampling.fx:322-339` — no
sun-tap loop. Per `19-gi-reservoir-scope.md` §3.1 + `02d` §1 Headline
Answer.

### Revert #2 — `crates/bevy_naadf/src/panel.rs:478`

```diff
     Knob {
         label: "  sun_shadow_taps",
         class: 'C',
         kind: KnobKind::U32 {
             getter: |g| g.sun_shadow_taps,
             setter: |g, v| g.sun_shadow_taps = v,
             nudge: 1,
             big_step: 4,
             min: 1,
             max: 32,
-            default: 4,
+            default: 1,
         },
     },
```

Panel range `min: 1, max: 32` **unchanged** — the tunable surface stays
1..=32. Only the `default` value (which `defaults_match_gi_settings_default`
at `panel.rs:1509-1546` welds to `GiSettings::default().sun_shadow_taps`)
moves to 1. The C# reference is the `GiSettings::default()` value, which
itself anchors to `renderSpatialResampling.fx:322-339`.

No other code edits needed:

- `gi.rs:371` continues to plumb `gi.sun_shadow_taps` → `GpuGiParams.sun_shadow_taps`
  unchanged; the only thing that shifts is the *value* of `gi.sun_shadow_taps`.
- `spatial_resampling.wgsl:529-583` shader loop unchanged — at N=1 the loop
  executes once (per `20-impl-phase-d-shadow-A.md` §4 bit-equivalence proof).
  The multi-tap capability stays available; only the default tap count drops.
- `GpuGiParams` layout unchanged (`gpu_types.rs:509`).
- `offset_of!` guards at `gpu_types.rs:860-861` unchanged.
- No test edits (`promoted_defaults_match_canonical_consts` at
  `panel.rs:1551-1562` pins MAX_RAY_STEPS_* + `spatial_iter_count`, **not**
  `sun_shadow_taps`).
- `reset_all_knobs_restores_defaults` at `panel.rs:1595-1616` exercises
  `max_ray_steps_primary` / `spatial_iter_count` / `is_denoise` /
  `spatial_resample_size` — none touch `sun_shadow_taps`, so no test edit
  needed.

---

## Sanctioned divergences (untouched)

Per `01-context.md` §2 Faithful-port rule + `12-alignment-gap.md` §3 D-G
+ the parent `/delegate` brief Constraints list:

1. **TonyMcMapface tonemap** (`12-alignment-gap.md` §3 D-G; `18-taa-fidelity.md`
   fix #2) — user-directed; port emits raw linear HDR which Bevy's
   `PostProcessPlugin` tonemaps with `TonyMcMapface`, replacing C#'s
   in-shader Reinhard (`WorldRenderBase.cs:435-437`). **Untouched.**
2. **TAA ring depth default 32** (`DEFAULT_TAA_RING_DEPTH`, `lib.rs:198`)
   — paper-canonical; already matches C# `taaSampleMaxAge = 32`
   (`WorldRenderBase.cs:17`). Configurable down to 16/24 via
   `AppArgs.taa_ring_depth`. **Already aligned, no action.**
3. **Probe-cap 250** (`construction/config.rs:149`) — C# value
   (`BlockHashingHandler.cs`). **Already aligned, no action.**
4. **Camera-init formula** (`camera/mod.rs::InitialCameraPose`) — user-
   directed `--vox`-loader pose that derives from world-size rather than
   C#'s hard-coded `(500, 200, 40)` (`WorldRender.cs:49`). Not a render-
   perf knob; explicitly carved out of this dispatch per brief Constraints
   ("Do not touch sanctioned post-Phase-C user-directed features: the
   camera-init formula, the `--vox` loader, the editor — all stay").
5. **`spatial_visibility_count = 80`** — port-side field exists at
   `lib.rs:159` mirroring the C# slider default
   (`WorldRenderBase.cs:19`'s `spatialResampleVisibilityTestMaxDepth = 80`)
   but **the uniform is dead on both sides** per `12-alignment-gap.md`
   §3 D-C (the live shader path uses the const
   `MAX_RAY_STEPS_VISIBILITY = 60` at `ray_tracing.wgsl:126`, matching
   the C# `rayTracing.fxh:11` const). Port keeps the C# default for the
   field even though it's dead — no behaviour change either way.

---

## Out-of-scope items surfaced

The user directive was *"align ALL with C# defaults first"* — strict
default-knob alignment only. The following render-perf concerns from `02d`
are surfaced for the orchestrator's separate dispatch decisions:

1. **Bevy `DefaultPlugins` curation** (`02d` §1 second-place finding,
   §Structural-overhead, §Recommended-next-steps §3). Cargo.toml at
   `crates/bevy_naadf/Cargo.toml:40-42` requests `bevy` with default
   features — pulling in `bevy_pbr`, `bevy_gltf`, `bevy_anti_alias`,
   `bevy_sprite`, `bevy_audio`, `bevy_animation`, `bevy_picking`,
   `bevy_gizmos`, `bevy_light`, `bevy_ui_widgets`, etc. that the port
   does not use. Expected recovery: additional 5-15 % FPS. **Brief
   Constraints: "Do not touch the Bevy `Cargo.toml` feature set yet …
   it's a separate dispatch the user hasn't yet authorized."** Deferred.

2. **`PipelinedRenderingPlugin` toggle** (`02d` §Recommended-next-steps
   §5). Bevy 0.19's parallel render sub-app may slightly hurt latency for
   no net throughput gain on a GPU-bound 41-FPS workload; bench by
   toggling `multi_threaded` feature off. Low priority. Out of scope.

3. **`RenderDiagnosticsPlugin` GPU timestamp-query overhead** (`02d`
   §Render-graph-dispatch-bind-group-caching). ~28 timestamps × frame =
   typically <0.1 ms; HUD reads them. Out of scope (the HUD is the only
   per-pass attribution path).

4. **Per-pass HUD timing capture** (`02d` §Recommended-next-steps §1).
   The investigation lacked empirical per-pass times; the user's smoke
   captures them in seconds via the existing HUD. Out of scope of this
   defaults-alignment dispatch.

---

## Tests + e2e verification

### Pre-edit baseline

(from latest HEAD `1c35c7f` per `20-impl-phase-d-shadow-A.md` §5)

```
cargo test --workspace --lib  → 112 passed, 1 ignored (Phase-D-shadow A baseline)
```

Track A/B/C work since landed additional tests; the latest cited count is
**179 passed** per `03c-impl-edit-pipeline-alignment.md` README entry
(line 24). The pre-revert count should match post-revert (this dispatch
adds no new tests and edits no test bodies).

### Post-edit gates (actual)

```bash
$ cargo build --workspace 2>&1 | tail -20
   Compiling bevy_naadf v0.1.0 (/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf)
    Finished `dev` profile [optimized + debuginfo] target(s) in 20.89s
   → exit 0, no new warnings on touched files (lib.rs:170, panel.rs:478)

$ cargo test --workspace --lib 2>&1 | tail -40
   ...
   cargo test: 179 passed, 1 ignored (3 suites, 4.50s)
   → 179 PASS / 1 ignored — same count as pre-revert baseline
     (`defaults_match_gi_settings_default` still PASS — both ends moved to
     1 in lockstep; `promoted_defaults_match_canonical_consts` PASS — those
     knobs untouched; `reset_all_knobs_restores_defaults` PASS — does not
     exercise `sun_shadow_taps`).

$ cargo run --release --bin e2e_render 2>&1 | tail -30
   region luminance — emissive 247.0, solid(GI-lit diffuse) 242.1, sky 145.9
   PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames
   → exit 0
   (Pre-revert baseline per `20-impl-phase-d-shadow-A.md` §5:
    emissive 247.1, solid 242.0, sky 145.9 — within 0.1 LSB; the
    bit-equivalence proof at `20:78-96` was correct.)

$ cargo run --release --bin e2e_render -- --validate-gpu-construction 2>&1 | tail -25
   region luminance — emissive 247.0, solid 242.0, sky 145.9
   GPU construction byte-equal to CPU oracle: 388 bytes compared
   PASS — CPU/GPU oracle parity gate green
   → exit 0

$ cargo run --release --bin e2e_render -- --edit-mode 2>&1 | tail -25
   region luminance — emissive 247.0, solid 242.0, sky 145.9
   edit-mode validation PASS: 1 set_voxel → 1 chunk + 1 block + 2 voxel records
   → exit 0

$ cargo run --release --bin e2e_render -- --entities 2>&1 | tail -25
   region luminance — emissive 247.0, solid 242.0, sky 145.9
   entity handler validation PASS: frame A: 8 chunk_updates, 1 entity_chunk_instances, 1 history
   PASS (batch 6)
   → exit 0
   (The entity_pixel luminance gate at threshold 80 — baseline 187.93 per
   `20-impl-phase-d-shadow-A.md` §5 — has a 2.35× margin and continues
   to pass.)

$ cargo run --release --bin e2e_render -- --vox-e2e 2>&1 | tail -30
   vox_geometry region luminance — centre rect mean rgba [251.09, 249.87, 243.34, 255], luminance 249.7 (threshold > 160)
   PASS (batch 6)
   → exit 0
   (Pre-revert baseline luminance 249.7 — bit-identical; gate has wide
   margin above its 160 threshold.)
```

**All 5 e2e modes PASS, all luminance gates green with safe margins.**

**Expected luminance behaviour at N=1 vs N=4:** per
`20-impl-phase-d-shadow-A.md` §5, the `solid` GI-lit diffuse rect is "mostly
*unshadowed* in the test scene; the multi-tap sun would shift it only if it
sat on a shadow penumbra, which it doesn't" — `solid`/`emissive`/`sky`
should be visually identical between N=1 and N=4 baselines (within 1 LSB).
The `entity_pixel` gate at threshold 80 has a 2.35× margin, safely above
the wide-tolerance region gates documented in `gates.rs:643`. Per
`19-gi-reservoir-scope.md` §3.1 bit-equivalence note: at N=1 the shader's
random stream advances by exactly two `next_rand` calls per pixel —
identical to the pre-Dispatch-A path — so the gates should pin to
pre-Dispatch-A baselines (which is what the e2e gates were tuned against
originally).

### Smoke runs (one per scenario, per memory `subagent-gpu-app-verification-loop`)

```bash
$ timeout 30 cargo run --release --bin bevy-naadf 2>&1 | head -50
   (test grid: 32 chunks, 1920 blocks, 7232 voxel-u32s; GPU producer
    chain dispatched; FreeCamera controls logged.)
   → exit 0, boot clean, no shader-compile errors, no panic.

$ timeout 60 cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox 2>&1 | head -80
   .vox loaded: 257 palette entries, world bounds 93×34×84 chunks (1488×544×1344 voxels),
   265608 chunks total, blocks_cpu 1617216 u32s, voxels_cpu 10498368 u32s (sparse path)
   camera framed at pos=(726.56, 850.00, 52.50), look_at=(726.56, 850.00, 53.50)
   → exit 0, .vox parses, world renders, camera framed via `--vox`-loader pose.
```

The user runs the live FPS check (see § What the user manually verifies);
sub-agent does not loop on visual artefacts (per memory
`subagent-gpu-app-verification-loop`).

---

## What the user manually verifies

1. **Live FPS on the default test grid:**
   ```bash
   cargo run --release --bin bevy-naadf
   ```
   Expected: meaningful jump above the previous ~41 FPS baseline.
   Per `02d` §1 + §Headline-answer + §Top-config-class-finding #1:
   conservative **20-35 %** FPS recovery; optimistic more. If recovery
   is < 10 %, the spatial-resampling-sun-ray-cost hypothesis is wrong
   (see Risks/follow-ups).

2. **Live FPS on Oasis_Hard_Cover.vox:**
   ```bash
   cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
   ```
   The dense-occluder scene is where N=4's sun-ray multiplier was most
   expensive. Expected: larger absolute FPS gain than on the test grid.

3. **The panel `sun_shadow_taps` slider is still tunable 1..32:**
   - Press F1 to open the quality panel.
   - Cursor down to the `  sun_shadow_taps` row (SPATIAL RESAMPLING
     section).
   - ←/→ to decrement/increment by 1 (nudge), PageUp/PageDown by 4 (big
     step).
   - Set to 4 — soft-shadow path active (the pre-revert default).
   - Set to 1 — C# bit-equivalent path (the new default; pressing `R`
     while selected returns to this value).
   - Verify the panel HUD reflects the live value and the rendered
     shadows visibly soften at higher tap counts.

---

## Risks / follow-ups

1. **If the live FPS gain is < 10 % (well below the predicted 20-35 %):**
   the spatial-resampling-sun-ray-cost hypothesis from `02d` §Top-config-
   class-finding #1 + §Decisions §1 is wrong. **Flip-trigger:** the
   per-pass HUD measurement (`02d` §Recommended-next-steps §1). User
   reads the HUD's `render/naadf_spatial_resampling/elapsed_gpu` figure
   pre- and post-revert; if it didn't drop by ~3×, the sun-ray loop was
   not the dominant cost in that pass. Next investigation step: dispatch
   a finer-grained per-tap instrumentation pass (`02d` §Recommended-
   next-steps §4(a)/(b)). Cost of the investigation: medium (timestamp-
   query plumbing).

2. **If FPS gain is in the 10-25 % range** (modest but real): the next
   structural lever is the **Bevy `DefaultPlugins` curation** (`02d`
   §Recommended-next-steps §3 — a separate dispatch the user has not
   yet authorized per the parent brief's Constraints). Expected
   additional recovery: 5-15 %. The architect would dispatch a `Cargo.toml`
   feature-curation worktree at that point.

3. **If FPS gain meets/exceeds 25-40 % prediction:** dispatch closes
   successfully. The deferred Bevy-feature-curation dispatch becomes a
   follow-on quality-of-life cleanup, not a perf-driven critical-path.

4. **Shader random-stream advancement at N=1:** per
   `20-impl-phase-d-shadow-A.md` §4, at N=1 the loop body runs once and
   draws the same two `next_rand` calls per pixel as the original
   single-tap code — bit-equivalent to the pre-Dispatch-A path. If e2e
   luminance shifts by more than 1 LSB on `--vox-e2e`, that is **proof
   of a rand-stream off-by-one regression** introduced somewhere
   between Dispatch A and this revert. Halt; surface to the user. Not
   expected per the bit-equivalence proof.

5. **Panel knob default reset (`R` keybind):** previously `R` on the
   `sun_shadow_taps` row restored to 4; now restores to 1. User
   familiarity may need a beat — surfaced for the user.

6. **The Phase-D-shadow Dispatch A capability stays available.** This
   revert does not remove the multi-tap shader loop, the `GpuGiParams`
   field, or the panel slider. Users who *want* the softer-shadow path
   (e.g. for cinematic captures) opt in via the panel; the engine's
   default returns to C# canonical.

7. **`02d` §C# default knob table did not surface `spatial_visibility_count`
   explicitly** — it was caught in `19-gi-reservoir-scope.md` §3.11 + my
   audit pass (the C# slider has a default `80` matching the port's
   `lib.rs:159` value; both are dead). No action needed; flagged for
   completeness.
