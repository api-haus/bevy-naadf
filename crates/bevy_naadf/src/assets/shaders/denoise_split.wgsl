// denoise_split.wgsl — the sparse separable bilateral GI denoiser.
//
// Derives from: render/versions/base/renderDenoiseSplit.fx
// `calcDenoiseHorizontal` + `calcDenoiseVertical` (`09-design-b.md` §5.1, §9.1).
// NAADF's denoiser is a SEPARABLE, SPARSE bilateral filter — kernel 21 (`y` /
// `x` ∈ [-10, 10]), σ = 10, with a random sparse per-row/-column offset ("on
// average every 2nd pixel"). The bilateral weight folds a Gaussian falloff, a
// TAA-weight-difference term, and a normal/material-state match term. Two
// separable compute passes, dispatched in sequence (`09-design-b.md` §4.9),
// gated on `is_denoise`:
//   * `calc_denoise_horizontal` — reads `denoise_preprocessed` (the transposed,
//     column-major scratch `spatial_resampling.wgsl` wrote), filters along `y`,
//     writes `denoise_preprocessed_horizontal` (row-major).
//   * `calc_denoise_vertical`   — reads `denoise_preprocessed_horizontal`,
//     filters along `x`, `lerp`s with the original colour, multiplies by
//     `first_hit_absorption`, and ADDS into `final_color`.
//
// Both passes `[numthreads(64,1,1)]` over `ceil(pixel_count / 64)` workgroups
// (`WorldRenderBase.cs:412-416`). The denoiser does not traverse the voxel
// world — it binds only `@group(0)` = `denoise_bind_group`.
//
// PORT NOTES (`09-design-b.md` §9.1):
// - `rcp(x)` → `1.0 / x`; `gaussianF` → the shared `common.wgsl` `gaussian_f`
//   (σ = 10); the `nextRand` sparse offset uses `init_rand` / `next_rand`.
// - The HLSL `Uint3` `denoisePreprocessed` / `denoisePreprocessedHorizontal`
//   are stored as `vec4<u32>` (`.w` unused padding — `09-design-b.md` §3.3); the
//   port reads `.xyz` and writes `.w = 0`.
// - The bilateral match term is a HLSL `bool` multiplied into a `float` — ported
//   with an explicit `select(0.0, 1.0, ...)`.
// - The transposed indexing is FAITHFUL: `denoise_preprocessed` is written
//   column-major by `spatial_resampling.wgsl`, read column-major here, written
//   row-major into `denoise_preprocessed_horizontal`, read row-major by the
//   vertical pass — ported exactly (`renderDenoiseSplit.fx:18,46,72,81-82,106`).
// - `02-research.md` divergence #11: NAADF ships ONLY this sparse bilateral —
//   there is no SVGF shader to port.
//
// naga-oil import module.

#import "shaders/gi_params.wgsl"::GpuGiParams
#import "shaders/render_pipeline_common.wgsl"::NORMAL
#import "shaders/ray_tracing_common.wgsl"::{init_rand, next_rand}
#import "shaders/common.wgsl"::gaussian_f

// --- @group(0) — the shared denoise bindings --------------------------------

@group(0) @binding(0) var<uniform> gi_params: GpuGiParams;
// `firstHitAbsorption` — the per-pixel primary-ray transmittance, read-only.
// Only the vertical pass uses it (it multiplies the denoised GI by absorption
// before adding into `final_color`).
@group(0) @binding(1) var<storage, read> first_hit_absorption: array<vec2<u32>>;
// `denoisePreprocessed` — the transposed (column-major) GI scratch
// `spatial_resampling.wgsl` wrote. Read by BOTH passes (horizontal reads it as
// the kernel source; vertical reads it for the original colour `colorOrig`).
// `vec4<u32>`-padded `Uint3` (`09-design-b.md` §3.3).
@group(0) @binding(2) var<storage, read> denoise_preprocessed: array<vec4<u32>>;
// `denoisePreprocessedHorizontal` — the horizontal pass writes it (row-major),
// the vertical pass reads it. `vec4<u32>`-padded `Uint3`.
@group(0) @binding(3) var<storage, read_write> denoise_preprocessed_horizontal: array<vec4<u32>>;
// `finalColor` — the GI working-colour buffer. The vertical pass ADDS the
// denoised GI into it (`renderDenoiseSplit.fx:128-131`).
@group(0) @binding(4) var<storage, read_write> final_color: array<vec2<u32>>;

// `calcDenoiseHorizontal` (`renderDenoiseSplit.fx:15-73`) — `[numthreads(64,1,1)]`.
@compute @workgroup_size(64, 1, 1)
fn calc_denoise_horizontal(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let screen_width = i32(gi_params.screen_width);
    let screen_height = i32(gi_params.screen_height);

    // Guard the indirect-free dispatch tail.
    if (global_id.x >= u32(screen_width * screen_height)) {
        return;
    }

    // The TRANSPOSED read index: `pixelPos = (globalID.x / h, globalID.x % h)`.
    let pixel_pos = vec2<i32>(
        i32(global_id.x) / screen_height,
        i32(global_id.x) % screen_height,
    );
    var rand = init_rand(vec3<u32>(vec2<u32>(pixel_pos), gi_params.rand_counter));

    let processed = denoise_preprocessed[global_id.x];

    let color_orig = vec3<f32>(
        unpack2x16float(processed.x & 0xFFFFu).x,
        unpack2x16float(processed.x >> 16u).x,
        unpack2x16float(processed.y & 0xFFFFu).x,
    );
    let taa_weight = unpack2x16float(processed.y >> 16u).x;

    // The horizontal pass masks `processed.z` with `0x7FFFF`
    // (`renderDenoiseSplit.fx:26`).
    let normal_tang_comp = processed.z & 0x7FFFFu;
    if (normal_tang_comp == 0u) {
        return;
    }
    let state = processed.z >> 19u;

    var color = vec3<f32>(0.0, 0.0, 0.0);
    var weight: f32 = 0.000001;
    var total_taa_weight: f32 = 0.000001;

    for (var y: i32 = -10; y <= 10; y = y + 1) {
        // The random sparse x-offset — "on average every 2nd pixel"
        // (`renderDenoiseSplit.fx:41`).
        let x = select(0, 1, next_rand(&rand) < 0.5 && y != 0);
        let cur_pixel_pos = pixel_pos + vec2<i32>(x, y * 2);

        if (cur_pixel_pos.x >= 0 && cur_pixel_pos.x < screen_width
            && cur_pixel_pos.y >= 0 && cur_pixel_pos.y < screen_height) {
            // The TRANSPOSED kernel-source index.
            let cur_index = cur_pixel_pos.y + cur_pixel_pos.x * screen_height;
            let cur_processed = denoise_preprocessed[u32(cur_index)];

            let cur_color = vec3<f32>(
                unpack2x16float(cur_processed.x & 0xFFFFu).x,
                unpack2x16float(cur_processed.x >> 16u).x,
                unpack2x16float(cur_processed.y & 0xFFFFu).x,
            );
            let cur_taa_weight = unpack2x16float(cur_processed.y >> 16u).x;

            let bilateral_fac =
                1.0 / (1.0 + abs(cur_taa_weight - taa_weight) * gi_params.denoise_thresh);
            // The HLSL `bool` match term multiplied into the `float` factor.
            let match_term = select(
                0.0, 1.0,
                normal_tang_comp == (cur_processed.z & 0x7FFFFu)
                    && state == (cur_processed.z >> 19u),
            );
            var fac = bilateral_fac * match_term;
            fac *= gaussian_f(f32(y), 10.0);
            color += cur_color * fac;
            total_taa_weight += cur_taa_weight * fac;
            weight += fac;
        }
    }

    total_taa_weight /= weight;
    color /= weight;

    let cur_color_comp = vec2<u32>(
        pack2x16float(vec2<f32>(color.x, color.y)),
        pack2x16float(vec2<f32>(color.z, 0.0)) & 0xFFFFu,
    );

    var new_denoise_processed = vec3<u32>(0u, 0u, 0u);
    new_denoise_processed.x = cur_color_comp.x;
    new_denoise_processed.y = (cur_color_comp.y & 0xFFFFu)
        | ((pack2x16float(vec2<f32>(total_taa_weight, 0.0)) & 0xFFFFu) << 16u);
    new_denoise_processed.z = processed.z;

    // Write ROW-major into the horizontal scratch.
    denoise_preprocessed_horizontal[u32(pixel_pos.x + pixel_pos.y * screen_width)] =
        vec4<u32>(new_denoise_processed, 0u);
}

// `calcDenoiseVertical` (`renderDenoiseSplit.fx:75-132`) — `[numthreads(64,1,1)]`.
@compute @workgroup_size(64, 1, 1)
fn calc_denoise_vertical(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let screen_width = i32(gi_params.screen_width);
    let screen_height = i32(gi_params.screen_height);

    if (global_id.x >= u32(screen_width * screen_height)) {
        return;
    }

    // Row-major `pixelPos = (globalID.x % w, globalID.x / w)`.
    let pixel_pos = vec2<i32>(
        i32(global_id.x) % screen_width,
        i32(global_id.x) / screen_width,
    );
    // HLSL `randCounter + 11` — the vertical pass uses a distinct salt offset.
    var rand = init_rand(vec3<u32>(vec2<u32>(pixel_pos), gi_params.rand_counter + 11u));

    // `processed` is read from `denoisePreprocessed` at the TRANSPOSED index
    // (the original GI colour `colorOrig` + the normal/state code).
    let processed = denoise_preprocessed[u32(pixel_pos.y + pixel_pos.x * screen_height)];
    // `processed2` is the horizontal pass's output at this pixel (row-major).
    let processed2 = denoise_preprocessed_horizontal[global_id.x];

    let color_orig = vec3<f32>(
        unpack2x16float(processed.x & 0xFFFFu).x,
        unpack2x16float(processed.x >> 16u).x,
        unpack2x16float(processed.y & 0xFFFFu).x,
    );
    let taa_weight = unpack2x16float(processed2.y >> 16u).x;

    // The vertical pass masks `processed.z` with `0xFFFFF`
    // (`renderDenoiseSplit.fx:87` — note the wider mask vs. the horizontal pass's
    // `0x7FFFF`; ported faithfully).
    let normal_tang_comp = processed.z & 0xFFFFFu;
    if (normal_tang_comp == 0u) {
        return;
    }
    let state = processed.z >> 19u;

    var color = vec3<f32>(0.0, 0.0, 0.0);
    var weight: f32 = 0.000001;

    for (var x: i32 = -10; x <= 10; x = x + 1) {
        // The random sparse y-offset.
        let y = select(0, 1, next_rand(&rand) < 0.5 && x != 0);
        let cur_pixel_pos = pixel_pos + vec2<i32>(x * 2, y);

        if (cur_pixel_pos.x >= 0 && cur_pixel_pos.x < screen_width
            && cur_pixel_pos.y >= 0 && cur_pixel_pos.y < screen_height) {
            // Row-major kernel-source index into the horizontal scratch.
            let cur_index = cur_pixel_pos.x + cur_pixel_pos.y * screen_width;
            let cur_processed = denoise_preprocessed_horizontal[u32(cur_index)];

            let cur_color = vec3<f32>(
                unpack2x16float(cur_processed.x & 0xFFFFu).x,
                unpack2x16float(cur_processed.x >> 16u).x,
                unpack2x16float(cur_processed.y & 0xFFFFu).x,
            );
            let cur_taa_weight = unpack2x16float(cur_processed.y >> 16u).x;

            let bilateral_fac =
                1.0 / (1.0 + abs(cur_taa_weight - taa_weight) * gi_params.denoise_thresh);
            let match_term = select(
                0.0, 1.0,
                normal_tang_comp == (cur_processed.z & 0x7FFFFu)
                    && state == (cur_processed.z >> 19u),
            );
            var fac = bilateral_fac * match_term;
            fac *= gaussian_f(f32(x), 10.0);
            color += cur_color * fac;
            weight += fac;
        }
    }

    color /= weight;
    var final_col = mix(color_orig, color, 0.92);

    let absorption_comp = first_hit_absorption[global_id.x];
    let absorption = vec3<f32>(
        unpack2x16float(absorption_comp.x & 0xFFFFu).x,
        unpack2x16float(absorption_comp.x >> 16u).x,
        unpack2x16float(absorption_comp.y).x,
    );
    final_col *= absorption;

    let final_col_comp = final_color[global_id.x];
    final_col += vec3<f32>(
        unpack2x16float(final_col_comp.x & 0xFFFFu).x,
        unpack2x16float(final_col_comp.x >> 16u).x,
        unpack2x16float(final_col_comp.y).x,
    );
    final_color[global_id.x] = vec2<u32>(
        pack2x16float(vec2<f32>(final_col.x, final_col.y)),
        pack2x16float(vec2<f32>(final_col.z, 0.0)) & 0xFFFFu,
    );
}
