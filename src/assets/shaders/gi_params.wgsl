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
// rejects trailing-digit identifiers and bare `_padN`). But the WGSL std140-ish
// uniform layout pads a `vec3` to a 16-byte slot anyway, so this struct needs
// NO explicit pad members and is still byte-identical to the padded Rust
// struct: `inv_view_proj` (0..64), `view_proj` (64..128), then four 16-byte
// `vec3` rows — `cam_pos_int` (128, slot 128..144), `cam_pos_frac` (144),
// `sky_sun_dir` (160), `sun_color` (176) — then the 24-`u32` scalar tail
// (`screen_width` at 192 … the last `u32` at 192 + 23*4 = 284, struct end 288).
// This is the *same* convention `render_pipeline_common.wgsl`'s `GpuCamera` /
// `GpuRenderParams` use (and it holds here because every `vec3` row IS followed
// by another `vec3` or by a u32 that the Rust struct also pads — verified
// field-by-field against `gpu_types::GpuGiParams`).
//
// (The `vec3`-then-scalar uniform trap that bit `AtmosphereParams` in Batch 1
// does NOT apply here: there is no `vec3` immediately followed by a lone scalar
// — the four `vec3` rows are contiguous, then the scalar tail begins on a fresh
// 16-byte boundary at offset 192.)
//
// naga-oil import module.

// Per-frame GI uniform (mirrors `gpu_types::GpuGiParams`).
struct GpuGiParams {
    // C# `invCamMatrix` — `getRayDir` in `globalIllum` / `spatialResampling`.
    inv_view_proj: mat4x4<f32>,
    // C# `camMatrix` — `sampleRefine`'s reproject (rotation-only view-proj).
    view_proj: mat4x4<f32>,
    // C# `camPosInt`.
    cam_pos_int: vec3<i32>,
    // C# `camPosFrac`.
    cam_pos_frac: vec3<f32>,
    // C# `skySunDir` — shared with the atmosphere uniform.
    sky_sun_dir: vec3<f32>,
    // C# `sunColor` = `Atmosphere.GetLightForPoint` (`09-design-b.md` §9.2).
    sun_color: vec3<f32>,
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
