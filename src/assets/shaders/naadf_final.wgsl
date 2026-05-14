// naadf_final.wgsl — the Phase-B `base/` final-blit fullscreen fragment pass.
//
// Derives from: render/versions/base/renderFinal.fx `MainPS` (`09-design-b.md`
// §5.9). A near-verbatim port: read the per-pixel accumulated colour, divide by
// its weight, tonemap (with the `tone_mapping_fac` uniform term), output.
//
// The C# draws a unit cube whose pixel shader runs over the screen; the Bevy
// port uses a standard fullscreen triangle (`02-research.md` divergence #9) —
// the vertex stage is Bevy's `FullscreenShader`, so this file is fragment-only.
//
// Blit source (`09-design-b.md` §5.9): the C# `base/` `MainPS` reads
// `taaSampleAccum` — written by the `base/` TAA passes `ReprojectOld` (the
// reprojected history sum) + `CalcNewTaaSample` (history + this frame's
// denoised GI light). Phase B Batch 6 reverted the Batch-2 temporary
// `final_color` blit seam — the blit source is `taa_sample_accum` again
// (`prepare_frame_gpu` clears `FLAG_BLIT_FINAL_COLOR` + binds
// `taa_sample_accum` at the blit slot).
//
// vs. the A-2 `albedo/renderFinal.fx`: (a) the tonemap denominator is the
// `tone_mapping_fac` uniform (`base/:55`) instead of a hardcoded `1.0`;
// (b) the `showRayStep` debug reads `first_hit_data[pixelIndex].z & 0x7FFF`
// directly (`base/:44`) instead of `col_samples.x`. `tone_mapping_fac` is a
// constant in the port — `prepare_frame_gpu` sets `GpuRenderParams
// .tone_mapping_fac = 1.0` (C# `Settings.data.general.toneMappingFac`).
//
// `HDR` is off in the port (`03-design.md` §5.4) — the C# `#ifdef HDR`
// branches are dropped.

#import "shaders/render_pipeline_common.wgsl"::{
    GpuRenderParams, FLAG_SHOW_RAY_STEP,
}

// --- the final-blit pass's own small bind group (`03-design.md` §2.6) -------
// first_hit_data (the `base/` `showRayStep` debug reads `.z` from it),
// taa_sample_accum (the blit source), render_params (screen size + exposure +
// tone_mapping_fac).
@group(0) @binding(0) var<storage, read> first_hit_data: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> taa_sample_accum: array<vec2<u32>>;
@group(0) @binding(2) var<uniform> params: GpuRenderParams;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // `int2 pixelPos = input.Position.xy;` — the framebuffer pixel coord.
    let pixel_pos = vec2<i32>(floor(position.xy));
    let pixel_index = u32(pixel_pos.x) + u32(pixel_pos.y) * params.screen_width;

    // `uint2 colSamples = taaSampleAccum[pixelIndex];` — `base/renderFinal.fx:
    // 38-40`. `weight = .x & 0xFFFF`, RGB = `(.x>>16, .y&0xFFFF, .y>>16)`
    // divided by `max(1, weight)`.
    let col_samples = taa_sample_accum[pixel_index];
    let lo = unpack2x16float(col_samples.x);
    let hi = unpack2x16float(col_samples.y);
    let weight = lo.x;
    var cur_color = vec3<f32>(lo.y, hi.x, hi.y) / max(1.0, weight);

    // Ray-step debug view (`base/renderFinal.fx:42-47`): the `base/` variant
    // reads the raw step count from `first_hit_data[pixelIndex].z & 0x7FFF`
    // (the `albedo/` variant read `colSamples.x` — `09-design-b.md` §5.9).
    if ((params.flags & FLAG_SHOW_RAY_STEP) != 0u) {
        let ray_steps = f32(first_hit_data[pixel_index].z & 0x7FFFu);
        let intensity = ray_steps * 0.01;
        cur_color = vec3<f32>(intensity, intensity, intensity);
    }

    // Tone mapping (`base/renderFinal.fx:53-56`):
    //   luminance = dot(curColor, (0.2126, 0.7152, 0.0722))
    //   tv = curColor / (toneMappingFac + curColor)
    //   colorNormalized = lerp(curColor / (exposure + luminance), tv, tv)
    let luminance = dot(cur_color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let tv = cur_color / (vec3<f32>(params.tone_mapping_fac) + cur_color);
    let color_normalized = mix(
        cur_color / (vec3<f32>(params.exposure) + vec3<f32>(luminance)),
        tv,
        tv,
    );

    return vec4<f32>(color_normalized, 1.0);
}
