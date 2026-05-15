// naadf_final.wgsl — the Phase-B `base/` final-blit fullscreen fragment pass.
//
// Derives from: render/versions/base/renderFinal.fx `MainPS` (`09-design-b.md`
// §5.9). Reads the per-pixel accumulated colour from `taaSampleAccum`, divides
// by its weight, and outputs **raw linear HDR** — NO tonemapping in this pass.
//
// TONEMAPPING — DELIBERATE USER-DIRECTED DEVIATION (2026-05-15, TAA-fidelity
// track). NAADF's C# `base/renderFinal.fx` does its own Reinhard-ish tonemap
// here (the `exposure` / `toneMappingFac` math). The user directed the port to
// instead "use bevy tonemapping, output raw hdr color from raymarching": the
// camera carries `Camera { hdr: true }` + a `Tonemapping` component
// (`TonyMcMapface`), the view target is an `Rgba16Float` HDR texture, this blit
// writes linear HDR into it, and Bevy's built-in `tonemapping` render-graph
// node (which runs AFTER the NAADF passes — `render/mod.rs` chains the NAADF
// nodes `.before(tonemapping)`) does the tonemap + sRGB encode. The custom
// `exposure` / `tone_mapping_fac` uniform fields are gone from `GpuRenderParams`
// (`18-taa-fidelity.md` fix #2).
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
// The `showRayStep` debug reads `first_hit_data[pixelIndex].z & 0x7FFF`
// directly (`base/:44`).

#import "shaders/render_pipeline_common.wgsl"::{
    GpuRenderParams, FLAG_SHOW_RAY_STEP,
}

// --- the final-blit pass's own small bind group (`03-design.md` §2.6) -------
// first_hit_data (the `base/` `showRayStep` debug reads `.z` from it),
// taa_sample_accum (the blit source), render_params (screen size + flags).
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

    // Output RAW LINEAR HDR — no tonemapping here (user-directed deviation, see
    // the file header). The view target is an `Rgba16Float` HDR texture; Bevy's
    // built-in `tonemapping` render-graph node runs after this pass and does
    // the tonemap (`TonyMcMapface`) + sRGB encode. NAADF's C# `renderFinal.fx`
    // tonemapped here with `exposure`/`toneMappingFac` — the port hands that
    // job to Bevy instead (`18-taa-fidelity.md` fix #2).
    return vec4<f32>(cur_color, 1.0);
}
