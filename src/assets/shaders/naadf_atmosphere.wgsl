// naadf_atmosphere.wgsl — the atmosphere precompute compute pass.
//
// Derives from: render/versions/base/renderAtmosphere.fx `precomputeAtmosphere`
// (`09-design-b.md` §5.1 / §9.2). One invocation handles
// `ID = globalID.x * 4 + (frameCount % 4)` — i.e. **one quarter of the
// octahedral atmosphere buffer per frame**, amortised over 4 frames
// (`renderAtmosphere.fx:12`).
//
// `[numthreads(64,1,1)]` in the HLSL → `@workgroup_size(64,1,1)`.
//
// --- Faithful-port deviations ----------------------------------------------
//   * The HLSL `RWStructuredBuffer<uint3> atmosphereComp` has a 12-byte stride;
//     WGSL `array<vec3<u32>>` has a 16-byte stride. The buffer is stored as
//     `array<vec4<u32>>` (`.w` unused padding) and sized accordingly
//     (`09-design-b.md` §3.3). This shader writes `.w = 0u`.
//   * `addLightForDirection` is the shared `atmosphere.wgsl` function; the
//     sky parameters arrive in the `AtmosphereParams` uniform.
//
// naga-oil import module entry point: `precompute_atmosphere`.

#import "shaders/ray_tracing_common.wgsl"::oct_decode
#import "shaders/atmosphere.wgsl"::{
    AtmosphereParams, AtmoLight, add_light_for_direction,
}

// The single bind group: the sky-param uniform + the precomputed atmosphere
// buffer (read-write — this pass writes one quarter of it per frame).
@group(0) @binding(0) var<uniform> params: AtmosphereParams;
@group(0) @binding(1) var<storage, read_write> atmosphere_comp: array<vec4<u32>>;

@compute @workgroup_size(64, 1, 1)
fn precompute_atmosphere(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // `ID = globalID.x * 4 + (frameCount % 4)` — the quarter-per-frame stride.
    let id = global_id.x * 4u + (params.frame_count % 4u);
    if (id >= params.atmosphere_tex_size_x * params.atmosphere_tex_size_y) {
        return;
    }

    let tex_pos = vec2<u32>(
        id % params.atmosphere_tex_size_x,
        id / params.atmosphere_tex_size_x,
    );
    let tex_pos_norm = vec2<f32>(tex_pos)
        / vec2<f32>(f32(params.atmosphere_tex_size_x), f32(params.atmosphere_tex_size_y));
    let oct_pos = vec2<f32>(tex_pos_norm.x, tex_pos_norm.y);
    var ray_dir = oct_decode(oct_pos);
    // The `rayDir.y` warp (`renderAtmosphere.fx:20-21`).
    ray_dir.y = pow(abs(ray_dir.y), 2.0) * sign(ray_dir.y);
    let xz_scale = sqrt(
        (1.0 - ray_dir.y * ray_dir.y)
        / (ray_dir.x * ray_dir.x + ray_dir.z * ray_dir.z),
    );
    ray_dir.x *= xz_scale;
    ray_dir.z *= xz_scale;

    // `addLightForDirection` takes the params by pointer (the shared module's
    // signature) — copy the uniform into a function-scope `var` to take `&`.
    var p = params;
    var acc: AtmoLight;
    acc.absorption = vec3<f32>(1.0, 1.0, 1.0);
    acc.light = vec3<f32>(0.0, 0.0, 0.0);
    acc = add_light_for_direction(
        &p,
        vec3<f32>(0.0, params.cam_pos.y, 0.0),
        ray_dir,
        1000000.0,
        acc,
        true,
        params.sky_main_ray_steps,
        params.sky_sub_scatter_steps,
    );

    var atmo_comp: vec4<u32>;
    atmo_comp.x = pack2x16float(vec2<f32>(acc.light.r, acc.light.g));
    atmo_comp.y = pack2x16float(vec2<f32>(acc.light.b, acc.absorption.r));
    atmo_comp.z = pack2x16float(vec2<f32>(acc.absorption.g, acc.absorption.b));
    atmo_comp.w = 0u;
    atmosphere_comp[id] = atmo_comp;
}
