// atmosphere.wgsl — the multiple-scattering sky model: the ray-marched
// `add_light_for_direction` and the precomputed-buffer sampler
// `apply_atmosphere`.
//
// Derives from: render/common/atmosphere/atmosphereRaw.fxh +
// atmospherePrecomputed.fxh (`09-design-b.md` §5.1 / §5.4 / §9.2).
//
// **Phase-B-only** — Phase A/A-2 used only the inline sun+ambient term in
// `naadf_first_hit.wgsl`. In Batch 1 this module is consumed by
// `naadf_atmosphere.wgsl` (the precompute entry); Batch 2+ wires
// `apply_atmosphere` into the 4-plane first-hit + the GI passes.
//
// The HLSL `atmosphereRaw.fxh` reads its sky parameters as free-standing
// uniform globals; the port bundles them into the `AtmosphereParams` struct
// (mirrors `gpu_types::GpuAtmosphereParams`) and threads it through every
// function explicitly — WGSL import modules cannot own bindings the entry
// shader supplies. `apply_atmosphere` cannot take the `atmosphere_comp` storage
// buffer by `ptr` either (WGSL forbids passing `ptr<storage,...>` into a
// function): instead `atmosphere_oct_index` computes the buffer index and the
// caller (which owns the binding) fetches the slot and passes its value in.
//
// The HLSL `addLightForDirection` has `const bool` / `const int` template-style
// params (`includeMie`, `mainIterationCount`, `secondIterationCount`,
// `includeSun`) — WGSL has no specialisation; they are plain runtime args.
//
// naga-oil import module.

#import "shaders/ray_tracing_common.wgsl"::{oct_encode, oct_decode}

// The sky-model parameters (mirrors `gpu_types::GpuAtmosphereParams`, 128 bytes).
//
// EXPLICIT padding members — UNLIKE the other shared structs. The
// `render_pipeline_common.wgsl` convention ("no explicit pad, rely on `vec3`
// 16-byte slotting") only works when every `vec3` is followed by another
// `vec3` or is the *last* field: a WGSL `vec3<f32>` has size 12 / align 16, so
// a trailing scalar packs into the vec3's 4th slot — but the Rust `#[repr(C)]`
// struct has an explicit `_pad` u32 there, so the layouts diverge from that
// point on. `GpuAtmosphereParams` has `sky_sun_color: Vec3` followed by the
// scalar `sky_mie_scatter`, so the pads MUST be explicit here to keep the
// uniform byte-identical to the Rust struct (this is exactly the `vec3`
// alignment gotcha in `09-design-b.md` §12 #3, in its uniform-struct form).
//
// Offsets: cam_pos(0) _pad0(12) sky_sun_dir(16) _pad1(28) sky_rayleigh_scatter
// (32) _pad2(44) sky_ozone_absorb(48) _pad3(60) sky_sun_color(64) _pad4(76)
// sky_mie_scatter(80) sky_sphere_radius(84) sky_atmosphere_thickness(88)
// sky_atmosphere_density(92) sky_absorb_intensity(96) sky_scatter_intensity
// (100) sky_mie_factor(104) sky_main_ray_steps(108) sky_sub_scatter_steps(112)
// atmosphere_tex_size_x(116) atmosphere_tex_size_y(120) frame_count(124) —
// total 128 bytes.
// PORT NOTE: the pad fields are named `pad_*` (no trailing digit) — naga-oil
// rejects trailing-digit identifiers in a composable-module struct ("must not
// require substitution according to naga writeback rules", the same rule that
// forced `SampleValid`'s `data1`/`data2` → `data_a`/`data_b`).
struct AtmosphereParams {
    cam_pos: vec3<f32>,
    pad_cam: u32,
    sky_sun_dir: vec3<f32>,
    pad_sun_dir: u32,
    sky_rayleigh_scatter: vec3<f32>,
    pad_rayleigh: u32,
    sky_ozone_absorb: vec3<f32>,
    pad_ozone: u32,
    sky_sun_color: vec3<f32>,
    pad_sun_color: u32,
    sky_mie_scatter: f32,
    sky_sphere_radius: f32,
    sky_atmosphere_thickness: f32,
    sky_atmosphere_density: f32,
    sky_absorb_intensity: f32,
    sky_scatter_intensity: f32,
    sky_mie_factor: f32,
    sky_main_ray_steps: u32,
    sky_sub_scatter_steps: u32,
    atmosphere_tex_size_x: u32,
    atmosphere_tex_size_y: u32,
    frame_count: u32,
}

// `rayleigh` — the Rayleigh phase term (HLSL `rayleigh`).
fn rayleigh(angle: f32) -> f32 {
    return (3.0 / 4.0) * (1.0 + angle * angle);
}

// `phaseFunction` — the Henyey-Greenstein-style Mie phase function (HLSL
// `phaseFunction`).
fn phase_function(angle: f32, g: f32) -> f32 {
    return ((3.0 * (1.0 - g * g)) / (2.0 * (2.0 + g * g)))
        * ((1.0 + angle * angle) / pow(abs(1.0 + g * g - 2.0 * g * angle), 3.0 / 2.0));
}

// `densityAtHeight` — Rayleigh / Mie / Ozone density at a normalised height
// (HLSL `densityAtHeight`).
fn density_at_height(p: ptr<function, AtmosphereParams>, height: f32) -> vec3<f32> {
    var density: vec3<f32>;
    density.x = exp(-height / 0.3) * (1.0 - height); // Rayleigh
    density.y = exp(-height / 0.2) * (1.0 - height); // Mie
    density.z = max(0.0, 1.0 - (abs(height - 0.25) / 0.15)); // Ozone
    return density * (*p).sky_atmosphere_density;
}

// `raySphere` — ray vs. sphere intersection, returns `(dstNear, dstFar-dstNear)`
// or `(0,0)` on a miss (HLSL `raySphere`).
fn ray_sphere(
    sphere_origin: vec3<f32>,
    sphere_radius: f32,
    ray_origin: vec3<f32>,
    ray_dir: vec3<f32>,
) -> vec2<f32> {
    let dif = ray_origin - sphere_origin;
    let a = 1.0;
    let b = 2.0 * dot(dif, ray_dir);
    let c = dot(dif, dif) - sphere_radius * sphere_radius;
    let d = b * b - 4.0 * a * c;

    if (d > 0.0) {
        let s = sqrt(d);
        let dst_near = max(0.0, (-b - s) / (2.0 * a));
        let dst_far = (-b + s) / (2.0 * a);
        if (dst_far >= 0.0) {
            return vec2<f32>(dst_near, dst_far - dst_near);
        }
    }
    return vec2<f32>(0.0, 0.0);
}

// `scatterForDensities` — combine the per-component densities into a scatter
// coefficient (HLSL `scatterForDensities`).
fn scatter_for_densities(p: ptr<function, AtmosphereParams>, densities: vec3<f32>) -> vec3<f32> {
    return (densities.x * (*p).sky_rayleigh_scatter
        + densities.y * (*p).sky_mie_scatter
        + densities.z * (*p).sky_ozone_absorb)
        * (*p).sky_scatter_intensity;
}

// `getScatterDensitiesAtPoint` — march from `pos` toward the sun, accumulating
// densities (HLSL `getScatterDensitiesAtPoint`).
fn get_scatter_densities_at_point(
    p: ptr<function, AtmosphereParams>,
    pos: vec3<f32>,
    second_iteration_count: u32,
) -> vec3<f32> {
    let earth_with_atmo_radius = (*p).sky_sphere_radius + (*p).sky_atmosphere_thickness;

    let ray_result = ray_sphere(
        vec3<f32>(0.0, 0.0, 0.0), earth_with_atmo_radius, pos, (*p).sky_sun_dir,
    );
    if (ray_result.y == 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let scale = ray_result.y / f32(second_iteration_count);

    var total_densities = vec3<f32>(0.0, 0.0, 0.0);
    // HLSL: `for (int i = secondIterationCount - 1; i >= 0; --i)`.
    for (var i = i32(second_iteration_count) - 1; i >= 0; i = i - 1) {
        let factor = f32(i) / f32(second_iteration_count);
        let cur_sample_point = pos
            + (*p).sky_sun_dir * ray_result.x
            + (*p).sky_sun_dir * ray_result.y * factor;
        let height = max(
            0.0,
            sqrt(dot(cur_sample_point, cur_sample_point)) - (*p).sky_sphere_radius,
        ) / (*p).sky_atmosphere_thickness;
        let densities = density_at_height(p, height) * scale;
        total_densities += densities;
    }
    return total_densities;
}

// The accumulator for `add_light_for_direction` — WGSL has no `inout` scalar
// params, so the function returns the updated `(absorption, light)` pair.
struct AtmoLight {
    absorption: vec3<f32>,
    light: vec3<f32>,
}

// `addLightForDirection` — ray-march the sky along `dir` and accumulate
// in-scattered light + transmittance (HLSL `addLightForDirection`). The HLSL
// `inout float3 absorption, light` become the `AtmoLight` in/out value.
// `includeSun` is unused in both HLSL call sites (defaults to false), so it is
// not a parameter here.
fn add_light_for_direction(
    p: ptr<function, AtmosphereParams>,
    pos: vec3<f32>,
    dir: vec3<f32>,
    max_length: f32,
    acc_in: AtmoLight,
    include_mie: bool,
    main_iteration_count: u32,
    second_iteration_count: u32,
) -> AtmoLight {
    var acc = acc_in;
    let earth_with_atmo_radius = (*p).sky_sphere_radius + (*p).sky_atmosphere_thickness;

    let ray_origin = pos + vec3<f32>(0.0, (*p).sky_sphere_radius, 0.0);
    var ray_result = ray_sphere(
        vec3<f32>(0.0, 0.0, 0.0), earth_with_atmo_radius, ray_origin, dir,
    );
    let ray_result_planet = ray_sphere(
        vec3<f32>(0.0, 0.0, 0.0), (*p).sky_sphere_radius, ray_origin, dir,
    );
    if (ray_result.y > 0.0) {
        if (ray_result_planet.y > 0.0) {
            ray_result.y = ray_result_planet.x - ray_result.x;
        }
        ray_result.y = min(ray_result.y, max_length);

        let scale = ray_result.y / f32(main_iteration_count);
        let angle = max(0.0, dot(dir, (*p).sky_sun_dir));
        var radiance = vec3<f32>(0.0, 0.0, 0.0);
        var total_densities = vec3<f32>(0.0, 0.0, 0.0);

        let rayleigh_mul = rayleigh(angle);
        let mie_mul = phase_function(angle, (*p).sky_mie_factor);

        // HLSL: `for (int i = 1; i <= mainIterationCount; ++i)`.
        for (var i = 1u; i <= main_iteration_count; i = i + 1u) {
            let factor = f32(i) / f32(main_iteration_count);
            let cur_sample_point = ray_origin
                + dir * ray_result.x
                + dir * ray_result.y * factor;
            let height = max(
                0.0,
                sqrt(dot(cur_sample_point, cur_sample_point)) - (*p).sky_sphere_radius,
            ) / (*p).sky_atmosphere_thickness;
            let cur_density = density_at_height(p, height) * scale;
            total_densities += cur_density;
            let densities_scatter = get_scatter_densities_at_point(
                p, cur_sample_point, second_iteration_count,
            );
            let t_scatter = exp(
                -scatter_for_densities(p, densities_scatter) * (*p).sky_absorb_intensity,
            );
            let scatter_radiance = t_scatter * (*p).sky_sun_color;

            let t_primary = exp(
                -scatter_for_densities(p, total_densities) * (*p).sky_absorb_intensity,
            );
            radiance += rayleigh_mul * scatter_radiance * t_primary
                * cur_density.x * (*p).sky_rayleigh_scatter * (*p).sky_scatter_intensity;
            if (include_mie) {
                radiance += mie_mul * scatter_radiance * t_primary
                    * cur_density.x * (*p).sky_mie_scatter * (*p).sky_scatter_intensity;
            }
        }

        acc.light += radiance;
        acc.absorption *= exp(
            -scatter_for_densities(p, total_densities) * (*p).sky_absorb_intensity,
        );
    }
    return acc;
}

// `atmosphere_oct_index` — the octahedral buffer index for `ray_dir`
// (`atmospherePrecomputed.fxh:11-16` — the `rayDir.y` warp + `octEncode`).
// Split out from `apply_atmosphere` because WGSL forbids passing a
// `ptr<storage, ...>` into a function (a hard wgpu/naga rule): the caller (the
// GI / first-hit entry shaders, which OWN the `atmosphere_comp` storage
// binding) fetches `atmosphere_comp[atmosphere_oct_index(...)]` itself and
// passes the resulting `vec4<u32>` slot value into `apply_atmosphere`.
fn atmosphere_oct_index(
    ray_dir_in: vec3<f32>,
    atmosphere_tex_size_x: u32,
    atmosphere_tex_size_y: u32,
) -> u32 {
    var ray_dir = ray_dir_in;
    // The `rayDir.y` warp (`atmospherePrecomputed.fxh:11-12`).
    ray_dir.y = pow(abs(ray_dir.y), 0.5) * sign(ray_dir.y);
    let xz_scale = sqrt(
        (1.0 - ray_dir.y * ray_dir.y)
        / (ray_dir.x * ray_dir.x + ray_dir.z * ray_dir.z),
    );
    ray_dir.x *= xz_scale;
    ray_dir.z *= xz_scale;

    let oct = oct_encode(ray_dir);
    // HLSL `uint2(oct.x * (texSizeX-1), ...)` implicitly truncates float→uint
    // in the `uint2(...)` constructor; WGSL requires explicit `u32()` casts
    // (naga rejects building a `vec2<u32>` from `f32` components).
    let comp_pos = vec2<u32>(
        u32(oct.x * f32(atmosphere_tex_size_x - 1u)),
        u32(oct.y * f32(atmosphere_tex_size_y - 1u)),
    );
    return comp_pos.x + comp_pos.y * atmosphere_tex_size_x;
}

// `applyAtmosphere` — fold the precomputed octahedral atmosphere slot's light +
// transmittance into `acc` (HLSL `applyAtmosphere`,
// `atmospherePrecomputed.fxh:9-22`). The HLSL reads the
// `StructuredBuffer<uint3> atmosphereComp` global; WGSL cannot pass that buffer
// by `ptr`, so the caller fetches the slot (via `atmosphere_oct_index`) and
// passes its value `atmo_comp` in. The buffer is stored as `array<vec4<u32>>`
// (`.w` padding) to match WGSL's 16-byte storage stride (`09-design-b.md`
// §3.3), so `atmo_comp` is a `vec4<u32>` and only `.xyz` is meaningful.
fn apply_atmosphere(
    atmo_comp: vec4<u32>,
    acc_in: AtmoLight,
    atmo_mul: f32,
) -> AtmoLight {
    var acc = acc_in;
    let atmo_light = vec3<f32>(
        unpack2x16float(atmo_comp.x).x,
        unpack2x16float(atmo_comp.x).y,
        unpack2x16float(atmo_comp.y).x,
    );
    let atmo_absorption = vec3<f32>(
        unpack2x16float(atmo_comp.y).y,
        unpack2x16float(atmo_comp.z).x,
        unpack2x16float(atmo_comp.z).y,
    );

    acc.light += acc.absorption * atmo_light * atmo_mul;
    acc.absorption *= atmo_absorption;
    return acc;
}
