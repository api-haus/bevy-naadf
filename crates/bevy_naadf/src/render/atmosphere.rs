//! Phase B atmosphere subsystem — the precomputed multiple-scattering sky
//! model (`09-design-b.md` §2.1, §3.9, §9.2).
//!
//! NAADF's `WorldRenderBase` precomputes an octahedral atmosphere buffer once
//! per frame (a quarter at a time, amortised over 4 frames —
//! `renderAtmosphere.fx:12`) and samples it from the first-hit + GI passes.
//! Phase A/A-2 used only the inline sun+ambient term in `naadf_first_hit.wgsl`;
//! Phase B ports the full model.
//!
//! This module owns:
//! - [`ATMOSPHERE`] — the sky-model constants (the `UiSkyDebug.cs` field
//!   defaults; no GUI in the port — `09-design-b.md` §1 / §3.9). The single
//!   source of truth shared by the GPU uniform and the CPU sun-colour function.
//! - [`Atmosphere::get_light_for_point`] — the CPU port of `Atmosphere.cs`'s
//!   `GetLightForPoint`, used (from Batch 3's `prepare_gi`) to compute the GI
//!   `sun_color` (`WorldRender.cs:96`).
//! - [`AtmosphereGpu`] — the render-world resource: the `atmosphere_comp`
//!   buffer + the `GpuAtmosphereParams` uniform + the precompute bind group.
//! - [`prepare_atmosphere`] — the `PrepareResources` system that creates
//!   `AtmosphereGpu` once and uploads the per-frame uniform.
//!
//! The render-graph node ([`crate::render::graph_b::naadf_atmosphere_node`])
//! dispatches `naadf_atmosphere.wgsl`'s `precompute_atmosphere` over
//! `AtmosphereGpu`.

use bevy::prelude::*;
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, Buffer, BufferDescriptor, BufferUsages,
    CommandEncoderDescriptor, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use crate::render::extract::{ExtractedCameraData, ExtractedCameraHistory};
use crate::render::gpu_types::GpuAtmosphereParams;
use crate::render::pipelines::NaadfPipelines;

/// The octahedral atmosphere-buffer edge length (C# `atmosphereTexSizeX/Y` —
/// `WorldRenderBase.cs:131-132`; a compile-time constant in the port).
pub const ATMOSPHERE_TEX_SIZE: u32 = 1024;

/// `naadf_atmosphere.wgsl`'s `[numthreads(64,1,1)]` (`renderAtmosphere.fx:9`).
pub const ATMOSPHERE_WORKGROUP_SIZE: u32 = 64;

/// The sky-model constants (the `UiSkyDebug.cs` field initialisers — there is
/// no sky-debug GUI in the port). Stored in their *raw* `UiSkyDebug` form; the
/// `UiSkyDebug.SetShaderData` scaling (`UiSkyDebug.cs:63-79`) is applied at the
/// use sites so the GPU uniform and the CPU [`Atmosphere::get_light_for_point`]
/// share exactly this source of truth.
pub struct AtmosphereConstants {
    /// `UiSkyDebug.skySunIntensity` (= `10.0`).
    pub sun_intensity: f32,
    /// `UiSkyDebug.skyRayleighScatter` (= `(5.802, 13.558, 33.1)`).
    pub rayleigh_scatter: Vec3,
    /// `UiSkyDebug.skyMieScatter` (= `2.5`).
    pub mie_scatter: f32,
    /// `UiSkyDebug.skyOzoneAbsorb` (= `(0.650, 1.881, 0.085)`).
    pub ozone_absorb: Vec3,
    /// `UiSkyDebug.skySunColor` (= `(1, 1, 1)`).
    pub sun_color: Vec3,
    /// `UiSkyDebug.skySphereRadius` (= `50000.0 * 100`).
    pub sphere_radius: f32,
    /// `UiSkyDebug.skyAtmosphereThickness` (= `50000.0`).
    pub atmosphere_thickness: f32,
    /// `UiSkyDebug.skyAtmosphereDensity` (= `14.0`) — *raw*; the `* 0.01`
    /// `SetShaderData` scaling is applied at the use sites.
    pub atmosphere_density: f32,
    /// `UiSkyDebug.skyAbsorbIntensity` (= `3.0`).
    pub absorb_intensity: f32,
    /// `UiSkyDebug.skyScatterIntensity` (= `1.35`) — *raw*; the `* 0.000001`
    /// `SetShaderData` scaling is applied at the use sites.
    pub scatter_intensity: f32,
    /// `UiSkyDebug.skyMieFactor` (= `0.85`).
    pub mie_factor: f32,
    /// `UiSkyDebug.skyMainRaySteps` (= `24`).
    pub main_ray_steps: u32,
    /// `UiSkyDebug.skySubScatterSteps` (= `6`).
    pub sub_scatter_steps: u32,
}

/// The sky-model constants (the `UiSkyDebug.cs` defaults).
pub const ATMOSPHERE: AtmosphereConstants = AtmosphereConstants {
    sun_intensity: 10.0,
    rayleigh_scatter: Vec3::new(5.802, 13.558, 33.1),
    mie_scatter: 2.5,
    ozone_absorb: Vec3::new(0.650, 1.881, 0.085),
    sun_color: Vec3::new(1.0, 1.0, 1.0),
    sphere_radius: 50000.0 * 100.0,
    atmosphere_thickness: 50000.0,
    atmosphere_density: 14.0,
    absorb_intensity: 3.0,
    scatter_intensity: 1.35,
    mie_factor: 0.85,
    main_ray_steps: 24,
    sub_scatter_steps: 6,
};

/// Build the per-frame [`GpuAtmosphereParams`] from [`ATMOSPHERE`] + the
/// per-frame `cam_pos` / `sky_sun_dir` / `frame_count`.
///
/// Applies the `UiSkyDebug.SetShaderData` scaling (`UiSkyDebug.cs:63-79`):
/// `skySunColor * skySunIntensity`, `skyAtmosphereDensity * 0.01`,
/// `skyScatterIntensity * 0.000001`.
pub fn build_atmosphere_params(
    cam_pos: Vec3,
    sky_sun_dir: Vec3,
    frame_count: u32,
) -> GpuAtmosphereParams {
    GpuAtmosphereParams {
        cam_pos,
        _pad0: 0,
        sky_sun_dir,
        _pad1: 0,
        sky_rayleigh_scatter: ATMOSPHERE.rayleigh_scatter,
        _pad2: 0,
        sky_ozone_absorb: ATMOSPHERE.ozone_absorb,
        _pad3: 0,
        sky_sun_color: ATMOSPHERE.sun_color * ATMOSPHERE.sun_intensity,
        _pad4: 0,
        sky_mie_scatter: ATMOSPHERE.mie_scatter,
        sky_sphere_radius: ATMOSPHERE.sphere_radius,
        sky_atmosphere_thickness: ATMOSPHERE.atmosphere_thickness,
        sky_atmosphere_density: ATMOSPHERE.atmosphere_density * 0.01,
        sky_absorb_intensity: ATMOSPHERE.absorb_intensity,
        sky_scatter_intensity: ATMOSPHERE.scatter_intensity * 0.000001,
        sky_mie_factor: ATMOSPHERE.mie_factor,
        sky_main_ray_steps: ATMOSPHERE.main_ray_steps,
        sky_sub_scatter_steps: ATMOSPHERE.sub_scatter_steps,
        atmosphere_tex_size_x: ATMOSPHERE_TEX_SIZE,
        atmosphere_tex_size_y: ATMOSPHERE_TEX_SIZE,
        frame_count,
    }
}

/// The CPU atmosphere model — a faithful port of `Atmosphere.cs`
/// (`09-design-b.md` §9.2). Only [`Atmosphere::get_light_for_point`] is needed:
/// `WorldRenderBase` passes its result as the GI `sunColor`
/// (`WorldRender.cs:96` — `Atmosphere.GetLightForPoint((0,10,0))`).
pub struct Atmosphere;

impl Atmosphere {
    /// `Atmosphere.DensityAtHeight` (`Atmosphere.cs:14-21`). The `* 0.01`
    /// matches the C# `* UiSkyDebug.skyAtmosphereDensity * 0.01f`.
    fn density_at_height(height: f32) -> Vec3 {
        let mut density = Vec3::ZERO;
        density.x = (-height / 0.3).exp() * (1.0 - height); // Rayleigh
        density.y = (-height / 0.2).exp() * (1.0 - height); // Mie
        density.z = (1.0 - (height - 0.25).abs() / 0.15).max(0.0); // Ozone
        density * ATMOSPHERE.atmosphere_density * 0.01
    }

    /// `Atmosphere.RaySphere` (`Atmosphere.cs:23-41`).
    fn ray_sphere(
        sphere_origin: Vec3,
        sphere_radius: f32,
        ray_origin: Vec3,
        ray_dir: Vec3,
    ) -> Vec2 {
        let dif = ray_origin - sphere_origin;
        let a = 1.0;
        let b = 2.0 * dif.dot(ray_dir);
        let c = dif.dot(dif) - sphere_radius * sphere_radius;
        let d = b * b - 4.0 * a * c;
        if d > 0.0 {
            let s = d.sqrt();
            let dst_near = ((-b - s) / (2.0 * a)).max(0.0);
            let dst_far = (-b + s) / (2.0 * a);
            if dst_far >= 0.0 {
                return Vec2::new(dst_near, dst_far - dst_near);
            }
        }
        Vec2::ZERO
    }

    /// `Atmosphere.ScatterForDensities` (`Atmosphere.cs:43-46`). The
    /// `* 0.000001` matches the C# `* UiSkyDebug.skyScatterIntensity * 0.000001f`.
    fn scatter_for_densities(densities: Vec3) -> Vec3 {
        (densities.x * ATMOSPHERE.rayleigh_scatter
            + Vec3::splat(densities.y * ATMOSPHERE.mie_scatter)
            + densities.z * ATMOSPHERE.ozone_absorb)
            * ATMOSPHERE.scatter_intensity
            * 0.000001
    }

    /// `Atmosphere.getScatterDensitiesAtPoint` (`Atmosphere.cs:48-69`). The
    /// CPU version uses a fixed `scatterSteps = 20` (note: *not*
    /// `skySubScatterSteps`).
    fn get_scatter_densities_at_point(pos: Vec3, sky_sun_dir: Vec3) -> Vec3 {
        let earth_with_atmo_radius =
            ATMOSPHERE.sphere_radius + ATMOSPHERE.atmosphere_thickness;
        let ray_result =
            Self::ray_sphere(Vec3::ZERO, earth_with_atmo_radius, pos, sky_sun_dir);
        if ray_result.y == 0.0 {
            return Vec3::ZERO;
        }
        const SCATTER_STEPS: i32 = 20;
        let scale = ray_result.y / SCATTER_STEPS as f32;
        let mut total_densities = Vec3::ZERO;
        for i in (0..SCATTER_STEPS).rev() {
            let factor = i as f32 / SCATTER_STEPS as f32;
            let cur_sample_point = pos
                + sky_sun_dir * ray_result.x
                + sky_sun_dir * ray_result.y * factor;
            let height = (cur_sample_point.dot(cur_sample_point).sqrt()
                - ATMOSPHERE.sphere_radius)
                .max(0.0)
                / ATMOSPHERE.atmosphere_thickness;
            total_densities += Self::density_at_height(height) * scale;
        }
        total_densities
    }

    /// `Atmosphere.GetLightForPoint` (`Atmosphere.cs:71-77`) — the GI sun
    /// colour. `pos` is the C# `(0, 10, 0)` sample point; `sky_sun_dir` is the
    /// per-frame sun direction.
    pub fn get_light_for_point(pos: Vec3, sky_sun_dir: Vec3) -> Vec3 {
        let densities_scatter = Self::get_scatter_densities_at_point(
            pos + Vec3::new(0.0, ATMOSPHERE.sphere_radius, 0.0),
            sky_sun_dir,
        );
        let scatter_for_exp =
            -Self::scatter_for_densities(densities_scatter) * ATMOSPHERE.absorb_intensity;
        let t = Vec3::new(
            scatter_for_exp.x.exp(),
            scatter_for_exp.y.exp(),
            scatter_for_exp.z.exp(),
        );
        t * ATMOSPHERE.sun_color * ATMOSPHERE.sun_intensity
    }
}

/// The render-world GPU resource owning the atmosphere buffer + uniform
/// (`09-design-b.md` §2.1, §10.3). Created once by [`prepare_atmosphere`].
#[derive(Resource)]
pub struct AtmosphereGpu {
    /// The octahedral precomputed atmosphere buffer — `ATMOSPHERE_TEX_SIZE²`
    /// `vec4<u32>` slots (the C# `Uint3` `atmosphereComp` stored as a 16-byte-
    /// stride `vec4<u32>` — `09-design-b.md` §3.3). Fixed-size (octahedral,
    /// resolution-independent), `STORAGE | COPY_DST`, zero-cleared on creation.
    pub atmosphere_comp: Buffer,
    /// The `GpuAtmosphereParams` uniform — rewritten every frame.
    pub atmosphere_params: Buffer,
    /// `@group(0)` for the atmosphere precompute pass: `atmosphere_params`
    /// (uniform), `atmosphere_comp` (read-write storage).
    pub bind_group: BindGroup,
}

/// `RenderSystems::PrepareResources` system: create [`AtmosphereGpu`] once and
/// upload the per-frame [`GpuAtmosphereParams`] (`09-design-b.md` §10.3).
///
/// Runs alongside `prepare_world_gpu` / `prepare_taa` in `PrepareResources`.
/// The atmosphere buffer is fixed-size — created once, never resized (it is
/// octahedral, so viewport changes do not affect it). Skips silently until the
/// camera has been extracted (it needs `cam_pos`).
///
/// Batch 1 note: the `sky_sun_dir` here is the same hand-tuned fixed direction
/// `prepare_frame_gpu` uses for Phase A's flat sun (`prepare.rs:281-288`) —
/// Batch 3's `prepare_gi` will share one extracted `sky_sun_dir` across the
/// atmosphere + GI uniforms. For Batch 1 the atmosphere precomputes against
/// this fixed sun; the rest of the pipeline is the unchanged A-2 path.
// Bevy systems legitimately exceed clippy's 7-argument ceiling.
#[allow(clippy::too_many_arguments)]
pub fn prepare_atmosphere(
    mut commands: Commands,
    extracted_camera: Res<ExtractedCameraData>,
    extracted_history: Res<ExtractedCameraHistory>,
    existing: Option<Res<AtmosphereGpu>>,
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    if !extracted_camera.valid || !extracted_history.valid {
        return;
    }

    // --- create the fixed-size resources once -------------------------------
    let (atmosphere_comp, atmosphere_params, bind_group) = match &existing {
        Some(gpu) => (
            gpu.atmosphere_comp.clone(),
            gpu.atmosphere_params.clone(),
            gpu.bind_group.clone(),
        ),
        None => {
            // `atmosphere_comp`: ATMOSPHERE_TEX_SIZE² × vec4<u32> (16 bytes
            // each) = 1024·1024·16 = 64 MiB (`09-design-b.md` §3.3).
            let atmosphere_comp = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_atmosphere_comp"),
                size: (ATMOSPHERE_TEX_SIZE as u64) * (ATMOSPHERE_TEX_SIZE as u64) * 16,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let atmosphere_params = render_device.create_buffer(&BufferDescriptor {
                label: Some("naadf_atmosphere_params"),
                size: std::mem::size_of::<GpuAtmosphereParams>() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // Zero-clear the comp buffer so the first ~4 frames — before the
            // octahedral buffer is fully precomputed — sample zeroed (black
            // sky, identity absorption) rather than garbage.
            let mut encoder =
                render_device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("naadf_clear_atmosphere_comp"),
                });
            encoder.clear_buffer(&atmosphere_comp, 0, None);
            render_queue.submit([encoder.finish()]);

            let bind_group = render_device.create_bind_group(
                "naadf_atmosphere_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.atmosphere_layout),
                &BindGroupEntries::sequential((
                    atmosphere_params.as_entire_buffer_binding(),
                    atmosphere_comp.as_entire_buffer_binding(),
                )),
            );
            (atmosphere_comp, atmosphere_params, bind_group)
        }
    };

    // --- upload the per-frame uniform ---------------------------------------
    // A simple fixed sun for the demo — the same hand-tuned direction
    // `prepare_frame_gpu` uses (`prepare.rs:281-288`). Batch 3 unifies this.
    let sun_elev = 0.9_f32;
    let sun_azim = 0.6_f32;
    let sky_sun_dir = Vec3::new(
        sun_elev.cos() * sun_azim.cos(),
        sun_elev.sin(),
        sun_elev.cos() * sun_azim.sin(),
    )
    .normalize();

    let params = build_atmosphere_params(
        extracted_camera.position_split.to_world(),
        sky_sun_dir,
        extracted_history.frame_count,
    );
    render_queue.write_buffer(&atmosphere_params, 0, bytemuck::bytes_of(&params));

    commands.insert_resource(AtmosphereGpu {
        atmosphere_comp,
        atmosphere_params,
        bind_group,
    });
}
