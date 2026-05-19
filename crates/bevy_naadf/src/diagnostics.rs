//! Press-`P` runtime diagnostics dump + the `device_snapshot` static device
//! capture (2026-05-19 — `wasm-chunk-aadf-determinism` diagnostic package).
//!
//! Two unrelated surfaces, deliberately co-located:
//!
//! 1. **Press-P dump** ([`dump_diagnostics_on_p`] + [`DiagnosticsPlugin`]). One
//!    read-only `Update` system that, on `KeyCode::KeyP` just_pressed,
//!    formats a single multi-line block and emits it via `info!` (which on
//!    wasm32 routes through Bevy's `LogPlugin` to `console.log`, so the same
//!    dump appears in the browser DevTools console). Mutates nothing.
//!
//! 2. **Device snapshot** ([`device_snapshot`] sub-module +
//!    [`DeviceSnapshotPlugin`]). One-shot render-app system that, on the first
//!    frame after `RenderAdapter`/`RenderAdapterInfo`/`RenderDevice`/
//!    `RenderQueue` are populated, captures every read-only field of the
//!    wgpu/WebGPU device surface (adapter info, full `Limits`, full
//!    `Features` bitmask, downlevel capabilities, queue timestamp period),
//!    serialises it to JSON, and emits it. On native the snapshot writes
//!    to `target/diagnostics/device-snapshot-native.json` via
//!    `std::fs::write`; on wasm32 it goes through `info!` with a
//!    `[device-snapshot]` sentinel prefix, which the Playwright
//!    `device-snapshot.spec.ts` harness filters out of `console.log` and
//!    writes to `target/diagnostics/device-snapshot-web.json`. Mirrors the
//!    existing `[aadf-probe]` pipeline.

use std::fmt::Write;

use bevy::camera::Camera;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::AppArgs;
use crate::camera::position_split::PositionSplit;
use crate::editor::ray::screen_to_ray;
use crate::world::data::{VoxelTypes, WorldData};

/// `Update` system: on `KeyP` just_pressed, log a single multi-line
/// diagnostics block covering camera, cursor → voxel raycast, and `AppArgs`.
pub fn dump_diagnostics_on_p(
    keys: Res<ButtonInput<KeyCode>>,
    args: Option<Res<AppArgs>>,
    world_data: Option<Res<WorldData>>,
    voxel_types: Option<Res<VoxelTypes>>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera_q: Query<
        (&Camera, &GlobalTransform, &Transform, Option<&PositionSplit>),
        With<Camera3d>,
    >,
) {
    if !keys.just_pressed(KeyCode::KeyP) {
        return;
    }

    let mut buf = String::from("\n=== diagnostics (KeyP) ===\n");

    if let Ok((camera, cam_gxf, cam_tx, pos_split)) = camera_q.single() {
        let p = cam_tx.translation;
        let fwd = cam_tx.forward();
        let g = cam_gxf.translation();
        let _ = writeln!(
            buf,
            "camera.translation = ({:.3}, {:.3}, {:.3})\n\
             camera.global      = ({:.3}, {:.3}, {:.3})\n\
             camera.forward     = ({:.3}, {:.3}, {:.3})\n\
             camera.rotation    = {:?}",
            p.x, p.y, p.z, g.x, g.y, g.z, fwd.x, fwd.y, fwd.z, cam_tx.rotation
        );
        if let Some(ps) = pos_split {
            let _ = writeln!(buf, "camera.position_split = {:?}", ps);
        }

        let cursor = window.single().ok().and_then(|w| w.cursor_position());
        match cursor {
            None => buf.push_str("cursor: <off-window>\n"),
            Some(cur) => {
                let _ = writeln!(buf, "cursor.viewport = ({:.1}, {:.1})", cur.x, cur.y);
                match screen_to_ray(camera, cam_gxf, cur) {
                    None => buf.push_str("ray: <viewport_to_world failed>\n"),
                    Some(ray) => {
                        let _ = writeln!(
                            buf,
                            "ray.origin = ({:.3}, {:.3}, {:.3})  dir = ({:.3}, {:.3}, {:.3})",
                            ray.origin.x, ray.origin.y, ray.origin.z,
                            ray.dir.x, ray.dir.y, ray.dir.z
                        );
                        let hit = world_data
                            .as_ref()
                            .and_then(|wd| wd.ray_traversal(ray.origin, ray.dir));
                        match hit {
                            None => buf.push_str("hit: <miss>\n"),
                            Some(hit) => {
                                let _ = writeln!(
                                    buf,
                                    "hit.voxel_pos     = {:?}\n\
                                     hit.world_pos     = ({:.3}, {:.3}, {:.3})\n\
                                     hit.normal        = ({:.2}, {:.2}, {:.2})\n\
                                     hit.distance      = {:.3}\n\
                                     hit.voxel_type_id = {:?}",
                                    hit.voxel_pos,
                                    hit.world_pos.x, hit.world_pos.y, hit.world_pos.z,
                                    hit.normal.x, hit.normal.y, hit.normal.z,
                                    hit.distance, hit.voxel_type
                                );
                                if let Some(vt) = voxel_types
                                    .as_ref()
                                    .and_then(|t| t.types.get(hit.voxel_type.0 as usize))
                                {
                                    let _ = writeln!(buf, "hit.voxel_type    = {:?}", vt);
                                }
                            }
                        }
                    }
                }
            }
        }
    } else {
        buf.push_str("camera: <no Camera3d entity found>\n");
    }

    if let Some(a) = args.as_ref() {
        let _ = writeln!(
            buf,
            "args.grid_preset         = {:?}\n\
             args.taa                 = {}\n\
             args.taa_ring_depth      = {}\n\
             args.spawn_test_entity   = {}\n\
             args.gi                  = {:#?}\n\
             args.construction_config = {:#?}",
            a.grid_preset,
            a.taa,
            a.taa_ring_depth,
            a.spawn_test_entity,
            a.gi,
            a.construction_config
        );
    } else {
        buf.push_str("args: <AppArgs resource missing>\n");
    }

    buf.push_str("===========================");
    info!(target: "diagnostics", "{}", buf);
}

/// Wires `dump_diagnostics_on_p` into the `Update` schedule. Registered only
/// outside the e2e harness (the e2e config is non-interactive).
pub struct DiagnosticsPlugin;

impl Plugin for DiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, dump_diagnostics_on_p);
    }
}

// ============================================================================
// device_snapshot — static device-capability JSON dump
// ============================================================================
//
// Design source: `docs/orchestrate/wasm-chunk-aadf-nondeterminism/
// 01-diagnostics-design.md` §C (Static device snapshot). Output channels:
//
//   - native: `target/diagnostics/device-snapshot-native.json` via fs::write
//             on first frame after `RenderAdapter` + `RenderDevice` resolved.
//   - web:    `[device-snapshot] {json}` info-log line on first such frame.
//             Captured by `e2e/tests/device-snapshot.spec.ts` (or the
//             existing parity gate's ConsoleCollector when extended) and
//             written to `target/diagnostics/device-snapshot-web.json` host-side.
//
// The capture runs in the render sub-app (where `RenderAdapter` etc. live);
// it relays the JSON to the main world via the `MainWorld` resource pattern
// (`ExtractSchedule` reads from a render-world `Resource`, copies to the
// main world, then a main-world `Update` system consumes it once and either
// writes to disk (native) or emits the sentinel `info!` line (web)).

pub mod device_snapshot {
    use std::sync::atomic::{AtomicBool, Ordering};

    use bevy::prelude::*;
    use bevy::render::renderer::{
        RenderAdapter, RenderAdapterInfo, RenderDevice, RenderQueue,
    };
    use bevy::render::{ExtractSchedule, Render, RenderApp, RenderSystems};
    use serde::Serialize;

    // ----- schema -----------------------------------------------------------

    pub const SCHEMA_VERSION: u32 = 1;

    /// One-line `[device-snapshot]` JSON sentinel — mirrors the
    /// `[aadf-probe]` convention so the Playwright filter is regex-cheap.
    pub const SENTINEL_PREFIX: &str = "[device-snapshot]";

    /// Closing sentinel — Bevy's wasm32 `info!` formatter appends a
    /// trailing `target:`/span-metadata payload AFTER the message text,
    /// so the spec can't rely on "everything after `[device-snapshot]` is
    /// JSON." The emitter now wraps the JSON between
    /// `SENTINEL_PREFIX … END_SENTINEL` and the spec extracts the
    /// substring between them. Added after a Playwright run captured
    /// 5812 bytes for a 5730-byte JSON body — 82 bytes of trailing
    /// tracing-formatter metadata broke `JSON.parse`.
    pub const END_SENTINEL: &str = "[device-snapshot-end]";

    #[derive(Serialize, Debug, Clone)]
    pub struct DeviceSnapshot {
        pub schema_version: u32,
        pub target: &'static str,
        pub captured_at_unix_seconds: u64,
        pub adapter_info: SnapshotAdapterInfo,
        pub adapter_features: Vec<String>,
        pub adapter_features_bits: [u64; 2],
        pub adapter_limits: SnapshotLimits,
        pub downlevel: SnapshotDownlevel,
        pub device_features: Vec<String>,
        pub device_features_bits: [u64; 2],
        pub device_limits: SnapshotLimits,
        pub limit_deltas: Vec<LimitDelta>,
        pub queue_timestamp_period_ns: f32,
        pub downlevel_is_webgpu_compliant: bool,
        pub build: SnapshotBuild,
    }

    #[derive(Serialize, Debug, Clone)]
    pub struct SnapshotAdapterInfo {
        pub name: String,
        pub vendor: u32,
        pub device: u32,
        pub device_pci_bus_id: String,
        pub driver: String,
        pub driver_info: String,
        pub backend: String,
        pub device_type: String,
        pub subgroup_min_size: u32,
        pub subgroup_max_size: u32,
        pub transient_saves_memory: bool,
    }

    /// Mirror of `wgpu_types::Limits` — every field flattened. Filled from
    /// `wgpu::Limits` via the [`limits_to_snapshot`] helper. The struct is
    /// exhaustive against wgpu 29.0.3; new fields in future wgpu versions
    /// will compile-error here, which is the desired behaviour (the impl
    /// agent updates the schema deliberately).
    #[derive(Serialize, Debug, Clone, PartialEq, Eq)]
    #[allow(non_snake_case)]
    pub struct SnapshotLimits {
        pub max_texture_dimension_1d: u32,
        pub max_texture_dimension_2d: u32,
        pub max_texture_dimension_3d: u32,
        pub max_texture_array_layers: u32,
        pub max_bind_groups: u32,
        pub max_bindings_per_bind_group: u32,
        pub max_dynamic_uniform_buffers_per_pipeline_layout: u32,
        pub max_dynamic_storage_buffers_per_pipeline_layout: u32,
        pub max_sampled_textures_per_shader_stage: u32,
        pub max_samplers_per_shader_stage: u32,
        pub max_storage_buffers_per_shader_stage: u32,
        pub max_storage_textures_per_shader_stage: u32,
        pub max_uniform_buffers_per_shader_stage: u32,
        pub max_binding_array_elements_per_shader_stage: u32,
        pub max_binding_array_acceleration_structure_elements_per_shader_stage: u32,
        pub max_binding_array_sampler_elements_per_shader_stage: u32,
        pub max_uniform_buffer_binding_size: u64,
        pub max_storage_buffer_binding_size: u64,
        pub max_vertex_buffers: u32,
        pub max_buffer_size: u64,
        pub max_vertex_attributes: u32,
        pub max_vertex_buffer_array_stride: u32,
        pub max_inter_stage_shader_variables: u32,
        pub min_uniform_buffer_offset_alignment: u32,
        pub min_storage_buffer_offset_alignment: u32,
        pub max_color_attachments: u32,
        pub max_color_attachment_bytes_per_sample: u32,
        pub max_compute_workgroup_storage_size: u32,
        pub max_compute_invocations_per_workgroup: u32,
        pub max_compute_workgroup_size_x: u32,
        pub max_compute_workgroup_size_y: u32,
        pub max_compute_workgroup_size_z: u32,
        pub max_compute_workgroups_per_dimension: u32,
        pub max_immediate_size: u32,
        pub max_non_sampler_bindings: u32,
        pub max_task_mesh_workgroup_total_count: u32,
        pub max_task_mesh_workgroups_per_dimension: u32,
        pub max_task_invocations_per_workgroup: u32,
        pub max_task_invocations_per_dimension: u32,
        pub max_mesh_invocations_per_workgroup: u32,
        pub max_mesh_invocations_per_dimension: u32,
        pub max_task_payload_size: u32,
        pub max_mesh_output_vertices: u32,
        pub max_mesh_output_primitives: u32,
        pub max_mesh_output_layers: u32,
        pub max_mesh_multiview_view_count: u32,
        pub max_blas_primitive_count: u32,
        pub max_blas_geometry_count: u32,
        pub max_tlas_instance_count: u32,
        pub max_acceleration_structures_per_shader_stage: u32,
        pub max_multiview_view_count: u32,
    }

    #[derive(Serialize, Debug, Clone)]
    pub struct SnapshotDownlevel {
        pub flags: Vec<String>,
        pub flags_bits: u32,
        pub shader_model: String,
    }

    #[derive(Serialize, Debug, Clone)]
    pub struct LimitDelta {
        pub field: String,
        pub adapter_value: u64,
        pub device_value: u64,
    }

    #[derive(Serialize, Debug, Clone)]
    pub struct SnapshotBuild {
        pub wgpu_version: &'static str,
        pub bevy_version: &'static str,
        pub git_sha: &'static str,
        pub profile: &'static str,
        pub target_arch: &'static str,
        pub target_os: &'static str,
    }

    // ----- capture (render world) ------------------------------------------

    /// Render-world resource that carries the captured snapshot from the
    /// `Render` schedule (where `RenderAdapter` lives) to the main world
    /// via the `ExtractSchedule` relay. Cleared after relay.
    #[derive(Resource, Default, Debug, Clone)]
    pub struct PendingRenderSnapshot(pub Option<DeviceSnapshot>);

    /// Main-world resource holding the snapshot once relayed from render
    /// world. Consumed (taken) by the writer system on the first frame it
    /// is populated.
    #[derive(Resource, Default, Debug, Clone)]
    pub struct PendingMainSnapshot(pub Option<DeviceSnapshot>);

    /// Gate so the render-world capture system runs once across the lifetime
    /// of the process.
    static CAPTURED: AtomicBool = AtomicBool::new(false);
    /// Gate so the main-world emit/write system fires at most once.
    static EMITTED: AtomicBool = AtomicBool::new(false);

    /// Render-world system: capture `RenderAdapter` + `RenderAdapterInfo` +
    /// `RenderDevice` + `RenderQueue` into a `DeviceSnapshot`, store it in
    /// the render-world `PendingRenderSnapshot` for `ExtractSchedule` to
    /// relay to the main world. Runs once.
    pub fn capture_device_snapshot(
        adapter: Res<RenderAdapter>,
        adapter_info: Res<RenderAdapterInfo>,
        device: Res<RenderDevice>,
        queue: Res<RenderQueue>,
        mut out: ResMut<PendingRenderSnapshot>,
    ) {
        if CAPTURED.swap(true, Ordering::AcqRel) {
            return;
        }

        // `RenderAdapter` derefs to `Arc<WgpuWrapper<Adapter>>`; deref twice
        // to reach `&wgpu::Adapter`.
        let adapter_ref: &wgpu::Adapter = &***adapter;
        let info: &wgpu::AdapterInfo = &**adapter_info;

        let adapter_features = adapter_ref.features();
        let adapter_limits = adapter_ref.limits();
        let downlevel = adapter_ref.get_downlevel_capabilities();
        let device_features = device.features();
        let device_limits = device.limits();
        let queue_timestamp_period_ns = (***queue).get_timestamp_period();

        let snapshot = DeviceSnapshot {
            schema_version: SCHEMA_VERSION,
            target: if cfg!(target_arch = "wasm32") {
                "web"
            } else {
                "native"
            },
            captured_at_unix_seconds: web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            adapter_info: snapshot_adapter_info(info),
            adapter_features: features_to_names(adapter_features),
            adapter_features_bits: features_to_bits(adapter_features),
            adapter_limits: limits_to_snapshot(&adapter_limits),
            downlevel: snapshot_downlevel(&downlevel),
            device_features: features_to_names(device_features),
            device_features_bits: features_to_bits(device_features),
            device_limits: limits_to_snapshot(&device_limits),
            limit_deltas: compute_limit_deltas(&adapter_limits, &device_limits),
            queue_timestamp_period_ns,
            downlevel_is_webgpu_compliant: downlevel.is_webgpu_compliant(),
            build: build_facts(),
        };

        out.0 = Some(snapshot);
    }

    /// `ExtractSchedule` system: copy the render-world `PendingRenderSnapshot`
    /// over to the main world via `MainWorld`, then clear the render-world
    /// side.
    pub fn extract_device_snapshot_to_main(
        mut main_world: ResMut<bevy::render::MainWorld>,
        mut pending: ResMut<PendingRenderSnapshot>,
    ) {
        let Some(snap) = pending.0.take() else { return };
        if let Some(mut main) =
            main_world.get_resource_mut::<PendingMainSnapshot>()
        {
            // Only store if main world hasn't already received one (the
            // capture system is gated; second arrivals would be the same
            // snapshot anyway).
            if main.0.is_none() {
                main.0 = Some(snap);
            }
        }
    }

    /// Main-world `Update` system: drain `PendingMainSnapshot` once and
    /// either write the JSON to disk (native) or emit it via `info!` with
    /// the `[device-snapshot]` sentinel prefix (wasm32). Runs at most once.
    pub fn emit_device_snapshot_main(mut pending: ResMut<PendingMainSnapshot>) {
        if EMITTED.load(Ordering::Acquire) {
            return;
        }
        let Some(snap) = pending.0.take() else { return };
        if EMITTED.swap(true, Ordering::AcqRel) {
            return;
        }

        let json = match serde_json::to_string(&snap) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    target: "device-snapshot",
                    "device snapshot serde_json::to_string failed: {e}"
                );
                return;
            }
        };

        // 1) Sentinel-prefixed info line — captured by Playwright filter
        //    on wasm; harmless on native (also picked up by stdout, where
        //    the native fs::write below is the authoritative output).
        //
        // No explicit `target:` — matches the existing `[aadf-probe]`
        // convention (the project's `RUST_LOG`/`LogPlugin` filter does not
        // include a custom-target allowlist, so untargeted info lines are
        // the path of guaranteed-reaches-console-on-web).
        info!("{} {} {}", SENTINEL_PREFIX, json, END_SENTINEL);

        // 2) Native: write JSON to `target/diagnostics/device-snapshot-native.json`.
        //    On wasm32 std::fs is unavailable, so the wasm path relies on
        //    the sentinel info line + Playwright capture.
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Workspace-relative path. Cwd at run-time is wherever the user
            // invoked `cargo run` from; the e2e gate / justfile expect
            // workspace-root cwd.
            let dir = std::path::Path::new("target/diagnostics");
            if let Err(e) = std::fs::create_dir_all(dir) {
                error!(
                    target: "device-snapshot",
                    "create_dir_all({}) failed: {e}",
                    dir.display(),
                );
                return;
            }
            let path = dir.join("device-snapshot-native.json");
            match std::fs::write(&path, &json) {
                Ok(()) => info!(
                    target: "device-snapshot",
                    "wrote device snapshot to {} ({} bytes)",
                    path.display(),
                    json.len(),
                ),
                Err(e) => error!(
                    target: "device-snapshot",
                    "fs::write({}) failed: {e}",
                    path.display(),
                ),
            }
        }
    }

    // ----- helpers ----------------------------------------------------------

    fn snapshot_adapter_info(info: &wgpu::AdapterInfo) -> SnapshotAdapterInfo {
        SnapshotAdapterInfo {
            name: info.name.clone(),
            vendor: info.vendor,
            device: info.device,
            device_pci_bus_id: info.device_pci_bus_id.clone(),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            backend: format!("{:?}", info.backend).to_lowercase(),
            device_type: format!("{:?}", info.device_type),
            subgroup_min_size: info.subgroup_min_size,
            subgroup_max_size: info.subgroup_max_size,
            transient_saves_memory: info.transient_saves_memory,
        }
    }

    fn features_to_names(f: wgpu::Features) -> Vec<String> {
        let mut names: Vec<String> = f
            .iter_names()
            .map(|(name, _)| name.to_lowercase().replace('_', "-"))
            .collect();
        names.sort();
        names
    }

    fn features_to_bits(f: wgpu::Features) -> [u64; 2] {
        // wgpu 29's `Features` is split: `f.features_wgpu` (native-only
        // extension set) + `f.features_webgpu` (spec-WebGPU set). Both have
        // a `.bits()` that returns the underlying integer. We emit them as
        // `[webgpu, wgpu]` so the diff is stable across native↔web (the
        // web side always has `wgpu_bits = 0`).
        [
            f.features_webgpu.bits() as u64,
            f.features_wgpu.bits() as u64,
        ]
    }

    fn limits_to_snapshot(l: &wgpu::Limits) -> SnapshotLimits {
        SnapshotLimits {
            max_texture_dimension_1d: l.max_texture_dimension_1d,
            max_texture_dimension_2d: l.max_texture_dimension_2d,
            max_texture_dimension_3d: l.max_texture_dimension_3d,
            max_texture_array_layers: l.max_texture_array_layers,
            max_bind_groups: l.max_bind_groups,
            max_bindings_per_bind_group: l.max_bindings_per_bind_group,
            max_dynamic_uniform_buffers_per_pipeline_layout: l
                .max_dynamic_uniform_buffers_per_pipeline_layout,
            max_dynamic_storage_buffers_per_pipeline_layout: l
                .max_dynamic_storage_buffers_per_pipeline_layout,
            max_sampled_textures_per_shader_stage: l
                .max_sampled_textures_per_shader_stage,
            max_samplers_per_shader_stage: l.max_samplers_per_shader_stage,
            max_storage_buffers_per_shader_stage: l
                .max_storage_buffers_per_shader_stage,
            max_storage_textures_per_shader_stage: l
                .max_storage_textures_per_shader_stage,
            max_uniform_buffers_per_shader_stage: l
                .max_uniform_buffers_per_shader_stage,
            max_binding_array_elements_per_shader_stage: l
                .max_binding_array_elements_per_shader_stage,
            max_binding_array_acceleration_structure_elements_per_shader_stage: l
                .max_binding_array_acceleration_structure_elements_per_shader_stage,
            max_binding_array_sampler_elements_per_shader_stage: l
                .max_binding_array_sampler_elements_per_shader_stage,
            max_uniform_buffer_binding_size: l.max_uniform_buffer_binding_size,
            max_storage_buffer_binding_size: l.max_storage_buffer_binding_size,
            max_vertex_buffers: l.max_vertex_buffers,
            max_buffer_size: l.max_buffer_size,
            max_vertex_attributes: l.max_vertex_attributes,
            max_vertex_buffer_array_stride: l.max_vertex_buffer_array_stride,
            max_inter_stage_shader_variables: l
                .max_inter_stage_shader_variables,
            min_uniform_buffer_offset_alignment: l
                .min_uniform_buffer_offset_alignment,
            min_storage_buffer_offset_alignment: l
                .min_storage_buffer_offset_alignment,
            max_color_attachments: l.max_color_attachments,
            max_color_attachment_bytes_per_sample: l
                .max_color_attachment_bytes_per_sample,
            max_compute_workgroup_storage_size: l
                .max_compute_workgroup_storage_size,
            max_compute_invocations_per_workgroup: l
                .max_compute_invocations_per_workgroup,
            max_compute_workgroup_size_x: l.max_compute_workgroup_size_x,
            max_compute_workgroup_size_y: l.max_compute_workgroup_size_y,
            max_compute_workgroup_size_z: l.max_compute_workgroup_size_z,
            max_compute_workgroups_per_dimension: l
                .max_compute_workgroups_per_dimension,
            max_immediate_size: l.max_immediate_size,
            max_non_sampler_bindings: l.max_non_sampler_bindings,
            max_task_mesh_workgroup_total_count: l
                .max_task_mesh_workgroup_total_count,
            max_task_mesh_workgroups_per_dimension: l
                .max_task_mesh_workgroups_per_dimension,
            max_task_invocations_per_workgroup: l
                .max_task_invocations_per_workgroup,
            max_task_invocations_per_dimension: l
                .max_task_invocations_per_dimension,
            max_mesh_invocations_per_workgroup: l
                .max_mesh_invocations_per_workgroup,
            max_mesh_invocations_per_dimension: l
                .max_mesh_invocations_per_dimension,
            max_task_payload_size: l.max_task_payload_size,
            max_mesh_output_vertices: l.max_mesh_output_vertices,
            max_mesh_output_primitives: l.max_mesh_output_primitives,
            max_mesh_output_layers: l.max_mesh_output_layers,
            max_mesh_multiview_view_count: l.max_mesh_multiview_view_count,
            max_blas_primitive_count: l.max_blas_primitive_count,
            max_blas_geometry_count: l.max_blas_geometry_count,
            max_tlas_instance_count: l.max_tlas_instance_count,
            max_acceleration_structures_per_shader_stage: l
                .max_acceleration_structures_per_shader_stage,
            max_multiview_view_count: l.max_multiview_view_count,
        }
    }

    fn snapshot_downlevel(d: &wgpu::DownlevelCapabilities) -> SnapshotDownlevel {
        let mut flags: Vec<String> = d
            .flags
            .iter_names()
            .map(|(name, _)| name.to_lowercase().replace('_', "-"))
            .collect();
        flags.sort();
        SnapshotDownlevel {
            flags,
            flags_bits: d.flags.bits(),
            shader_model: format!("{:?}", d.shader_model),
        }
    }

    fn compute_limit_deltas(
        adapter: &wgpu::Limits,
        device: &wgpu::Limits,
    ) -> Vec<LimitDelta> {
        let pairs: Vec<(&'static str, u64, u64)> = vec![
            ("max_texture_dimension_1d", adapter.max_texture_dimension_1d as u64, device.max_texture_dimension_1d as u64),
            ("max_texture_dimension_2d", adapter.max_texture_dimension_2d as u64, device.max_texture_dimension_2d as u64),
            ("max_texture_dimension_3d", adapter.max_texture_dimension_3d as u64, device.max_texture_dimension_3d as u64),
            ("max_texture_array_layers", adapter.max_texture_array_layers as u64, device.max_texture_array_layers as u64),
            ("max_bind_groups", adapter.max_bind_groups as u64, device.max_bind_groups as u64),
            ("max_bindings_per_bind_group", adapter.max_bindings_per_bind_group as u64, device.max_bindings_per_bind_group as u64),
            ("max_dynamic_uniform_buffers_per_pipeline_layout", adapter.max_dynamic_uniform_buffers_per_pipeline_layout as u64, device.max_dynamic_uniform_buffers_per_pipeline_layout as u64),
            ("max_dynamic_storage_buffers_per_pipeline_layout", adapter.max_dynamic_storage_buffers_per_pipeline_layout as u64, device.max_dynamic_storage_buffers_per_pipeline_layout as u64),
            ("max_sampled_textures_per_shader_stage", adapter.max_sampled_textures_per_shader_stage as u64, device.max_sampled_textures_per_shader_stage as u64),
            ("max_samplers_per_shader_stage", adapter.max_samplers_per_shader_stage as u64, device.max_samplers_per_shader_stage as u64),
            ("max_storage_buffers_per_shader_stage", adapter.max_storage_buffers_per_shader_stage as u64, device.max_storage_buffers_per_shader_stage as u64),
            ("max_storage_textures_per_shader_stage", adapter.max_storage_textures_per_shader_stage as u64, device.max_storage_textures_per_shader_stage as u64),
            ("max_uniform_buffers_per_shader_stage", adapter.max_uniform_buffers_per_shader_stage as u64, device.max_uniform_buffers_per_shader_stage as u64),
            ("max_binding_array_elements_per_shader_stage", adapter.max_binding_array_elements_per_shader_stage as u64, device.max_binding_array_elements_per_shader_stage as u64),
            ("max_uniform_buffer_binding_size", adapter.max_uniform_buffer_binding_size, device.max_uniform_buffer_binding_size),
            ("max_storage_buffer_binding_size", adapter.max_storage_buffer_binding_size, device.max_storage_buffer_binding_size),
            ("max_vertex_buffers", adapter.max_vertex_buffers as u64, device.max_vertex_buffers as u64),
            ("max_buffer_size", adapter.max_buffer_size, device.max_buffer_size),
            ("max_vertex_attributes", adapter.max_vertex_attributes as u64, device.max_vertex_attributes as u64),
            ("max_vertex_buffer_array_stride", adapter.max_vertex_buffer_array_stride as u64, device.max_vertex_buffer_array_stride as u64),
            ("max_inter_stage_shader_variables", adapter.max_inter_stage_shader_variables as u64, device.max_inter_stage_shader_variables as u64),
            ("max_color_attachments", adapter.max_color_attachments as u64, device.max_color_attachments as u64),
            ("max_color_attachment_bytes_per_sample", adapter.max_color_attachment_bytes_per_sample as u64, device.max_color_attachment_bytes_per_sample as u64),
            ("max_compute_workgroup_storage_size", adapter.max_compute_workgroup_storage_size as u64, device.max_compute_workgroup_storage_size as u64),
            ("max_compute_invocations_per_workgroup", adapter.max_compute_invocations_per_workgroup as u64, device.max_compute_invocations_per_workgroup as u64),
            ("max_compute_workgroup_size_x", adapter.max_compute_workgroup_size_x as u64, device.max_compute_workgroup_size_x as u64),
            ("max_compute_workgroup_size_y", adapter.max_compute_workgroup_size_y as u64, device.max_compute_workgroup_size_y as u64),
            ("max_compute_workgroup_size_z", adapter.max_compute_workgroup_size_z as u64, device.max_compute_workgroup_size_z as u64),
            ("max_compute_workgroups_per_dimension", adapter.max_compute_workgroups_per_dimension as u64, device.max_compute_workgroups_per_dimension as u64),
            ("max_immediate_size", adapter.max_immediate_size as u64, device.max_immediate_size as u64),
            ("max_non_sampler_bindings", adapter.max_non_sampler_bindings as u64, device.max_non_sampler_bindings as u64),
        ];
        let mut deltas = Vec::new();
        for (name, adapter_v, device_v) in pairs {
            if device_v < adapter_v {
                deltas.push(LimitDelta {
                    field: name.to_string(),
                    adapter_value: adapter_v,
                    device_value: device_v,
                });
            }
        }
        deltas
    }

    fn build_facts() -> SnapshotBuild {
        SnapshotBuild {
            wgpu_version: "29.0.3",
            bevy_version: "0.19.0-rc.1",
            git_sha: option_env!("BEVY_NAADF_GIT_SHA").unwrap_or("unknown"),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
        }
    }

    // ----- plugin -----------------------------------------------------------

    /// Wires the static device-snapshot capture into both worlds.
    ///
    /// - **Render world** — `PendingRenderSnapshot` resource +
    ///   `capture_device_snapshot` system in `RenderSystems::Prepare` (the
    ///   first set where `RenderAdapter`/`RenderDevice` are guaranteed
    ///   live) + `extract_device_snapshot_to_main` in `ExtractSchedule`.
    /// - **Main world** — `PendingMainSnapshot` resource +
    ///   `emit_device_snapshot_main` in `Update`.
    pub struct DeviceSnapshotPlugin;

    impl Plugin for DeviceSnapshotPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<PendingMainSnapshot>()
                .add_systems(Update, emit_device_snapshot_main);

            let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
                return;
            };
            render_app
                .init_resource::<PendingRenderSnapshot>()
                .add_systems(
                    Render,
                    capture_device_snapshot
                        .in_set(RenderSystems::Prepare),
                )
                .add_systems(ExtractSchedule, extract_device_snapshot_to_main);
        }
    }
}
