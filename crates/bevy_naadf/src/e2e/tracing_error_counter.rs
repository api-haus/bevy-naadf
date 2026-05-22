//! `tracing::error!` event counter — installed via Bevy 0.19's
//! [`bevy_log::LogPlugin::custom_layer`] hook.
//!
//! web-vox-async-loading 2026-05-18 follow-up Step 8 / Q5 design (see
//! `03-architecture.md` § Q5 "tracing::error! counter"). Asserts no
//! `tracing::error!` calls fire during the `--vox-web-parity-loaded`
//! sub-mode of the new native gate.
//!
//! The [`PipelineScanResult`](super::checks::PipelineScanResult) only counts
//! `bevy_render::PipelineCache` errors — shader compile, bind-group
//! validation. `tracing::error!` invocations from anywhere else
//! (system-side error logs, async-task failures, brush errors, etc.) are
//! not counted by that mechanism. The new gate folds BOTH metrics into its
//! verdict.
//!
//! ## Mechanism
//!
//! 1. A static [`AtomicUsize`] [`TRACING_ERROR_COUNT`] lives at module
//!    scope. Custom `tracing` `Layer`s registered through `LogPlugin`
//!    cannot capture from a closure (the hook is `fn(&mut App) -> ...`,
//!    not `Fn(...)`), so the counter must be reachable through a static.
//! 2. [`CountingLayer`] implements [`tracing_subscriber::Layer`] for the
//!    Bevy default `Registry` subscriber. Its `on_event` checks the event
//!    metadata level and `fetch_add(1)`s on `Level::ERROR`.
//! 3. [`vox_web_parity_log_layer`] is the `LogPlugin::custom_layer` hook
//!    that returns the layer wrapped in a `BoxedLayer`. Used by
//!    [`crate::e2e::vox_web_parity::run_vox_web_parity_loaded_phase`].
//! 4. [`tracing_error_count`] reads the counter — called by the gate's
//!    compare phase to fold zero-errors-fired into the verdict.
//!
//! ## Resource — [`TracingErrorCounter`]
//!
//! Initialised on the BRP-driven e2e SUT (the `e2e_brp` install path) so any
//! system — and the `naadf/get_state` verb — can read the current count of
//! `tracing::error!` events fired during the run.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use bevy::log::BoxedLayer;
use tracing::{Level, Subscriber};
use tracing_subscriber::Layer;

/// Process-global tracing-error counter. Incremented by [`CountingLayer`]
/// on every `tracing::Event` at `Level::ERROR`.
///
/// **Process-global** because Bevy's `LogPlugin::custom_layer` hook is a
/// `fn(&mut App) -> Option<BoxedLayer>` (function pointer, not closure) —
/// the layer cannot capture per-app state. The e2e harness is one-shot
/// (one process = one app), so the static is effectively the app's
/// counter; tests that boot multiple apps in-process would observe stale
/// counts but no e2e gate does that.
pub static TRACING_ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Reset the static counter to zero. Called by the e2e startup wiring so
/// the loaded-phase doesn't inherit error events from a prior test in the
/// same process.
pub fn reset_tracing_error_count() {
    TRACING_ERROR_COUNT.store(0, Ordering::Release);
}

/// Read the current static counter value. Called by the compare phase to
/// fold "any tracing-error fired?" into the gate's verdict.
pub fn tracing_error_count() -> usize {
    TRACING_ERROR_COUNT.load(Ordering::Acquire)
}

/// Tracing `Layer` that increments [`TRACING_ERROR_COUNT`] on every
/// `Level::ERROR` event. Registered via Bevy's
/// [`bevy_log::LogPlugin::custom_layer`] hook.
pub struct CountingLayer;

impl<S: Subscriber> Layer<S> for CountingLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() == Level::ERROR {
            TRACING_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Bevy `LogPlugin::custom_layer` hook implementation — returns the
/// [`CountingLayer`] boxed as a [`BoxedLayer`]. Use as:
///
/// ```ignore
/// app.add_plugins(DefaultPlugins.set(bevy::log::LogPlugin {
///     custom_layer: vox_web_parity_log_layer,
///     ..default()
/// }));
/// ```
///
/// Counter starts at zero on every process boot (static initializer).
pub fn vox_web_parity_log_layer(_app: &mut App) -> Option<BoxedLayer> {
    Some(Box::new(CountingLayer))
}

/// Bevy `Resource` mirror of the static counter. Reading this resource is
/// equivalent to calling [`tracing_error_count`] — provided so Bevy systems
/// can use the idiomatic `Res<TracingErrorCounter>` pattern.
#[derive(Resource, Default, Clone, Copy)]
pub struct TracingErrorCounter;

impl TracingErrorCounter {
    /// Read the underlying static counter.
    pub fn get(&self) -> usize {
        tracing_error_count()
    }
}
