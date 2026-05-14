// gi_params.wgsl — the shared per-frame GI uniform struct + the GI flag consts.
//
// Derives from: the union of every `base/` GI pass's scalar uniforms
// (`09-design-b.md` §3.8 — verified against `rayQueueCalc.fx:9-10`,
// `renderGlobalIllum.fx:16-28`, `renderSampleRefine.fx:21-32`,
// `renderSpatialResampling.fx:15-27`, `renderDenoiseSplit.fx:11-14`). One shared
// uniform bound by every GI pass. The CPU counterpart is
// `gpu_types::GpuGiParams` (288 bytes — kept in step by the compile-time size
// assert there).
//
// LAYOUT — naga-oil composable-module structs cannot carry the `_pad0`-style /
// `data1`-style identifiers the Rust `#[repr(C)]` struct uses (naga writeback
// rejects trailing-digit identifiers and bare `_padN`).
//
// CRITICAL — the `vec3`-then-scalar uniform trap (the SAME bug that bit
// `AtmosphereParams` in Batch 1 and `GpuTaaParams` in Batch 6). A WGSL
// `vec3<T>` has size 12 / align 16. When a `vec3` is followed by another
// 16-byte-aligned member (or ends the struct) WGSL's `vec3`→16-byte slotting
// reproduces the padded Rust `#[repr(C)]` layout — so the first three rows
// (`cam_pos_int`/`cam_pos_frac`/`sky_sun_dir`, each `vec3` followed by a `vec3`)
// would be fine as bare `vec3`. But the FOURTH row, `sun_color` (`vec3<f32>`,
// 176..188), is followed by `screen_width` (a lone `u32`) — and WGSL packs that
// scalar into `sun_color`'s trailing 4 bytes (offset 188), whereas the Rust
// struct has an explicit `_pad3: u32` there and writes `screen_width` at 192.
// A bare-`vec3` `sun_color` therefore shifts EVERY scalar-tail field 4 bytes
// early: `screen_width` reads Rust's `_pad3` (always 0) ⇒ `pixel_count == 0`,
// `bucket_count` reads a wrong value ⇒ `clear_buckets_and_calc_mask`'s
// `global_id.x >= bucket_count` guard rejects every lane ⇒ `bucket_info` never
// populated ⇒ the entire `renderSampleRefine → renderSpatialResampling` GI
// reservoir chain produces nothing ⇒ NO visible GI bounce on diffuse geometry.
// (The original "verified field-by-field — no explicit pad needed" claim here
// was WRONG, exactly as the identical `GpuTaaParams` claim was — `10-impl-b.md`
// Batch-6 TAA-path fix.)
//
// THE FIX (2026-05-15) — declare ALL FOUR position/colour rows as `vec4`: the
// Rust `_pad0`/`_pad1`/`_pad2`/`_pad3` `u32`s become the `.w` lanes, so every
// member from `screen_width` on lands at exactly the offset the 288-byte Rust
// `#[repr(C)]` struct writes it to. Consumers read `.xyz`. This is the standard
// idiomatic WGSL way to mirror a padded `repr(C)` struct (the same fix
// `GpuTaaParams` got). Layout: `inv_view_proj` (0..64), `view_proj` (64..128),
// `cam_pos_int` vec4 (128..144), `cam_pos_frac` vec4 (144..160), `sky_sun_dir`
// vec4 (160..176), `sun_color` vec4 (176..192), then the 24-`u32` scalar tail
// (`screen_width` at 192 … the last `u32` at 284, struct end 288).
//
// naga-oil import module.

// Per-frame GI uniform (mirrors `gpu_types::GpuGiParams`).
struct GpuGiParams {
    // C# `invCamMatrix` — `getRayDir` in `globalIllum` / `spatialResampling`.
    inv_view_proj: mat4x4<f32>,
    // C# `camMatrix` — `sampleRefine`'s reproject (rotation-only view-proj).
    view_proj: mat4x4<f32>,
    // C# `camPosInt` — `vec4` so the Rust `_pad0` is the `.w` lane (see the
    // LAYOUT note above); consumers read `.xyz`.
    cam_pos_int: vec4<i32>,
    // C# `camPosFrac` — `vec4` (Rust `_pad1` in `.w`); consumers read `.xyz`.
    cam_pos_frac: vec4<f32>,
    // C# `skySunDir` — shared with the atmosphere uniform. `vec4` (Rust `_pad2`
    // in `.w`); consumers read `.xyz`.
    sky_sun_dir: vec4<f32>,
    // C# `sunColor` = `Atmosphere.GetLightForPoint` (`09-design-b.md` §9.2).
    // `vec4` (Rust `_pad3` in `.w`) — this is the row whose bare-`vec3` form was
    // the bug; consumers read `.xyz`.
    sun_color: vec4<f32>,
    // --- the 24-u32 scalar tail (offset 192) -------------------------------
    screen_width: u32,
    screen_height: u32,
    // C# `frameCount` / `frameIndex`.
    frame_count: u32,
    // C# `taaIndex`.
    taa_index: u32,
    // `globalIllumMaxAccum - (frameCount % globalIllumMaxAccum) - 1`.
    accum_index: u32,
    // Per-frame RNG salt (C# `randCounter`).
    rand_counter: u32,
    // Second per-frame RNG salt (C# `randCounter2`). PORT NOTE: naga-oil rejects
    // trailing-digit identifiers in a composable-module struct ("must not
    // require substitution according to naga writeback rules"), so the WGSL
    // field is `rand_counter_b` — same content as the Rust `rand_counter2`,
    // naga-oil-safe name (the field is read positionally by offset, the name is
    // not load-bearing — same fix as Batch 1's `SampleValid.data_a`/`data_b`).
    rand_counter_b: u32,
    // Max secondary-ray bounce count (C# `bounceCount` = 3).
    max_bounce_count: u32,
    // 8×8 bucket-grid cell size in pixels.
    bucket_size_x: u32,
    bucket_size_y: u32,
    // Total bucket count.
    bucket_count: u32,
    // GI accumulation-ring depth (C# `globalIllumMaxAccum` = 128).
    sample_max_accum: u32,
    // Lit-sample ring depth multiplier (C# = 2).
    valid_sample_storage_count: u32,
    // Unlit-sample ring depth multiplier (C# = 8).
    invalid_sample_storage_count: u32,
    // Per-bucket refined-sample capacity (C# = 32).
    bucket_storage_count: u32,
    // Per-bucket compressed-sample capacity (C# = 8).
    refined_bucket_storage_count: u32,
    // Spatial-resampling neighbour-search size (C# = 500.0).
    spatial_resample_size: f32,
    // Lit-radius factor (C# = 3.0).
    radius_lit_factor: f32,
    // Noise-suppression factor (C# = 0.4).
    noise_suppression_factor: f32,
    // Denoiser threshold (C# = 400.0).
    denoise_thresh: f32,
    // Packed GI flags — see the `GI_FLAG_*` consts.
    flags: u32,
    // Trailing pad u32s (mirror the Rust struct's `_pad4` / `_pad5` / `_pad6`).
    pad_a: u32,
    pad_b: u32,
    pad_c: u32,
}

// `flags` bits (mirror `gpu_types::GI_FLAG_*`).
const GI_FLAG_SKIP_SAMPLES: u32 = 1u;
const GI_FLAG_IS_DENOISE: u32 = 2u;
const GI_FLAG_IS_SAMPLE_LEVELING: u32 = 4u;
const GI_FLAG_IS_VARYING_RADIUS: u32 = 8u;
const GI_FLAG_IS_ATMOSPHERE_INTERACTION: u32 = 16u;
