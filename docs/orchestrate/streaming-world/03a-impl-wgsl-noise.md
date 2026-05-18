# 03a — Phase-1 impl: WGSL FastNoiseLite port + CPU oracle + `--wgsl-noise-oracle`

Implementation log for Phase 1 of the streaming-world orchestration
(`02b-design-plan-b.md` § A / § B / § C, augmented by `README.md` § Scope
amplifier — the "chef's kitchen" override of D.B2).

## Files added / edited

| Path | LOC | What's in it |
|---|---:|---|
| `crates/bevy_naadf/src/assets/shaders/noise_fastnoiselite.wgsl` | 1492 | WGSL port of the FULL FastNoiseLite GLSL 3D feature surface. All 6 noise families (OpenSimplex2, OpenSimplex2S, Cellular, Perlin, ValueCubic, Value), all 4 fractal types (None, FBm, Ridged, PingPong), all 3 domain-warp variants (OpenSimplex2, OpenSimplex2Reduced, BasicGrid), full cellular sub-matrix (4 distance × 7 return), `FnlState` config struct (80 B, 5×16 B std140-aligned rows), `fnl_get_noise_3d` + `fnl_domain_warp_3d` public entry points. Inlined `GRADIENTS_3D` (256 f32), `RAND_VECS_3D` (1024 f32), `GRADIENTS_2D` (256 f32), `RAND_VECS_2D` (512 f32). 2D singles deliberately not ported (3D-first voxel use case; per design assumption + amplifier scope). |
| `crates/bevy_naadf/src/assets/shaders/noise_oracle_dispatch.wgsl` | 53 | Thin compute wrapper. One workgroup-size-64 entry `dispatch_oracle` that reads per-invocation `(SamplePoint, FnlState)` from storage buffers + bounds-checks against a `count` uniform + writes `fnl_get_noise_3d(state, p.x, p.y, p.z)` to `output[i]`. |
| `crates/bevy_naadf/src/streaming/mod.rs` | 23 | Phase-1 module root: `pub mod noise_fastnoiselite; pub mod noise_fastnoiselite_cpu_oracle;`. |
| `crates/bevy_naadf/src/streaming/noise_fastnoiselite.rs` | 682 | Shader-source `include_str!` consts (`NOISE_FASTNOISELITE_SHADER_SRC` + `NOISE_FASTNOISELITE_SHADER_PATH` + `NOISE_ORACLE_DISPATCH_SHADER_SRC`); `build_oracle_dispatch_shader_src()` (concats noise module + dispatch wrapper, strips `#define_import_path` directive, splices at `// @begin` marker); `OracleCase` (sample point + state + pre-computed CPU value); `build_test_plan()` (deterministic 1796-case matrix covering 290 distinct combos); `run_wgsl_noise_oracle()` (headless `MinimalPlugins + RenderPlugin` app, dispatch, readback, compare); `#[test]` units. |
| `crates/bevy_naadf/src/streaming/noise_fastnoiselite_cpu_oracle.rs` | 1582 | Rust port of the SAME GLSL — `#[repr(C)]` `FnlState` (with `bytemuck::Pod` + static-asserted offsets that lock the WGSL std140 layout in place), enum-discriminant modules (`noise_type::*`, `fractal_type::*`, etc.), 3D versions of every WGSL noise function with `i32::wrapping_*` arithmetic to match GLSL `int` overflow semantics. `#[test]` units for determinism, in-range output, FBm-octave-1 equivalence, hash determinism, edge coherency. |
| `crates/bevy_naadf/src/e2e/wgsl_noise_oracle.rs` | 52 | `--wgsl-noise-oracle` gate: thin `ExitCode`-shaped façade over `run_wgsl_noise_oracle()`. Reports `total_cases`, `combos`, `max_abs_diff`, worst-case tag/pos/cpu/gpu on success. |
| `crates/bevy_naadf/src/e2e/mod.rs` | +1 | `pub mod wgsl_noise_oracle;` |
| `crates/bevy_naadf/src/bin/e2e_render.rs` | +9 | `--wgsl-noise-oracle` flag parse + short-circuit dispatch BEFORE the windowed e2e boot. Returns `run_wgsl_noise_oracle_gate()`'s `ExitCode` directly. |
| `crates/bevy_naadf/src/lib.rs` | +1 | `pub mod streaming;` |

Phase-1 new LOC: **3884** (slightly above the 2900–3600 estimate — the
constants inlined as `const array<f32, N>` plus the `cargo test` units inflate
the count modestly). Touched LOC: **11**.

No edits to `render/construction/mod.rs`, no `GridPreset` variant, no
`voxel_noise` Cargo dep change — Phase 1 is purely additive and self-contained.

## Verification gates run

- `cargo build --workspace --release` — **EXIT 0**. Clean build, no warnings from the new code (one `dead_code` allow-attribute on the unused-in-Phase-1 2D hash helpers, justified by future-Phase-2 use).
- `cargo test --workspace --lib --release` — **EXIT 0**. 205 passed, 1 ignored (pre-existing, unrelated), 0 failed.
  - Includes 5 unit tests added by Phase 1: `deterministic_repeat`, `open_simplex_2_origin_in_range`, `fbm_one_octave_matches_single`, `hash_3d_deterministic`, `edge_coherency` (CPU oracle); plus `dispatch_shader_inlines_noise_module`, `test_plan_is_bounded`, `run_oracle_passes` (GPU harness).
- `cargo run --release --bin e2e_render -- --wgsl-noise-oracle` — **EXIT 0**.
  ```
  WGSL noise oracle PASS: 1796 cases across 290 distinct combos.
  max_abs_diff = 1.4901e-6 on tag=`none` at pos=[100.25, 50.75, 25.125, 0.0]
  (cpu=-0.58389103, gpu=-0.5838925).
  ```
  All 1796 sample points pass at the design tolerance (`< 1e-5` non-cellular, `< 1e-4` cellular). Worst-case absolute error is ≈ 1.5e-6 — well within the f32 round-off floor at the test's frequency × magnitude scale. No mismatches.
- `cargo run --release --bin e2e_render -- baseline` — **EXIT 0**. Default-scene baseline gate passes; the e2e harness's 96 warmup + 48 motion + 1 settle frames complete cleanly, region luminance + non-black ratio gates green. **No regression from Phase 1's additions.**

The combinations exercised by the oracle gate:

| Group | Count | Notes |
|---|---:|---|
| 5 non-cellular noise × 4 fractal types × 5 seeds × 8 sample points | 800 | rotation = NONE (the fractal already does plenty of work) |
| 5 non-cellular noise × 1 fractal (NONE) × 3 rotations (NONE/XY/XZ) × 5 seeds × 8 sample points − duplicates | (covered above) | rotation sweep |
| Cellular: 4 distance × 7 return × 5 seeds × 4 sample points | 560 | dist=EUCLIDEAN tightens the post-sqrt path |
| Edge-coherency (3 noise × 3 seeds × 4 paired points) | 36 | catches accidental coord-truncation bugs |
| Cross-fractal × cross-noise sweep above | 400 | total sweep |

Total distinct `(noise_type, fractal_type, rotation, domain_warp, cellular_distance, cellular_return, seed)` tuples reached: **290**.

## Surprises during implementation

- **WGSL constexpr eval rejects `i32 << 1u` overflow at compile time, even
  when GLSL accepts the same expression as defined two's-complement wrap.**
  The hash function uses `PRIME_Y << 1` / `PRIME_Z << 1` / `PRIME_Y * 2` /
  `PRIME_Z * 2` (`1136930381 * 2 = 2273860762`, overflows i32). GLSL produces
  the wrapped negative result on every real-world driver; WGSL's constexpr
  evaluator rejects with `"<< operation overflowed"` and refuses to compile
  the shader. **Fix:** introduced precomputed `PRIME_X_2 / PRIME_Y_2 /
  PRIME_Z_2` constants with the wrapped values written out by hand
  (`-2021106534` and `-854139810`). Bit-equal to what GLSL evaluates at
  runtime. The first attempt to use `bitcast<i32>(u32(PRIME_Y) << 1u)` ALSO
  failed because WGSL's constexpr eval rejects `bitcast` in const context
  (`"Not implemented as constant expression: bitcast built-in function"`).
  The hand-written literals are the only path that works on the current naga
  version (Bevy 0.19-rc.1). Documented in the WGSL file's comment block at
  the `PRIME_*_2` declaration site.
- **`#define_import_path` directive interaction with the test-marker
  strip.** The dispatch-shader assembly function strips the
  `#define_import_path` line and replaces it with a `// (stripped ...)`
  comment; the smoke test then incorrectly checked `src.contains("#define_import_path")`
  which false-positived on the replacement comment. Fixed by changing the
  test to scan line-by-line, only matching lines that *start with* the
  directive.
- **WGSL `i32(-0.5 - xi)` vs GLSL `int(-0.5f - xi)` truncation semantics.**
  Both languages truncate-toward-zero on the float-to-int cast. Verified by
  reading the WGSL spec § 14.4.3 + running the CPU oracle's
  `f as i32` cast: bit-equal in both directions.
- **GLSL `int(round(...))` is round-half-to-even.** Rust's `f32::round` is
  round-half-away-from-zero. The CPU oracle's `fast_round` uses an explicit
  `round_half_to_even` helper to match the GLSL semantics. At all the
  oracle's sample points the result is identical between the two rounding
  modes (the points are not exact half-integers), but the helper is
  load-bearing for any future test that hits a tie value.
- **WGSL `>>` on `i32` is arithmetic shift** (sign-extends) per WGSL spec,
  matching GLSL. The hash chain uses `hash >> 15` for sign-preserving
  dispersion — bit-equal across both ports.
- **Bevy 0.19's `BindGroupLayoutDescriptor` uses constructor `::new(name, &entries)`,
  not struct-literal syntax**, and `ComputePipelineDescriptor` no longer has
  `push_constant_ranges` (renamed/removed in this Bevy version — the new
  field is `immediate_size`). Mirrored the pattern at `chunk_calc.rs:61-92`
  / `chunk_calc.rs:106-118` exactly; `..default()` fills the optional
  fields.
- **WGSL `Shader::from_wgsl` + inlined `#define_import_path`.** The design
  flagged Bevy's `#import` cross-module resolution as "unpredictable across
  naga versions"; I followed the `chunk_calc.wgsl:39-44` precedent of
  inlining at the source-text level (concatenating the two shader files in
  Rust before `Shader::from_wgsl`). This sidesteps the entire composition
  surface and gives a single-translation-unit compile path.

No deviations from the architect's design at the algorithm level. The CPU
oracle ports each WGSL function 1:1; the WGSL port is a 1:1 translation of
the GLSL.

## What's left for Phase 2

Phase 1 ships a **library** + **verification gate** but does **not yet wire
the WGSL noise into any rendering surface**. Specifically: no
`GridPreset::ProceduralStreaming` variant, no `noise_terrain.wgsl` shader,
no per-frame W5 driver inversion, no residency manager, no
`--streaming-window` gate. Phase 1's WGSL noise module is a passive
import-able library that future shaders can `#import "shaders/noise_fastnoiselite.wgsl"`
from (or inline at the source-text level, the way Phase 1's oracle
dispatcher does).

Phase 1 leaves intact every existing renderer code path: `--vox-e2e`,
`--validate-gpu-construction`, `--edit-mode`, `--oasis-edit-visual`,
`--runtime-edit-mode`, `baseline`. The baseline gate run confirms no
regression.

## Hand-off notes for Phase 2

### `FnlState` layout (load-bearing — Phase 2's `noise_terrain.wgsl` MUST match this)

The WGSL `FnlState` struct (`assets/shaders/noise_fastnoiselite.wgsl:95-120`)
is 80 B = 5 × 16-byte rows. Rust mirror at
`streaming/noise_fastnoiselite_cpu_oracle.rs::FnlState` has matching `#[repr(C)]`
+ static-asserted offsets. Both representations are bytemuck-`Pod`/`Zeroable`,
so Phase 2 can `bytemuck::bytes_of(&state)` into a uniform buffer and the
WGSL shader will read it directly.

Field offsets (used by Phase 2 when laying out the noise-params uniform
buffer for `noise_terrain.wgsl`):

| Offset | Field | Type | WGSL type |
|---:|---|---|---|
| 0 | `seed` | `i32` | `i32` |
| 4 | `frequency` | `f32` | `f32` |
| 8 | `noise_type` | `u32` | `u32` |
| 12 | `rotation_type_3d` | `u32` | `u32` |
| 16 | `fractal_type` | `u32` | `u32` |
| 20 | `octaves` | `i32` | `i32` |
| 24 | `lacunarity` | `f32` | `f32` |
| 28 | `gain` | `f32` | `f32` |
| 32 | `weighted_strength` | `f32` | `f32` |
| 36 | `ping_pong_strength` | `f32` | `f32` |
| 40 | `cellular_distance_func` | `u32` | `u32` |
| 44 | `cellular_return_type` | `u32` | `u32` |
| 48 | `cellular_jitter_mod` | `f32` | `f32` |
| 52 | `domain_warp_type` | `u32` | `u32` |
| 56 | `domain_warp_amp` | `f32` | `f32` |
| 60 | `_pad0..4` | `u32` × 5 | reserved for future fields |

### Shader-import path

```wgsl
#import "shaders/noise_fastnoiselite.wgsl"::{FnlState, fnl_get_noise_3d, fnl_domain_warp_3d}
```

The module exports the `#define_import_path noise_fnl` so Bevy's
import resolver will pick it up. If Phase 2 hits any `#import`
unreliability (the chunk_calc.wgsl precedent flags this as a known
wgpu/naga issue), it can use the same source-text concatenation path
Phase 1's oracle dispatcher uses: see
`streaming::noise_fastnoiselite::build_oracle_dispatch_shader_src` in
`streaming/noise_fastnoiselite.rs` for the inlining helper. Phase 2 can
extract that helper into a shared `fn inline_noise_module_into(_: &str) -> String`
under the `streaming` module.

### Public WGSL entry points exposed

- `fn fnl_create_state(seed: i32) -> FnlState` — defaults match the GLSL
  `fnlCreateState(seed)` (frequency=0.01, OpenSimplex2, octaves=3,
  domain_warp_amp=30.0, etc).
- `fn fnl_get_noise_3d(state: FnlState, x: f32, y: f32, z: f32) -> f32` —
  unified dispatcher, returns noise in `[-1, 1]`. Phase 2's
  `noise_terrain.wgsl` calls this per-voxel and thresholds against a
  `threshold` parameter to decide solid/empty.
- `fn fnl_domain_warp_3d(state: FnlState, p: vec3<f32>) -> vec3<f32>` —
  warps a coordinate. Composable with `fnl_get_noise_3d` (call warp first,
  then sample noise at the warped position).

Per-noise-type single-noise entry points (`fnl_single_opensimplex2_3d`,
`fnl_single_perlin_3d`, etc) are also exposed at module scope and can be
called directly when a caller wants to bypass the dispatcher. The
underlying fractal wrappers (`fnl_gen_fractal_fbm_3d`,
`fnl_gen_fractal_ridged_3d`, `fnl_gen_fractal_pingpong_3d`) are likewise
public.

### Rust mirror exposed

```rust
use bevy_naadf::streaming::noise_fastnoiselite_cpu_oracle as cpu;

let state = cpu::fnl_create_state(1337);
let v = cpu::fnl_get_noise_3d(&state, x, y, z);
```

Phase 2's residency manager can use the CPU oracle for any host-side
preview / sanity check, but the production render path samples noise
directly on GPU via the WGSL module.

### Known-good test fixture for Phase 2's `noise_terrain.wgsl`

If Phase 2's `noise_terrain.wgsl` produces unexpected output, the
existing oracle gate
(`cargo run --release --bin e2e_render -- --wgsl-noise-oracle`) is the
load-bearing sanity check that the underlying noise primitives are correct.
A failure of `noise_terrain.wgsl` while the oracle gate stays green
indicates the bug is in Phase 2's coordinate transform / threshold /
solid-voxel encoding — NOT in the noise functions themselves. This
narrows Phase 2's debugging surface by an order of magnitude.
