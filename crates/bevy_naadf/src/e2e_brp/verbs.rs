//! The custom `naadf/*` BRP verbs — the project's domain control surface for
//! the external e2e runner.
//!
//! Entirely behind the `e2e-brp` cargo feature (the `e2e_brp` module is
//! `#[cfg]`-gated at the `lib.rs` `pub mod` site). Each verb is an ordinary
//! Bevy system with the BRP handler shape — `fn(In(Option<Value>), &mut World)
//! -> BrpResult` for *instant* methods, `fn(In(Option<Value>), &mut World) ->
//! BrpResult<Option<Value>>` for *watching* methods (verified against
//! `bevy_remote 0.19.0-rc.1` `src/lib.rs:591-666`).
//!
//! ## Phase 1 scope
//!
//! Only the three verbs the design (`02-design.md` §3 / §4) puts in Phase 1:
//!
//! - [`step`] — `naadf/step`, instant: queue N frames of advancement.
//! - [`run_until_idle`] — `naadf/run_until_idle`, watching: stream one final
//!   chunk once the SUT has been at rest for `idle_frames` consecutive frames
//!   (or `max_frames` elapsed).
//! - [`get_state`] — `naadf/get_state`, instant: a small status snapshot.
//!
//! The other 8 verbs from the design (`naadf/capture`, `naadf/apply_brush`,
//! `naadf/pipeline_scan`, …) are Phase 2 — they are **not** in this module yet.

use bevy::prelude::*;
use bevy::remote::{error_codes, BrpError, BrpResult};
use serde_json::{json, Value};

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
    /// [`advance_e2e_control`]. The runner reads this (`naadf/get_state`,
    /// `naadf/step`'s return value) to know how far the SUT has advanced.
    pub frame: u64,
    /// Logical step budget. `naadf/step { frames: N }` adds `N`;
    /// [`advance_e2e_control`] saturating-decrements it each `Update`. The SUT
    /// is "running" while `> 0` and "at rest" at `0`.
    pub frames_remaining: u32,
}

/// Advance [`E2eControl`] once per `Update` (`02-design.md` §4.1).
///
/// Registered by [`super::install_brp_server`] in the `Update` schedule. The
/// app is `WinitSettings::Continuous`, so this runs every real winit-paced
/// frame — every counted frame is a genuine rendered frame, identical to how
/// the legacy in-app driver counted ticks (`E2eState.phase_ticks`), just with
/// the orchestration moved out-of-process.
pub fn advance_e2e_control(mut control: ResMut<E2eControl>) {
    control.frame += 1;
    control.frames_remaining = control.frames_remaining.saturating_sub(1);
}

// --- naadf/step -----------------------------------------------------------

/// `naadf/step` — instant main-world handler. Queue `frames` frames of
/// advancement (`02-design.md` §4.2).
///
/// It does **not** loop the schedule (pumping `Update` from inside
/// `RemoteLast` would re-enter the schedule mid-frame and decouple the
/// rendered frames from the logical step count — the design's D3 rejection).
/// It simply adds `frames` to [`E2eControl::frames_remaining`] and returns the
/// *current* frame number. The runner then waits — via `naadf/run_until_idle`
/// or by polling `naadf/get_state` — for the SUT to have advanced that many
/// frames before its next assertion.
///
/// Params: `{ frames: u32 }`. Returns: `{ frame: u64 }` (frame count *now*,
/// before the queued frames elapse).
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

// --- naadf/run_until_idle -------------------------------------------------

/// `naadf/run_until_idle` — watching main-world handler. The deterministic
/// "advance then assert" primitive (`02-design.md` §4.3).
///
/// `process_ongoing_watching_requests` (`bevy_remote/src/lib.rs:1427`) re-runs
/// this handler every frame for as long as the request is open. The contract
/// (verified against `bevy_remote 0.19.0-rc.1` `src/lib.rs:1431-1435`):
///
/// - `Ok(None)` — no message sent this frame; the runner keeps blocking.
/// - `Ok(Some(value))` — `value` is delivered as the next SSE chunk.
/// - `Err(_)` — an error chunk is delivered.
///
/// Semantics: return `Ok(None)` every frame while the SUT is *running*
/// (`frames_remaining > 0`) or has not yet been idle long enough; once
/// `frames_remaining == 0` has held for `idle_frames` consecutive frames — or
/// `max_frames` total frames have elapsed since the watch began — return one
/// `Ok(Some({ done: true, frame, timed_out }))`. The runner's blocking SSE
/// read resolves on that single chunk and closes the connection;
/// `remove_closed_watching_requests` then drops the watch.
///
/// `max_frames` is a hard ceiling so a hung SUT fails fast rather than
/// streaming `Ok(None)` forever (project memory: e2e gates must fail fast).
///
/// Params: `{ max_frames: u32, idle_frames: u32 }`. Final chunk:
/// `{ done: true, frame: u64, timed_out: bool }`.
///
/// ## Per-watch state
///
/// A watching handler has no per-request storage in the `World`, so the
/// "frames consecutively idle" / "frames since watch began" counters live in
/// the [`RunUntilIdleWatch`] resource, keyed by the *frame the watch first
/// ran*. Phase 1's runner issues one `run_until_idle` at a time (it is
/// synchronous test code), so a single-slot resource is sufficient and
/// correct; a concurrent-watch design is out of Phase 1 scope. If the handler
/// observes a *different* watch already in flight it resets the slot to its
/// own watch — last-writer-wins, which is benign for the one-at-a-time runner.
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

    // First run of *this* watch (or a fresh watch superseding a stale slot):
    // anchor the budget to the current frame and start the idle streak at 0.
    let elapsed = match watch.started_at_frame {
        Some(start) if frame >= start => frame - start,
        _ => {
            watch.started_at_frame = Some(frame);
            watch.consecutive_idle = 0;
            0
        }
    };

    // Track the consecutive-idle streak.
    if frames_remaining == 0 {
        watch.consecutive_idle = watch.consecutive_idle.saturating_add(1);
    } else {
        watch.consecutive_idle = 0;
    }
    let consecutive_idle = watch.consecutive_idle;

    let settled = consecutive_idle as u64 >= idle_frames;
    let timed_out = elapsed >= max_frames;

    if settled || timed_out {
        // Clear the slot so the next `run_until_idle` watch anchors fresh.
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

/// Single-slot per-watch state for [`run_until_idle`] (see its doc comment).
/// Initialised by [`super::install_brp_server`].
#[derive(Resource, Default, Debug)]
pub struct RunUntilIdleWatch {
    /// The [`E2eControl::frame`] value at which the in-flight watch first ran;
    /// `None` when no watch is in flight. The `max_frames` budget is measured
    /// from here.
    pub started_at_frame: Option<u64>,
    /// How many consecutive frames `frames_remaining == 0` has held for the
    /// in-flight watch.
    pub consecutive_idle: u32,
}

// --- naadf/get_state ------------------------------------------------------

/// `naadf/get_state` — instant main-world handler. A small status snapshot
/// the runner polls (`02-design.md` §3 table).
///
/// Returns:
/// - `frame: u64` — the [`E2eControl`] monotonic frame counter.
/// - `frames_remaining: u32` — the logical step budget (0 ⇒ at rest).
/// - `world_loaded: bool` — whether a [`WorldData`](crate::world::data::WorldData)
///   resource is present (the voxel world has been installed).
/// - `pipeline_errors: string | null` — the main-world side of the
///   `PipelineScanResult` cross-world channel, when present; `null` in Phase 1
///   because that channel is wired by `add_e2e_systems` (off in the `e2e_sut`
///   profile) — the render-world `naadf/pipeline_scan` verb that feeds it is
///   Phase 2. `null` here means "not scanned", not "no errors".
/// - `tracing_errors: u64` — the process-global `tracing::error!` count
///   (`crate::e2e::tracing_error_counter`). Always readable: the counter is a
///   static, independent of the e2e-systems wiring.
///
/// Params: `null` (ignored).
pub fn get_state(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let control = world.resource::<E2eControl>();
    let frame = control.frame;
    let frames_remaining = control.frames_remaining;

    let world_loaded = world.contains_resource::<crate::world::data::WorldData>();

    // The `PipelineScanResult` channel is only present when `add_e2e_systems`
    // wired it; the `e2e_sut` profile leaves it off, so this is `null` in
    // Phase 1. Phase 2 moves the channel into `install_brp_server`'s setup.
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
        "pipeline_errors": pipeline_errors,
        "tracing_errors": tracing_errors,
    }))
}

// --- helpers --------------------------------------------------------------

/// Build a JSON-RPC `-32602 Invalid params` [`BrpError`].
fn invalid_params(message: &str) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}
