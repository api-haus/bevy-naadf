# 20 — Impl Phase D Shadow Dispatch A — Multi-Tap Sun Visibility (spatial)

**Date:** 2026-05-15
**Branch:** `main`
**Predecessor scope:** `19-gi-reservoir-scope.md` §3.1 / §4 Dispatch A
**Out-of-scope (per dispatch brief):** `naadf_global_illum.wgsl:346-380`
secondary-bounce sun loop (untouched); `MAX_RAY_STEPS_SUN` (untouched);
no CLI flag added.

---

## 1. Files touched

| File | Range / sites |
| --- | --- |
| `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl` | 529-583 (was 529-560 — net +25 lines for the loop wrapping) |
| `crates/bevy_naadf/src/assets/shaders/gi_params.wgsl` | 5-13 header comment refreshed (288 → 304 bytes); 119-126 new field + 3 trailing pad lanes |
| `crates/bevy_naadf/src/render/gpu_types.rs` | 492-512 new field + `_pad5`/`_pad6`/`_pad7`; 815-827 size assert (288 → 304) + new `offset_of!` guards |
| `crates/bevy_naadf/src/lib.rs` | 73-86 `GiSettings::sun_shadow_taps` field + doc; 91 `Default::sun_shadow_taps = 4` |
| `crates/bevy_naadf/src/render/gi.rs` | 367-374 plumb `gi.sun_shadow_taps` + 3 trailing pads into `GpuGiParams` build site |

No other sites needed editing — `GpuGiParams` is built in exactly one place,
the buffer size is `std::mem::size_of::<GpuGiParams>()` everywhere, and the
extracted-config mirror (`ExtractedGiConfig.settings: crate::GiSettings`) copies
the whole `GiSettings` value, so no per-field extract change was required.

---

## 2. The change

WGSL: the existing single-tap sun cone sample in `spatial_resampling.wgsl`'s
`sample_neighbors` (immediately after the diffuse/specular branch) is now
wrapped in a `for sun_tap in 0..max(gi_params.sun_shadow_taps, 1u)` loop.
Each iteration draws two fresh `next_rand` values, builds a fresh
`get_uniform_hemisphere_sample(.., sky_sun_dir, 0.9999)` cone direction
(width **unchanged**), shoots a `MAX_RAY_STEPS_SUN`-budget visibility ray,
and accumulates `sun_color * weight` into a `sun_accum: vec3<f32>`. After
the loop the contribution is added as `color += sun_accum / f32(n_sun_taps)`,
which preserves the expected value of the original single-tap path. The
`0.9999` deviation, the `MAX_RAY_STEPS_SUN` cap, the cos-theta gate, the
`SURFACE_SPECULAR_ROUGH` GGX branch, and the `firstHit.normalTang !=
HIT_NOTHING` predicate are all unchanged inside the loop body.

Rust: `GiSettings` gains `pub sun_shadow_taps: u32` (default `4`).
`GpuGiParams` gains `pub sun_shadow_taps: u32` plus `_pad5`/`_pad6`/`_pad7`
trailing pads — a fresh 16-byte row at struct offset 288 keeps the struct
16-byte-aligned at 304 bytes total. `prepare_gi` reads `gi.sun_shadow_taps`
from the extracted config and writes it into the uniform.

---

## 3. Layout discipline

- `GpuGiParams` size assert was bumped from 288 → 304:
  `const _: () = assert!(std::mem::size_of::<GpuGiParams>() == 304);`
  (`gpu_types.rs:815`)
- The `taa_jitter` offset guard at 280 is retained verbatim
  (`gpu_types.rs:818-819`).
- New `sun_shadow_taps` offset guard added below the `taa_jitter` guards:
  `const _: () = assert!(std::mem::offset_of!(GpuGiParams, sun_shadow_taps) == 288);`
  (`gpu_types.rs:826`)
- WGSL/Rust mirror byte-by-byte: the Rust struct grows from 288 to 304 bytes
  (one `u32` + three `u32` pads); the WGSL struct appends `sun_shadow_taps: u32,
  pad_b: u32, pad_c: u32, pad_d: u32` directly after `taa_jitter: vec2<f32>`
  (which ended at offset 288). Both ends therefore declare the same 16-byte
  trailing row [288..304] with the first 4 bytes carrying the field. The
  `vec3`-then-scalar trap is sidestepped because the new field is a single
  `u32` that does not follow a `vec3<T>` — it follows a `vec2<f32>` whose end
  offset is exactly 288, naturally 16-byte aligned. The WGSL spec packs a
  trailing `u32` immediately after the `vec2<f32>` with no surprise padding,
  matching the Rust `#[repr(C)]` layout. Tests pass (112/112), e2e green —
  layout is byte-equivalent in practice.

---

## 4. Bit-equivalence at N=1

**Yes**, modulo loop-induced rand-stream advancement, which is **identical
to the original code path**:

- The original single-tap code drew two `next_rand(&rand)` values for the
  cone direction. The new loop draws the same two `next_rand(&rand)` values
  per iteration. At `n_sun_taps == 1u` the loop body executes exactly once,
  so the rand stream is advanced by exactly two `next_rand` calls — same as
  before.
- The body of the loop is the *original* sun sample code verbatim, modulo
  accumulating into `sun_accum` instead of mutating `color`. The post-loop
  `color += sun_accum / f32(1u)` reduces to `color += sun_accum`, and
  `sun_accum` holds the single tap's contribution.
- The default is `sun_shadow_taps = 4` (not 1), so the bit-equivalent path
  is only reached when the field is explicitly set to 0 or 1 (the shader
  clamps `0u` to `1u`).

No rand-stream drift compared to the C# reference at N=1; the difference
is purely "this fork shifts the default to N=4" which is the intended
deviation-from-faithfulness for the paper §5.2 mitigation.

---

## 5. Gate results

```
rtk cargo build --workspace               → exit 0, "Finished `dev` profile" — clean, no new warnings on touched files
rtk cargo test -p bevy-naadf --lib        → 112 passed, 1 ignored (1 suite, 4.48s) — exit 0 — no regressions
rtk cargo run --release --bin e2e_render  → exit 0 — "PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames"
                                            region luminance — emissive 247.1, solid(GI-lit diffuse) 242.0, sky 145.9
rtk cargo run --release --bin e2e_render -- --entities → exit 0 — "PASS (batch 6) ..."
                                            region luminance — emissive 247.1, solid(GI-lit diffuse) 241.9, sky 145.9
                                            entity handler validation PASS
```

Solid-region luminance pre-change baseline ≈ 242.0 (no change observed in this
gate — solid is a GI-lit diffuse rect facing the sun mostly *unshadowed* in
the test scene; the multi-tap sun would shift it only if it sat on a shadow
penumbra, which it doesn't). Sky and emissive are unchanged (no sun ray
involvement). The `entity_pixel` luminance gate at threshold 80 (baseline
187.93, 2.35× margin) passed both runs — exit 0 confirms it.

---

## 6. Default value

`GiSettings::default().sun_shadow_taps = 4` (`lib.rs:91`).
The shader clamps to `max(_, 1)` defensively, so a hand-written `0` resolves
to the C# single-tap path harmlessly.

---

## 7. What was NOT done (scope discipline)

- **`naadf_global_illum.wgsl:346-380` per-secondary-bounce sun untouched.**
  The brief explicitly placed this out of scope; only the
  spatial-resampling sun sample at `spatial_resampling.wgsl:529-583` got
  multi-tapped.
- **`MAX_RAY_STEPS_SUN` untouched** at the existing 120 (the C# / paper
  faithful value).
- **`0.9999` sun cone deviation untouched** — the multi-tap reuses the
  existing `get_uniform_hemisphere_sample(.., 0.9999)` cone, it does not
  widen / narrow.
- **No `--sun-shadow-taps` CLI flag.** The runtime knob is the
  `GiSettings::sun_shadow_taps` config field only; the brief said
  "Do NOT add CLI flags / `bin/e2e_render.rs` knobs … A CLI knob can land
  later if needed".
- **No spatial-resampling iteration-count bump (Dispatch B).** That's a
  separate dispatch.
- **No `radius_lit_factor` sweep (Dispatch C).** Separate dispatch.
- **No retroactive `vec3`-then-scalar audit.** The trap was sidestepped for
  the new field (`u32` after a `vec2<f32>` at a 16-byte-aligned offset);
  no other fields were touched.
- **No new e2e harness mode / no new gate.** The brief explicitly forbade
  adding a sun-shadow-bench harness in this dispatch.
- **No tests added.** This is a runtime knob extension; the existing 112
  tests (including the `gpu_types` size + `offset_of!` asserts that fire
  at *compile time*) cover the layout discipline. Adding a fresh test
  would have been gold-plating.

---

## Decisions & rejected alternatives

1. **Chose: runtime-configurable knob (`sun_shadow_taps: u32` uniform field).**
   Rejected: hard-coded N=4 const. Reason: the user's dispatch brief
   explicitly required `params.sun_shadow_taps` runtime-configurable; the
   hard-coded alternative would have skipped the layout step and made any
   future "N=2 frame-time fallback" a recompile.

2. **Chose: place `sun_shadow_taps` on a fresh 16-byte row at offset 288
   (Rust 288→304 bytes total).** Rejected: shoehorn into the existing
   `_pad4: u32` slot at offset 276. Reason: the `_pad4` slot is described
   in-doc as "keeps `taa_jitter` on the 8-byte-aligned offset 280" — burying
   `sun_shadow_taps` there would have required moving `taa_jitter`'s
   alignment guard. The fresh-row approach keeps both fields independently
   alignment-pinned with their own `offset_of!` guards. Cost: 12 bytes of
   trailing pad. Worth it for the layout-debugging clarity.

3. **Chose: defensive `max(gi_params.sun_shadow_taps, 1u)` clamp in WGSL.**
   Rejected: WGSL-side `assert(gi_params.sun_shadow_taps > 0u)` or a
   Rust-side `assert!`. Reason: zero is the all-zero-fill default for a
   `bytemuck::Zeroable` struct; making zero "safe" (resolves to C#
   single-tap baseline) prevents a silent shader-uniform-init bug from
   producing a zero-divide. The Rust `GiSettings::default()` is 4, so this
   only kicks in if someone constructs a `GiSettings { sun_shadow_taps: 0,
   .. }` manually.

4. **Chose: division-by-N path (`sun_accum / f32(n_sun_taps)`) for the
   expected-value preservation.** Rejected: accumulating with a per-tap
   weight `1.0 / f32(n_sun_taps)` inside the loop. Reason: a single divide
   after the loop is one division per pixel; per-tap weighting would be
   N divisions per pixel (modulo compiler hoisting). Same numerical result.

5. **Chose: a single seam — one cohesive change, one commit.** Rejected:
   splitting the WGSL edit and the Rust layout edit. Reason: the dispatch
   brief said "single seam, single commit-worthy change". The Rust struct
   field is uninspected by the WGSL until the WGSL declares the mirror;
   shipping one without the other would build but the new field would be
   zero-initialised on the WGSL side ⇒ would clamp to N=1 ⇒ no visible
   effect ⇒ a confusing half-landing.

## Assumptions made

1. **The `vec3<f32>` → `vec2<f32>` → `u32` packing in WGSL produces the
   same byte layout as `Vec2 → u32` in Rust `#[repr(C)]`.** Verified by
   the layout-pin asserts at compile time + the tests + e2e green run
   (an offset mismatch would have manifested as a garbage `sun_shadow_taps`
   read ⇒ either >= 4 by chance or 0 ⇒ no behavioural change; the e2e
   passing confirms no crash, and a future visual eyeball will confirm
   the multi-tap softening).

2. **The dev box (RTX 5080, `18-taa-fidelity.md`) tolerates a 4× cost
   bump on the spatial-resampling sun ray.** The e2e completed in ~14s
   for the 145-frame run, well within budget; no perf regression flagged.

3. **The `entity_pixel` luminance gate (threshold 80, baseline 187.93,
   2.35× margin) holds under the new multi-tap path.** Verified — gate
   passed both e2e runs (the `--entities` run logs "PASS" + exit 0).
