//! The custom `naadf/*` BRP verbs — the project's domain control surface for
//! the external e2e runner.
//!
//! Entirely behind the `e2e-brp` cargo feature (the `verbs` module is
//! `#[cfg]`-gated at the `e2e_brp/mod.rs` `pub mod` site). Each verb is an
//! ordinary Bevy system with the BRP handler shape — `fn(In(Option<Value>),
//! &mut World) -> BrpResult` for *instant* methods, `fn(In(Option<Value>),
//! &mut World) -> BrpResult<Option<Value>>` for *watching* methods (verified
//! against `bevy_remote 0.19.0-rc.1` `src/lib.rs:591-642`).
//!
//! ## Verb set (design §3)
//!
//! Main-world instant: [`step`], [`get_state`], [`capture`], [`apply_brush`],
//! [`set_camera`], [`load_world`], [`region_gate`], [`resize_window`].
//! Main-world watching: [`run_until_idle`], [`await_capture`].
//! Render-world instant: [`pipeline_scan`].
//!
//! Each verb is a thin wrapper over an existing project primitive — the brush
//! fns (`editor::tools`), capture (`e2e::readback`), framebuffer decode
//! (`e2e::framebuffer`), the pipeline-scan channel (`e2e::checks`). The wire
//! schema (the param / return structs) lives in [`super::schema`], compiled
//! unconditionally so the runner crate can import it.

use base64::Engine as _;
use bevy::prelude::*;
use bevy::remote::{error_codes, BrpError, BrpResult};
use bevy::window::{PrimaryWindow, Window};
use serde_json::{json, Value};

use crate::camera::position_split::PositionSplit;
use crate::e2e::framebuffer::{Framebuffer, Rect};
use crate::e2e::readback::{shoot_primary_window, E2eScreenshot};
use crate::voxel::VoxelTypeId;
use crate::world::data::WorldData;

use super::schema::BrushKind;

/// Voxels per chunk axis — `CELL_DIM² = 4² = 16` (the `WorldData::size_in_chunks`
/// → voxels conversion factor, matching `editor::tools` `CHUNK_VOXELS`).
const CHUNK_VOXELS_PER_AXIS: u32 = (crate::voxel::CELL_DIM as u32) * (crate::voxel::CELL_DIM as u32);

// ===========================================================================
// Frame stepping (Phase 1)
// ===========================================================================

/// The in-SUT frame-stepping gate (`02-design.md` §4.1).
///
/// The SUT always ticks — it is `WinitSettings::Continuous` (installed by
/// [`super::install_brp_server`]); it does not literally stop. `frames_remaining`
/// is a *logical* step budget the external runner manipulates: the runner
/// treats "the app is at rest" as `frames_remaining == 0`.
///
/// Initialised by [`super::install_brp_server`] via `init_resource`;
/// [`advance_e2e_control`] mutates it once per `Update`.
#[derive(Resource, Default, Debug)]
pub struct E2eControl {
    /// Monotonic frame counter — incremented every `Update` by
    /// [`advance_e2e_control`].
    pub frame: u64,
    /// Logical step budget. `naadf/step { frames: N }` adds `N`;
    /// [`advance_e2e_control`] saturating-decrements it each `Update`. The SUT
    /// is "running" while `> 0` and "at rest" at `0`.
    pub frames_remaining: u32,
}

/// Advance [`E2eControl`] once per `Update` (`02-design.md` §4.1).
pub fn advance_e2e_control(mut control: ResMut<E2eControl>) {
    control.frame += 1;
    control.frames_remaining = control.frames_remaining.saturating_sub(1);
}

/// `naadf/step` — instant main-world handler. Queue `frames` frames of
/// advancement (`02-design.md` §4.2).
///
/// It does **not** loop the schedule (pumping `Update` from inside `RemoteLast`
/// would re-enter the schedule mid-frame and decouple the rendered frames from
/// the logical step count — the design's D3 rejection). It adds `frames` to
/// [`E2eControl::frames_remaining`] and returns the *current* frame number.
///
/// Params: `{ frames: u32 }`. Returns: `{ frame: u64 }`.
pub fn step(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let frames = params
        .as_ref()
        .and_then(|v| v.get("frames"))
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_params("naadf/step requires an integer `frames` field"))?;
    let frames = u32::try_from(frames)
        .map_err(|_| invalid_params("naadf/step `frames` exceeds u32::MAX"))?;

    let mut control = world.resource_mut::<E2eControl>();
    control.frames_remaining = control.frames_remaining.saturating_add(frames);
    let frame = control.frame;

    Ok(json!({ "frame": frame }))
}

/// Single-slot per-watch state for [`run_until_idle`].
#[derive(Resource, Default, Debug)]
pub struct RunUntilIdleWatch {
    /// The [`E2eControl::frame`] the in-flight watch first ran at; `None` when
    /// no watch is in flight.
    pub started_at_frame: Option<u64>,
    /// Consecutive frames `frames_remaining == 0` has held for the watch.
    pub consecutive_idle: u32,
}

/// `naadf/run_until_idle` — watching main-world handler. The deterministic
/// "advance then assert" primitive (`02-design.md` §4.3).
///
/// `process_ongoing_watching_requests` re-runs the handler every frame. Returns
/// `Ok(None)` while running; one `Ok(Some({ done, frame, timed_out }))` once
/// `frames_remaining == 0` has held for `idle_frames` consecutive frames — or
/// `max_frames` total elapsed (hard ceiling so a hung SUT fails fast).
///
/// Params: `{ max_frames: u32, idle_frames: u32 }`.
pub fn run_until_idle(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    let max_frames = params
        .as_ref()
        .and_then(|v| v.get("max_frames"))
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            invalid_params("naadf/run_until_idle requires an integer `max_frames` field")
        })?;
    let idle_frames = params
        .as_ref()
        .and_then(|v| v.get("idle_frames"))
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            invalid_params("naadf/run_until_idle requires an integer `idle_frames` field")
        })?;

    let frame = world.resource::<E2eControl>().frame;
    let frames_remaining = world.resource::<E2eControl>().frames_remaining;

    let mut watch = world.resource_mut::<RunUntilIdleWatch>();

    let elapsed = match watch.started_at_frame {
        Some(start) if frame >= start => frame - start,
        _ => {
            watch.started_at_frame = Some(frame);
            watch.consecutive_idle = 0;
            0
        }
    };

    if frames_remaining == 0 {
        watch.consecutive_idle = watch.consecutive_idle.saturating_add(1);
    } else {
        watch.consecutive_idle = 0;
    }
    let consecutive_idle = watch.consecutive_idle;

    let settled = consecutive_idle as u64 >= idle_frames;
    let timed_out = elapsed >= max_frames;

    if settled || timed_out {
        watch.started_at_frame = None;
        watch.consecutive_idle = 0;
        return Ok(Some(json!({
            "done": true,
            "frame": frame,
            "timed_out": timed_out,
        })));
    }

    Ok(None)
}

// ===========================================================================
// Status (Phase 1, pipeline-scan wired in Phase 2)
// ===========================================================================

/// `naadf/get_state` — instant main-world handler. A small status snapshot
/// (`02-design.md` §3 table).
///
/// Returns `frame`, `frames_remaining`, `world_loaded`, `world_size_voxels`
/// (`null` until the world is installed), `pipeline_errors` (the main-world
/// side of the `PipelineScanResult` channel — `null` until the render-world
/// scan has run, a string on error), and `tracing_errors`.
pub fn get_state(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let control = world.resource::<E2eControl>();
    let frame = control.frame;
    let frames_remaining = control.frames_remaining;

    let world_size_voxels: Option<[u32; 3]> = world
        .get_resource::<WorldData>()
        .map(|wd| {
            let v = wd.size_in_chunks * CHUNK_VOXELS_PER_AXIS;
            [v.x, v.y, v.z]
        });
    let world_loaded = world_size_voxels.is_some();

    // The `PipelineScanResult` channel is wired by `install_brp_server` (Phase
    // 2); it is `null` here only until the render-world scan has produced its
    // first result. `null` means "not scanned", not "no errors".
    let pipeline_errors: Value = match world
        .get_resource::<crate::e2e::checks::PipelineScanResult>()
    {
        Some(scan) => match scan.0.lock() {
            Ok(guard) => match &*guard {
                Some(Ok(())) => Value::Null,
                Some(Err(msg)) => Value::String(msg.clone()),
                None => Value::Null,
            },
            Err(_) => Value::String("PipelineScanResult mutex poisoned".to_string()),
        },
        None => Value::Null,
    };

    let tracing_errors = crate::e2e::tracing_error_counter::tracing_error_count() as u64;

    Ok(json!({
        "frame": frame,
        "frames_remaining": frames_remaining,
        "world_loaded": world_loaded,
        "world_size_voxels": world_size_voxels,
        "pipeline_errors": pipeline_errors,
        "tracing_errors": tracing_errors,
    }))
}

// ===========================================================================
// Capture (Phase 2)
// ===========================================================================

/// `naadf/capture` — instant main-world handler. Spawn a
/// `Screenshot::primary_window()` entity (`02-design.md` §3).
///
/// Capture is async — the `ScreenshotCaptured` observer fires one or more
/// frames later and stashes the `Image` into [`E2eScreenshot`]. This verb
/// clears any stale stash, spawns the screenshot entity via
/// [`shoot_primary_window`] (the same free fn the legacy driver's `SHOOT` step
/// uses), and returns immediately. The runner collects the pixels via
/// [`await_capture`].
///
/// Params: `null` (ignored). Returns: `{ pending: true }`.
pub fn capture(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    // Clear any stale capture so `await_capture` blocks for *this* shot.
    if let Some(mut stash) = world.get_resource_mut::<E2eScreenshot>() {
        stash.0 = None;
    } else {
        return Err(internal_error(
            "naadf/capture: E2eScreenshot resource missing — install_brp_server \
             did not init it (Phase 2 wiring bug)",
        ));
    }
    // `shoot_primary_window` needs `Commands`; run it through a one-shot
    // exclusive scope so the spawn + observer-attach are applied immediately.
    let mut queue = bevy::ecs::world::CommandQueue::default();
    {
        let mut commands = Commands::new(&mut queue, world);
        shoot_primary_window(&mut commands);
    }
    queue.apply(world);

    Ok(json!({ "pending": true }))
}

/// Single-slot per-watch state for [`await_capture`].
#[derive(Resource, Default, Debug)]
pub struct AwaitCaptureWatch {
    /// The [`E2eControl::frame`] the in-flight watch first ran at.
    pub started_at_frame: Option<u64>,
}

/// The framebuffer of the most-recently-delivered [`await_capture`] — the
/// surface [`region_gate`] reads.
#[derive(Resource, Default)]
pub struct LastCapture(pub Option<Framebuffer>);

/// Default frame ceiling for [`await_capture`] when the caller passes `0` /
/// omits `max_frames`. The legacy `OASIS_DRAIN_FRAMES` was `16` *driver*
/// frames, but the BRP SUT ticks at its native (hundreds-of-FPS) rate while
/// `await_capture` polls, and the async `ScreenshotCaptured` observer can take
/// many native frames to fire when the renderer is under post-edit GPU load.
/// `2000` native frames is still sub-10 s wall-time and a real fail-fast
/// ceiling for a genuinely hung capture.
const AWAIT_CAPTURE_DEFAULT_CEILING: u64 = 2000;

/// `naadf/await_capture` — watching main-world handler. Stream one chunk once
/// the async screenshot from [`capture`] has been delivered (`02-design.md`
/// §3).
///
/// Each frame: if [`E2eScreenshot`] holds an `Image`, decode it via
/// [`Framebuffer::from_image`], stash it in [`LastCapture`] (so [`region_gate`]
/// can read it), encode it as a base64 PNG, and stream one
/// `{ ready, width, height, png_b64 }` chunk. If `max_frames` frames elapse
/// without delivery, stream an `Err` chunk (fail-fast). `Ok(None)` otherwise.
///
/// Params: `{ max_frames?: u32 }` (`0`/absent ⇒ [`AWAIT_CAPTURE_DEFAULT_CEILING`]).
pub fn await_capture(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    let max_frames = params
        .as_ref()
        .and_then(|v| v.get("max_frames"))
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
        .unwrap_or(AWAIT_CAPTURE_DEFAULT_CEILING);

    let frame = world.resource::<E2eControl>().frame;
    let elapsed = {
        let mut watch = world.resource_mut::<AwaitCaptureWatch>();
        match watch.started_at_frame {
            Some(start) if frame >= start => frame - start,
            _ => {
                watch.started_at_frame = Some(frame);
                0
            }
        }
    };

    // Has the capture delivered?
    let captured: Option<Image> = world
        .get_resource_mut::<E2eScreenshot>()
        .and_then(|mut s| s.0.take());

    if let Some(image) = captured {
        // End the watch.
        world.resource_mut::<AwaitCaptureWatch>().started_at_frame = None;

        let fb = match Framebuffer::from_image(&image) {
            Ok(fb) => fb,
            Err(msg) => {
                return Err(internal_error(&format!(
                    "naadf/await_capture: framebuffer decode failed: {msg}"
                )));
            }
        };
        let (width, height) = (fb.width(), fb.height());
        let png = match encode_png_bytes(&fb) {
            Ok(bytes) => bytes,
            Err(msg) => {
                return Err(internal_error(&format!(
                    "naadf/await_capture: PNG encode failed: {msg}"
                )));
            }
        };
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        // Stash for `naadf/region_gate`.
        world
            .get_resource_or_insert_with(LastCapture::default)
            .0 = Some(fb);

        return Ok(Some(json!({
            "ready": true,
            "width": width,
            "height": height,
            "png_b64": png_b64,
        })));
    }

    if elapsed >= max_frames {
        world.resource_mut::<AwaitCaptureWatch>().started_at_frame = None;
        return Err(internal_error(&format!(
            "naadf/await_capture: screenshot never delivered within {max_frames} \
             frames — the renderer produced no frame (check the SUT CWD / asset \
             path) or no `naadf/capture` was issued"
        )));
    }

    Ok(None)
}

/// Encode a [`Framebuffer`] to in-memory PNG bytes — the in-memory equivalent
/// of `Framebuffer::save_png` (which writes to disk). RGB, alpha dropped, same
/// as the on-disk path.
fn encode_png_bytes(fb: &Framebuffer) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity((fb.width() as usize) * (fb.height() as usize) * 3);
    for y in 0..fb.height() {
        for x in 0..fb.width() {
            let p = fb.pixel(x, y);
            rgb.push(p[0]);
            rgb.push(p[1]);
            rgb.push(p[2]);
        }
    }
    let buf: image::RgbImage = image::ImageBuffer::from_raw(fb.width(), fb.height(), rgb)
        .ok_or_else(|| {
            format!("RGB buffer size mismatch for {}x{}", fb.width(), fb.height())
        })?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("PNG write failed: {e}"))?;
    Ok(out.into_inner())
}

// ===========================================================================
// Brush (Phase 2)
// ===========================================================================

/// `naadf/apply_brush` — instant main-world handler. Apply one brush stroke
/// (`02-design.md` §3 / D6).
///
/// Calls the pure brush fns (`editor::tools::{sphere_brush, cube_brush,
/// paint_brush}`) directly with `&mut WorldData` — exactly the runtime path the
/// editor's `apply_edit_tool` and the legacy `oasis_edit_visual::apply_erase_brush`
/// take. It does **not** touch `EditorState` (the smoothed-pos / stroke
/// semantics are mouse-input artefacts irrelevant to one-shot programmatic
/// application — design D6).
///
/// Returns the producer-side deltas (`voxels_delta`, `blocks_delta`, `batches`)
/// the legacy oasis gate logs, so a test body can assert the producer side
/// cheaply alongside the framebuffer diff.
///
/// Params: `{ kind, pos:[f32;3], radius:f32, voxel_type?:u32, erase?:bool }`.
pub fn apply_brush(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("naadf/apply_brush requires params"))?;
    let p: super::schema::ApplyBrushParams = serde_json::from_value(params)
        .map_err(|e| invalid_params(&format!("naadf/apply_brush bad params: {e}")))?;

    let Some(mut wd) = world.get_resource_mut::<WorldData>() else {
        return Err(internal_error(
            "naadf/apply_brush: WorldData resource missing — the world is not \
             yet installed (issue naadf/step + naadf/run_until_idle first)",
        ));
    };

    let pos = Vec3::new(p.pos[0], p.pos[1], p.pos[2]);
    let ty = VoxelTypeId(p.voxel_type as u16);

    let v_before = wd.voxels_cpu.len() as i64;
    let b_before = wd.blocks_cpu.len() as i64;
    let batches_before = wd.pending_edits.batches.len();

    match p.kind {
        BrushKind::Sphere => {
            crate::editor::tools::sphere_brush(&mut wd, pos, p.radius, ty, p.erase)
        }
        BrushKind::Cube => {
            crate::editor::tools::cube_brush(&mut wd, pos, p.radius, ty, p.erase)
        }
        BrushKind::Paint => {
            // The C# paint brush has no erase mode (`schema::BrushKind` doc);
            // `erase` is ignored for paint.
            crate::editor::tools::paint_brush(&mut wd, pos, p.radius, ty)
        }
    }

    let voxels_delta = wd.voxels_cpu.len() as i64 - v_before;
    let blocks_delta = wd.blocks_cpu.len() as i64 - b_before;
    let batches = (wd.pending_edits.batches.len() - batches_before) as u32;

    Ok(json!({
        "voxels_delta": voxels_delta,
        "blocks_delta": blocks_delta,
        "batches": batches,
    }))
}

// ===========================================================================
// Camera (Phase 2)
// ===========================================================================

/// `naadf/set_camera` — instant main-world handler. Pin the `Camera3d` pose
/// (`02-design.md` §3).
///
/// Mutates the `Camera3d` entity's `Transform` and `PositionSplit` — the same
/// pair of writes the legacy `pin_oasis_camera` does (`oasis_edit_visual.rs:326-328`).
/// `PositionSplit` is re-derived from the new translation via
/// [`PositionSplit::from_world`]; `sync_position_split` (still in the SUT's
/// schedule) keeps it consistent on subsequent frames, but writing it here
/// avoids a one-frame lag between the pose change and its split origin.
///
/// Params: `{ translation:[f32;3], look_at:[f32;3], up?:[f32;3] }`.
pub fn set_camera(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("naadf/set_camera requires params"))?;
    let p: super::schema::SetCameraParams = serde_json::from_value(params)
        .map_err(|e| invalid_params(&format!("naadf/set_camera bad params: {e}")))?;

    let translation = Vec3::new(p.translation[0], p.translation[1], p.translation[2]);
    let look_at = Vec3::new(p.look_at[0], p.look_at[1], p.look_at[2]);
    let up = Vec3::new(p.up[0], p.up[1], p.up[2]);
    let pose = Transform::from_translation(translation).looking_at(look_at, up);

    let mut query =
        world.query_filtered::<(&mut Transform, &mut PositionSplit), With<Camera3d>>();
    let Ok((mut transform, mut split)) = query.single_mut(world) else {
        return Err(internal_error(
            "naadf/set_camera: no unique Camera3d entity with a PositionSplit \
             (the camera is not yet spawned, or there is more than one)",
        ));
    };
    *transform = pose;
    *split = PositionSplit::from_world(pose.translation);

    Ok(Value::Null)
}

// ===========================================================================
// World re-load (Phase 2 — demoted, design §3.1)
// ===========================================================================

/// `naadf/load_world` — instant main-world handler. Set the [`GridPreset`]
/// resource (`02-design.md` §3 table + §3.1).
///
/// **Demoted verb.** `GridPreset` is consumed by `setup_test_grid` at
/// `Startup`; a verb issued after the app is running cannot retroactively
/// re-run `Startup`. The 13 gates therefore load their fixture through the
/// `--vox` *spawn* flag (Forbidden Move #4 — boot-time config rides the spawn
/// contract), not this verb. `naadf/load_world` is kept only as a
/// schema-complete convenience: it mutates `Res<GridPreset>` so a future
/// re-runnable world-install path (or an interactive console) could pick it
/// up. It is **not on the critical path** for any Phase 2/3 gate.
///
/// Params: `{ vox_path?: string }`.
pub fn load_world(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: super::schema::LoadWorldParams = match params {
        Some(v) => serde_json::from_value(v)
            .map_err(|e| invalid_params(&format!("naadf/load_world bad params: {e}")))?,
        None => super::schema::LoadWorldParams::default(),
    };

    let preset = match p.vox_path {
        Some(path) if !path.is_empty() => crate::GridPreset::Vox {
            path: std::path::PathBuf::from(path),
        },
        _ => crate::GridPreset::Default,
    };
    world.insert_resource(preset);

    Ok(Value::Null)
}

// ===========================================================================
// Region gate (Phase 2)
// ===========================================================================

/// `naadf/region_gate` — instant main-world handler. Fractional-rect
/// statistics over the most recent capture (`02-design.md` §3).
///
/// Operates on the [`LastCapture`] framebuffer that [`await_capture`] populated
/// — the runner issues `naadf/capture` → `naadf/await_capture` →
/// `naadf/region_gate`. Wraps `Framebuffer::region_mean` + `Framebuffer::luminance`
/// (`e2e/framebuffer.rs`).
///
/// Params: `{ rect_fracs:[f32;4] }`. Returns: `{ mean_rgba:[f32;4], luminance:f32 }`.
pub fn region_gate(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("naadf/region_gate requires params"))?;
    let p: super::schema::RegionGateParams = serde_json::from_value(params)
        .map_err(|e| invalid_params(&format!("naadf/region_gate bad params: {e}")))?;

    let Some(last) = world.get_resource::<LastCapture>() else {
        return Err(internal_error(
            "naadf/region_gate: no LastCapture resource — install_brp_server \
             wiring bug",
        ));
    };
    let Some(fb) = last.0.as_ref() else {
        return Err(internal_error(
            "naadf/region_gate: no capture available — issue naadf/capture + \
             naadf/await_capture before naadf/region_gate",
        ));
    };

    let [fx0, fy0, fx1, fy1] = p.rect_fracs;
    let rect = Rect::from_fractional(fb, fx0, fy0, fx1, fy1);
    let mean = fb.region_mean(rect);
    let luminance = Framebuffer::luminance(mean);

    Ok(json!({
        "mean_rgba": mean,
        "luminance": luminance,
    }))
}

// ===========================================================================
// Window resize (Phase 2 — design D10)
// ===========================================================================

/// `naadf/resize_window` — instant main-world handler. Drive the primary
/// `Window`'s resolution (`02-design.md` §3 / D10).
///
/// Mutating `Window::resolution` triggers the same winit resize chain a
/// compositor-driven resize does — this replaces the legacy `--resize-test`
/// gate's machine-specific `hyprctl` path with a programmatic, platform-neutral
/// resize (design D10).
///
/// Params: `{ width:u32, height:u32 }`.
pub fn resize_window(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("naadf/resize_window requires params"))?;
    let p: super::schema::ResizeWindowParams = serde_json::from_value(params)
        .map_err(|e| invalid_params(&format!("naadf/resize_window bad params: {e}")))?;
    if p.width == 0 || p.height == 0 {
        return Err(invalid_params(
            "naadf/resize_window: width and height must both be non-zero",
        ));
    }

    let mut query = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    let Ok(mut window) = query.single_mut(world) else {
        return Err(internal_error(
            "naadf/resize_window: no unique PrimaryWindow entity",
        ));
    };
    window
        .resolution
        .set(p.width as f32, p.height as f32);

    Ok(Value::Null)
}

// ===========================================================================
// Demo-region voxel count (Phase 3a — small_edit_visual gate)
// ===========================================================================

/// `naadf/count_demo_voxels` — instant main-world handler. Count the non-empty
/// voxels in the `GridPreset::Default` demo embed region (Phase 3a — the
/// `small_edit_visual` gate's Mode-2 phantom-voxel signal).
///
/// ## Why this verb exists (Phase 3a migration finding)
///
/// The `small_edit_visual` gate's load-bearing Mode-2 check is "a single-voxel
/// `cube_brush(radius=1)` produces exactly +1 *non-empty voxel*". The
/// `naadf/apply_brush` verb returns `voxels_delta` — the change in
/// `WorldData::voxels_cpu` *length* — but `voxels_cpu` is a flat `u32` array
/// where one 4×4×4 voxel block is a 32-`u32` record (64 voxels, packed
/// 2-per-`u32`). Editing a previously-empty block allocates the whole 32-`u32`
/// record, so `voxels_delta` is `32` for a single new voxel — NOT the +1
/// non-empty-voxel signal the gate's `assert_small_edit_landed` Mode-2 check
/// needs. This verb wraps `e2e::small_edit_visual::count_non_empty_voxels` —
/// which decodes the three-layer chunk/block/voxel cells and counts genuine
/// non-empty voxels, scoped to the ~131k-voxel demo embed (the full
/// 4096³-voxel world would be ~8.5G iterations / multi-second) — so the test
/// body gets the exact pre/post count the legacy `apply_small_cube_edit` took.
///
/// Params: `null`. Returns: `{ count: u64 }`.
pub fn count_demo_voxels(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Some(wd) = world.get_resource::<WorldData>() else {
        return Err(internal_error(
            "naadf/count_demo_voxels: WorldData resource missing — the world \
             is not yet installed",
        ));
    };
    let count = crate::e2e::small_edit_visual::count_non_empty_voxels(wd);
    Ok(json!({ "count": count }))
}

// ===========================================================================
// Pipeline scan (Phase 2 — design §6.3 / D7)
// ===========================================================================

/// `naadf/pipeline_scan` — instant **main-world** handler. Report the
/// `PipelineCache` health scan (`02-design.md` §3 / §6.3 / D7).
///
/// ## Design correction — main-world, not render-world
///
/// Design §3 / D7 specified this as a `with_method_render` (render-world) verb
/// "because `PipelineCache` is a render-world resource." Phase 2 moves it to
/// the **main world** for two grounded reasons:
///
/// 1. **It reads the channel, not `PipelineCache`.** The render-world resource
///    in question is the `PipelineScanResult` `Arc<Mutex>` *cross-world
///    channel*, which D7 explicitly KEEPS. `scan_pipeline_errors_render_system`
///    (render-world, wired by `install_brp_server`) writes it; the channel's
///    other clone lives in the main world and carries the identical scan
///    result — that is the channel's entire purpose (`e2e/checks.rs` module
///    doc). `naadf/get_state` already reads that main-world clone. A
///    render-world verb here would read the *same* `Arc<Mutex>`, just from the
///    other end — zero behavioural difference, no direct `PipelineCache` read.
/// 2. **`bevy_remote`'s render-world HTTP server is on a fixed second port**
///    (`RemoteHttpPlugin` `render_port`, default `15703`, with **no builder to
///    override it** — `bevy_remote 0.19.0-rc.1` `http.rs:118`). A render-world
///    verb would force the runner onto a fixed port that collides between
///    concurrent gate test processes. Keeping the verb main-world lets every
///    gate use one OS-assigned free port for *all* its verbs.
///
/// So Phase 2 does not call `with_method_render` at all. The render-world scan
/// *system* still runs in the render world (it must — it reads `PipelineCache`
/// directly); only the *verb* that surfaces its result is main-world.
///
/// Params: `null`. Returns: `{ result: "ok" | <error string> }`.
pub fn pipeline_scan(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Some(scan) = world.get_resource::<crate::e2e::checks::PipelineScanResult>() else {
        return Err(internal_error(
            "naadf/pipeline_scan: PipelineScanResult not present — install_brp_server \
             did not insert the cross-world channel (Phase 2 wiring bug)",
        ));
    };
    let result = crate::e2e::checks::pipeline_scan_result(scan);
    let result_str = match result {
        Ok(()) => "ok".to_string(),
        Err(msg) => msg,
    };
    Ok(json!({ "result": result_str }))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a JSON-RPC `-32602 Invalid params` [`BrpError`].
fn invalid_params(message: &str) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

/// Build a JSON-RPC `-32603 Internal error` [`BrpError`].
fn internal_error(message: &str) -> BrpError {
    BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: message.to_string(),
        data: None,
    }
}
