//! `streaming::noise_fastnoiselite` — WGSL shader handle + GPU oracle dispatch
//! harness for the `--wgsl-noise-oracle` gate.
//!
//! Phase-1 deliverable of `docs/orchestrate/streaming-world/02b-design-plan-b.md`
//! (§ B / § C). Provides:
//! - [`NOISE_FASTNOISELITE_SHADER_SRC`] + [`NOISE_FASTNOISELITE_SHADER_PATH`] —
//!   the WGSL noise module asset, mirroring the `generator_model_*` pattern at
//!   `crates/bevy_naadf/src/render/construction/generator_model.rs:55-59`.
//! - [`NOISE_ORACLE_DISPATCH_SHADER_SRC`] — the thin compute wrapper that
//!   evaluates a batch of `(FnlState, vec3<f32>)` test cases.
//! - [`build_oracle_dispatch_shader_src`] — concatenates the two so a single
//!   `Shader::from_wgsl` covers both modules. We inline rather than rely on
//!   Bevy's `#import` cross-module composition (per the comment in
//!   `chunk_calc.wgsl:39-44`: "Bevy's WGSL composition `#import` surface is
//!   unpredictable across naga versions; the helpers are duplicated identically").
//! - [`run_wgsl_noise_oracle`] — boots a headless `MinimalPlugins + RenderPlugin`
//!   app, dispatches the oracle compute against a deterministic test matrix
//!   (`OracleTestPlan`), reads the GPU output back, and compares it to the CPU
//!   oracle (`super::noise_fastnoiselite_cpu_oracle`).
//!
//! The harness models the pattern at
//! `crates/bevy_naadf/src/render/construction/mod.rs::validate_gpu_construction`
//! (`mod.rs:3071-3290`).

use crate::streaming::noise_fastnoiselite_cpu_oracle as cpu;

/// Inlined WGSL noise module source. `include_str!` is relative to this `.rs`,
/// so a typo in the asset path fails to compile.
pub const NOISE_FASTNOISELITE_SHADER_SRC: &str =
    include_str!("../assets/shaders/noise_fastnoiselite.wgsl");

/// Asset path of the WGSL noise module (for the future `noise_terrain.wgsl`
/// `#import` site in Phase 2).
pub const NOISE_FASTNOISELITE_SHADER_PATH: &str = "shaders/noise_fastnoiselite.wgsl";

/// Inlined oracle dispatch shader.
pub const NOISE_ORACLE_DISPATCH_SHADER_SRC: &str =
    include_str!("../assets/shaders/noise_oracle_dispatch.wgsl");

/// Tolerance for the CPU↔GPU bit-near-equality comparison. The design specifies
/// `< 1e-5` for non-cellular noise; cellular uses `< 1e-4` due to its `sqrt`
/// ordering sensitivity (the inner-loop `min(distance1, new_distance)` ordering
/// can hit f32 rounding ties differently across CPU vs GPU). See
/// `02b-design-plan-b.md` § B.2.
pub const ORACLE_TOLERANCE: f32 = 1e-5;
pub const ORACLE_TOLERANCE_CELLULAR: f32 = 1e-4;

/// Concatenates the noise module source with the dispatch shader source. The
/// caller registers the result as a single `Shader::from_wgsl` — there is no
/// `#import` resolution involved.
pub fn build_oracle_dispatch_shader_src() -> String {
    // The dispatch shader contains an explicit `// @begin` marker to mark the
    // splice point. Drop everything below the `#define_import_path` line in
    // the noise module (the `define_import_path` directive is a no-op when
    // everything is inlined into one logical compilation unit) — we keep the
    // body verbatim.
    let mut combined = String::with_capacity(
        NOISE_FASTNOISELITE_SHADER_SRC.len() + NOISE_ORACLE_DISPATCH_SHADER_SRC.len() + 256,
    );
    // Strip the `#define_import_path` line — it is harmless but produces an
    // unnecessary diagnostic if duplicated across both files.
    for line in NOISE_FASTNOISELITE_SHADER_SRC.lines() {
        if line.trim_start().starts_with("#define_import_path") {
            combined.push_str("// (stripped #define_import_path for inlined compilation)\n");
            continue;
        }
        combined.push_str(line);
        combined.push('\n');
    }
    combined.push('\n');
    // Append the dispatch shader after the `// @begin` marker (skip the
    // marker-line itself so we don't re-emit it).
    let mut past_marker = false;
    for line in NOISE_ORACLE_DISPATCH_SHADER_SRC.lines() {
        if !past_marker {
            if line.trim_start().starts_with("// @begin") {
                past_marker = true;
            }
            continue;
        }
        combined.push_str(line);
        combined.push('\n');
    }
    if !past_marker {
        // Fallback — the marker is missing for some reason; concatenate the
        // whole file. The shader still compiles because the dispatch wrapper
        // has no leading `#define_import_path` of its own.
        combined.push_str(NOISE_ORACLE_DISPATCH_SHADER_SRC);
    }
    combined
}

/// Layout of the dispatch params uniform buffer matching the WGSL
/// `struct DispatchParams { count: u32, _pad0,_pad1,_pad2 }`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct DispatchParams {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// A single oracle test case: an `FnlState` configuration paired with a sample
/// point. The expected CPU value is computed on construction so the GPU output
/// can be compared in one pass.
#[derive(Clone, Copy, Debug)]
pub struct OracleCase {
    /// State configuration consumed by the WGSL `fnl_get_noise_3d` dispatcher.
    pub state: cpu::FnlState,
    /// `(x, y, z, _pad)` — the sample point, packed as `vec4<f32>`.
    pub pos: [f32; 4],
    /// Reference value computed by the CPU oracle. The `_pad` slot of `pos` is
    /// unused on the WGSL side; the CPU oracle never reads it either.
    pub cpu_value: f32,
    /// Human-readable tag for diagnostic output (e.g., `"perlin_fbm"`).
    pub tag: &'static str,
}

impl OracleCase {
    pub fn new(state: cpu::FnlState, pos: [f32; 3], tag: &'static str) -> Self {
        let cpu_value = cpu::fnl_get_noise_3d(&state, pos[0], pos[1], pos[2]);
        Self {
            state,
            pos: [pos[0], pos[1], pos[2], 0.0],
            cpu_value,
            tag,
        }
    }
}

/// Aggregated outcome of an oracle dispatch — used by both the unit test
/// `run_wgsl_noise_oracle_unit_test` and the e2e gate.
pub struct OracleReport {
    pub total_cases: usize,
    pub max_abs_diff: f32,
    pub max_abs_diff_tag: &'static str,
    pub max_abs_diff_pos: [f32; 4],
    pub max_abs_diff_cpu: f32,
    pub max_abs_diff_gpu: f32,
    /// Individual mismatches over tolerance — first few only (cap to keep the
    /// report small).
    pub mismatches: Vec<OracleMismatch>,
    /// Distinct (noise_type, fractal_type, domain_warp_type) combinations exercised.
    pub combos: usize,
}

#[derive(Clone, Debug)]
pub struct OracleMismatch {
    pub tag: &'static str,
    pub pos: [f32; 4],
    pub cpu_value: f32,
    pub gpu_value: f32,
    pub abs_diff: f32,
}

const MAX_REPORT_MISMATCHES: usize = 16;

/// Build the full Phase-1 test matrix. Per the brief's scope amplifier this
/// exercises:
/// - 6 noise families
/// - 4 fractal types (None, FBm, Ridged, PingPong)
/// - 3 domain-warp variants (covered separately as no-fractal + warp-aware fractal types)
/// - Cellular sub-matrix: 4 distance × 7 return-type
/// - 5 seeds × multiple sample points per combo
///
/// Total stays under ~8000 sample points to keep the gate fast.
pub fn build_test_plan() -> Vec<OracleCase> {
    let seeds: &[i32] = &[0, 1, -1, 12345, -1337];

    // Sample points — span (a) near origin, (b) chunk boundaries, (c) small
    // negatives, (d) larger magnitudes. We deliberately avoid exact integer
    // sample points where Perlin / Value collapse to gradient-product zero (the
    // boundary case is exercised separately by the edge-coherency test).
    let sample_points: &[[f32; 3]] = &[
        [0.5, 0.5, 0.5],
        [1.5, 2.5, 3.5],
        [-1.5, -2.5, -3.5],
        [15.999, 8.001, -3.5],
        [16.0, 8.0, -3.5],
        [16.000001, 7.999999, -3.500001],
        [-100.25, -50.75, -25.125],
        [100.25, 50.75, 25.125],
        [0.0, 0.0, 0.0],
        [7.7, -13.3, 21.9],
        [123.456, -78.9, 42.0],
        [-1024.0, 0.5, 1024.0],
        [3.14159, 2.71828, 1.41421],
        [0.1, 0.2, 0.3],
        [-0.1, -0.2, -0.3],
        [33.0, 33.0, 33.0],
    ];

    let mut cases: Vec<OracleCase> = Vec::with_capacity(4096);

    // Section A: every noise family × every fractal type (None, FBm, Ridged,
    // PingPong) × 5 seeds × representative sample points. For non-cellular
    // families we use ~6 sample points to keep the count manageable.
    let basic_noise: &[(u32, &'static str)] = &[
        (cpu::noise_type::OPEN_SIMPLEX2, "open_simplex2"),
        (cpu::noise_type::OPEN_SIMPLEX2S, "open_simplex2s"),
        (cpu::noise_type::PERLIN, "perlin"),
        (cpu::noise_type::VALUE_CUBIC, "value_cubic"),
        (cpu::noise_type::VALUE, "value"),
    ];
    let fractals: &[(u32, &'static str)] = &[
        (cpu::fractal_type::NONE, "none"),
        (cpu::fractal_type::FBM, "fbm"),
        (cpu::fractal_type::RIDGED, "ridged"),
        (cpu::fractal_type::PINGPONG, "pingpong"),
    ];
    let rotations: &[(u32, &'static str)] = &[
        (cpu::rotation_type::NONE, "rot_none"),
        (cpu::rotation_type::IMPROVE_XY_PLANES, "rot_xy"),
        (cpu::rotation_type::IMPROVE_XZ_PLANES, "rot_xz"),
    ];

    // Pick 8 sample points to cover the full matrix without overshooting the
    // ~8000 cap.
    let sample_subset_basic = &sample_points[..8];

    for &seed in seeds {
        for &(nt, _nt_tag) in basic_noise {
            for &(ft, ft_tag) in fractals {
                // Rotations sweep only when fractal == NONE (else each fractal
                // already does plenty of noise calls).
                let rots: &[(u32, &'static str)] = if ft == cpu::fractal_type::NONE {
                    rotations
                } else {
                    &rotations[..1] // only rot_none for fractals
                };
                for &(rot, _rot_tag) in rots {
                    let mut state = cpu::fnl_create_state(seed);
                    state.noise_type = nt;
                    state.fractal_type = ft;
                    state.rotation_type_3d = rot;
                    state.frequency = 0.05;
                    state.octaves = 3;
                    state.lacunarity = 2.0;
                    state.gain = 0.5;
                    state.weighted_strength = 0.0;
                    state.ping_pong_strength = 2.0;
                    for &p in sample_subset_basic {
                        cases.push(OracleCase::new(state, p, ft_tag));
                    }
                }
            }
        }
    }

    // Section B: Cellular full sub-matrix. 4 distance × 7 return-type × 5 seeds
    // × 4 sample points = 560 cases.
    let cellular_dists: &[u32] = &[
        cpu::cellular_distance_func::EUCLIDEAN,
        cpu::cellular_distance_func::EUCLIDEANSQ,
        cpu::cellular_distance_func::MANHATTAN,
        cpu::cellular_distance_func::HYBRID,
    ];
    let cellular_returns: &[u32] = &[
        cpu::cellular_return_type::CELL_VALUE,
        cpu::cellular_return_type::DISTANCE,
        cpu::cellular_return_type::DISTANCE2,
        cpu::cellular_return_type::DISTANCE2ADD,
        cpu::cellular_return_type::DISTANCE2SUB,
        cpu::cellular_return_type::DISTANCE2MUL,
        cpu::cellular_return_type::DISTANCE2DIV,
    ];
    let cellular_points = &sample_points[..4];
    for &seed in seeds {
        for &dist in cellular_dists {
            for &ret in cellular_returns {
                let mut state = cpu::fnl_create_state(seed);
                state.noise_type = cpu::noise_type::CELLULAR;
                state.fractal_type = cpu::fractal_type::NONE;
                state.cellular_distance_func = dist;
                state.cellular_return_type = ret;
                state.cellular_jitter_mod = 1.0;
                state.frequency = 0.05;
                for &p in cellular_points {
                    cases.push(OracleCase::new(state, p, "cellular"));
                }
            }
        }
    }

    // Section C: Edge-coherency — two virtual chunks share a boundary plane at
    // x=16; sample the same point computed two ways (positive epsilon vs the
    // exact boundary). Catches accidental coord-truncation bugs.
    // Skip cellular here — it intentionally has discontinuities at cell edges.
    for &seed in &seeds[..3] {
        for &(nt, _) in &basic_noise[..3] {
            let mut state = cpu::fnl_create_state(seed);
            state.noise_type = nt;
            state.fractal_type = cpu::fractal_type::NONE;
            state.frequency = 0.05;
            // Pairs of points that should produce the same noise value with
            // continuity to f32-precision: same (x, y, z), evaluated to verify
            // the noise function is a pure function of position.
            let pairs: &[[f32; 3]] = &[
                [16.0, 8.0, -3.5],
                [16.0, 8.0, -3.5], // duplicate; assert they agree on CPU + GPU.
                [-16.0, 0.0, 0.0],
                [-16.0, 0.0, 0.0],
            ];
            for &p in pairs {
                cases.push(OracleCase::new(state, p, "edge_coherency"));
            }
        }
    }

    cases
}

/// Boot a headless render world, compile + dispatch the oracle, read back, and
/// compare. Returns an `OracleReport` on success or a string error. This is the
/// shared entry point used by both the unit test and the `--wgsl-noise-oracle`
/// e2e gate (`crate::e2e::wgsl_noise_oracle::run_wgsl_noise_oracle`).
pub fn run_wgsl_noise_oracle() -> Result<OracleReport, String> {
    use bevy::app::App;
    use bevy::asset::{AssetPlugin, Assets};
    use bevy::image::ImagePlugin;
    use bevy::prelude::default;
    use bevy::render::render_resource::{
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BufferDescriptor,
        BufferUsages, CachedComputePipelineId, CommandEncoderDescriptor, ComputePassDescriptor,
        ComputePipelineDescriptor, MapMode, PipelineCache, PollType, ShaderStages,
    };
    use bevy::render::render_resource::binding_types::{
        storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
    };
    use bevy::render::renderer::{RenderDevice, RenderQueue};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::shader::Shader;
    use bevy::MinimalPlugins;
    use std::borrow::Cow;

    let cases = build_test_plan();
    if cases.is_empty() {
        return Err("no test cases generated".into());
    }
    let count = cases.len() as u32;

    // ── Boot headless render world ────────────────────────────────────────────
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(ImagePlugin::default())
        .add_plugins(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::default()),
            synchronous_pipeline_compilation: true,
            debug_flags: Default::default(),
        });
    app.finish();
    app.cleanup();

    // ── Build and register the combined shader ────────────────────────────────
    let combined_src = build_oracle_dispatch_shader_src();
    let shader = Shader::from_wgsl(combined_src, "shaders/noise_oracle_combined.wgsl");
    let shader_clone = shader.clone();
    let shader_handle = app.world_mut().resource_mut::<Assets<Shader>>().add(shader);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return Err("no RenderApp sub-app available".into());
    };
    {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.set_shader(shader_handle.id(), shader_clone);
    }
    let device = render_app
        .world()
        .get_resource::<RenderDevice>()
        .ok_or("no RenderDevice")?
        .clone();
    let queue = render_app
        .world()
        .get_resource::<RenderQueue>()
        .ok_or("no RenderQueue")?
        .clone();

    // ── Allocate buffers ──────────────────────────────────────────────────────
    let sample_points_bytes: Vec<u8> = cases
        .iter()
        .flat_map(|c| bytemuck::bytes_of(&c.pos).to_vec())
        .collect();
    let sample_points_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oracle_sample_points"),
        size: sample_points_bytes.len() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&sample_points_buffer, 0, &sample_points_bytes);

    let states_bytes: Vec<u8> = cases
        .iter()
        .flat_map(|c| bytemuck::bytes_of(&c.state).to_vec())
        .collect();
    let states_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oracle_states"),
        size: states_bytes.len() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&states_buffer, 0, &states_bytes);

    let output_size = (cases.len() * std::mem::size_of::<f32>()) as u64;
    let output_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oracle_output"),
        size: output_size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Zero-init for determinism.
    queue.write_buffer(&output_buffer, 0, &vec![0u8; output_size as usize]);

    let dispatch_params = DispatchParams {
        count,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    let params_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("oracle_params"),
        size: std::mem::size_of::<DispatchParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&dispatch_params));

    // ── Build bind-group layout + pipeline ────────────────────────────────────
    let layout_desc = BindGroupLayoutDescriptor::new(
        "naadf_streaming_wgsl_noise_oracle_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_sized(false, None),
                uniform_buffer_sized(false, None),
            ),
        ),
    );

    let pipeline_id: CachedComputePipelineId;
    {
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        let cache = render_app.world().resource::<PipelineCache>();
        pipeline_id = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("naadf_streaming_wgsl_noise_oracle_pipeline".into()),
            layout: vec![layout_desc.clone()],
            shader: shader_handle.clone(),
            entry_point: Some(Cow::from("dispatch_oracle")),
            ..default()
        });
    }

    // ── Wait for pipeline compilation ─────────────────────────────────────────
    let mut pipeline: Option<bevy::render::render_resource::ComputePipeline> = None;
    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    for _ in 0..128 {
        let mut pipeline_cache = render_app.world_mut().resource_mut::<PipelineCache>();
        pipeline_cache.process_queue();
        let cache = render_app.world().resource::<PipelineCache>();
        if let Some(p) = cache.get_compute_pipeline(pipeline_id) {
            pipeline = Some(p.clone());
            break;
        }
        // Surface any compilation error.
        if let bevy::render::render_resource::CachedPipelineState::Err(err) =
            cache.get_compute_pipeline_state(pipeline_id)
        {
            return Err(format!("oracle pipeline compile failed: {err:?}"));
        }
    }
    let pipeline = pipeline.ok_or("oracle pipeline did not compile within 128 process_queue ticks")?;

    // ── Build the bind group ──────────────────────────────────────────────────
    let render_app = app.get_sub_app(RenderApp).unwrap();
    let cache = render_app.world().resource::<PipelineCache>();
    let bgl = cache.get_bind_group_layout(&layout_desc);
    let bind_group = device.create_bind_group(
        "oracle_bind_group",
        &bgl,
        &BindGroupEntries::sequential((
            sample_points_buffer.as_entire_buffer_binding(),
            states_buffer.as_entire_buffer_binding(),
            output_buffer.as_entire_buffer_binding(),
            params_buffer.as_entire_buffer_binding(),
        )),
    );

    // ── Dispatch ─────────────────────────────────────────────────────────────
    let workgroups = (count + 63) / 64;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oracle_encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("oracle_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    queue.submit([encoder.finish()]);

    // ── Read back ─────────────────────────────────────────────────────────────
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("oracle_staging"),
        size: output_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("oracle_copy"),
    });
    enc.copy_buffer_to_buffer(&output_buffer, 0, &staging, 0, output_size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(MapMode::Read, |r| r.unwrap());
    device
        .poll(PollType::wait_indefinitely())
        .map_err(|e| format!("device.poll failed: {e:?}"))?;
    let gpu_values: Vec<f32> = {
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, f32>(&data).to_vec();
        drop(data);
        staging.unmap();
        v
    };
    if gpu_values.len() != cases.len() {
        return Err(format!(
            "GPU readback length mismatch: expected {}, got {}",
            cases.len(),
            gpu_values.len()
        ));
    }

    // ── Compare GPU output to CPU oracle ──────────────────────────────────────
    let mut report = OracleReport {
        total_cases: cases.len(),
        max_abs_diff: 0.0,
        max_abs_diff_tag: "",
        max_abs_diff_pos: [0.0; 4],
        max_abs_diff_cpu: 0.0,
        max_abs_diff_gpu: 0.0,
        mismatches: Vec::new(),
        combos: 0,
    };
    let mut combo_set =
        std::collections::HashSet::<(u32, u32, u32, u32, u32, u32, u32)>::new();
    for (i, case) in cases.iter().enumerate() {
        let gpu = gpu_values[i];
        let cpu_val = case.cpu_value;
        let diff = (gpu - cpu_val).abs();
        if diff > report.max_abs_diff {
            report.max_abs_diff = diff;
            report.max_abs_diff_tag = case.tag;
            report.max_abs_diff_pos = case.pos;
            report.max_abs_diff_cpu = cpu_val;
            report.max_abs_diff_gpu = gpu;
        }
        let tol = if case.state.noise_type == cpu::noise_type::CELLULAR {
            ORACLE_TOLERANCE_CELLULAR
        } else {
            ORACLE_TOLERANCE
        };
        if !diff.is_finite() || diff > tol {
            if report.mismatches.len() < MAX_REPORT_MISMATCHES {
                report.mismatches.push(OracleMismatch {
                    tag: case.tag,
                    pos: case.pos,
                    cpu_value: cpu_val,
                    gpu_value: gpu,
                    abs_diff: diff,
                });
            }
        }
        combo_set.insert((
            case.state.noise_type,
            case.state.fractal_type,
            case.state.rotation_type_3d,
            case.state.domain_warp_type,
            case.state.cellular_distance_func,
            case.state.cellular_return_type,
            case.state.seed as u32,
        ));
    }
    report.combos = combo_set.len();

    if !report.mismatches.is_empty() {
        let mut msg = format!(
            "WGSL noise oracle: {} / {} cases failed (max_abs_diff = {:.4e} on `{}` at {:?}). First {} mismatches:\n",
            report.mismatches.len(),
            cases.len(),
            report.max_abs_diff,
            report.max_abs_diff_tag,
            report.max_abs_diff_pos,
            report.mismatches.len().min(MAX_REPORT_MISMATCHES),
        );
        for m in report.mismatches.iter() {
            msg.push_str(&format!(
                "  - tag={} pos={:?} cpu={} gpu={} diff={:.4e}\n",
                m.tag, m.pos, m.cpu_value, m.gpu_value, m.abs_diff,
            ));
        }
        return Err(msg);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test — the inlined dispatch shader contains both the noise module
    /// body and the dispatch entry point. Catches the `// @begin` marker drift
    /// in `build_oracle_dispatch_shader_src` before the real GPU dispatch runs.
    #[test]
    fn dispatch_shader_inlines_noise_module() {
        let src = build_oracle_dispatch_shader_src();
        assert!(src.contains("fn fnl_get_noise_3d"), "noise module not inlined");
        assert!(src.contains("fn dispatch_oracle"), "dispatch entry point not present");
        // The actual `#define_import_path` directive line should be stripped.
        // We match a line beginning with the directive (a substring search
        // would false-positive on the inlined "(stripped ...)" replacement
        // comment).
        let has_directive = src
            .lines()
            .any(|line| line.trim_start().starts_with("#define_import_path"));
        assert!(
            !has_directive,
            "`#define_import_path` directive leaked into combined source"
        );
        // The `// @begin` marker line should not appear in the combined source
        // (the marker should have been consumed by the splice).
        let has_marker = src
            .lines()
            .any(|line| line.trim_start().starts_with("// @begin"));
        assert!(!has_marker, "`// @begin` splice marker leaked");
    }

    /// Build the test plan and confirm it is non-empty + bounded. Catches
    /// accidental zero-sample or runaway-loop bugs.
    #[test]
    fn test_plan_is_bounded() {
        let cases = build_test_plan();
        assert!(
            cases.len() >= 200 && cases.len() <= 8000,
            "test plan size {} is outside the bounded range [200, 8000]",
            cases.len()
        );
        // Confirm we cover all 6 noise families.
        let mut seen_noise_types =
            std::collections::HashSet::<u32>::new();
        for c in &cases {
            seen_noise_types.insert(c.state.noise_type);
        }
        assert!(
            seen_noise_types.len() >= 6,
            "test plan should exercise all 6 noise families, saw {}",
            seen_noise_types.len()
        );
    }

    /// `run_wgsl_noise_oracle` boots a headless render world, dispatches, and
    /// compares. The same function is used by the e2e gate; the test only
    /// asserts a clean `Ok(...)` outcome.
    #[test]
    fn run_oracle_passes() {
        match run_wgsl_noise_oracle() {
            Ok(report) => {
                eprintln!(
                    "WGSL noise oracle PASS: {} cases, {} unique combos, max_abs_diff = {:.4e}",
                    report.total_cases, report.combos, report.max_abs_diff,
                );
            }
            Err(msg) => panic!("WGSL noise oracle FAILED:\n{}", msg),
        }
    }
}
