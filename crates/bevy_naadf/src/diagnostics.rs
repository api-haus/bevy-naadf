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

use crate::AppConfig;
use crate::GiSettings;
use crate::GridPreset;
use crate::camera::position_split::PositionSplit;
use crate::editor::ray::screen_to_ray;
use crate::render::construction::{ConstructionConfig, SpawnTestEntity};
use crate::render::taa::{TaaConfig, TaaRingConfig};
use crate::world::data::{VoxelTypes, WorldData};

/// `Update` system: on `KeyP` just_pressed, log a single multi-line
/// diagnostics block covering camera, cursor → voxel raycast, and the
/// per-domain configuration resources (`GridPreset`, `TaaConfig`,
/// `TaaRingConfig`, `SpawnTestEntity`, `GiSettings`, `ConstructionConfig`).
#[allow(clippy::too_many_arguments)]
pub fn dump_diagnostics_on_p(
    keys: Res<ButtonInput<KeyCode>>,
    grid_preset: Option<Res<GridPreset>>,
    spawn_test_entity: Option<Res<SpawnTestEntity>>,
    taa: Option<Res<TaaConfig>>,
    taa_ring: Option<Res<TaaRingConfig>>,
    gi: Option<Res<GiSettings>>,
    construction: Option<Res<ConstructionConfig>>,
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

    // Steps 2-5 + 8 of the config-as-resource refactor: `taa_ring_depth`,
    // `taa`, `gi`, `construction_config`, `grid_preset` and
    // `spawn_test_entity` migrated off `AppArgs` onto standalone per-domain
    // main-world resources. The diagnostics dump fans out — per Q4 of
    // `docs/orchestrate/config-as-resource-refactor/01-context.md`. The
    // dump no longer reads `AppArgs` at all (the remaining `AppArgs` fields
    // are e2e mode booleans the dump never showed; the `DiagnosticsPlugin`
    // self-skips under e2e regardless).
    {
        let grid_preset_str = grid_preset
            .as_ref()
            .map(|g| format!("{:?}", **g))
            .unwrap_or_else(|| "<GridPreset resource missing>".to_string());
        let spawn_test_entity_str = spawn_test_entity
            .as_ref()
            .map(|s| s.0.to_string())
            .unwrap_or_else(|| "<SpawnTestEntity resource missing>".to_string());
        let taa_ring_depth_str = taa_ring
            .as_ref()
            .map(|r| r.depth.to_string())
            .unwrap_or_else(|| "<TaaRingConfig resource missing>".to_string());
        let taa_str = taa
            .as_ref()
            .map(|t| t.enabled.to_string())
            .unwrap_or_else(|| "<TaaConfig resource missing>".to_string());
        let gi_str = gi
            .as_ref()
            .map(|g| format!("{:#?}", **g))
            .unwrap_or_else(|| "<GiSettings resource missing>".to_string());
        let construction_str = construction
            .as_ref()
            .map(|c| format!("{:#?}", **c))
            .unwrap_or_else(|| "<ConstructionConfig resource missing>".to_string());
        let _ = writeln!(
            buf,
            "grid_preset              = {}\n\
             taa                      = {}\n\
             taa_ring_depth           = {}\n\
             spawn_test_entity        = {}\n\
             gi                       = {}\n\
             construction_config      = {}",
            grid_preset_str,
            taa_str,
            taa_ring_depth_str,
            spawn_test_entity_str,
            gi_str,
            construction_str,
        );
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
