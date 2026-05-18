# 02-research — web-vox-color-divergence (diagnose-first observation)

2026-05-18

Observation phase of the diagnose-first protocol per
`01-context.md` Decision 2. No fix proposed; this file feeds the
architect at the design hard gate.

## Instrumentation added

Three `info!`-level diagnostic logs with the stable prefixes
`[palette-upload]` / `[palette-install]`. All marked with comments
calling out that they will be demoted to `debug!` by the implementer
per `01-context.md` forbidden move 11.

- **`[palette-upload]`** at
  `crates/bevy_naadf/src/render/prepare.rs:489-502` (immediately before
  the `voxel_types.upload_all(...)` call at the GPU palette upload
  site). Logs `palette_len` and the first 5 entries' raw packed
  `[u32; 4]` (the f16-packed `data` field documented at
  `crates/bevy_naadf/src/render/gpu_types.rs:262-294`). Raw `[u32; 4]`
  was used instead of f16-decoded triples to keep the instrumentation
  surface small and avoid pulling in an inverse f16 helper; entry 0
  with `[1006632960, 0, 0, 0]` decodes to
  `(roughness=f16(1.0)<<16 | base=0 | layer=0, 0, 0, 0)` — i.e. the
  all-zero palette default. Non-zero `color_base` shows up as non-zero
  bytes in `data[1]` and `data[2]`.
- **`[palette-install]`** inside `install_imported_vox` at
  `crates/bevy_naadf/src/voxel/grid.rs:624-642` (immediately before the
  `commands.insert_resource(VoxelTypes { types: imp.palette })` at the
  audit's cited palette-write site). Logs the `source_label` arg,
  `palette_len`, and the first 5 entries' `color_base` as floats so the
  .vox install can be told apart from the default-scene install in the
  timeline.
- **`[palette-install]`** inside
  `install_default_embedded_in_fixed_world` at
  `crates/bevy_naadf/src/voxel/grid.rs:332-352`. Same format,
  `label="default-scene"`. Critical for distinguishing
  "default-scene Startup install" from ".vox post-parse install"
  on the web timeline.

A fourth `[palette-install]` was also added at
`crates/bevy_naadf/src/voxel/grid.rs:206-226` inside `install_empty_world`
(the skybox-only path, `label="skybox-only"`) for completeness — the
`--vox-web-parity-skybox` subprocess on native and the
`?skybox=1` test in Playwright both flow through this site and emitting
a palette-trace from it makes the timeline self-explaining.

Stage-A also patched the Playwright test
`e2e/tests/vox-loading.spec.ts:155-171` and
`e2e/tests/vox-loading.spec.ts:206-220` to forward any browser console
message whose text contains `[palette-upload]` or `[palette-install]`
to Node-side `console.log` so `just test-wasm 2>&1 | tee` captures it.
The tracing-wasm bridge emits these as `console.log` calls with CSS
markup, which Playwright otherwise swallows.

## Native log readout

`/tmp/native-palette-trace.log` — captured via
`timeout 180s cargo run --bin e2e_render -- --vox-web-parity 2>&1 | tee`.
Exit code 0 (gate PASS, SSIM = 0.0177 < 0.85).

Filtered to `[palette-*]` lines (ordered, ANSI-stripped, trimmed):

```
T+0.59s  bevy_naadf::voxel::grid:  [palette-install] install_empty_world label="skybox-only"                                       palette_len=13  first_5_color_base=[(0.0, 0.0, 0.0), (0.55, 0.55, 0.58), (0.8, 0.3, 0.22), (0.25, 0.45, 0.8), (0.3, 0.7, 0.32)]
T+0.68s  bevy_naadf::render::prepare: [palette-upload] prepare_world_gpu uploading voxel_types to GPU                              (palette_len=13,  first_5_raw=[[1006632960, 0, 0, 0], [993198080, 946223206, 14500, 0], [979763200, 885865062, 13066, 0], [979763200, 926102528, 14950, 0], [966393856, 966407373, 13599, 0]])
--- (loaded-phase subprocess starts) ---
T+4.49s  bevy_naadf::voxel::grid:  [palette-install] install_imported_vox    label="crates/bevy_naadf/assets/test/oasis_hard_cover.vox" palette_len=257 first_5_color_base=[(0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)]
T+4.60s  bevy_naadf::render::prepare: [palette-upload] prepare_world_gpu uploading voxel_types to GPU                              (palette_len=257, first_5_raw=[[1006632960, 0, 0, 0], [1006632960, 0, 0, 0], [1006632960, 0, 0, 0], [1006632960, 0, 0, 0], [1006632960, 0, 0, 0]])
```

On native, `--vox-web-parity` spawns two subprocesses (skybox-only and
loaded), so there are two `palette-install` → `palette-upload` pairs,
one per subprocess. Each subprocess is short-lived and only ever runs
its own install + its own upload — the build-once gate fires once per
process with the correct palette. The all-zero `color_base` for the
first 5 entries of the .vox palette is a property of the
`oasis_hard_cover.vox` file, not a bug: `vox_palette_to_voxel_types`
(`crates/bevy_naadf/src/voxel/vox_import.rs:964-1004`) prepends a
`VoxelType::default()` at index 0 (all-zero, the reserved empty
placeholder) and the file's own palette indices 0-3 in the Oasis
fixture happen to also be unused black entries.

## Web log readout

`/tmp/web-palette-trace.log` — captured via
`timeout 360s just test-wasm 2>&1 | tee` after a fresh
`just web-build-release`. The vox-loading SSIM-dissimilar test FAILED
(SSIM = 0.930556, threshold < 0.85 — i.e. the loaded canvas looks
**almost identical** to the skybox baseline, reproducing the
"near-black voxel render" symptom). The skybox-baseline test passed.

Filtered to `[palette-*]` lines (Playwright forwarded them through the
`[wasm-console]` prefix; tracing-wasm `%cINFO%c` CSS markers stripped
for readability):

```
TEST 1 — skybox-baseline (one browser context, ?skybox=1)
  T+ A  voxel::grid:  [palette-install] install_empty_world                       label="skybox-only"   palette_len=13  first_5_color_base=[(0.0, 0.0, 0.0), (0.55, 0.55, 0.58), (0.8, 0.3, 0.22), (0.25, 0.45, 0.8), (0.3, 0.7, 0.32)]
  T+ B  render::prepare: [palette-upload] prepare_world_gpu uploading voxel_types  (palette_len=13, first_5_raw=[[1006632960, 0, 0, 0], [993198080, 946223206, 14500, 0], [979763200, 885865062, 13066, 0], [979763200, 926102528, 14950, 0], [966393856, 966407373, 13599, 0]])

TEST 2 — loaded (fresh browser context, ?vox=/test-fixtures/oasis_hard_cover.vox)
  T+ C  voxel::grid:  [palette-install] install_default_embedded_in_fixed_world  label="default-scene" palette_len=13  first_5_color_base=[(0.0, 0.0, 0.0), (0.55, 0.55, 0.58), (0.8, 0.3, 0.22), (0.25, 0.45, 0.8), (0.3, 0.7, 0.32)]
  T+ D  render::prepare: [palette-upload] prepare_world_gpu uploading voxel_types  (palette_len=13, first_5_raw=[[1006632960, 0, 0, 0], [993198080, 946223206, 14500, 0], [979763200, 885865062, 13066, 0], [979763200, 926102528, 14950, 0], [966393856, 966407373, 13599, 0]])
  T+ E  voxel::grid:  [palette-install] install_imported_vox                      label="/test-fixtures/oasis_hard_cover.vox" palette_len=257 first_5_color_base=[(0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)]
  *** NO subsequent [palette-upload] line ***
```

The `palette_len=257` install at `T+E` lands AFTER the `palette_len=13`
upload at `T+D`. The build-once gate inside `prepare_world_gpu` has
already closed (it set `WorldGpu` via `commands.insert_resource(...)`
at `prepare.rs:567`); the gate at `prepare.rs:201-203` is now
`is_some()` for every subsequent frame, so the `[palette-upload]` log
never re-fires with the .vox palette. The GPU's
`voxel_types: GrowableBuffer<GpuVoxelType>` permanently holds the
13-entry default palette.

## Confirmed root cause

**The audit's smoking-gun hypothesis is CONFIRMED, verbatim.** On web,
the build-once gate inside `prepare_world_gpu`
(`crates/bevy_naadf/src/render/prepare.rs:201-203`, paired with the
`stage_world_gpu_buildonce` gate at
`crates/bevy_naadf/src/render/extract.rs:201-203`) uploads the
default-scene palette to the GPU on the very first frame after
Startup. Three to four frames later, the async rayon parse completes
and `install_imported_vox` overwrites the main-world `VoxelTypes`
resource at `grid.rs:624` with the 257-entry .vox palette — but no
system re-extracts and no `[palette-upload]` re-fires. The GPU
`voxel_types` buffer remains pinned to the 13-entry default. Every
voxel-type lookup in `decompress_voxel_type` (shader
`assets/shaders/render_pipeline_common.wgsl:102-114`, called from
`naadf_first_hit.wgsl:228-235`) goes through `voxel_types[hit_type]`
on the GPU buffer, but the geometry's `hit_type` values come from the
.vox model's W5 GPU-producer chain and reference palette indices
0..256. Indices ≥13 read out-of-bounds zeros (WebGPU silently clamps
oob reads on storage buffers), and even valid indices 0..12 are
reading **the wrong colors** — the default-scene's 13 entries, not the
Oasis fixture's 257. Either way the `acc.absorption = acc.absorption *
voxel_type.color_base` multiply produces near-black output: the .vox
hover-type readout works because the W5 readback populates the
**CPU-side** `WorldData.voxels_cpu` mirror correctly (see the
`Q3 readback: stage … → Done` log lines, native log
T+5.69s — that path is geometry, NOT palette), but the rendered
framebuffer reads the stale palette.

Native does not hit this because `setup_test_grid`'s
`GridPreset::Vox { path }` arm (`crates/bevy_naadf/src/voxel/grid.rs`,
the sync install branch) inserts the .vox `VoxelTypes` SYNCHRONOUSLY
at Startup, BEFORE the first `ExtractSchedule` runs. The native
`--vox-web-parity-loaded` subprocess has only ONE `[palette-install]`
(the .vox one at 257 entries) and the subsequent `[palette-upload]`
correctly carries that palette. No collision, no asymmetry.

## Decisions & rejected alternatives

- **Decision:** The audit's hypothesis (Hypothesis 3 — default-scene
  palette interference × build-once gate, combined with Hypothesis 1 —
  parse/install split lost a side-effect: the implied GPU re-upload)
  is the load-bearing root cause. The web timeline shows exactly the
  ordering the audit predicted: Startup `install_default` → Frame 1
  `[palette-upload] palette_len=13` → N frames later
  `[palette-install] palette_len=257` → NO subsequent
  `[palette-upload]`. **Verdict: hypothesis confirmed by direct
  observation.**

- **Rejected: Hypothesis 4 (cross-frame Q3 readback timing).** Native
  logs show the Q3 readback machinery reaches `Done` at T+5.69s on the
  .vox path (`chunks_cpu.len() = 2097152, blocks_cpu.len() = 12882752,
  voxels_cpu.len() = 10479392` — geometry transferred correctly). The
  hover-on-voxels behavior the user reported on web also requires Q3
  readback to populate the CPU mirror — so Q3 works on web AND the
  geometry is correct on the GPU side. The Q3 path does not touch
  `voxel_types` (audit row at `mod.rs:1028-1325`). The palette
  divergence is independent of Q3 by construction.

- **Rejected: Hypothesis 5 (wgpu 29 / WebGPU binding shape
  divergence).** The bind-group entry for `voxel_types` is the same
  `GrowableBuffer<GpuVoxelType>` on both targets (`prepare.rs:559` →
  `bind_group_world` slot 3, consumed by
  `world_data.wgsl:73` as `voxel_types: array<vec4<u32>>`). The web
  log proves the GPU buffer is `palette_len=13` not because of binding
  divergence but because that's the data that was uploaded. The shader
  is fine; the buffer's contents are wrong. Binding-shape divergence
  would manifest as wgpu validation errors during the bind-group
  creation, not as a successful upload of the wrong data.

- **Rejected: Hypothesis 6 (tonemap/exposure).** Native renders the
  same fixture with full colors using the same tonemap. The native
  `--vox-web-parity-loaded.png` reference capture is colorful and
  passes the SSIM dissimilarity check (0.0177); the web canvas of the
  same fixture is near-black and FAILS the SSIM dissimilarity check
  (0.93). Same shader, same tonemap, same exposure pipeline — only the
  per-voxel `color_base` data differs between targets. Tonemap is not
  the diverging axis.

- **Rejected: Hypothesis 2 (Bevy `Changed<T>` not firing on
  insert_resource).** This is now a downstream design question for the
  architect (the fix shape will hinge on it) but it's NOT what
  produces the bug. Even if `Changed<VoxelTypes>` fired correctly,
  there is **no system that queries it** anywhere in the codebase
  (audit row at `editor/mod.rs:140` — zero `Changed<VoxelTypes>`
  queries crate-wide). The change-detection plumbing is absent at the
  query side, not at the publisher side. The architect's "Re-buildable
  extract path with `Changed<T>` queries" candidate from Decision 3 in
  `01-context.md` is the candidate that closes the gap; whether it
  works depends on Bevy's `insert_resource`-over-existing semantics
  for `Changed<R>`, which is a design-phase question.

## Assumptions made

- **Assumption:** The order of log lines in stdout reflects the actual
  frame / system-tick order for these systems. `[palette-install]`
  emits from `Commands::insert_resource` inside `install_*` (main
  world, Update/Startup); `[palette-upload]` emits from
  `prepare_world_gpu` (RenderApp's render schedule). Both write to the
  same stdout pipe via `tracing::info!`. Bevy's renderer pipelines the
  render world one frame behind by default, so the `[palette-upload]`
  for frame N is observed at wall-clock time AFTER the Update-schedule
  logs for frame N+1. The web timeline shows
  `install_default → upload(13) → … → install_vox(257)` with no later
  upload — even allowing for a 1-frame lag, the absence of a third
  upload line after a 5+ second wait is conclusive: it never fires.
  **Risk if wrong:** if `[palette-upload]` is somehow buffered/batched
  on wasm such that it appears late, the conclusion could be premature
  — but five seconds of wall clock between the .vox install and test
  failure is far past any tracing-wasm flush latency.

- **Assumption:** The Playwright vox-loading test's SSIM=0.93 failure
  (loaded canvas ≈ skybox baseline) is the same color-divergence bug
  the user reported (near-black voxels). Both originate from a
  GPU-side palette pinned to wrong values; both render a frame that
  looks like a near-uniform dark background. The SSIM test failed
  spontaneously without any other change to the branch, so the test
  is reproducing the exact regression the user observed. **Risk if
  wrong:** if the SSIM failure has a different cause (e.g.
  camera-pose drift, sky shader regression), our log-driven diagnosis
  would still be valid for the palette-not-uploading bug but the user-
  observed symptom could have a second cause. Mitigation: the
  per-voxel `color_base = ZERO` arithmetic
  (`naadf_first_hit.wgsl:235` `acc.absorption = … * voxel_type.color_base`)
  directly produces near-black; no second mechanism is needed.

- **Assumption:** No other system mutates `WorldGpu` or its bind group
  between the build-once upload and the .vox install. The audit's
  grep-zero finding for `commands.remove_resource::<WorldGpu>()`
  (`01-context.md` "Negative findings") and the absence of any
  `ResMut<WorldGpu>` outside of `prepare_world_gpu` itself together
  rule this out. The web log corroborates: only one `[palette-upload]`
  fires, period.

## What the architect needs

The architect chooses among the three fix-shape candidates listed in
`01-context.md` Decision 3. The observation supports / rules out each
as follows.

- **Candidate 1 — Re-buildable extract path with `Changed<T>`
  queries.** *Applicable.* The web log proves
  `commands.insert_resource(VoxelTypes { … })` at `grid.rs:624` does
  run; the missing piece is a system that observes the change and
  removes `WorldGpu` (or re-flags `WorldGpuStaging`). Whether Bevy
  0.19's `Commands::insert_resource` over an existing resource trips
  `Changed<R>` for the next-tick query is the
  architect/implementer's design call (Bevy docs say yes; the audit
  cautions verifying). The fix would also need to handle
  `Changed<WorldData>` since the .vox install rewrites that too.
- **Candidate 2 — Cache-invalidate at install site.** *Applicable.*
  Strict superset of Candidate 1's effect, more explicit. The
  audit-noted grep-zero of `remove_resource::<WorldGpu>()` confirms
  this would be a new pattern in the codebase. Less idiomatic but
  guaranteed to work regardless of Bevy's `Changed<R>` semantics on
  `insert_resource`-over-existing.
- **Candidate 3 — Suppress default scene during pending .vox.**
  *Applicable for the user-observed symptom only, NOT for the
  underlying architectural gap.* The web log proves the default-scene
  install IS what's locking in the wrong palette; removing it would
  prevent THIS bug, but the audit's
  `extract.rs:60-66` docstring explicitly calls out the same gap for
  "world reload / live re-import" — Candidate 3 leaves that future
  feature pre-broken. Architect should weigh whether to fix the
  immediate user-visible issue or the root cause.

**Ruled out / not applicable:**

- Anything in the `#[cfg(target_arch = "wasm32")]` direction — see
  forbidden move 1 in `01-context.md`. The fix must work for any
  async install scenario (drag-and-drop on native, web HTTP fetch,
  hypothetical future live-reload), not just web.
- Anything that mocks GPU work or rebuilds the bind group from the
  render world without going through the standard extract → prepare
  flow — would re-fragment the build-once invariant the rest of the
  codebase relies on.

**New constraints the observation surfaced:**

- The 13-entry default palette × 257-entry .vox palette length
  asymmetry means even index-0 hits read a DIFFERENT default color
  than intended (default `MaterialBase::Diffuse, color_base=ZERO` is
  the same VoxelType in both palettes, but indices 1..12 differ in
  semantic between "ground/box/sphere/emissive" defaults and
  ".vox material 1..12"). The fix shape that re-uploads must re-upload
  the **full** new palette, not just resize the buffer — i.e. the
  `voxel_types` GrowableBuffer must `upload_all` the new contents, not
  `append`. (Candidate 1's `Changed<>`-triggered re-extract naturally
  does this; Candidate 2's `remove_resource::<WorldGpu>()` rebuilds
  the whole thing.)

- The `[palette-upload]` and `[palette-install]` logs are NOT
  per-frame — they fire exactly once each per process today and the
  observation depends on that one-shot-ness. After the fix lands, the
  expected new shape is **exactly two** `[palette-upload]` lines on
  web (one default, one .vox) and **exactly one** on native (the
  .vox). The implementer should preserve this signal: keep the logs
  at `debug!` (per Decision 11) so future divergences can be
  rediscovered. If the architect's fix introduces per-frame re-uploads
  (e.g. by accident — every render tick), the demoted-debug log
  becomes the smoke detector.

## Files touched (instrumentation)

Per `git diff --stat` after Stage A:

```
crates/bevy_naadf/src/render/prepare.rs | 21 +++++++++++
crates/bevy_naadf/src/voxel/grid.rs     | 65 +++++++++++++++++++++++++++++++++
e2e/tests/vox-loading.spec.ts           | 24 +++++++++++-
3 files changed, 109 insertions(+), 1 deletion(-)
```

Exact instrumentation sites for the implementer to find when demoting
the logs to `debug!`:

- `crates/bevy_naadf/src/render/prepare.rs` — added a block at
  lines 488-503, immediately before
  `voxel_types.upload_all(&voxel_types_data, &render_device, &render_queue);`.
  Single `info!` call with tag `[palette-upload]`.
- `crates/bevy_naadf/src/voxel/grid.rs` — three added blocks:
  - lines 206-226 inside `install_empty_world`,
  - lines 332-352 inside `install_default_embedded_in_fixed_world`,
  - lines 624-642 inside `install_imported_vox`.
  All three are `info!` calls with tag `[palette-install]` and a
  per-site `label=...` discriminator.
- `e2e/tests/vox-loading.spec.ts` — small handler additions in both
  `test("captures skybox baseline …")` (lines ~155-171) and
  `test("startup-fetches and installs the default .vox …")` (lines
  ~206-220) that forward `[palette-upload]` / `[palette-install]`
  console messages from the wasm bridge to Node-side
  `console.log`. The implementer can decide whether to keep the
  forwarder permanently (recommended — pairs with the rust-side
  `debug!` demote) or remove it.
