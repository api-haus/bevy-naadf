//! Scenario helper layer — high-level operations a gate test body composes
//! (`02-design.md` §7.1 `scenario.rs`).
//!
//! Each helper wraps one or more `naadf/*` BRP calls into the shape the legacy
//! `e2e/driver.rs` phase machine expressed inline: warm up N frames, capture a
//! framebuffer, apply a brush, set the camera, run a region gate. The helpers
//! take `&mut BrpClient` and return typed results / `Framebuffer`s decoded from
//! `bevy_naadf::e2e::framebuffer` — the *pure* assertion code stays in
//! `bevy_naadf`, the runner only orchestrates.

use base64::Engine as _;
use serde_json::{json, Value};

use bevy_naadf::e2e::framebuffer::Framebuffer;
use bevy_naadf::e2e_brp::schema;

use crate::client::{BrpClient, BrpClientError, BrpResult};

/// Advance the SUT by exactly `frames` rendered frames, then block until it is
/// at rest. Maps the legacy driver's "count a fixed frame budget then ASSERT"
/// onto the two BRP primitives: `naadf/step` queues the budget,
/// `naadf/run_until_idle` blocks until the budget has elapsed and the SUT has
/// been idle for `idle_frames` consecutive frames.
///
/// `max_frames` (the `run_until_idle` hard ceiling) is set to `frames` plus a
/// generous margin so a hung SUT fails fast rather than blocking the whole
/// test.
pub fn advance(c: &mut BrpClient, frames: u32) -> BrpResult<()> {
    advance_with_idle(c, frames, 8)
}

/// Advance the SUT by exactly one rendered frame, then block until it is back
/// at rest. The thin per-frame primitive a gate body uses when it must change
/// SUT state (camera pose, …) *between individual frames* — e.g. the
/// `standard` gate's camera-motion sweep, which the legacy in-app driver drove
/// one `Update` tick at a time.
///
/// `idle_frames` is `1` (not the [`advance`] default of `8`): the caller is
/// stepping frame-by-frame and re-issues `naadf/*` calls every iteration, so a
/// long multi-frame settle window per step would multiply the round-trip count
/// for no benefit — one idle frame is enough to confirm the single stepped
/// frame elapsed.
pub fn advance_one_frame(c: &mut BrpClient) -> BrpResult<()> {
    advance_with_idle(c, 1, 1)
}

/// Like [`advance`] but with an explicit `idle_frames` settle count.
pub fn advance_with_idle(c: &mut BrpClient, frames: u32, idle_frames: u32) -> BrpResult<()> {
    c.call("naadf/step", json!({ "frames": frames }))?;
    let max_frames = frames.saturating_add(frames / 2).saturating_add(120);
    let result: schema::RunUntilIdleResult = c.call_typed(
        "naadf/run_until_idle",
        json!({ "max_frames": max_frames, "idle_frames": idle_frames }),
    )?;
    if result.timed_out {
        return Err(BrpClientError::Protocol(format!(
            "advance({frames}) — run_until_idle hit its {max_frames}-frame ceiling \
             without the SUT settling (frame {}); the SUT may be stuck",
            result.frame
        )));
    }
    Ok(())
}

/// Fetch the SUT status snapshot (`naadf/get_state`).
pub fn get_state(c: &mut BrpClient) -> BrpResult<schema::GetStateResult> {
    c.call_typed("naadf/get_state", Value::Null)
}

/// Capture the current framebuffer: issue `naadf/capture`, then block on
/// `naadf/await_capture` and decode the base64 PNG into a [`Framebuffer`] via
/// the *library's* pure decode path (`Framebuffer::from_raw_rgba`).
pub fn capture(c: &mut BrpClient) -> BrpResult<Framebuffer> {
    c.call("naadf/capture", Value::Null)?;
    // `max_frames` is the watch's hard fail-fast ceiling, measured in SUT
    // frames. The SUT renders at hundreds of FPS, so a generous ceiling is
    // still sub-second wall-time; the screenshot itself delivers within a
    // handful of frames (the legacy `OASIS_DRAIN_FRAMES` was 16).
    let shot: schema::AwaitCaptureResult =
        c.call_typed("naadf/await_capture", json!({ "max_frames": 2000 }))?;
    decode_png_b64(&shot.png_b64, shot.width, shot.height)
}

/// Decode a base64-encoded PNG into a [`Framebuffer`].
pub fn decode_png_b64(png_b64: &str, width: u32, height: u32) -> BrpResult<Framebuffer> {
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .map_err(|e| BrpClientError::Protocol(format!("base64 decode of capture: {e}")))?;
    let img = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
        .map_err(|e| BrpClientError::Protocol(format!("PNG decode of capture: {e}")))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w != width || h != height {
        return Err(BrpClientError::Protocol(format!(
            "captured PNG is {w}x{h} but the verb reported {width}x{height}"
        )));
    }
    let data: Vec<[u8; 4]> = rgba
        .pixels()
        .map(|p| [p.0[0], p.0[1], p.0[2], p.0[3]])
        .collect();
    Ok(Framebuffer::from_raw_rgba(data, w, h))
}

/// Pin the SUT camera (`naadf/set_camera`). `up` defaults to `+Y` when `None`.
pub fn set_camera(
    c: &mut BrpClient,
    translation: [f32; 3],
    look_at: [f32; 3],
    up: Option<[f32; 3]>,
) -> BrpResult<()> {
    let mut params = json!({
        "translation": translation,
        "look_at": look_at,
    });
    if let Some(up) = up {
        params["up"] = json!(up);
    }
    c.call("naadf/set_camera", params).map(|_| ())
}

/// Apply an erase-sphere brush (`naadf/apply_brush`, `kind: "sphere"`,
/// `erase: true`) — the load-bearing runtime edit path. Returns the
/// producer-side deltas.
pub fn erase_sphere(
    c: &mut BrpClient,
    pos: [f32; 3],
    radius: f32,
) -> BrpResult<schema::ApplyBrushResult> {
    c.call_typed(
        "naadf/apply_brush",
        json!({
            "kind": "sphere",
            "pos": pos,
            "radius": radius,
            "voxel_type": 0,
            "erase": true,
        }),
    )
}

/// Run a fractional-rect region gate over the most recent capture
/// (`naadf/region_gate`).
pub fn region_gate(c: &mut BrpClient, rect_fracs: [f32; 4]) -> BrpResult<schema::RegionGateResult> {
    c.call_typed("naadf/region_gate", json!({ "rect_fracs": rect_fracs }))
}

/// Scan the SUT's render-world `PipelineCache` health (`naadf/pipeline_scan`).
/// Returns `Ok(())` when every pipeline reached its terminal `Ok` state; an
/// error carrying the failure description otherwise.
pub fn pipeline_scan(c: &mut BrpClient) -> BrpResult<()> {
    let scan: schema::PipelineScanResult = c.call_typed("naadf/pipeline_scan", Value::Null)?;
    if scan.is_ok() {
        Ok(())
    } else {
        Err(BrpClientError::Protocol(format!(
            "pipeline scan reported failures: {}",
            scan.result
        )))
    }
}

/// Count the non-empty voxels in the `GridPreset::Default` demo embed
/// (`naadf/count_demo_voxels`). The `small_edit_visual` gate's Mode-2
/// phantom-voxel signal — `apply_brush`'s `voxels_delta` measures `voxels_cpu`
/// *array length* (32 `u32`s per touched 4×4×4 block), not non-empty voxels.
pub fn count_demo_voxels(c: &mut BrpClient) -> BrpResult<u64> {
    let r: schema::CountDemoVoxelsResult = c.call_typed("naadf/count_demo_voxels", Value::Null)?;
    Ok(r.count)
}

/// Resize the SUT window (`naadf/resize_window`).
pub fn resize_window(c: &mut BrpClient, width: u32, height: u32) -> BrpResult<()> {
    c.call("naadf/resize_window", json!({ "width": width, "height": height }))
        .map(|_| ())
}
