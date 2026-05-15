# 16 — Phase C impl log — W5 (worldgen)

## W5 — World generator (2026-05-15)

The wave-1b worldgen workstream of Phase C: port NAADF's
`generatorModel.fx` to WGSL, build the `aadf::generator::generate_segment_cpu`
CPU oracle, and wire the W5 pipeline into the W0 seam without disturbing the
production CPU world-build path (`15-design-c.md` §2.1 W5 row, §4.5, §1.6;
`16-impl-c-W0.md` "Seam contract" row "W5: `generator_model_pipeline`").

W5 is the FIRST step of NAADF's regime-1 startup construction
(`generator → chunk_calc → bounds_init` per §1.2 / §3). Until W1 lands, the
W5 path is exercised only via the bit-exact unit test
(`render::construction::tests::generator_model_gpu_vs_cpu_bit_exact`); the
production CPU world-build path remains untouched.

### Changes by file

**New files (3):**

- `crates/bevy_naadf/src/aadf/generator.rs` (~480 lines) — `ModelData` +
  `generate_segment_cpu` CPU oracle + per-thread `get_voxel_type_in_model`
  port. Faithful translation of `generatorModel.fx:16-72` line-for-line:
  - `ModelData` mirrors NAADF's `World/Model/ModelData.cs:23-31` byte layout
    (flat `data_chunk` / `data_block` / `data_voxel` + `size_in_chunks`).
  - `get_voxel_type_in_model` ports HLSL `:16-52` (the 30-bit discriminator
    walk + the Y-clamp at `:48-49`).
  - `generate_segment_cpu` ports the workgroup body `:54-72`: 32 iterations ×
    2 voxels per thread → 2048 u32s per chunk, packed `voxel1 | (voxel2 << 16)`,
    full-flag at `:67-68`.
  - 5 unit tests: `empty_model_produces_zeros`,
    `generator_model_cpu_deterministic`, `generator_model_y_clamp_above_model`,
    `generator_model_oob_voxels_clamp_to_zero`, `generator_model_mixed_single_voxel`.
- `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl` (~115 lines) —
  WGSL port of `generatorModel.fx`. One entry point
  `fill_chunk_data_with_model_data_16`, `numthreads(4,4,4)`. Bindings exactly
  as `15-design-c.md` §4.5 prescribes. Faithful line-for-line port: the HLSL
  `>> 30` discriminator branches, the open-ended-`if`/`else if` chain, the
  Y-clamp, the full-flag pattern, and the `chunk_data[group_index * 2048 +
  local_index * 32 + i]` write site all match the HLSL byte-for-byte. WGSL
  conventions: `RWStructuredBuffer<uint>` ↔ `var<storage, read_write> array<u32>`;
  `StructuredBuffer<uint>` ↔ `var<storage, read>`; flat `Effect.Parameters`
  collapse into one `GeneratorModelParams` uniform (the same simplification
  `15-design-c.md` §1.8 makes for `GpuConstructionParams`).
- `crates/bevy_naadf/src/render/construction/generator_model.rs` (~260 lines)
  — the W5 pipeline + bind-group-layout helpers + `dispatch_generator_model`
  entry point. Hosts:
  - `GpuGeneratorModelParams` — Rust mirror of the WGSL `GeneratorModelParams`
    struct, 64 B = 4 × 16-byte rows. 7 compile-time `const _: () = assert!(...)`
    layout guards (size + per-row offset + `% 16 == 0` on every `vec3<u32>`
    field) per `15-design-c.md` §1.5 discipline. Runtime mirror in
    `generator_model_params_layout` test.
  - `generator_model_layout_descriptor()` — builds the `@group(0)` layout per
    §4.5 (chunk_data_rw + 3 model_data_ro + params uniform).
  - `queue_generator_model_pipeline()` — production path, queues via
    `AssetServer`.
  - `queue_generator_model_pipeline_with_handle()` — test path, queues against
    an already-resolved `Handle<Shader>` (the headless test inlines the WGSL
    via `include_str!` rather than going through the asset loader).
  - `dispatch_generator_model()` — one regime-1 dispatch over
    `(group_size_in_chunks.x, .y, .z)` workgroups.
  - `GENERATOR_MODEL_SHADER_SRC` — `include_str!` of the WGSL file (used by
    the unit test fixture; the prod path still goes through the AssetServer).

**Edited files (3):**

- `crates/bevy_naadf/src/aadf/mod.rs` — added `pub mod generator;` + a 2-line
  doc comment pointing at this impl doc.
- `crates/bevy_naadf/src/render/construction/mod.rs` — flipped
  `ConstructionPipelines` from W0's empty-struct-with-`Default`-derived-`FromWorld`
  to a real two-field struct + explicit `FromWorld` impl that queues the W5
  pipeline. Added `pub mod generator_model;`. Added the load-bearing
  `generator_model_gpu_vs_cpu_bit_exact` test + a headless `render_fixture()`
  helper. The seam itself (the `ConstructionGpu`/`ConstructionBindGroups`
  fields, the `prepare_construction` body, the `run_gpu_construction_startup`
  body, the `ConstructionPlugin::build` wiring) is **byte-identical to W0** —
  W5 only adds fields to `ConstructionPipelines`. Per the W0 seam contract,
  no Phase-C workstream re-edits the seam itself.
- `crates/bevy_naadf/src/render/construction/config.rs` — added
  `run_worldgen_only: bool` field to `ConstructionConfig` (default `false`)
  + the compile-time-pin block entry. Per the W5 brief: "Until W1 lands,
  W5's path is callable but only via an explicit code-path triggered by a
  config flag (e.g. `ConstructionConfig.run_worldgen_only`)". The flag is
  declared but not currently consumed — the W5 isolation path is exercised
  via the unit test alone; the flag is reserved for W1's regime-1 driver to
  honour (W1's `run_gpu_construction_startup` body will check it to decide
  whether to dispatch only the generator step or the full chain).

**Not edited (by design):**

- `crates/bevy_naadf/src/render/pipelines.rs::NaadfPipelines` — explicitly
  off-limits per `15-design-c.md` §1.3 / §2.1 + the W0 seam contract.
  Construction pipelines live in `ConstructionPipelines`.
- `crates/bevy_naadf/src/aadf/{bounds,construct,cell}.rs` — W6 just rewrote
  `bounds.rs` and `construct.rs`'s Phase-3 layer; touching them risks
  masking W6's intent. W5 only adds the new `generator.rs` module.
- `crates/bevy_naadf/src/render/prepare.rs` — W0's chunks-texture
  `STORAGE_BINDING` widening is already in place; W5 doesn't touch chunks
  (W5 writes `segment_voxel_buffer`, not chunks).
- `crates/bevy_naadf/src/bin/e2e_render.rs` — W0's
  `--validate-gpu-construction` placeholder still applies until W1 wires the
  full validation. W5's GPU path is not yet on the production startup chain,
  so the placeholder remains a no-op log line.

### Decisions & rejected alternatives

1. **`ModelData` location: `crates/bevy_naadf/src/aadf/generator.rs` (chosen)
   vs `crates/bevy_naadf/src/voxel/model.rs` vs `crates/bevy_naadf/src/world/model.rs`.**
   Chose `aadf::generator` because `ModelData` is **directly consumed by the
   AADF construction chain** — the GPU shader's output is exactly the
   "segment voxel buffer" that `chunk_calc.fx` (W1) reads. The C# layering
   puts `World/Model/ModelData.cs` next to `World/Generator/WorldGeneratorModel.cs`,
   not under `World/VoxelType.cs`. The port mirrors that: `ModelData` lives
   with the generator that consumes it, in `aadf::generator`, not under
   `voxel::` (which is the typed-voxel / palette concern) or `world::`
   (which is the dense-volume / runtime concern). **Rejected** putting it in
   `voxel/model.rs` — would mix the typed `VoxelType`/`VoxelTypeId` palette
   with the flat `u32`-encoded byte arrays the generator reads; the two have
   different ownership semantics (`VoxelType` is colour/material metadata;
   `ModelData` is pre-encoded voxel hierarchy).
2. **GPU generator enablement: behind a config flag, off by default
   (chosen) vs unconditionally exercised at startup.** Chose flag-gated.
   The W5 brief is explicit: "DEFAULT: existing CPU world-build path stays
   the production path; W5 does NOT replace it. W1 flips the default to the
   GPU path." The `ConstructionConfig.run_worldgen_only` flag carries the
   intent declaratively. The unit test is the load-bearing exerciser; the
   flag is reserved for W1's driver to honour. **Rejected**
   unconditionally-on — would change the production producer to a half-
   built GPU chain (no chunk_calc behind it), which contradicts the
   "GPU path is dormant until W1" contract.
3. **`ConstructionPipelines` shape: real-fields struct + explicit `FromWorld`
   (chosen) vs `Default`-derived `FromWorld` + post-hoc append.** Chose
   real fields. W0 left `ConstructionPipelines` as a `#[derive(Default)]`
   empty struct so the seam compiled; W5 is the first workstream to add
   real pipelines. The seam contract (W0 doc, "field set planned per
   `15-design-c.md` §1.3") expects each workstream to add its fields one
   by one; my `FromWorld` impl is the entry point each later workstream
   (W1..W4) extends with two lines: a layout build + a pipeline queue,
   plus one field-add in the struct literal. **Rejected** a post-hoc field
   append via a separate `add_pipeline` method on `ConstructionPipelines`
   — would force `Mutex`/`Option<>` field types so the resource stays
   mutable post-`FromWorld`, fighting Bevy's expectation that
   pipeline-cache handles are immutable for the resource's lifetime.
4. **Test fixture: insert shader via `PipelineCache::set_shader` directly
   (chosen) vs run `ExtractSchedule` via `SubApp::extract` vs build an
   `App` with `DefaultPlugins`.** Chose direct `set_shader`. The Bevy 0.19
   `RenderPlugin` registers render-asset extract systems
   (`render_asset.rs:280`) that read `MessageReader<AssetEvent<A>>` for
   every `RenderAsset` type; with only `MinimalPlugins + AssetPlugin +
   ImagePlugin + RenderPlugin`, several of those messages are unregistered
   and the systems panic with "Message not initialized" on the first call
   to either `app.update()` or `sub_app.extract()`. `PipelineCache::set_shader`
   is `pub` precisely so headless tests can drive shader injection without
   the full schedule plumbing — it's what `extract_shaders` calls
   internally anyway. **Rejected** running `DefaultPlugins` — would force
   a winit window + full asset loader + every render-asset plugin to boot,
   making the test depend on platform-specific (X11/Wayland) availability
   in CI. The W5 test passes against any wgpu adapter wgpu's `Automatic`
   probe finds; no display required.
5. **Bind-group-layout choice: new dedicated `generator_model_layout`
   (chosen) vs reuse W0's-future `construction_world_layout`.** Chose
   dedicated. `15-design-c.md` §4.5 explicitly prescribes "a new
   `generator_model_layout` `@group(0)` with `chunk_data_rw`,
   `model_data_chunk_ro`, `model_data_block_ro`, `model_data_voxel_ro`,
   and a params uniform". The generator does NOT read/write `chunks` /
   `blocks` / `voxels` — those are the *output* of W1's `chunk_calc`, not
   the generator's. Reusing `construction_world_layout` would force-bind
   unused storage buffers + the construction-params uniform with fields
   the generator ignores; a dedicated 5-slot layout is the smaller, more
   self-documenting choice. The §1.3 "borderline calls" wgpu-rule
   concerns (`STORAGE_READ_WRITE` × `STORAGE_READ_ONLY` on the same
   buffer) don't apply: `segment_voxel_buffer` is read-write in the
   generator (write-only really, but the binding type lets the next
   workstream read+write it from chunk_calc) and read-only in W1's
   `construction_world_layout` — *two layouts over the same underlying
   buffer*, the standard parallel-layout pattern §1.3 establishes.
6. **WGSL `local_invocation_index` vs computed `lx + ly*4 + lz*16`.**
   Chose the built-in `local_invocation_index`. WGSL's
   `@builtin(local_invocation_index)` returns exactly `lx + ly*nx +
   lz*nx*ny` for `numthreads(nx,ny,nz)` (verified against the WGSL spec
   §6.4.6), which matches HLSL's `SV_GroupIndex`. The CPU oracle
   re-derives the same value at `let local_index = lx + ly * 4 + lz * 16;`
   to keep the byte-for-byte parity sturdy without trusting the (per-API
   identical) builtin formula.
7. **Inlined `include_str!` shader source.** The test bakes the WGSL into
   the test binary via `include_str!("../../assets/shaders/generator_model.wgsl")`.
   This serves three purposes: (a) the test runs without a working asset
   loader; (b) a typo in the asset path fails to *compile* the test
   binary, not at test run time; (c) the test is self-contained — it
   doesn't depend on the asset directory layout, so future repository
   reorganisation doesn't silently skip the test. The production path
   still loads the WGSL via `AssetServer::load(GENERATOR_MODEL_SHADER)`
   — both readers see the same bytes.

### Assumptions made

- **The `MinimalPlugins + RenderPlugin` headless fixture works in this
  environment.** Verified against `crates/bevy_naadf/src/world/buffer.rs`'s
  test fixture, which uses the same pattern + has passed for the entire
  Phase-A timeline. Test passes with the NVIDIA RTX 5080 + Vulkan backend
  the production app uses.
- **`synchronous_pipeline_compilation = true` + manual `process_queue()`
  drives compile-to-completion in one tick.** Verified: my test loops 0
  iterations of the 64-iteration cap (first `process_queue` returns the
  ready pipeline). The cap exists for defensive headroom only.
- **The baseline test count is 59 (as `main` HEAD `564a1f4` reports
  before W5 lands), not 58.** The W5 brief states "current main has 58
  tests (54 baseline + W0's 1 + W6's 3); add yours, all must pass. Target:
  60+." Actual: 59 on main (`cargo test -p bevy-naadf --lib` against
  `main`). W5 adds 7 tests → 66 total, which clears the 60+ target.
  Likely the brief's "58" is one off because W6's `aadf_layer_speedup_at_scale`
  is `#[ignore]`-marked (it shows up in the "1 ignored" count of `66
  passed, 1 ignored`, not the "passed" count).
- **The W5 brief's "byte-for-byte" requirement on the GPU/CPU oracle
  is met by `Vec<u32> == Vec<u32>` equality.** WGSL writes `u32`s in
  little-endian on every backend wgpu supports today (Vulkan / Metal /
  D3D12 / WebGPU); the readback via `Buffer::map_async + cast_slice` and
  the CPU oracle's `Vec<u32>` are both little-endian on this platform.
  The compile-time `cfg!(target_endian = "little")` is implicit (all wgpu
  target-platforms are little-endian).
- **Y-clamp interpretation of `generatorModel.fx:48-49`.** The HLSL is
  `if (modelIndexY > 0) type = 0;` where
  `modelIndexY = voxelPos.y / (modelSizeInChunksY * 16);`. Both the WGSL
  port and the CPU oracle replicate this exactly. The clamp's effect is
  that only the *ground-level* copy of the model materialises vertically
  — anything above the model's Y extent (modulo the model height)
  becomes empty. This is consistent with NAADF's "model is stamped
  horizontally, ground-level only" worldgen semantics
  (`WorldGeneratorModel.cs` + `WorldData.cs:GenerateWorld`).
- **`ConstructionConfig.run_worldgen_only` is declared in W5, consumed
  in W1.** The W5 brief allows two paths: (a) unconditionally run the
  generator at Startup behind the flag, or (b) make the flag declarative
  + exercise via test only. I took option (b) for two reasons: option
  (a) requires `run_gpu_construction_startup` to grow a full RenderQueue
  command-buffer submission against test buffers, which is W1's territory
  per `15-design-c.md` §1.2 regime 1, and the W0 doc explicitly says
  "W1 fills the body" of that startup; (b) keeps W5 single-purpose
  (port the shader + ship the oracle).
- **`ConstructionPipelines` `FromWorld` runs against `init_gpu_resource`
  at `RenderStartup`.** The W0 plugin registers
  `init_gpu_resource::<ConstructionPipelines>()`; with W5's real
  `FromWorld` impl, this triggers at the same point in the schedule
  W0 already wires it. The `AssetServer::load` + `PipelineCache::queue_compute_pipeline`
  calls inside `from_world` are non-blocking — the actual compile
  happens later in `PipelineCache::process_pipeline_queue_system`, the
  same way every existing pipeline in `NaadfPipelines::from_world` works.

### Verification

- **Build (`cargo build -p bevy-naadf`):** clean, 0 errors, 0 warnings on
  Phase-C-touched files. The single remaining workspace warning
  (`texture_array/saver.rs:146` — `repeat().take()` lint) is pre-existing
  on `main` and reproduced on the W6 baseline; not in W5 scope.
- **Tests (`cargo test -p bevy-naadf --lib`):** **66 passed, 1 ignored**
  (59 baseline + W5's 7 new). Full workspace `cargo test`: 79 passed, 6
  ignored across 10 suites. All Phase-A / A-2 / B / W0 / W6 tests stay
  green.
- **Load-bearing W5 gate — `render::construction::tests::generator_model_gpu_vs_cpu_bit_exact`:**
  PASS. Test setup: 2×1×2 chunk model, uniform-full of type `0x42`,
  queried over a 2×1×2 chunk segment. Buffer: 2×1×2 chunks × 2048 u32s
  = 8192 u32s = 32 KiB. Both the GPU pipeline and the CPU oracle
  produce identical 32 KiB of packed-voxel u32s; the test asserts
  `gpu_out == cpu_out` (byte-for-byte). Also spot-checks the first u32
  packs `0x8042 | (0x8042 << 16)` (type 0x42 with the full flag set,
  in both half-words). Bytes compared: **32 768 bytes** (8192 u32s × 4
  bytes). Result: byte-equal.
- **CPU oracle test suite — `aadf::generator::tests::*`:** 5 tests, all
  PASS: `empty_model_produces_zeros`, `generator_model_cpu_deterministic`
  (two calls produce identical output), `generator_model_y_clamp_above_model`
  (model 1×1×1 over 1×2×1 segment — Y-clamp clears the upper chunk),
  `generator_model_oob_voxels_clamp_to_zero` (segment exceeds
  `sizeInVoxels` — the OOB branch at `generatorModel.fx:18-19`
  short-circuits to type 0), `generator_model_mixed_single_voxel`
  (verifies the `>> 30` mixed-block walk + the even/odd voxel slot
  branch at `:40`).
- **Layout-pin test — `render::construction::generator_model::tests::generator_model_params_layout`:**
  PASS. Runtime mirror of the 7 `const _: () = assert!(...)` guards
  above the struct; ensures `GpuGeneratorModelParams` is 64 B with the
  correct field offsets (size_in_voxels @ 0, model_size_in_chunks @ 16,
  group_offset_in_chunks @ 32, group_size_in_chunks_x @ 44,
  group_size_in_chunks_y @ 48).
- **e2e (`cargo run --bin e2e_render`):** PASS. Gate values
  `emissive 247.0, solid 242.1, sky 145.9` — identical to the pre-W5
  baseline (W0+W6's `emissive 247.0, solid 242.0–242.1, sky 145.9`).
  Screenshot at `target/e2e-screenshots/e2e_latest.png` visually
  unchanged: same scene topology, same gate luminance values.
- **e2e validate flag (`cargo run --bin e2e_render -- --validate-gpu-construction`):**
  PASS. Exits 0, emits the W0 placeholder log line "phase-c W0 seam —
  gpu construction validation placeholder (no-op until W1 lands)" after
  the e2e exit. Gate values: `emissive 247.0, solid 242.0, sky 145.9`
  — identical to the W0 baseline. W5 does not change the e2e path; the
  GPU generator runs only inside the unit test.

### Seam contract update

W5 modifies the W0 seam in the following ways (downstream workstreams /
the integration agent will need to be aware):

| seam element | W0 state | W5 state |
|---|---|---|
| `ConstructionPipelines` | empty struct, `#[derive(Default)]` | two real fields: `generator_model_layout: BindGroupLayoutDescriptor`, `generator_model_pipeline: CachedComputePipelineId`; explicit `FromWorld` impl |
| `ConstructionPipelines::from_world` | derived (via `Default`) | builds the W5 layout + queues the W5 pipeline against it. W1/W2/W3/W4 each extend the body with two lines + one struct-literal field-add |
| `ConstructionConfig.run_worldgen_only` | absent | declared `bool`, default `false`. Reserved for W1's `run_gpu_construction_startup` body to honour |
| `ConstructionGpu.segment_voxel_buffer` | `Option<Buffer>::None` | UNCHANGED — W5 allocates `segment_voxel_buffer` only inside its unit test against test buffers; W1 owns the production allocation per the W0 contract |
| `ConstructionBindGroups.*` | `Option<BindGroup>::None` for all | UNCHANGED — W5's test builds an ad-hoc one-shot bind group; the production bind groups are W1's responsibility |
| `prepare_construction` body | `init_resource` shells only | UNCHANGED — W5's pipeline-build happens in `ConstructionPipelines::from_world` (one-shot at `RenderStartup`), not in `prepare_construction` (per-frame) |
| `run_gpu_construction_startup` body | gated-no-op + `info!` placeholder | UNCHANGED — W5 deliberately does NOT populate the body; the generator runs at startup only in W1's merge per the W0 contract |
| `Core3d` chain in `render/mod.rs` | three commented TODO node placeholders | UNCHANGED — W5 is a startup-schedule one-shot, not a `Core3d` node |
| chunks texture `STORAGE_BINDING` usage flag | added by W0 | UNCHANGED — W5 writes `segment_voxel_buffer`, not `chunks` |
| `e2e_render --validate-gpu-construction` flag | placeholder log line | UNCHANGED — W1 replaces with the real bit-exact assertion |

**Public API additions** for W1 to consume:
- `crate::aadf::generator::ModelData` — the three-layer model the generator reads.
- `crate::aadf::generator::generate_segment_cpu(...)` — the CPU oracle.
- `crate::render::construction::generator_model::GpuGeneratorModelParams` — the
  Rust mirror of the per-dispatch uniform.
- `crate::render::construction::generator_model::generator_model_layout_descriptor()`
  — produces a fresh `BindGroupLayoutDescriptor` for binding.
- `crate::render::construction::generator_model::dispatch_generator_model(...)`
  — one regime-1 dispatch helper for W1's `run_gpu_construction_startup` body.
- `crate::render::construction::generator_model::create_storage_buffer_u32(...)`
  / `create_params_uniform(...)` — small helpers W1 may reuse for its
  segment-voxel-buffer allocation.

The seam stays additive: every Phase-C workstream after W5 can land its row
without re-editing W5's fields. The `FromWorld` impl on `ConstructionPipelines`
is the new collaborative entry point; the seam contract from W0 otherwise
holds verbatim.
