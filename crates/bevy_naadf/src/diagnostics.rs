//! Press-`P` runtime diagnostics dump.
//!
//! One read-only `Update` system that, on `KeyCode::KeyP` just_pressed,
//! formats a single multi-line block and emits it via `info!` (which on
//! wasm32 routes through Bevy's `LogPlugin` to `console.log`, so the same
//! dump appears in the browser DevTools console). Mutates nothing.

use std::fmt::Write;

use bevy::camera::Camera;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::AppArgs;
use crate::AppConfig;
use crate::camera::position_split::PositionSplit;
use crate::editor::ray::screen_to_ray;
use crate::render::taa::TaaRingConfig;
use crate::world::data::{VoxelTypes, WorldData};

/// `Update` system: on `KeyP` just_pressed, log a single multi-line
/// diagnostics block covering camera, cursor → voxel raycast, and `AppArgs`.
pub fn dump_diagnostics_on_p(
    keys: Res<ButtonInput<KeyCode>>,
    args: Option<Res<AppArgs>>,
    taa_ring: Option<Res<TaaRingConfig>>,
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
        // Step 2 of the config-as-resource refactor: `taa_ring_depth` migrated
        // off `AppArgs` onto the standalone `TaaRingConfig` main-world
        // resource. The diagnostics dump fans out — per Q4 of
        // `docs/orchestrate/config-as-resource-refactor/01-context.md`.
        let taa_ring_depth_str = taa_ring
            .as_ref()
            .map(|r| r.depth.to_string())
            .unwrap_or_else(|| "<TaaRingConfig resource missing>".to_string());
        let _ = writeln!(
            buf,
            "args.grid_preset         = {:?}\n\
             args.taa                 = {}\n\
             taa_ring_depth           = {}\n\
             args.spawn_test_entity   = {}\n\
             args.gi                  = {:#?}\n\
             args.construction_config = {:#?}",
            a.grid_preset,
            a.taa,
            taa_ring_depth_str,
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

/// Wires `dump_diagnostics_on_p` into the `Update` schedule. Self-skips
/// under the e2e harness (`AppConfig.add_e2e_systems` true) — the harness is
/// non-interactive + resources the dump reads may be absent there.
pub struct DiagnosticsPlugin;

impl Plugin for DiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            dump_diagnostics_on_p
                .run_if(|cfg: Option<Res<AppConfig>>| {
                    cfg.map(|c| !c.add_e2e_systems).unwrap_or(true)
                }),
        );
    }
}
