# 02b — Design: Plan B — WGSL FastNoiseLite + per-frame W5 residency

Architect: `delegate-architect` (2026-05-18). Brief: `docs/orchestrate/streaming-world/README.md` § Plan-B addendum + this file's dispatch brief. Reuse palette: `00-reuse-audit.md`. v1 trace: `02-design.md`.

This is a **redesign** of the original design (`02-design.md`, "Plan A"). The user redirected at the hard gate. Plan A chose CPU noise on `bevy_tasks` + a W5 bypass (D.2, D.10) and accepted "empty patches under fast traversal" (D.7) as the price. The user redirected because GPU noise is ~30–100× faster per segment — eliminating the cold-start cost that Plan A's failure mode was driven by — and because the user wants to author noise in WGSL.

**Plan B keeps Q1–Q3 (i32 residency-only widening, per-segment residency, per-chunk-local dedup).** **Plan B overrides Q4 (noise backend):** noise is a WGSL FastNoiseLite port, not the `voxel_noise` crate. W5's GPU producer goes from "dead code in streaming preset" to **the primary GPU producer**, gated per-frame instead of once-at-startup.

**The impl is staged into two phases** (user directive — "wgsl noise goes first"):

- **Phase 1 (`03a-impl-wgsl-noise.md`):** WGSL port + Rust CPU oracle + headless CPU↔GPU oracle test + new `--wgsl-noise-oracle` e2e gate. **No residency code, no preset wiring, no W5 changes.** Phase 1 ships when the oracle gate is green.
- **Phase 2 (`03b-impl-residency.md`):** residency manager + W5 gate inversion + `GridPreset::ProceduralStreaming` + `--streaming-window` e2e gate.

Section ordering is required reading order. `## Δ-StreamingResidency`, `## Decisions & rejected alternatives`, and `## Assumptions made` after `## Design` are load-bearing.

---

## Design

### A. WGSL FastNoiseLite port (Phase 1 deliverable)

#### A.1 Scope — what gets ported, what defers

`FastNoiseLite.glsl` is ~2400 LOC and ships **all** of: 4 noise families × 2/3D × FBM/Ridged/PingPong fractals × 2 domain-warp modes × cellular distance/return modes. Porting the whole surface is gold-plating against this session's goal.

**Phase-1 ported surface (≈ 800–1200 LOC of WGSL):**

| Group | Functions | Why ported |
|---|---|---|
| Util/math helpers | `_fnlFastMin/Max/Abs/Sqrt`, `_fnlFastFloor/Round`, `_fnlLerp`, `_fnlInterpHermite`, `_fnlInterpQuintic`, `_fnlCalculateFractalBounding` | Required by everything below. |
| Hash | `_fnlHash3D`, `_fnlValCoord3D` | Required by 3D noise + value-coord lookups. |
| Gradient | `_fnlGradCoord3D` | Required by Perlin-3D + OpenSimplex2-3D. |
| 3D noise singles | `_fnlSingleOpenSimplex23D`, `_fnlSinglePerlin3D` | The two we expose at Phase 1. OpenSimplex2 is what FastNoiseLite recommends; Perlin gives us a second non-overlapping algorithm for the test matrix. |
| 3D coord transform | `_fnlTransformNoiseCoordinate3D` | Applies frequency scaling + OpenSimplex2 skew (load-bearing for OpenSimplex2 — without it, output is wrong). |
| 3D fractal wrapper | `_fnlGenNoiseSingle3D` (limited to the 2 noise types above), `_fnlGenFractalFBM3D` | FBM is the standard "terrain noise" combinator. |
| Public surface | `fnl_state` struct (subset of fields), `fnlCreateState`, `fnlGetNoise3D` | One entry point the streaming-side dispatch shader composes. |

**Deferred (NOT in Phase 1):**

- 2D variants (`*2D` family). Streaming is volumetric; 2D variants ride into a later session if a heightmap noise preset is requested.
- OpenSimplex2-Smooth (`_fnlSingleOpenSimplex2S3D`), Value (`_fnlSingleValue3D`), Value-Cubic (`_fnlSingleValueCubic3D`).
- Cellular (`_fnlSingleCellular3D`) — large code surface; not needed for a terrain demo.
- Domain warp (`_fnlDoSingleDomainWarp*`, `_fnlDomainWarpFractal*`, `fnlDomainWarp3D`) — the entire `inout` warp machinery is the porting risk concentration. Defer.
- Ridged/PingPong fractals.

The deferred functions are listed in `02b-design-plan-b.md` so a future session can pull them in incrementally without redesign. The WGSL module's internal structure follows the GLSL function order, so adding deferred functions is paste-with-`fn`-keyword-substitution work.

#### A.2 Module layout

- **Shader file:** `crates/bevy_naadf/src/assets/shaders/noise_fastnoiselite.wgsl` (~800–1200 LOC).
  - Co-located with the other `.wgsl` files (`generator_model.wgsl`, `chunk_calc.wgsl`, `bounds_calc.wgsl`, etc.) under `src/assets/shaders/`, matching the project convention at `crates/bevy_naadf/src/assets/shaders/generator_model.wgsl:55-59` (asset path `shaders/<name>.wgsl`).
  - Asset path: `shaders/noise_fastnoiselite.wgsl`.
  - `pub const NOISE_FASTNOISELITE_SHADER_SRC: &str = include_str!("../../assets/shaders/noise_fastnoiselite.wgsl");` exported from the consumer's Rust module (per `generator_model.rs:55-59` pattern).
- **Consumer module (Rust) — Phase 1:** `crates/bevy_naadf/src/streaming/noise_fastnoiselite.rs` (~150 LOC). Holds the shader const, a `dispatch_noise_oracle_*` helper, and the GPU `fnl_state` uniform mirror.
- **CPU oracle module:** `crates/bevy_naadf/src/streaming/noise_fastnoiselite_cpu_oracle.rs` (~250–400 LOC). A Rust port of the same GLSL functions; bit-for-bit reference for the test.
- **Phase-1 root:** `crates/bevy_naadf/src/streaming/mod.rs` (~30 LOC) — declares `pub mod noise_fastnoiselite;` and `pub mod noise_fastnoiselite_cpu_oracle;`. Phase 2 extends this with `pub mod residency;` etc.

The `streaming` module exists even at Phase 1 to give the WGSL noise + oracle code a clean home. Phase 2 just adds peer modules; no Phase-1 file moves.

#### A.3 API surface (Phase 1)

WGSL public surface — what other shaders import:

```wgsl
// In noise_fastnoiselite.wgsl:

// Subset of `fnl_state`. Only the fields we actually consume in Phase 1.
struct fnl_state {
    seed: i32,
    frequency: f32,
    noise_type: i32,    // 0 = OpenSimplex2, 1 = Perlin
    fractal_type: i32,  // 0 = None, 1 = FBM
    octaves: i32,
    lacunarity: f32,
    gain: f32,
    weighted_strength: f32,
    // Derived in fnlCreateState (Rust uploads it, not on-GPU):
    fractal_bounding: f32,
    // 3D rotation type fixed at 0 (none) for Phase 1.
}

fn fnl_create_state(seed: i32) -> fnl_state { ... }
fn fnl_get_noise_3d(state: fnl_state, x: f32, y: f32, z: f32) -> f32
```

The Phase-1 surface mirrors the GLSL `fnlGetNoise3D(state, x, y, z) -> float`. `fractal_bounding` is precomputed Rust-side rather than on-GPU because (a) the GLSL `_fnlCalculateFractalBounding` is a loop on `state.octaves` and depending on uniform iteration counts is fragile across WGSL backends, (b) `state` is uploaded as a uniform so the field is essentially-free, (c) the Rust CPU oracle has the same computation co-located, eliminating one porting drift surface.

**Convention deviations to address explicitly:**

| GLSL form | WGSL port |
|---|---|
| `inout float xo`, `out float xo` | All `out`/`inout` are returned via tuple or struct; no `ptr<function, T>` (verbose for Phase-1 surface). Example: `_fnlGradCoordOut3D` → `fn fnl_grad_coord_out_3d(seed, xp, yp, zp) -> vec3<f32>`. |
| `int mod` on potentially-negative ints | The hash chain (`_fnlHash3D`) uses only XOR/multiply, no `mod` (confirmed by WebFetch inventory: "Negative-Integer Modulo Usage: None detected"). **No `floor_mod` helper needed.** |
| `int * int` xor chain (hash) | `i32 * i32` in WGSL wraps mod 2^32 — same as GLSL. Bit-identical. |
| `bitcast<u32>(f32)` for float-to-bits | Not used by FastNoiseLite (hash is int-only). |
| `static const float GRADIENTS_3D[]` | `const GRADIENTS_3D: array<f32, N> = array<f32, N>(...)`. Arrays of f32 must be initialised inline; no module-scope `var` for these. |
| `float foo(float x)` | `fn foo(x: f32) -> f32`. Mechanical sed. |
| GLSL `float` aliased as `FNLfloat` (sometimes `double`) | `f32`. Phase 1 is f32-only. (The GLSL ifdef'd `FNL_USE_DOUBLES` path stays unported.) |

#### A.4 Determinism

Same GPU + same driver should be bit-deterministic (the noise function is pure arithmetic, no atomics, no race). **Cross-GPU determinism is NOT required** — this codebase is not a deterministic-multiplayer engine. Flagged in `## Assumptions made`.

The CPU oracle ports the same GLSL arithmetic in the same order; on the test harness's CPU and on the test GPU, the f32 round-off should be `< 1e-5` (tolerance set in § B.2). Different CPUs reorder mul-adds slightly; different GPUs do too — the test tolerance absorbs both.

---

### B. CPU↔GPU oracle test (Phase 1 verification)

This is the load-bearing safety net for the WGSL port. Phase 1 ships when this is green.

#### B.1 Why a Rust port of the same GLSL file, not `voxel_noise`

The brief lays out two options for the CPU reference:

- **Option (i): `voxel_noise`'s `fastnoise2` backend** as the CPU reference. **Rejected.** FastNoise2 ≠ FastNoiseLite. Same author (Jordan Peck / Auburn), different libraries, different algorithm details (FastNoise2 has a node graph + SIMD-vector batch path; FastNoiseLite is a single-header scalar implementation with different gradient tables). A "near-identical" tolerance would need to be loose enough to absorb algorithmic differences — gutting the test.
- **Option (ii): Port the same `FastNoiseLite.glsl` to Rust as a tiny CPU oracle.** **Chosen.** ~250–400 LOC of mechanical translation: mostly `fn foo(x: f32) -> f32` renames, `inout`/`out` → tuple returns, `float` → `f32`. The two ports compute the same operations in the same order, so cross-port drift is bounded by f32 round-off only.

The CPU oracle module is **test-only by intent** (no `#[cfg(test)]` gating because the production `--wgsl-noise-oracle` e2e gate consumes it too, but it is never wired into the runtime renderer in Phase 2).

#### B.2 Test shape — headless compute readback (`cargo test --workspace --lib`)

A `#[test]` unit test in `crates/bevy_naadf/src/streaming/noise_fastnoiselite.rs::tests`. Following the canonical pattern at `crates/bevy_naadf/src/render/construction/mod.rs:3071-3120` (`validate_gpu_construction`):

1. Boot a `MinimalPlugins + AssetPlugin + RenderPlugin { synchronous_pipeline_compilation: true }` headless app (no `WinitPlugin`, no window).
2. Insert the WGSL noise shader via `Assets<Shader>::add` + `PipelineCache::set_shader` (per `mod.rs:3110-3119`).
3. Allocate two storage buffers: `output: array<f32, N>` (write) + `sample_points: array<vec4<f32>, N>` (read).
4. Build a thin dispatch shader (~30 LOC) `noise_oracle_dispatch.wgsl` that calls `fnl_get_noise_3d(state, sp.x, sp.y, sp.z) -> output[i]` per sample point. State is a uniform.
5. Queue + compile the pipeline; `process_queue` poll loop (per `mod.rs:3234-3249`).
6. `RenderQueue::write_buffer(sample_points, ...)`; dispatch; map-read `output`; CPU `process_queue` poll until ready.
7. Loop over the ~64 sample points and assert `(wgsl_output[i] - cpu_oracle::fnl_get_noise_3d(state, p)).abs() < TOLERANCE` with `TOLERANCE = 1e-5`.

Sample points: 64 chosen to cover (a) the unit cube corners `(±1, ±1, ±1)`, (b) origin `(0, 0, 0)` and small offsets, (c) **negative coordinates** (`(-100, -50, -25)` etc — the hash function's wrap behaviour on negatives is the most porting-error-prone region), (d) `(100, 50, 25)` and similar mid-range. Same fixed-seed (`1337`), several noise/fractal-type combos `[OpenSimplex2 single, OpenSimplex2 FBM, Perlin single, Perlin FBM]` per point — 64 × 4 = 256 GPU samples.

`TOLERANCE = 1e-5` justification: f32 has ~7 decimal digits; one mul-add reordering can lose 1–2 ulps ≈ 1e-7 relative; FBM with 4 octaves accumulates ~4× that. `1e-5` is a generous safety margin while still trapping a real algorithmic divergence.

#### B.3 Edge-coherency test (a second unit test)

Modelled on `crates/voxel_noise/src/lib.rs:138-214`'s `test_adjacent_chunk_edge_coherency`:

Two virtual chunks A and B share a boundary plane. Compute noise across both chunks' voxel extents and assert the boundary samples match bit-identically (this is a "chunk A's right-edge equals chunk B's left-edge" test). For a noise function that is a pure function of `(x, y, z)`, this is a sanity check (it can only fail if the WGSL implementation accidentally depends on chunk-relative coordinates, e.g., by truncating instead of flooring). Confirms our port is grid-coherent — the property the streaming layer (Phase 2) inherits for free.

**Pass criterion:** zero mismatches at `1e-6` tolerance over the shared face.

---

### C. New named e2e gate for Phase 1 — `--wgsl-noise-oracle`

Per `CLAUDE.md`: new runtime behaviour requires an e2e gate. Phase 1's runtime behaviour is "WGSL noise dispatch produces values matching the CPU oracle". The gate proves this end-to-end.

**Module:** `crates/bevy_naadf/src/e2e/wgsl_noise_oracle.rs` (~100 LOC). Lives in `e2e/` because that's where the named gate handlers live (`vox_gpu_oracle.rs`, `oasis_edit_visual.rs`, etc.). **Caveat:** the existing e2e gates all boot a windowed `DefaultPlugins` app (see `e2e/mod.rs:359-370`). This gate does **not** need a window — it's a pure compute test. It follows the `render::construction::validate_gpu_construction` pattern (`mod.rs:3071-3290`) instead: a headless `MinimalPlugins + RenderPlugin` app, no winit.

The convention question: where does the dispatch live? Two options:

- **Option (i):** wire `--wgsl-noise-oracle` into `bin/e2e_render.rs`'s flag-ladder (per `e2e_render.rs:140-178`, the `--vox-gpu-oracle`/`--validate-gpu-construction-*` patterns short-circuit BEFORE the e2e winit boot). The flag returns its own `ExitCode`.
- **Option (ii):** make it a `cargo test` only.

**Chosen: Option (i)** — `--wgsl-noise-oracle` is a real named gate in the verification surface (so it appears in `CLAUDE.md`'s list of approved gates and the impl agent can run it). The body internally calls the same `streaming::noise_fastnoiselite::tests::run_wgsl_noise_oracle()` function the unit test uses, but reports its result via `ExitCode` instead of `assert!`.

`e2e_render.rs` flag ladder additions:

```rust
let wgsl_noise_oracle_mode = args.iter().any(|a| a == "--wgsl-noise-oracle");
// ...
if wgsl_noise_oracle_mode {
    match bevy_naadf::e2e::wgsl_noise_oracle::run_wgsl_noise_oracle() {
        Ok(report) => { eprintln!("WGSL noise oracle PASS: {report}"); return ExitCode::from(0); }
        Err(msg) => { eprintln!("WGSL noise oracle FAILED: {msg}"); return ExitCode::from(1); }
    }
}
```

Inserted ABOVE the existing `--validate-gpu-construction*` ladder block at `bin/e2e_render.rs:142-177` so it short-circuits before the windowed e2e boot — same structural shape.

**Gate body** (in `e2e/wgsl_noise_oracle.rs`):

- Boots headless render world (`MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin { synchronous_pipeline_compilation: true }`).
- Loads `noise_fastnoiselite.wgsl` + `noise_oracle_dispatch.wgsl` via `Shader::from_wgsl`.
- Allocates `output`, `sample_points`, `state_uniform` buffers (≤ 1 KiB total).
- Compiles + dispatches; map-read.
- Loops 256 samples (64 points × 4 noise/fractal combos). Counts mismatches > `1e-5`. Returns `Ok(format!("{N} samples bit-near-equal, max_diff = {x:.2e}"))` or `Err(msg)` on failure.

**Phase 1 ships when this gate is green.** No framebuffer captures, no luminance checks, no traversal — pure oracle equality.

---

### D. Phase-2 residency layer (mostly carried over from v1's § A, E, F, H)

Sections A.1–A.5 from `02-design.md` v1 (residency manager, window geometry, indirection table, VRAM budget enforcement, failure modes) **stay**. Re-read `02-design.md:11-135`. The diff vs v1 in the design surface:

- **v1 § A (Residency manager)** — **unchanged.** Reuse `Residency`, `WorldSegmentPos`, `SlotIndex`, `SlotState` shape verbatim (`02-design.md:29-71`). One small adjustment: `SlotState::Encoded(Box<EncodedSegment>)` is **dropped** (no CPU-side encoded segment now — GPU produces directly). Replace with `SlotState::Generating { dispatched_frame: u64 }` — Phase 2 detail.
- **v1 § B (CPU noise → chunk adapter)** — **REPLACED** by § E below (GPU dispatch pipeline).
- **v1 § C (W2 record synthesis)** — **MOSTLY UNCHANGED** but with the option to bypass for the admission path; see § F below.
- **v1 § D (Driver — disable W5)** — **INVERTED.** See § G below.
- **v1 § E (Coordinate widening)** — **unchanged.** `02-design.md:326-383` carries over verbatim.
- **v1 § F (GridPreset variant + CLI)** — **unchanged** except `noise_preset` is now an index into a Phase-1 WGSL preset (`SimpleTerrain` analogue), not a `voxel_noise::NoisePreset`. § J below.
- **v1 § G (`--streaming-window` e2e gate)** — **unchanged** in shape; assertion thresholds may shift slightly because GPU noise removes the cold-start arithmetic. See § K below.
- **v1 § H (`trait ChunkSource`)** — **carried as a Phase-2 forward-compat seam** but the Phase-2 impl has exactly one impl (`WgslNoiseChunkSource`); the trait stays a stub for future `VoxChunkSource` / `MinecraftChunkSource`.

### E. Phase 2 — WGSL noise → ModelData → W5 chain wiring

This replaces v1 § B (CPU noise on `bevy_tasks`). The streaming-side per-segment GPU dispatch chain becomes:

```
For each newly-admitted segment in Residency::admissions_this_frame:
  1) Encode segment uniform: (seg_origin_in_voxels: vec3<i32>, fnl_state).
  2) Dispatch noise_terrain.wgsl on `segment_voxel_buffer` (the W5's chunk_data_rw scratch).
     - Workgroups: `(SEGMENT_CHUNKS, SEGMENT_CHUNKS, SEGMENT_CHUNKS) = (16,16,16)`.
     - Each invocation produces 64 voxels' packed u32 chunk-data, identical bit-layout to generator_model.wgsl's output.
  3) On the SAME encoder, dispatch chunk_calc::dispatch_calc_block_from_raw_data_world_sized
     to populate WorldGpu.{chunks_buffer, blocks, voxels} from segment_voxel_buffer.
  4) submit() (per-segment submit, matching the W5 ordering-bug constraint inherited from
     mod.rs:2427-2453).
```

The W5 fixed `model_data_*` ladder (chunk → block → voxel lookup at `generator_model.wgsl:62-119`) is **bypassed**: instead of running `generator_model.wgsl` with model-data bindings, we run a **new** noise-driven shader `noise_terrain.wgsl` that produces the same `chunk_data_rw[group_index * 2048 + local_index * 32 + i]` byte layout (per `generator_model.wgsl:121-160`). Bypassing the model-data lookup eliminates:

- The 3 × `model_data_*_buffer` allocations (chunk/block/voxel — see `mod.rs:1550-1593`). For streaming these would have to be re-populated per segment, which defeats the point. Streaming-preset code path skips that.
- The `_pad`-laden `GpuGeneratorModelParams` ModelData layout (`generator_model.rs:74-119`). Replaced by a tighter noise-params uniform.

The W5 generator-model **bind-group layout** is left as-is (other tracks use it). The streaming track has its **own** bind-group layout. The two pipelines coexist on the same `segment_voxel_buffer` since their outputs are the same shape.

#### E.1 New shader: `noise_terrain.wgsl`

**Module:** `crates/bevy_naadf/src/assets/shaders/noise_terrain.wgsl` (~150 LOC).

```wgsl
#import "shaders/noise_fastnoiselite.wgsl"::{fnl_state, fnl_get_noise_3d}

struct NoiseTerrainParams {
    // Row 0 (16 B): segment origin in voxels (world-space).
    seg_origin_in_voxels: vec3<i32>,
    threshold: f32,                  // noise > threshold => solid voxel
    // Row 1 (16 B): group_size_in_chunks_x, group_size_in_chunks_y, terrain_voxel_type_id (low 15 bits), _pad.
    group_size_in_chunks_x: u32,
    group_size_in_chunks_y: u32,
    terrain_voxel_type_id: u32,
    _pad0: u32,
    // Row 2 (48 B): fnl_state. Aligned per std140; fnl_state is 36 B + 12 B pad = 48 B.
    state: fnl_state,
}

@group(0) @binding(0) var<storage, read_write> chunk_data_rw: array<u32>;
@group(0) @binding(1) var<uniform> params: NoiseTerrainParams;

@compute @workgroup_size(4, 4, 4)
fn fill_chunk_data_with_noise(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let group_index = group_id.x
        + group_id.y * params.group_size_in_chunks_x
        + group_id.z * params.group_size_in_chunks_x * params.group_size_in_chunks_y;

    for (var i: u32 = 0u; i < 32u; i = i + 1u) {
        let i2 = i * 2u;
        let vpib = vec3<u32>(i2 % 4u, (i2 / 4u) % 4u, i2 / 16u);

        // Per-voxel WORLD-SPACE position. Note the i32: we add a signed world-space
        // origin to the local segment-space u32 offset.
        let local_offset_v = vec3<i32>(
            i32(group_id.x * 16u + local_id.x * 4u + vpib.x),
            i32(group_id.y * 16u + local_id.y * 4u + vpib.y),
            i32(group_id.z * 16u + local_id.z * 4u + vpib.z),
        );
        let world_v1_i = params.seg_origin_in_voxels + local_offset_v;
        let world_v2_i = world_v1_i + vec3<i32>(1, 0, 0);

        // Convert to f32 for noise sample. Q1 says shader sees window-local coords —
        // BUT noise input is the segment's WORLD origin in voxels (so the noise field
        // is stable across window shifts). The renderer never sees these
        // world coords — only chunk_calc + bounds consume the segment_voxel_buffer.
        let w1 = vec3<f32>(f32(world_v1_i.x), f32(world_v1_i.y), f32(world_v1_i.z));
        let w2 = vec3<f32>(f32(world_v2_i.x), f32(world_v2_i.y), f32(world_v2_i.z));

        let n1 = fnl_get_noise_3d(params.state, w1.x, w1.y, w1.z);
        let n2 = fnl_get_noise_3d(params.state, w2.x, w2.y, w2.z);

        var voxel1: u32 = 0u;
        var voxel2: u32 = 0u;
        if (n1 > params.threshold) { voxel1 = (params.terrain_voxel_type_id & 0x7FFFu) | (1u << 15u); }
        if (n2 > params.threshold) { voxel2 = (params.terrain_voxel_type_id & 0x7FFFu) | (1u << 15u); }

        let dst = group_index * 2048u + local_index * 32u + i;
        chunk_data_rw[dst] = voxel1 | (voxel2 << 16u);
    }
}
```

Output layout is **byte-identical** to `generator_model.wgsl`'s `chunk_data_rw[group_index * 2048 + local_index * 32 + i] = voxel1 | (voxel2 << 16u)` (`generator_model.wgsl:157-158`). chunk_calc downstream consumes this byte layout — no chunk_calc changes.

#### E.2 Why this is "feeding the existing W5 GPU producer"

The brief says "WGSL noise feeds the existing W5 GPU producer per-frame for newly-resident segments". Reading the W5 dispatch chain (`mod.rs:2384-2566`): the W5 producer is a **two-stage** chain — `generator_model` (writes `segment_voxel_buffer`) THEN `chunk_calc` (reads `segment_voxel_buffer`, writes `WorldGpu.{chunks_buffer, blocks, voxels}`). **The chunk_calc half is the heavy lift, and it is preserved verbatim.** What gets replaced is just the upstream of the segment_voxel_buffer — a model-data-driven scan with a noise-driven scan. The output buffer shape is identical, the downstream chain is identical, the dispatch shape is identical. This is faithful to the brief's "feeding W5" language: stage-2 of W5 is reused; stage-1 has an alternative input source.

The alternative — running `generator_model.wgsl` AS-IS, with `model_data_*` re-uploaded per segment from CPU noise — would defeat the entire point of GPU noise (every segment would round-trip CPU→GPU upload bandwidth). Confirmed rejected; see Decision D.B1.

#### E.3 GPU bind groups + buffer reuse

The streaming path needs:
- A new bind-group layout (`construction_noise_terrain_layout`): `chunk_data_rw` (binding 0) + `noise_terrain_params` uniform (binding 1).
- A new pipeline (`construction_noise_terrain_pipeline`).
- A new uniform buffer `gpu.noise_terrain_params_buffer` (48 B + 16 B padding round-up).
- A new bind group `bind_groups.construction_noise_terrain`.

Slot into `ConstructionPipelines` + `ConstructionGpu` + `ConstructionBindGroups` alongside the existing W5 fields (see `mod.rs:1489-1620` for the W5 lifecycle pattern to mirror). Build-once at startup; the noise pipeline is one of `gpu_construction_enabled = true`'s allocations under the streaming preset.

The `segment_voxel_buffer` (128 MiB at `(16,16,16) × 2048 × 4 B`) is **shared** with the W5 path. Streaming reuses the existing allocation at `mod.rs:1527-1547`. The bind group binds `gpu.segment_voxel_buffer.as_ref().unwrap()` exactly as `construction_generator_model` does.

The chunk_calc bind group (`construction_world`) is already allocated by `prepare_construction` (`mod.rs:1620+`); the streaming dispatch uses it as-is.

### F. GPU upload — direct chunk_calc into WorldGpu (W2 bypass for admissions)

v1 § C synthesised W2 records (`pending_edits.batches`) for each admitted segment — 4096 chunks × 4 GPU passes per segment. Under Plan B, the noise→chunk_calc chain already writes directly into `WorldGpu.{chunks_buffer, blocks, voxels}` (this is what `chunk_calc` is FOR — see `mod.rs:2555-2560` `dispatch_calc_block_from_raw_data_world_sized`). **W2 admission synthesis is not needed for admissions** — the GPU chain already terminates at `WorldGpu`'s persistent buffers.

This is a major simplification over v1 § C. Plan B's W2 surface:

- **Admissions:** No W2 records. Direct `noise_terrain → chunk_calc → WorldGpu` per segment.
- **Evictions:** Still use W2 (per v1 § C.3). Synthesise an `EditBatch` with `changed_chunks` entries of `new_state = 0` (Empty `ChunkCell::Empty(0).encode()`) for each chunk in the evicted segment. Same per-frame surface as a brush stroke; W2 chain handles it byte-identically. **Cost:** 4096 × 2 u32 = 32 KiB of `changed_chunks` per evicted segment — cheap.

**Why this divergence from v1 § C is safe:** the W5 chunk_calc pass writes directly into `WorldGpu.chunks_buffer` (see `mod.rs:2555-2570` chain + the `validate_gpu_construction` proof that this round-trip is byte-equal to the CPU oracle). The persistent buffers carry the resident set forward across frames; the only mutation needed is the "write zeros into the evicted segments" step, which W2 handles.

**Open question / minor risk:** the W5 chain also runs the **bounds chain** (`add_initial_groups` + `compute_voxel_bounds` + `compute_block_bounds` — see `mod.rs:2568-2592`). The bounds chain is currently ONCE-after-the-segment-loop in W5; for streaming, bounds need to be RE-RUN every frame any segment was admitted or evicted. Per-frame bounds recompute over the whole world is cheap (it's already what runs once at startup; the cost scales with `WORLD_SIZE_IN_CHUNKS = (256, 32, 256) = 2M chunks` once per frame — well within frame budget). **Design decision:** the streaming driver runs the bounds chain at the end of every frame in which any admission OR eviction happened. See § G below.

### G. Driver — invert the once-at-startup gate (this is load-bearing)

v1 § D *disabled* the W5 gate (by not installing `ModelData`). Plan B **inverts** it.

#### G.1 The current gate at `mod.rs:2384-2566`

The current W5 driver gates on:

```rust
if gpu_construction_enabled && !gpu_producer_has_run {
  if let Some(model_data) = model_data.as_deref() {
    // (a) W5 — per-segment generator_model + chunk_calc loop over 512 segments
    // (b) bounds chain
    gpu.gpu_producer_has_run = true;
  }
}
```

It runs once per app lifetime over the full `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16) = 512` segments.

#### G.2 The streaming gate

Replace with a per-frame driver, gated on the **streaming preset only** (`GridPreset::ProceduralStreaming` flips a `ConstructionConfig.streaming_mode: bool` flag, or equivalent — verify naming with `render::construction::ConstructionConfig`).

```rust
// In a NEW render-world system after prepare_construction:
if streaming_mode_active {
    let admissions = residency_render.admissions_this_frame; // extracted from main
    let evictions  = residency_render.evictions_this_frame;
    let max_segments_per_frame = config.max_segments_per_frame; // default 4

    // 1) Per admitted segment (capped): dispatch noise_terrain + chunk_calc.
    for seg in admissions.iter().take(max_segments_per_frame as usize) {
        // (a) write noise_terrain_params with seg_origin_in_voxels
        // (b) write GpuConstructionParams.chunk_offset = seg_origin_in_chunks
        // (c) per-segment fresh encoder + submit (inheriting the W5 ordering-bug fix
        //     at mod.rs:2427-2453; same writes-vs-dispatches ordering hazard).
        let mut enc = device.create_command_encoder(...);
        dispatch_noise_terrain(&mut enc, pipeline_noise, bg_noise, [16,16,16]);
        chunk_calc::dispatch_calc_block_from_raw_data_world_sized(&mut enc, ..., [16,16,16]);
        queue.submit([enc.finish()]);
    }

    // 2) Per evicted segment: synthesise W2 EditBatch with zeros (per § F).
    //    Pushed into WorldData.pending_edits.batches; the existing
    //    extract_world_changes + naadf_world_change_node handles upload.

    // 3) Re-run bounds chain once at end of frame if any admission/eviction
    //    happened — same dispatch as mod.rs:2568-2592, but on the
    //    `render_context` encoder (not per-segment).
    if !admissions.is_empty() || !evictions.is_empty() {
        let enc = render_context.command_encoder();
        bounds_calc::dispatch_add_initial_groups(enc, ...);
        bounds_calc::dispatch_compute_voxel_bounds(enc, ...);
        bounds_calc::dispatch_compute_block_bounds(enc, ...);
    }
}
```

#### G.3 Per-segment submit constraint — INHERITED from W5

The W5 ordering bug (`mod.rs:2427-2453`) — `Queue::write_buffer` schedules BEFORE the next `Queue::submit`, so multiple writes to the same uniform on one encoder all see the last write — applies here too: we write the noise_terrain_params uniform per-segment, then dispatch per-segment. The fix is the same: **per-segment fresh encoder + submit**. Per-frame cost is `min(admissions.len(), max_segments_per_frame) ≤ 4 submits/frame` (with default budget). Compare to W5's 512 submits at startup — comfortably in budget.

#### G.4 `max_segments_per_frame` budget — default 4

The budget bounds the per-frame cost. GPU noise should generate one segment in ~1–2 ms on a modern GPU (16³ chunks × 64 voxels/thread × ~10 noise samples/voxel × ~50 cycles/sample = ~3 M cycles per chunk × 4096 chunks = ~12 G cycles / 10 TFLOPS ≈ 1.2 ms). chunk_calc is comparable. So 4 segments × ~4 ms = 16 ms / frame worst case (full frame budget at 60 fps).

At default budget 4 segments/frame: cold-start fills the full 512-segment window in `512 / 4 = 128 frames ≈ 2.1 s`. Compare to v1's CPU-noise ~170 frames ≈ 2.8 s (D.7's cold-start figure). GPU noise's cold-start is 25% faster AND consumes far less CPU.

**Fast traversal:** at 1 segment/frame net residency change × 60 fps = 60 segments/s admission rate. 4 admissions/frame × 60 fps = 240 segments/s budget. So at default settings the budget exceeds the traversal rate by 4×. Empty-patch failure mode (v1 D.7) is correspondingly less likely; flagged in § Δ-StreamingResidency.

**The `max_segments_per_frame` is a CLI knob** — added to `AppArgs` as `--max-segments-per-frame N` (default 4). Allows the user to dial it up for "I want the world to populate faster, frame rate can dip" or down for "I want stable frame time, accept occasional empty patches".

#### G.5 Same-submit vs separate-submit for noise+chunk_calc

**Same submit** (one encoder, two compute passes back-to-back, then `submit`). chunk_calc reads from `segment_voxel_buffer` after noise writes to it. wgpu inserts a STORAGE→STORAGE barrier between the two compute passes automatically (per `mod.rs:218-228`'s rationale for the `_with_encoder` chaining). The W5 startup loop already does this (`mod.rs:2549-2560`); we do the same.

#### G.6 Initial cold-start

On the first frame, `Residency::admissions_this_frame` contains every segment in the camera-centered window (~512 segments). At the default budget of 4 segments/frame, the residency manager spreads this across frames. The order is camera-distance-first (v1 D.11 carries over).

### H. Coordinate widening (unchanged from v1 § E)

`02-design.md:326-383` carries over verbatim.

**One detail explicit to Plan B:** the noise shader receives `seg_origin_in_voxels: vec3<i32>` (signed, NOT chunked u32). This is the segment's WORLD origin — the noise function is sampled in world-voxel coords so the noise field is stable across window shifts. The renderer never sees these coords; only `noise_terrain.wgsl` does. `Residency::origin` provides the conversion: `seg_origin_in_voxels = (residency_world_seg.0) * SEGMENT_CHUNKS * 16`.

The renderer side is unchanged — it sees window-local coords per Q1 (`02-design.md:357-382`).

### I. `GridPreset::ProceduralStreaming` + CLI — unchanged from v1 § F

Carries over from `02-design.md:386-441`. The one Plan-B-specific change: `noise_preset` (the `u32` variant payload) now indexes a built-in **WGSL preset**, not a `voxel_noise::NoisePreset`. Phase 1 supports one preset:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum WgslNoisePreset {
    SimpleTerrain = 0,
    // (placeholder for future presets — e.g. PlanetTerrain)
}
```

A `WgslNoisePreset` maps to a hard-coded `fnl_state` + `threshold` + `terrain_voxel_type_id`. `SimpleTerrain` config (mirroring `voxel_noise::presets::SimpleTerrain` shape):

- `noise_type: OpenSimplex2`
- `fractal_type: FBM`
- `octaves: 4`
- `lacunarity: 2.0`
- `gain: 0.5`
- `frequency: 0.02`
- `threshold: 0.0` (noise > 0 → solid)
- `terrain_voxel_type_id`: read from palette (e.g. `TY_GROUND`).

CLI:
- `--streaming-window` (boolean — sets `grid_preset = ProceduralStreaming{..}`).
- `--noise-preset SimpleTerrain` (default `SimpleTerrain`).
- `--seed <i32>` (default `1337`).
- `--vram-budget-mib <N>` (default `1024`).
- `--max-segments-per-frame <N>` (default `4`).

Flag-parsing addition: per the `e2e_render.rs:71-130` `args.iter().any(...)` pattern; values are scanned with the `peekable iter` pattern that the existing `--vox <path>` parsing uses (see `bin/bevy_naadf.rs` for that pattern).

### J. New e2e gate for Phase 2 — `--streaming-window`

Carries over from `02-design.md:443-489` (v1 § G) mostly verbatim. The Plan-B-specific changes:

- **Cold-start frame count:** v1 expected ~170 frames (CPU pool 3 seg/frame); Plan B expects ~128 frames at default budget 4/frame. Phase 2's `StreamingWarmupA` frame count: bump to 250 frames to keep margin against TAA-converge overhead.
- **Assertion D (VRAM budget):** add a `noise_terrain_params_buffer` line item (negligible, < 1 KiB).
- The rest (assertions A/B/C, shift geometry, screenshot diffs) is unchanged.

### K. Forward-compat seams (unchanged — v1 § H carries over)

`trait ChunkSource` and per-chunk eviction seams stay (per `02-design.md:493-527`). Plan B's Phase-2 single impl is `WgslNoiseChunkSource` instead of `NoiseChunkSource`. `Box<dyn ChunkSource>` is held by the residency resource; future `.vox`-streaming and Minecraft converters slot in without residency-layer changes.

### L. File-level diff sketch

#### Phase 1 (Plan B)

| Action | Path | Approx LOC | Why |
|---|---|---|---|
| new | `crates/bevy_naadf/src/assets/shaders/noise_fastnoiselite.wgsl` | ~1000 | WGSL port of FastNoiseLite.glsl Phase-1 subset |
| new | `crates/bevy_naadf/src/assets/shaders/noise_oracle_dispatch.wgsl` | ~30 | Thin oracle dispatch wrapper for the test |
| new | `crates/bevy_naadf/src/streaming/mod.rs` | ~30 | Module root (Phase-1 form) |
| new | `crates/bevy_naadf/src/streaming/noise_fastnoiselite.rs` | ~150 | Shader const + GPU oracle test runner |
| new | `crates/bevy_naadf/src/streaming/noise_fastnoiselite_cpu_oracle.rs` | ~300 | Rust port of the same GLSL functions |
| new | `crates/bevy_naadf/src/e2e/wgsl_noise_oracle.rs` | ~100 | `--wgsl-noise-oracle` gate |
| edit | `crates/bevy_naadf/src/e2e/mod.rs` | +1 | `pub mod wgsl_noise_oracle;` |
| edit | `crates/bevy_naadf/src/bin/e2e_render.rs` | +12 | Flag parse + short-circuit dispatch |
| edit | `crates/bevy_naadf/src/lib.rs` | +1 | `pub mod streaming;` |
| **no edit** | `crates/bevy_naadf/Cargo.toml` | 0 | `voxel_noise` NOT wired as runtime dep in Phase 1 (workspace member only) |
| **no edit** | `crates/bevy_naadf/src/render/construction/mod.rs` | 0 | Phase 1 doesn't touch the renderer at all |
| **no edit** | any other `.wgsl` | 0 | Phase-1 surface is self-contained |

Total Phase-1 new LOC: ~1610. Touched-LOC: ~14.

#### Phase 2 (Plan B)

| Action | Path | Approx LOC | Why |
|---|---|---|---|
| new | `crates/bevy_naadf/src/assets/shaders/noise_terrain.wgsl` | ~150 | Noise → segment_voxel_buffer dispatch |
| new | `crates/bevy_naadf/src/streaming/residency.rs` | ~280 | Residency manager (per v1 § A) |
| new | `crates/bevy_naadf/src/streaming/chunk_source.rs` | ~50 | `trait ChunkSource` forward-compat seam |
| new | `crates/bevy_naadf/src/streaming/noise_dispatch.rs` | ~200 | Per-frame GPU dispatch wiring (noise + chunk_calc per admission) |
| new | `crates/bevy_naadf/src/e2e/streaming_window.rs` | ~280 | `--streaming-window` e2e gate |
| edit | `crates/bevy_naadf/src/streaming/mod.rs` | +20 | Add residency/dispatch modules + `StreamingPlugin` |
| edit | `crates/bevy_naadf/src/render/construction/mod.rs` | +~120 | Streaming branch: when `streaming_mode_active`, run noise_terrain + chunk_calc per admitted segment + bounds chain after. Slotted alongside `model_data.is_some()` branch at `:2384-2566`; does NOT modify the existing W5 startup path. |
| edit | `crates/bevy_naadf/src/render/construction/mod.rs` | +~80 | `ConstructionPipelines`/`ConstructionGpu`/`ConstructionBindGroups` extensions: `noise_terrain_layout` + `noise_terrain_pipeline` + `noise_terrain_params_buffer` + `construction_noise_terrain` bind group, per `:1489-1620` pattern. |
| edit | `crates/bevy_naadf/src/voxel/grid.rs` | +~80 | `install_procedural_streaming_world` + arm at `setup_test_grid:104` |
| edit | `crates/bevy_naadf/src/lib.rs:65-78` | +~10 | `GridPreset::ProceduralStreaming` variant |
| edit | `crates/bevy_naadf/src/lib.rs::AppArgs` | +~8 | `vram_budget_mib`, `streaming_window_mode`, `noise_seed`, `noise_preset`, `max_segments_per_frame` |
| edit | `crates/bevy_naadf/src/lib.rs::build_app_with_args` | +1 | Register `StreamingPlugin` |
| edit | `crates/bevy_naadf/src/e2e/mod.rs` | +1 | `pub mod streaming_window;` |
| edit | `crates/bevy_naadf/src/bin/e2e_render.rs` | +~12 | `--streaming-window` flag parse + dispatch |
| **no edit** | `crates/bevy_naadf/src/aadf/edit.rs` | 0 | W2 packing/extract reused as-is (for evictions only) |
| **no edit** | `crates/bevy_naadf/src/render/construction/world_change.rs` | 0 | W2 GPU pipelines reused as-is (for evictions only) |
| **no edit** | `crates/bevy_naadf/src/aadf/construct.rs` | 0 | NO `encode_one_chunk` — v1's D.6 dropped (Plan B encodes on GPU) |
| **no edit** | `crates/bevy_naadf/Cargo.toml` for `voxel_noise` | 0 | Not wired as runtime dep at all under Plan B |

Total Phase-2 new LOC: ~960. Touched-LOC: ~230.

**Plan B vs Plan A LOC delta:** Plan A's total was ~810 new + ~225 edit = 1035. Plan B's total is ~2570 new + ~244 edit = 2814. The 2.7× growth is concentrated in the WGSL noise port (~1000 LOC of shader) — exactly the user's stated scope: "i'm really into authoring world noise in wgsl … lets port FastNoiseLite.glsl to wgsl".

---

## Δ-StreamingResidency

This design diverges from C# NAADF and is approved per `01-context.md` Q&A Step 4 (initial) + the user's Plan-B redirect at the v1 hard gate (`README.md` § Plan-B addendum).

**The divergence (carried over from v1 with one update):** C# NAADF (`NAADF/World/Data/WorldData.cs:120-156`) runs a one-shot `GenerateWorld` at startup with a fixed `ModelData` input. The world is fully populated once; the camera roams within a fixed 4096³ × 512 voxel container; mutations are user edits only.

Plan B replaces this with:

1. **Per-frame sliding-window residency driver** (`streaming::residency::residency_driver`) — checks each frame whether the camera has crossed a segment boundary; if so, evicts trailing segments and admits leading ones. World content is generated lazily per segment.
2. **WGSL FastNoiseLite + a noise-driven W5 stage-1 alternative** (`noise_terrain.wgsl`) — replaces C#'s `WorldGeneratorModel` `ModelData`-driven generation with procedural noise generation entirely on GPU. The existing C# `WorldGeneratorModel` is a fixed-`ModelData`-table sampler; Plan B's noise is a different content source feeding the same downstream chain (chunk_calc → bounds chain → WorldGpu buffers).
3. **W5 once-at-startup gate is INVERTED to per-frame** for the streaming preset only. Other presets (`GridPreset::Default`, `GridPreset::Vox`) preserve the one-shot W5 path bit-identically.

**User-stated motivation:** `01-context.md` § Goal + `README.md` § Plan-B addendum. Quoted:

> "we have to be able to demo large voxel worlds … sliding window approach, for streaming in worlds larger than VRAM allows under specified VRAM budget, generating infinite worlds, a large coordinate system. lets focus this session on implementing procedural world generation with a sliding window with fixed VRAM budget"

> "i'm really into authoring world noise in wgsl … lets port FastNoiseLite.glsl to wgsl … wgsl noise goes first"

**C# surfaces replaced (UPDATED from v1):**

- `NAADF/World/Data/WorldData.cs:120-156` (`GenerateWorld`) — the orchestrator. **Replaced by `streaming::residency::residency_driver`.**
- `NAADF/World/Generator/WorldGeneratorModel.cs:11-22` (`CopyToChunkData`) — the per-segment dispatch entry. **Replaced by `streaming::noise_dispatch::dispatch_noise_terrain_per_admitted_segment` + the new `noise_terrain.wgsl` GPU shader.**
- `Content/shaders/world/generator/generatorModel.fx` — the fixed-`ModelData`-table sampling shader. **Replaced (for the streaming preset only) by `noise_terrain.wgsl`. The existing port `generator_model.wgsl` is preserved untouched as the primary GPU producer for `GridPreset::Default` + `GridPreset::Vox`.**

**What this divergence preserves:**

- The W2 edit chain (`world_change.wgsl`, `pending_edits`) — **untouched.** Used by streaming for evictions only.
- The `(cx:11, cy:10, cz:11)` packing — **preserved** (Q1).
- The `WORLD_SIZE_IN_*` fixed-world constants — **preserved.** The world container does not change shape; only its origin shifts.
- The W5 once-at-startup producer (`generator_model.wgsl` + chunk_calc + bounds chain at `mod.rs:2384-2566`) — **preserved verbatim for non-streaming presets.** For the streaming preset, the noise-feeding stage replaces the `generator_model.wgsl` dispatch; chunk_calc + bounds dispatch chain are **reused as-is.**
- The W5 per-segment-submit ordering fix (`mod.rs:2427-2453`) — **inherited** by the streaming driver.

**Update to v1's note (the line "The W5 once-at-startup producer is preserved as dead code in the streaming preset"):** that is **no longer true under Plan B.** W5's chunk_calc + bounds dispatches are the primary downstream of streaming's noise generation. Only the W5 *stage-1* shader (`generator_model.wgsl`) is bypassed in the streaming preset, and that's because we have an alternative stage-1 (`noise_terrain.wgsl`) producing the same byte-layout output.

**Approval status:** per `01-context.md` § Q&A Step 4 (the user motivated the divergence) + the user's Plan-B redirect at the v1 hard gate (the user picked GPU noise + W5 inversion explicitly). Q1 (i32 residency widening), Q2 (per-segment), Q3 (per-chunk-local dedup) stand; Q4 (noise backend) is overridden by Plan B.

---

## Decisions & rejected alternatives

### v1 decisions carried over (unchanged)

The following v1 decisions are unchanged. The rationales in `02-design.md` remain authoritative; impl agents read them there.

- **D.1 (Indirection table — dense Vec + sparse HashMap):** carried over from `02-design.md` § D.1.
- **D.3 (Eviction via W2 record synthesis):** **NARROWED** in scope — Plan B uses W2 for **evictions only**, not admissions. Admission upload goes direct via noise→chunk_calc→WorldGpu. See § F.
- **D.4 (Window shape — rectangular AABB):** carried over.
- **D.5 (Window-shift trigger — per-segment):** carried over.
- **D.8 (`GridPreset::ProceduralStreaming` variant name):** carried over.
- **D.9 (`--vram-budget-mib 1024` default):** carried over.
- **D.11 (Slot iteration order — camera-distance first):** carried over.

### v1 decisions INVERTED by Plan B

#### D.2 (INVERTED v1) — Where noise runs: CPU pool vs GPU compute

- **v1 chose:** CPU `AsyncComputeTaskPool` with one `NoiseChunkSource` task per admitted segment.
- **Plan B chooses:** **GPU compute** via a WGSL FastNoiseLite port. Noise is one workgroup per chunk in `noise_terrain.wgsl`, same dispatch shape as `generator_model.wgsl`.
- **v1's rationale (rejected):** parallelism across CPU pool threads, reusing the proven `voxel_noise` crate, avoiding GPU readback round-trips.
- **Plan B's rationale:** (a) **throughput** — GPU noise is ~30–100× faster per segment than CPU FastNoise2, so the cold-start cost (which dominated v1 D.7's empty-patches failure) is amortised; (b) **no readback** — noise output lives in `segment_voxel_buffer` and is consumed in-place by chunk_calc, eliminating CPU↔GPU bandwidth entirely; (c) **user directive** — the user explicitly preferred authoring noise in WGSL.
- **Fact that would flip this back:** if a future session needs CPU-side noise sampling (e.g., for collision queries or non-rendering consumers), the WGSL port can be Rust-mirrored (the CPU oracle is already ~80% of that work). But the rendering pipeline never wants the noise on CPU.

#### D.7 (INVERTED v1) — Failure mode under fast traversal

- **v1 chose:** empty patches under traversal faster than CPU pool can generate (visual degradation, no stall).
- **Plan B chooses:** **substantially-reduced empty-patch frequency** by virtue of GPU noise's higher throughput. The failure mode is *qualitatively* the same — segments not generated yet appear as empty — but the throughput delta (4 segments/frame at 60 fps = 240 seg/s, well above any reasonable traversal rate) means the user rarely sees the empty patches in practice.
- **v1's rationale:** demo-target traversal speed × CPU generate rate → empty patches were the accepted price.
- **Plan B's rationale:** GPU noise is fast enough that the empty-patch regime sits well past normal traversal speed. The `--max-segments-per-frame` knob lets the user trade frame-time stability against generation speed.
- **Fact that would flip this back:** if profile data shows GPU noise generation budget is consistently blowing the frame budget (i.e., 4 segments/frame at the default budget is not enough; or each segment is taking ≫ 4 ms), reduce the budget and accept some empty patches as v1 did.

#### D.10 (INVERTED v1) — W5 once-at-startup gate

- **v1 chose:** **Disable** the W5 gate for the streaming preset (fall through to "do nothing"; CPU pool handles everything).
- **Plan B chooses:** **Invert** the W5 gate — make it per-frame, fired only for newly-admitted segments (via the `admissions_this_frame` queue extracted from the main world). The per-segment dispatch shape is the same as W5's startup loop, just driven by a different population trigger.
- **v1's rationale:** the CPU pool path bypassed W5 entirely; inverting it would duplicate work; cleaner to leave W5 as Default/Vox-only.
- **Plan B's rationale:** Plan B's noise IS GPU-side. The most efficient path is to write the noise output directly into W5's `segment_voxel_buffer` (via a new sibling shader `noise_terrain.wgsl`), then run W5's chunk_calc downstream as-is. The "invert vs disable" question collapses: under Plan B, inverting reuses W5's chunk_calc + bounds chain (saving ~200 LOC of redundant code); disabling would require duplicating that chain in a streaming-specific path. **Inverting is strictly cheaper.**
- **Fact that would flip this back:** if the per-frame chunk_calc + bounds dispatch turns out to be too expensive (it shouldn't — it's the same work as one segment of the W5 startup chain), splitting bounds out of the per-frame chain becomes an optimisation rather than a redesign.

### New Plan-B decisions

#### D.B1 — Noise feeds `segment_voxel_buffer` directly, NOT `ModelData` buffers

- **Chosen:** A new `noise_terrain.wgsl` shader writes the same byte layout into `segment_voxel_buffer` that `generator_model.wgsl` produces, then chunk_calc runs on top unchanged.
- **Rejected:** Run the existing `generator_model.wgsl` AS-IS but re-upload the `model_data_*` buffers per admitted segment from CPU-noise-derived data.
- **Reason:** the rejected option puts CPU↔GPU upload bandwidth back on the critical path (the whole reason for moving noise to GPU is to *avoid* that bandwidth). Direct GPU noise → `segment_voxel_buffer` skips the upload entirely. And it's strictly less code: one new shader + one new pipeline + one new bind group, vs. CPU noise + ModelData encoder + per-segment ModelData uploads.
- **Fact that would flip this:** if a future world preset wants procedural noise + a pre-baked `ModelData` overlay (e.g., for hand-authored landmarks in a procedural world), the existing `generator_model.wgsl` may be co-dispatched; the two shaders are non-exclusive at the bind-group level.

#### D.B2 — Phase-1 noise port scope (OVERRIDDEN by user post-confirmation)

**ORIGINAL ARCHITECT DECISION (kept for trace):**

- **Chosen:** Port `_fnlSingleOpenSimplex23D`, `_fnlSinglePerlin3D`, `_fnlGenFractalFBM3D`, their helpers, and the `fnl_state` skeleton in Phase 1.
- **Rejected:** Port the entire `FastNoiseLite.glsl` (all 4 noise families × all fractal types × domain warp × cellular).
- **Reason:** the full surface is ~2400 LOC. A streaming-world demo needs exactly one usable terrain noise; OpenSimplex2 + FBM is that. Perlin is included as a second, algorithmically-distinct noise (gives the oracle test a non-trivial matrix).

**USER OVERRIDE (post-Q2-confirmation, see `README.md` § Scope amplifier):**

> "we dont have to port all of the fastnoiselite controls, but all of its features - yes. we'd like to have a chefs kitchen of tools so that we can mix&match a beautiful fast 3d voxel noise generator for our world, extended with biome types and complex merges and stuff"

**Revised Phase-1 scope:** port the **full FastNoiseLite feature surface** — every noise family (OpenSimplex2, OpenSimplex2S, Cellular, Perlin, Value-Cubic, Value), every fractal type (FBm, Ridged, PingPong, off), every domain-warp variant (OpenSimplex2, OpenSimplex2Reduced, BasicGrid), every cellular distance function (Euclidean, EuclideanSq, Manhattan, Hybrid) × return type (CellValue, Distance, Distance2, Distance2Add/Sub/Mul/Div). **Controls** can be simplified (no C++-style getter/setter mirror — a unified uniform `FnlState` with the essential fields).

**Why the override:** the user wants a "chef's kitchen" — a primitive library for future biome/composition work where multiple noise outputs are mix-and-matched. The narrow scope optimised for the smallest credible demo but precluded the composition use case.

**Phase-1 LOC estimate revised:** ~2000–2500 LOC shader (was ~1000), ~600–800 LOC CPU oracle (was ~300). Phase-1 total ~2900–3600 LOC. Phase 2 unchanged.

**Oracle test scope expanded:** every noise × fractal × domain-warp combination has a fixed-sample-point equality test against the CPU oracle. Cellular configs are sub-matrixed (every distance × every return type is its own test case). Tolerance unchanged at `< 1e-5`.

**API shape implications for the unified dispatcher:** `fn fnl_get_noise_3d(state: FnlState, x: f32, y: f32, z: f32) -> f32` is the public entry point. `FnlState` carries `noise_type` (`u32`), `fractal_type` (`u32`), `domain_warp_type` (`u32`), `cellular_distance_func` (`u32`), `cellular_return_type` (`u32`), `seed` (`i32`), `frequency` (`f32`), `octaves` (`u32`), `lacunarity` (`f32`), `gain` (`f32`), `weighted_strength` (`f32`), `ping_pong_strength` (`f32`), `cellular_jitter_mod` (`f32`), `domain_warp_amp` (`f32`) — i.e. the full FastNoiseLite parameter set. The internal switch is per the GLSL function dispatcher; no shader-defs (OQ.2 = pure-WGSL).

#### D.B3 — CPU oracle is a Rust port of `FastNoiseLite.glsl`, NOT `voxel_noise`

- **Chosen:** Port the same GLSL functions to Rust as a CPU oracle module (`noise_fastnoiselite_cpu_oracle.rs`, ~300 LOC). Strict bit-near-equality assertion (`< 1e-5`) against the WGSL output.
- **Rejected (Option i in the brief):** Use `voxel_noise`'s `fastnoise2` backend as the CPU reference.
- **Reason:** FastNoise2 ≠ FastNoiseLite (different libraries, different algorithm details). The "near-identical" tolerance would need to absorb algorithmic differences, gutting the test's failure-detection power. Rust-porting the same GLSL is mechanical (mostly `fn foo(x: f32) -> f32` renames) and gives a strict equality test.
- **Fact that would flip this:** if `voxel_noise` ever adds a FastNoiseLite backend (it currently has only `fastnoise2`), the Rust oracle could in principle be retired.

#### D.B4 — Phase-1 e2e gate is `cargo run --bin e2e_render -- --wgsl-noise-oracle`, headless compute

- **Chosen:** Plumb a `--wgsl-noise-oracle` flag through `bin/e2e_render.rs`'s short-circuit ladder (per `--validate-gpu-construction*` pattern at `:142-177`). The gate runs `MinimalPlugins + RenderPlugin`, dispatches the WGSL noise, reads back, compares against the CPU oracle, exits with `ExitCode`.
- **Rejected:** Make Phase 1's verification a `cargo test --workspace --lib` only.
- **Reason:** per `CLAUDE.md` ("The named e2e gates ... are the verification surface. ... If a gate is missing for a behavior the agent needs to verify, the right move is to add a gate to `e2e_render`"). The unit test SHOULD also exist, but the canonical Phase-1 verification is the named gate.
- **Fact that would flip this:** none. This is `CLAUDE.md` policy.

#### D.B5 — Same-encoder noise + chunk_calc dispatch per admitted segment

- **Chosen:** Per admitted segment, one encoder records noise dispatch THEN chunk_calc dispatch, THEN submit. wgpu inserts STORAGE→STORAGE barriers between the two compute passes.
- **Rejected:** Two separate submits (one for noise, one for chunk_calc).
- **Reason:** the W5 startup loop already chains generator_model + chunk_calc on the same encoder (`mod.rs:2549-2560`), proving the barrier insertion works. Same submit halves the per-segment submission overhead.
- **Fact that would flip this:** if a future requirement needs to overlap noise generation for segment N+1 with chunk_calc for segment N (pipelining), separate submits become necessary. Not in scope this session.

#### D.B6 — `max_segments_per_frame` budget = 4

- **Chosen:** Default 4 segments/frame; CLI knob `--max-segments-per-frame`.
- **Rejected:** 1 (too slow cold-start), 8 (frame-time risk), unbounded ("do as many as fit in frame budget" — needs measurement to be safe).
- **Reason:** at ~4 ms/segment (estimated; see § Assumptions), 4 segments × 4 ms = 16 ms ≈ a full 60 fps frame. So 4 is the upper bound where we still hit 60 fps even in the worst case. Cold-start at 4/frame = 128 frames ≈ 2.1 s, acceptable for a demo. Lower values give better frame-time stability; higher values trade frame time for faster cold-start.
- **Fact that would flip this:** profile data showing per-segment cost is materially lower or higher than 4 ms.

#### D.B7 — Bounds chain re-runs per-frame whenever segments admitted or evicted

- **Chosen:** After per-segment noise+chunk_calc loop, run the full bounds chain (`add_initial_groups` + `compute_voxel_bounds` + `compute_block_bounds` from `mod.rs:2568-2592`) once on the shared `render_context` encoder if any admissions or evictions happened.
- **Rejected:** Per-segment bounds (would conflict with the per-segment write_buffer ordering bug and would over-dispatch); skip bounds entirely (would break the AADF acceleration structure on edited chunks).
- **Reason:** the bounds chain reads from `blocks`/`voxels` and writes the AADF chunk-level acceleration structure. After any admission or eviction modifies those buffers, the AADF index needs refreshing. The bounds chain is cheap-enough to amortise once at the end of frame (2M chunks × small per-chunk cost; W5 startup loop already pays it once and the runtime budget per-frame is roughly the same order).
- **Fact that would flip this:** profile data showing bounds-chain dominates per-frame cost. Mitigation: only re-run bounds over the *affected* segments via a `dirty_segments` list. Not in scope this session.

---

## Assumptions made

1. **WGSL `i32 * i32` wraps mod 2^32 the same way GLSL `int * int` does.** Per the WGSL spec, integer multiplication is two's-complement wrap. Same as GLSL. The hash chain in FastNoiseLite is purely XOR/multiply on i32, so bit-identical on both. (If WGSL ever specifies signed overflow as UB, the port needs `bitcast<u32>` round-trips; not the case today.)

2. **`fnl_state` as a uniform is ≤ 256 B + std140-aligned.** The Phase-1 subset of fields is ~36 B; with std140 padding rounded to 48 B. Fits in a uniform binding without issue (uniform binding cap is 64 KiB on every wgpu backend).

3. **`max_segments_per_frame = 4` gives ~4 ms per segment at 60 fps target.** This is an estimate, not a benchmark. Derived from: noise pure-arithmetic cost (~10 noise samples per voxel × 50 cycles per sample × 4096 voxels per chunk × 4096 chunks per segment = ~8 G cycles; @ 10 TFLOPS ≈ 1 ms) + chunk_calc cost (existing W5 startup loop takes ~1 ms per segment from the W5 instrumentation comments; same as W5). Impl agent should add a per-segment timestamp query to validate. If significantly off, dial `max_segments_per_frame` accordingly.

4. **wgpu inserts a STORAGE→STORAGE barrier between compute passes on the same encoder.** Confirmed by the W5 chaining pattern (`mod.rs:218-228` comment block — "so wgpu auto-inserts the STORAGE→STORAGE barrier"). Same property used here.

5. **`SEGMENT_CHUNKS = 16` permanently** (per v1 assumption #2). Inherited.

6. **Bevy 0.19-rc.1's `Shader::from_wgsl` + `#import "shaders/<file>"::{symbol}` directive works for module-style imports** (per `naadf_first_hit.wgsl:50`, `denoise_split.wgsl:39-42` — multi-line import works). The Phase-1 oracle dispatch shader uses this to import `fnl_state` + `fnl_get_noise_3d`. Verified by grepping existing imports in `src/assets/shaders/`.

7. **Cross-GPU bit-determinism is NOT required.** This codebase has no deterministic-multiplayer story. Same-GPU same-driver determinism is required (it's a property of the underlying shader, not a feature). The oracle test runs on whichever GPU the harness has and asserts `< 1e-5` against a CPU reference; running on a different GPU might fail at `1e-8` but should pass at `1e-5`.

8. **The streaming preset's W5 driver chain runs in a render-world system after `prepare_construction`.** Bevy 0.19's render-world system scheduling — Plan B adds a `streaming_construction_render` system to the `Render` schedule, ordered after `prepare_construction`'s render-world body. Need to verify exact `SystemSet` placement during impl (the existing W5 chain runs *inside* `prepare_construction`; the streaming variant either extracts a helper from that body or runs as a second system after it).

9. **`ConstructionConfig` extension for `streaming_mode_active`** — adding a `bool` field to `ConstructionConfig` (`render::construction`) for the streaming-mode gate is a localised edit. If the field doesn't naturally fit there, equivalent gating via a separate `StreamingMode` render-world resource works.

10. **CPU oracle module compiles as part of `bevy_naadf` (not in a separate crate).** The oracle is ~300 LOC of pure-math Rust; co-locating it in `streaming/noise_fastnoiselite_cpu_oracle.rs` keeps the test surface in one place. If it bloats the production build size (it shouldn't — pure arithmetic, no big arrays), `#[cfg(any(test, feature = "wgsl_noise_oracle_gate"))]`-gating it is a low-risk follow-up.

11. **`Residency` resource provides `admissions_this_frame` and `evictions_this_frame` as per-frame deltas the render-world can `extract`.** The exact data structure (e.g., `Vec<WorldSegmentPos>` swap-out each frame) is impl detail; the render-world driver reads it via `ExtractResource` per Bevy 0.19 convention.

12. **Initial-camera-pose for streaming preset spawns at the world's central segment** (per v1 assumption #9; carries over).

13. **The shader-import path `shaders/noise_fastnoiselite.wgsl` is correct for the project's asset server config.** Per `lib.rs:626-644`'s `AssetPlugin { file_path: "src/assets" }`, asset paths are relative to `src/assets/`. `shaders/<file>` resolves to `src/assets/shaders/<file>`. Verified by reading the existing `generator_model.rs:55-59` and `GENERATOR_MODEL_SHADER = "shaders/generator_model.wgsl"`.

---

## Open questions for the user (if any)

These are questions Plan B surfaces that weren't covered by the initial Q&A. The orchestrator may want to route them via Step-4-shape Q&A before impl dispatches.

### OQ.1 — Noise-output classification: threshold-based vs height-relative?

**Context:** the Phase-2 `noise_terrain.wgsl` shader needs to decide which voxels are "solid" vs "empty" from the noise value. Two natural choices:

- **(a) Threshold:** `noise > threshold → solid`. Simple. Produces a "blob terrain" pattern (3D Perlin/Simplex blobs). This is what `voxel_noise::SimpleTerrain`'s test code does (`crates/voxel_noise/src/lib.rs:109-127`).
- **(b) Height-relative:** `noise > (world_y - sea_level) / amplitude → solid`. Produces actual "ground + rolling hills + caves" terrain. More like Minecraft. More parameters (sea_level, amplitude).

**Plan B currently assumes (a)** as the simplest Phase-2 default. If the user wants Minecraft-style hills+caves, (b) is a small Phase-2 add (a few more uniform fields).

**Suggested call:** ship (a) in Phase 2; add (b) as a follow-up `WgslNoisePreset::PlanetTerrain` variant if needed for the demo.

### OQ.2 — Should the WGSL noise port use Bevy shader-defs or be pure-WGSL?

**Context:** `crates/bevy_naadf/src/assets/shaders/taa.wgsl` uses `#{TAA_SAMPLE_RING_DEPTH}` shader-defs for compile-time consts. The WGSL noise port could use shader-defs for `NUM_OCTAVES`, gradient-table sizes, etc.

**Plan B currently assumes pure-WGSL (no shader-defs).** Defs would shave a tiny bit of uniform-buffer traffic but introduce pipeline-specialisation per-config. Pure-WGSL is simpler and the Phase-1 oracle test is one fixed-config.

**Suggested call:** ship Phase 1 as pure-WGSL. Revisit if perf profiling motivates per-config specialisation.

### OQ.3 — Should the bounds chain run every frame, or only on segment admissions/evictions?

**Context:** § G.4 + D.B7. Plan B re-runs the full bounds chain only when admissions OR evictions happened (skip on no-op frames). This is correct but may have an edge case: a user edit (brush stroke) on a resident chunk would invalidate bounds without an admission/eviction. The existing W5 startup pays bounds once; the W2 edit chain (`world_change.wgsl`) does NOT re-run bounds — implying user edits already mutate bounds (or don't, and the existing code accepts that approximation).

**Plan B currently inherits whatever the existing edit-path bounds story is.** No edits to `world_change.wgsl` or its consumers.

**Suggested call:** confirm with the user that streaming admissions/evictions are the only path that needs bounds refresh. If user edits ALSO need it (regression possibility), that's a follow-up.
