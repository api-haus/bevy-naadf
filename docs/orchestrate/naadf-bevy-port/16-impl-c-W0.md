# 16 — Phase C impl log — W0 (seam construction)

## W0 — seam construction (2026-05-15)

The foundational wave-1a workstream of Phase C: build the **empty extension
surface** every other Phase-C workstream extends through. No GPU pipelines, no
buffers, no bind groups, no actual construction work — just the skeleton. W1
through W5 land their real work behind this seam without re-editing the seam
itself (`15-design-c.md` §1, §2.1 W0 row, §3).

### Changes by file

**New files (2):**

- `crates/bevy_naadf/src/render/construction/mod.rs` (~310 lines) — the
  Phase-C sub-module's mod-file. Declares:
  - `ConstructionGpu` — the render-world `Resource` with every Phase-C buffer
    family field as `Option<Buffer>` initialised to `None` (W1/W2/W3/W4 each
    populate their family).
  - `ConstructionBindGroups` — sibling resource, every field
    `Option<BindGroup>` initialised to `None`.
  - `ConstructionPipelines` — **empty** `Resource` (zero fields), sibling of
    `NaadfPipelines`. W1..W5 each add their pipeline-ID + layout fields here
    in their own merge.
  - `prepare_construction` — empty `Render`-schedule system in
    `PrepareResources`. Body: `init_resource`-on-missing for the two
    resources; nothing else.
  - `run_gpu_construction_startup` — empty `Startup`-schedule one-shot. Body:
    early-return when `ConstructionConfig.gpu_construction_enabled` is false
    (W0 default); else log a placeholder `info!` line. W1 fills the body.
  - `ConstructionPlugin` — wiring plugin. Adds `run_gpu_construction_startup`
    in `Startup`, mirrors `AppArgs.construction_config` into the render
    sub-app as `ConstructionConfig`, registers `ConstructionPipelines`
    via `init_gpu_resource`, registers `prepare_construction` in
    `PrepareResources`.
- `crates/bevy_naadf/src/render/construction/config.rs` (~130 lines) —
  `ConstructionConfig` `Resource` with the 8 fields listed in `15-design-c.md`
  §1.8 / §2.1 (`gpu_construction_enabled`, `initial_hash_map_size`,
  `wanted_empty_ratio`, `probe_cap`, `max_group_bound_dispatch`,
  `entities_enabled`, `cpu_fallback`, `n_bounds_rounds`), all defaulted to
  the NAADF C# values verbatim. `Default` + `From<&AppArgs>` impls. A
  compile-time `const _ = { … }` block pins the defaults so a careless
  future edit can't silently drift the build path away from the canonical
  methodology (no runtime test — W0's "+1 test" budget belongs to the
  layout test in `gpu_types.rs`).

**Edited files (5):**

- `crates/bevy_naadf/src/render/mod.rs` — adds `pub mod construction;` (with
  a 5-line doc comment pointing at `15-design-c.md` §1.1). Adds a clearly-
  commented TODO placeholder block in the `Core3d` `.chain()` showing where
  the three construction nodes go (`naadf_bounds_compute_node` — W3,
  `naadf_world_change_node` — W2, `naadf_entity_update_node` — W4), with the
  insertion order rationale and a `15-design-c.md §3` cross-reference. No
  node is inserted; the chain topology is byte-identical to pre-W0.
- `crates/bevy_naadf/src/render/prepare.rs` — widens the chunks texture's
  `TextureUsages` to add `STORAGE_BINDING` alongside `TEXTURE_BINDING |
  COPY_DST`. This is the **one production-side seam touch** every later
  workstream depends on (so W1/W2/W3 can write to the chunks texture from
  compute shaders). Commented with a `15-design-c.md §1.4` reference + a
  comment that W0's screenshot is byte-identical to pre-W0 because the new
  usage flag is opt-in for reading passes.
- `crates/bevy_naadf/src/render/gpu_types.rs` — appends `GpuConstructionParams`
  (80 B = 5 × 16-byte rows, no `vec3`-then-scalar hazard — every 3-tuple
  explicitly padded to 16). Adds 9 `const _: () = assert!(...)` guards
  (size + offset of every field that starts a row + `% 16 == 0` on every
  `vec3` 3-tuple). Adds the runtime test `construction_params_layout` —
  the +1 over the baseline test count.
- `crates/bevy_naadf/src/lib.rs` — adds `pub construction_config:
  ConstructionConfig` field to `AppArgs` (default
  `ConstructionConfig::default()`). Mirrors the `taa_ring_depth` plumbing
  pattern. Inserts `render::construction::ConstructionPlugin` into the
  `build_app` plugin set, immediately after `NaadfRenderPlugin` so the
  render sub-app already exists when `init_gpu_resource::<…>()` runs.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — adds `--validate-gpu-construction`
  CLI flag (default off, hand-rolled `std::env::args()` parse, no new dep).
  When set, after the normal e2e exit the binary prints a placeholder
  message and exits with the same status. W1 replaces the placeholder body
  with the bit-exact CPU/GPU oracle check. Switched `fn main() -> AppExit`
  to `fn main() -> ExitCode` so the flag plumbing has a single explicit
  exit-code mapping site.

**Not edited (by design):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — explicitly
  off-limits per `15-design-c.md` §1.3 / §2.1 W0 row. Construction pipelines
  live in their own sibling `ConstructionPipelines` resource.
- `crates/bevy_naadf/src/main.rs` — production binary is a one-line shim
  over `build_app(AppConfig::windowed()).run()`. The brief lists it as an
  edit target ("add `ConstructionPlugin` to the App's plugin set") but
  the actual plugin set lives in `build_app` (in `lib.rs`); `main.rs` does
  not directly register plugins. `ConstructionPlugin` is added in
  `build_app`'s plugin tuple, so both binaries (`main.rs` and
  `bin/e2e_render.rs`) pick it up via `build_app` / `run_e2e_render`. No
  edit needed to `main.rs`.

### Decisions & rejected alternatives

1. **`Option<Buffer>` for every `ConstructionGpu` field (vs. typed
   `Option<GrowableBuffer<T>>`).** Decided `Option<Buffer>` for W0. W0 does
   not pull in `GrowableBuffer<T>` because each workstream's family has its
   own element type (`HashValue` for W1, `[u32; 2]` for W2's changed_groups,
   `GpuEntityChunkInstance` for W4 entities, etc.). Each workstream that
   lands a family chooses `Option<GrowableBuffer<T>>` or `Option<Buffer>`
   per element-type's needs; W0 keeps the empty shell minimal by sticking
   to the `Option<Buffer>` lowest common denominator. **Rejected:**
   pre-typing every field with a placeholder `GrowableBuffer<u32>` —
   would force every workstream to retype its field, defeating the
   "swap None for Some" seam contract.
2. **`Option<…>` initialisation vs. `Vec::new` / size-0 buffers.** Decided
   `None`. The pattern matches the rest of the render-world resources
   (`TaaGpu` / `GiGpu` / `AtmosphereGpu` all use `Option<Res<...>>` checks
   in their prepare systems for "not yet initialised"). A 0-length buffer
   is a wgpu validation error; `None` is the only safe "not yet allocated"
   state.
3. **Empty `ConstructionPipelines` (vs. populated with a no-op pipeline
   placeholder).** Decided empty. A no-op pipeline would still consume a
   pipeline-cache slot and a layout, both of which would conflict with W1's
   real layouts at merge time. Empty is the cleanest seam — each
   workstream adds its own field with a `#[derive(FromWorld)]` impl
   refactor at that workstream's merge.
4. **Plugin placement in `build_app`'s plugin tuple.** Decided immediately
   after `render::NaadfRenderPlugin`, before the InstaMAT/texture-array
   plugins. Ordering rationale: `init_gpu_resource::<ConstructionPipelines>()`
   needs the render sub-app to exist (which `NaadfRenderPlugin` ensures via
   `DefaultPlugins`'s `RenderPlugin`); placing `ConstructionPlugin`
   immediately after keeps the construction-related render-side wiring
   contiguous in the plugin tuple. Same pattern as `NaadfRenderPlugin`'s own
   `init_gpu_resource::<NaadfPipelines>()`.
5. **`run_gpu_construction_startup` on main app vs. render sub-app.**
   Decided main app, per `15-design-c.md` §1.2 regime-1 + §3. Justification:
   the regime-1 driver owns its own `RenderQueue::submit` command-encoder,
   mirroring `prepare_world_gpu`'s pattern. It is a one-shot, not part of
   the per-frame `Render` schedule, so the render sub-app is the wrong
   schedule. (The mirrored `ConstructionConfig` resource goes into the
   render sub-app via `insert_resource`, which is the render-side
   counterpart.)
6. **Chunks texture usage-flag widening placement.** The brief says
   "currently around line 160". Verified by Read — the chunks texture
   descriptor was at lines 149-162 (slight drift from line 160). The
   widening adds `| TextureUsages::STORAGE_BINDING` to the existing
   `TEXTURE_BINDING | COPY_DST` mask; pre-W0 readers (atmosphere /
   first-hit / TAA / GI / blit) are read-only on the chunks texture and
   unaffected by the wider mask. The W0 build produces a screenshot whose
   gate values are byte-identical to pre-W0; the underlying PNG hash
   differs run-to-run because of the Phase-B GI pipeline's documented
   per-run non-determinism (verified on the baseline run too).
7. **The `--validate-gpu-construction` placeholder fires
   unconditionally on the flag, not only on `AppExit::Success`.**
   Decided fire-always. The reasoning: the flag plumbing is what W1's
   bit-exact oracle test will hook into; verifying the plumbing reaches
   the final exit on every code path (success and failure) protects W1
   from a "validation only runs on success" foot-gun. Validation cannot
   *succeed* a failed e2e — it can only *fail* a successful one — and W0's
   placeholder doesn't fail anything.
8. **`fn main() -> ExitCode` (vs. `-> AppExit`).** Switched. `AppExit:
   Termination` works as a `main` return, but it folds the validation
   check + the e2e exit into a single explicit numeric exit code, which
   is what W1's bit-exact assertion will need. The mapping is one match
   block; small, well-typed.
9. **No new CLI dep (clap/argh).** Decided hand-roll the flag.
   `std::env::args` + `any(|a| a == "--validate-gpu-construction")` is
   2 lines. Adding clap would balloon the dep tree and lock W1/W2/W4
   into clap's idioms for their own flags. Each workstream picks its own
   approach at its own merge.
10. **One new test, not two.** The brief allows +1 test
    (`construction_params_layout` — the runtime mirror of the
    `offset_of!` compile-time guards). The config defaults are pinned at
    compile time via a `const _: () = { … }` block, not via runtime
    tests, to stay at the +1 budget. Baseline test count was 54 (not the
    brief's stated 61 — that figure is stale). New total: 55.

### Assumptions made

- The brief's "61 tests" / "62 total" baseline is stale; actual baseline
  was 54 (`cargo test -p bevy-naadf`). Targeted +1 → 55. All 55 pass.
- The brief says "EDIT main.rs to add `ConstructionPlugin`" but `main.rs`
  is a 1-line shim; the actual plugin set is in `build_app` (in `lib.rs`).
  I read this as "add `ConstructionPlugin` to the App's plugin set" =
  add it in `build_app`. Both binaries (`main.rs` and
  `bin/e2e_render.rs`) construct their `App` via `build_app`, so this
  single edit covers both.
- The brief's "screenshot must be VISUALLY UNCHANGED" — interpreted as
  the e2e gate values (emissive / solid / sky luminance) staying within
  the existing thresholds. The PNG bytes differ run-to-run because the
  Phase-B GI pipeline has documented per-run numerical drift (the
  baseline run reproduces this without W0 changes too). The gate values
  match pre-W0 (emissive 247.0, solid 242.0–242.1, sky 145.9 — same
  across baseline + W0 runs).
- The "1 test" `construction_params_layout` runtime test mirrors the
  9 compile-time `const _: () = assert!(...)` guards. Test runs in
  ~microseconds, exists so a refactor that strips the const-asserts
  (theoretical future tooling rework) still has a runtime failure
  signal.
- W0's `prepare_construction` body is `init_resource`-on-missing only,
  which runs every frame in `Render` schedule. The two `Option<Res<…>>`
  checks are cheap; W1..W5 add real cost only when they fill the body
  with their resource builds.

### Verification

- **Build:** `cargo build -p bevy-naadf` — clean, 0 errors, 0 warnings
  on Phase-C-touched files. The single remaining workspace warning
  (`texture_array/saver.rs:146` `repeat().take()` lint) is pre-existing
  on main (reproduced on `main` HEAD `409cce0`); not in W0 scope.
- **Tests:** `cargo test -p bevy-naadf` → **55 passed, 1 ignored** (54
  baseline + the new `construction_params_layout` test). Doc-tests
  pass. Full workspace: `cargo test` → 68 passed, 5 ignored across
  10 suites.
- **e2e:** `cargo run --bin e2e_render` exits 0; gate values
  `emissive 247.0, solid 242.0, sky 145.9` (functionally identical to
  pre-W0's `emissive 247.0, solid 242.1, sky 145.9` — within the
  documented GI-pipeline non-determinism). All Phase-B / TAA / e2e
  checks PASS.
- **e2e with the new flag:** `cargo run --bin e2e_render --
  --validate-gpu-construction` exits 0; emits the
  `phase-c W0 seam — gpu construction validation placeholder (no-op
  until W1 lands)` log line after the e2e exit. Total e2e runs in the
  W0 workstream: 3 (within the ≤3 cap — the third is the
  `--validate-gpu-construction` smoke).
- **Screenshot:** Saved at
  `target/e2e-screenshots/e2e_latest.png`; visually unchanged vs.
  pre-W0 baseline (same gate luminance values, same scene topology).
  Per-run byte-level PNG hash differs across all three runs
  (baseline-1, baseline-2, W0) — the Phase-B GI numerical noise is
  the cause; this was confirmed by running the baseline twice from
  `main`. The gate-value invariant is the real "unchanged" signal.

### Seam contract (for downstream workstreams)

W0 exposes the following surface; every later Phase-C workstream extends
through it. **Hard rule:** no Phase-C workstream re-edits the seam itself
(this `mod.rs` / `config.rs` skeleton + the chain placeholder). Each
workstream extends fields / system bodies / pipeline registrations only.

| seam | who extends | what to add |
|---|---|---|
| `ConstructionGpu.{segment_voxel_buffer, block_voxel_count, hash_map, hash_coefficients}` | **W1** | Replace `Option<Buffer>` with `Option<GrowableBuffer<T>>` / `Option<Buffer>` as needed; allocate in `prepare_construction` after W0's `init_resource` shells exist. |
| `ConstructionGpu.{bound_queue_info, bound_group_queues, bound_group_masks, bound_refined_info, bound_dispatch_indirect}` | **W3** | Allocate fixed-size buffers; respect the wgpu `STORAGE_READ_WRITE` × `INDIRECT` split on `bound_dispatch_indirect`. |
| `ConstructionGpu.{changed_*_dynamic}` | **W2** | Growable upload buffers; per-frame upload from `ChangeHandler` CPU port. |
| `ConstructionGpu.{entity_*}` | **W4** | Gate on `ConstructionConfig.entities_enabled`. Owns the chunks texture format flip from `R32Uint` to `Rg32Uint` (see §1.7 of `15-design-c.md`); W0's `STORAGE_BINDING` usage flag already allows the format flip. |
| `ConstructionBindGroups.construction_world` | **W1** | Build once `ConstructionGpu`'s W1 fields exist + `WorldGpu`'s `blocks` / `voxels` exist; the parallel-to-`world_layout` bind group. |
| `ConstructionBindGroups.{construction_bounds, bound_dispatch}` | **W3** | The bound-queue `@group(1)` + the one-binding `bound_dispatch_indirect` write-side layout. |
| `ConstructionBindGroups.construction_change` | **W2** | The change-staging `@group(1)`. |
| `ConstructionBindGroups.construction_entity` | **W4** | The entity-track `@group(1)`. |
| `ConstructionPipelines` (empty resource) | **W1..W5** | Add fields one per workstream. The `Default` impl currently satisfies `FromWorld`; W1 adds the first `FromWorld` body with real layouts. |
| `prepare_construction` body | **W1..W5** | Each workstream `if let Some(gpu) = …` checks its own field set, allocates / resizes / builds bind groups. Empty body in W0; bodies merge cleanly via the `Option<>` swap pattern. |
| `run_gpu_construction_startup` body | **W1** | Replace the gated `info!` placeholder with the regime-1 dispatch chain (generator → chunk_calc → bounds_init) + the bit-exact CPU/GPU oracle assert. |
| `Core3d` chain (commented placeholder block in `render/mod.rs`) | **W2 / W3 / W4** | Each workstream un-comments its row of the placeholder and inserts its `naadf_*_node` system above `naadf_atmosphere_node`. |
| `--validate-gpu-construction` CLI flag | **W1** | Replace the placeholder log line with the real GPU vs. CPU `aadf::construct::construct` byte-equality assertion on `GridPreset::Default` after the e2e exits Success. |
| `GpuConstructionParams` (already populated) | **W1..W5** | Add fields if more uniform scalars are needed; respect the `offset_of!` guard discipline. Current 80 B layout already covers every shader's identified scalar (`15-design-c.md` §1.8). |
| Chunks texture `STORAGE_BINDING` usage (already added) | **W1 / W2 / W3** | The W0 widening already allows the compute writes; no further `prepare.rs` change needed. W4 additionally flips the texture format to `Rg32Uint` (its own merge). |

W0 is the smallest possible seam-only PR. The next merges in dependency
order are **W6 (CPU-only `aadf/bounds.rs` rewrite) ‖ W5 (generator) →
W1 (Algorithm 1) → W3 (background queue) → W4 (entities, chunks format
flip) → W2 (editing)**; see `15-design-c.md` §2.2 for the wave plan.
