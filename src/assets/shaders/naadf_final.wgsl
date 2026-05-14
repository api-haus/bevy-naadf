// naadf_final.wgsl — the Phase-A final-blit fullscreen fragment pass.
//
// Derives from: render/versions/albedo/renderFinal.fx `MainPS` (`03-design.md`
// §5.5, §5.4). A near-verbatim port: read the per-pixel accumulated colour,
// divide by its weight, tonemap, output.
//
// The C# draws a unit cube whose pixel shader runs over the screen; the Bevy
// port uses a standard fullscreen triangle (`02-research.md` divergence #9) —
// the vertex stage is Bevy's `FullscreenShader`, so this file is fragment-only.
//
// Blit source (`03-design.md` §5.3, `06-design-a2.md` §5.4): the C# `MainPS`
// reads `taaSampleAccum`. Phase A used a `shaded_color` stand-in built to the
// `taaSampleAccum` `vec2<u32>` element format; Phase A-2 renamed it to
// `taa_sample_accum` and the buffer is now the real `taaSampleAccum` (owned by
// `TaaGpu`, written by the first-hit pass — and, in Batch 2, accumulated by the
// TAA reproject node). The tonemap below is the C# `MainPS` unchanged — the
// element format never changed, so the swap is logic-free.
//
// `HDR` is off in Phase A (`03-design.md` §5.4) — the C# `#ifdef HDR` branches
// are dropped.

#import "shaders/render_pipeline_common.wgsl"::{GpuRenderParams, FLAG_SHOW_RAY_STEP}

// --- the final-blit pass's own small bind group (`03-design.md` §2.6) -------
// first_hit_data (unused in the blit but bound for layout stability),
// taa_sample_accum (the blit source), render_params (screen size + exposure).
@group(0) @binding(0) var<storage, read> first_hit_data: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> taa_sample_accum: array<vec2<u32>>;
@group(0) @binding(2) var<uniform> params: GpuRenderParams;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // `int2 pixelPos = input.Position.xy;` — the framebuffer pixel coord.
    let pixel_pos = vec2<i32>(floor(position.xy));
    let pixel_index = u32(pixel_pos.x) + u32(pixel_pos.y) * params.screen_width;

    // `uint2 colSamples = taaSampleAccum[pixelIndex];`
    let col_samples = taa_sample_accum[pixel_index];
    // `float weight = f16tof32(colSamples.x & 0xFFFF);`
    let weight = unpack2x16float(col_samples.x).x;
    // RGB is `f16(.x>>16), f16(.y&0xFFFF), f16(.y>>16)` / max(1, weight).
    let rgb_x = unpack2x16float(col_samples.x);
    let rgb_y = unpack2x16float(col_samples.y);
    var cur_color = vec3<f32>(rgb_x.y, rgb_y.x, rgb_y.y) / max(1.0, weight);

    // Ray-step debug view: `.x` holds the raw step count, shown as greyscale.
    if ((params.flags & FLAG_SHOW_RAY_STEP) != 0u) {
        let ray_steps = f32(col_samples.x);
        let intensity = ray_steps * 0.01;
        cur_color = vec3<f32>(intensity, intensity, intensity);
    }

    // Tone mapping (HLSL `MainPS`):
    //   luminance = dot(curColor, (0.2126, 0.7152, 0.0722))
    //   tv = curColor / (1 + curColor)
    //   colorNormalized = lerp(curColor / (exposure + luminance), tv, tv)
    let luminance = dot(cur_color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let tv = cur_color / (vec3<f32>(1.0, 1.0, 1.0) + cur_color);
    let color_normalized = mix(
        cur_color / (vec3<f32>(params.exposure) + vec3<f32>(luminance)),
        tv,
        tv,
    );

    // Keep `first_hit_data` referenced so the binding is not stripped from the
    // layout (Phase A's blit does not read it — plane reconstruction is the
    // Phase-B `getHitDataFromPlanes` path).
    let touch = first_hit_data[0u].x;

    return vec4<f32>(color_normalized, 1.0) + vec4<f32>(0.0, 0.0, 0.0, f32(touch) * 0.0);
}
