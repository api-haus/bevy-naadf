//! The `naadf/*` BRP verb wire schema — the param / return structs
//! (`02-design.md` §7.1, D8).
//!
//! ## Why this sub-module is compiled unconditionally
//!
//! Unlike [`super::verbs`] (the BRP handlers, behind `#[cfg(feature =
//! "e2e-brp")]`), this module is **always compiled** — it has no `bevy_remote`
//! dependency, only `serde`/`serde_json`. The external `naadf_e2e` runner crate
//! depends on `bevy_naadf` for these structs and the pure `e2e::framebuffer` /
//! `e2e::ssim` assertion code; if the schema were feature-gated the runner
//! would have to build `bevy_naadf` with `e2e-brp` (dragging in the whole
//! `hyper`/`async-io` HTTP-transport tail) just to name a param struct. Keeping
//! the schema here, ungated, is design decision D8 / assumption A7.
//!
//! ## Wire format
//!
//! Every verb's params and return travel as JSON over BRP's JSON-RPC 2.0
//! envelope. The handlers in [`super::verbs`] parse `serde_json::Value`
//! directly today (for tolerant error messages); these structs are the
//! single typed definition the runner serialises against and the handlers
//! *can* deserialise into where convenient. Field names here are the wire
//! names — they must match what the handlers read.
//!
//! All structs derive `Serialize + Deserialize`. `Default` is derived where a
//! verb has an all-optional param set.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// naadf/step
// ---------------------------------------------------------------------------

/// Params for `naadf/step` — queue `frames` frames of advancement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StepParams {
    /// Number of frames to add to the SUT's logical step budget.
    pub frames: u32,
}

/// Return of `naadf/step` — the SUT frame counter *now* (before the queued
/// frames elapse).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StepResult {
    /// `E2eControl::frame` at the moment the request was serviced.
    pub frame: u64,
}

// ---------------------------------------------------------------------------
// naadf/run_until_idle
// ---------------------------------------------------------------------------

/// Params for `naadf/run_until_idle` — the watching "advance then assert"
/// primitive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RunUntilIdleParams {
    /// Hard ceiling: stop the watch once this many frames have elapsed since
    /// it began, even if the SUT never reaches rest (fail-fast on a hung SUT).
    pub max_frames: u32,
    /// The SUT is declared "settled" once `frames_remaining == 0` has held for
    /// this many consecutive frames.
    pub idle_frames: u32,
}

/// The single final streamed chunk of `naadf/run_until_idle`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RunUntilIdleResult {
    /// Always `true` — the watch only emits one chunk, on completion.
    pub done: bool,
    /// `E2eControl::frame` at completion.
    pub frame: u64,
    /// `true` if the watch ended on the `max_frames` ceiling rather than the
    /// SUT genuinely settling.
    pub timed_out: bool,
}

// ---------------------------------------------------------------------------
// naadf/get_state
// ---------------------------------------------------------------------------

/// Return of `naadf/get_state` — a small status snapshot the runner polls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetStateResult {
    /// Monotonic SUT frame counter.
    pub frame: u64,
    /// Logical step budget (`0` ⇒ at rest).
    pub frames_remaining: u32,
    /// Whether a `WorldData` resource is present (the voxel world is installed).
    pub world_loaded: bool,
    /// World size in voxels `[x, y, z]` — `None` until `world_loaded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_size_voxels: Option<[u32; 3]>,
    /// The latest `PipelineCache` scan, when the render-world scan has run:
    /// `null` ⇒ not scanned, `""`/`"ok"`-shaped success or an error string.
    /// Carried as a free-form JSON value because the BRP handler emits either
    /// `Value::Null` or a `Value::String`.
    #[serde(default)]
    pub pipeline_errors: Option<String>,
    /// Process-global `tracing::error!` count.
    pub tracing_errors: u64,
}

// ---------------------------------------------------------------------------
// naadf/capture + naadf/await_capture
// ---------------------------------------------------------------------------

/// Return of `naadf/capture` — the screenshot entity was spawned; the actual
/// pixels arrive asynchronously and are collected via `naadf/await_capture`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CaptureResult {
    /// Always `true` — capture is now pending.
    pub pending: bool,
}

/// Params for `naadf/await_capture`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AwaitCaptureParams {
    /// Hard ceiling: give up after this many frames if the capture never
    /// delivers (fail-fast). `0` ⇒ use the verb's default ceiling.
    #[serde(default)]
    pub max_frames: u32,
}

/// The single final streamed chunk of `naadf/await_capture` — the decoded
/// framebuffer, base64-encoded as a PNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaitCaptureResult {
    /// Always `true` — the watch only emits one chunk, when the capture is
    /// ready (or on `Err` for a decode failure / timeout).
    pub ready: bool,
    /// Framebuffer width in physical pixels.
    pub width: u32,
    /// Framebuffer height in physical pixels.
    pub height: u32,
    /// The captured framebuffer as a base64-encoded standard sRGB RGB PNG.
    /// The runner base64-decodes this and feeds it to `image`/`Framebuffer`.
    pub png_b64: String,
}

// ---------------------------------------------------------------------------
// naadf/apply_brush
// ---------------------------------------------------------------------------

/// Brush kind for `naadf/apply_brush` — mirrors the three `editor::tools`
/// brush fns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrushKind {
    /// Euclidean solid sphere ([`crate::editor::tools::sphere_brush`]).
    Sphere,
    /// Chebyshev solid cube ([`crate::editor::tools::cube_brush`]).
    Cube,
    /// Replace existing non-empty voxels only ([`crate::editor::tools::paint_brush`]).
    Paint,
}

/// Params for `naadf/apply_brush` — a one-shot programmatic brush application.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ApplyBrushParams {
    /// Which brush shape to apply.
    pub kind: BrushKind,
    /// Brush centre in world voxel coordinates.
    pub pos: [f32; 3],
    /// Brush radius in voxels.
    pub radius: f32,
    /// Voxel type id to write (ignored when `erase` is `true`; for `paint`
    /// it is the replacement type).
    #[serde(default)]
    pub voxel_type: u32,
    /// Erase mode — writes `VoxelTypeId::EMPTY`. Ignored for `paint` (the
    /// paint brush has no erase mode in the C# source).
    #[serde(default)]
    pub erase: bool,
}

/// Return of `naadf/apply_brush` — the producer-side deltas the legacy oasis
/// gate logs (`oasis_edit_visual.rs` `apply_erase_brush`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ApplyBrushResult {
    /// Change in `WorldData::voxels_cpu` length.
    pub voxels_delta: i64,
    /// Change in `WorldData::blocks_cpu` length.
    pub blocks_delta: i64,
    /// Number of `pending_edits` batches the brush produced.
    pub batches: u32,
}

// ---------------------------------------------------------------------------
// naadf/set_camera
// ---------------------------------------------------------------------------

/// Params for `naadf/set_camera` — pin the `Camera3d` pose.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SetCameraParams {
    /// Camera world-space position.
    pub translation: [f32; 3],
    /// World-space point the camera looks at.
    pub look_at: [f32; 3],
    /// "Up" reference vector for the look-at basis. Defaults to `+Y`; the
    /// oasis birdseye pose uses `+X` (see `oasis_edit_visual::birdseye_pose`).
    #[serde(default = "default_up")]
    pub up: [f32; 3],
}

fn default_up() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

// ---------------------------------------------------------------------------
// naadf/load_world
// ---------------------------------------------------------------------------

/// Params for `naadf/load_world` — a runtime world re-load convenience
/// (design §3.1: most gates load their fixture via the `--vox` spawn flag, not
/// this verb). An absent / empty `vox_path` re-installs the default scene.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadWorldParams {
    /// Path to a `.vox` / `.cvox` fixture, or `None` for the default scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vox_path: Option<String>,
}

// ---------------------------------------------------------------------------
// naadf/region_gate
// ---------------------------------------------------------------------------

/// Params for `naadf/region_gate` — fractional-rect statistics over the most
/// recent capture.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RegionGateParams {
    /// Fractional screen rect `[fx0, fy0, fx1, fy1]` (each `0.0..=1.0`).
    pub rect_fracs: [f32; 4],
}

/// Return of `naadf/region_gate`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RegionGateResult {
    /// Mean RGBA over the rect, each channel `0.0..=255.0`.
    pub mean_rgba: [f32; 4],
    /// Rec.709 luminance of the rect mean (`0.0..=255.0`).
    pub luminance: f32,
}

// ---------------------------------------------------------------------------
// naadf/resize_window
// ---------------------------------------------------------------------------

/// Params for `naadf/resize_window` — drives the primary `Window`'s resolution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResizeWindowParams {
    /// New logical width.
    pub width: u32,
    /// New logical height.
    pub height: u32,
}

// ---------------------------------------------------------------------------
// naadf/count_demo_voxels
// ---------------------------------------------------------------------------

/// Return of `naadf/count_demo_voxels` — the non-empty voxel count of the
/// `GridPreset::Default` demo embed region (Phase 3a — the `small_edit_visual`
/// gate's Mode-2 phantom-voxel signal; see the verb handler doc for why
/// `apply_brush`'s `voxels_delta` is the wrong measure).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CountDemoVoxelsResult {
    /// Non-empty voxels in the demo embed (the full chunk/block/voxel cell
    /// decode, scoped to the ~131k-voxel demo region).
    pub count: u64,
}

// ---------------------------------------------------------------------------
// naadf/pipeline_scan
// ---------------------------------------------------------------------------

/// Return of `naadf/pipeline_scan` — the render-world `PipelineCache` health
/// scan. `result` is `"ok"` on success or carries the error string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineScanResult {
    /// `"ok"` when every pipeline reached its terminal `Ok` state; otherwise a
    /// human-readable failure description.
    pub result: String,
}

impl PipelineScanResult {
    /// Whether the scan reported a clean pipeline cache.
    pub fn is_ok(&self) -> bool {
        self.result == "ok"
    }
}
