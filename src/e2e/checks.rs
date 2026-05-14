//! Batch-agnostic checks: the load-bearing `PipelineCache` error-state scan
//! (`e2e-render-test.md` §3.1) and the render-graph node-dispatch check (§8).
//!
//! ## Where these run — and why not "post-`app.run()`"
//!
//! The design's §9/§11-step-7 sketch had `run_e2e_render` running these *after*
//! `app.run()` returns. That does not work with the real `WinitPlugin`:
//! `App::run()` does `core::mem::replace(self, App::empty())` — it **moves the
//! `App` into the winit runner and leaves an empty `App` behind**; the winit
//! runner consumes it and never hands it back. So there is no `App` to inspect
//! post-run. The design anticipated exactly this and offered the alternative in
//! §6.5: run the checks *inside* the app, in the `ASSERT` step.
//!
//! - **Node-dispatch check** — `DiagnosticsStore` is a *main-world* resource
//!   (`FrameTimeDiagnosticsPlugin` / `DiagnosticsPlugin` register it there;
//!   `RenderDiagnosticsPlugin`'s `sync_diagnostics` copies the render-node
//!   spans into it each frame). So [`assert_nodes_dispatched`] runs as a plain
//!   main-world read in the driver's `ASSERT` step.
//! - **`PipelineCache` error scan** — `PipelineCache` is a *render-world*
//!   resource a main-world system cannot reach. So a render-world system
//!   ([`scan_pipeline_errors_render_system`]) scans it every frame and writes
//!   the result through a shared `Arc<Mutex<…>>` resource ([`PipelineScanResult`])
//!   that is inserted into **both** worlds (the same cross-world-channel pattern
//!   `RenderDiagnosticsPlugin` itself uses for `RenderDiagnosticsMutex`). The
//!   driver's `ASSERT` step reads the latest result from the main-world handle.
//!   By the time `ASSERT` runs (frame `E2E_WARMUP_FRAMES + E2E_MOTION_FRAMES +
//!   E2E_SETTLE_FRAMES + …`) the cache is long settled and
//!   `synchronous_pipeline_compilation` guarantees every queued pipeline has
//!   reached its terminal `Ok`/`Err` state.

use std::sync::{Arc, Mutex};

use bevy::diagnostic::{DiagnosticPath, DiagnosticsStore};
use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};

/// A cross-world channel carrying the latest `PipelineCache` scan result.
///
/// Inserted (cloned) into both the main world and the `RenderApp` —
/// the render-world system writes, the main-world `ASSERT` step reads.
/// `None` = the scan has not run yet; `Some(Ok)` / `Some(Err)` = the latest
/// terminal-state scan.
#[derive(Resource, Clone, Default)]
pub struct PipelineScanResult(pub Arc<Mutex<Option<Result<(), String>>>>);

/// Render-world system: scan every pipeline in `PipelineCache` for the `Err`
/// terminal state — the **main** shader/pipeline/bind-group error check
/// (`e2e-render-test.md` §3.1).
///
/// naga-oil composition failures, WGSL validation failures,
/// bind-group-layout/pipeline-layout mismatches, and `ptr<storage>`-param
/// rejections all land in `CachedPipelineState::Err` at pipeline-creation time.
/// This catches the entire `10-impl-b.md` shader-bug catalogue in a single run
/// — unlike the old live smoke-run that aborted on the *first* bad shader.
///
/// Runs every render frame and overwrites the shared result; the driver reads
/// the *latest* one at `ASSERT`, by which point the cache is settled.
pub fn scan_pipeline_errors_render_system(
    cache: Res<PipelineCache>,
    result: Res<PipelineScanResult>,
) {
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut not_ready = 0usize;
    for cached in cache.pipelines() {
        total += 1;
        match &cached.state {
            CachedPipelineState::Err(err) => {
                let label = pipeline_label(&cached.descriptor);
                failures.push(format!("  [{label}] {err}"));
            }
            CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
                not_ready += 1;
            }
            CachedPipelineState::Ok(_) => {}
        }
    }

    let scan = if !failures.is_empty() {
        Err(format!(
            "{} of {total} pipeline(s) FAILED to create:\n{}",
            failures.len(),
            failures.join("\n")
        ))
    } else if not_ready > 0 {
        // With synchronous compilation a still-`Queued` pipeline means a node
        // never ran (its upstream resources never appeared) — the frame budget
        // was too short or a conditional node's condition was false. That is a
        // real failure: the scan would otherwise pass blind. (Only treated as
        // failure once the cache has been non-empty — see the driver, which
        // only reads this after the frame budget.)
        Err(format!(
            "{not_ready} of {total} pipeline(s) still Queued/Creating — a render node never \
             ran (frame budget too short, or a conditional node's condition was false in the \
             e2e scene). Bump E2E_WARMUP_FRAMES or check the scene."
        ))
    } else if total == 0 {
        Err(
            "PipelineCache holds zero pipelines — no render node ever queued one (the render \
             graph never executed)"
                .to_string(),
        )
    } else {
        Ok(())
    };

    if let Ok(mut slot) = result.0.lock() {
        *slot = Some(scan);
    }
}

/// Extract a human-readable label from a `PipelineDescriptor` for the error
/// list — falls back to the variant kind if the descriptor has no label.
fn pipeline_label(descriptor: &PipelineDescriptor) -> String {
    match descriptor {
        PipelineDescriptor::RenderPipelineDescriptor(d) => d
            .label
            .as_ref()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "<unlabelled render pipeline>".to_string()),
        PipelineDescriptor::ComputePipelineDescriptor(d) => d
            .label
            .as_ref()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "<unlabelled compute pipeline>".to_string()),
    }
}

/// Read the latest `PipelineCache` scan result from the shared channel — called
/// by the driver at `ASSERT`. `Err` if the render-world scan never produced a
/// result (the render schedule never ran).
pub fn pipeline_scan_result(result: &PipelineScanResult) -> Result<(), String> {
    match result.0.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(r) => r.clone(),
            None => Err(
                "PipelineCache scan never ran — the RenderApp `Render` schedule did not \
                 execute the scan system (no render frames produced?)"
                    .to_string(),
            ),
        },
        Err(_) => Err("PipelineScanResult mutex poisoned".to_string()),
    }
}

/// Assert that every expected render-graph span for the current batch has a
/// recorded measurement — i.e. the node actually ran (`e2e-render-test.md` §8).
///
/// The nodes wrap their work in a `time_span(encoder, SPAN)`;
/// `RenderDiagnosticsPlugin` surfaces each as `render/<span>/elapsed_cpu` (and
/// `_gpu`), and its `sync_diagnostics` system copies them into the *main-world*
/// `DiagnosticsStore` each frame. A node that early-returns because its
/// pipeline failed records *no* span — so a missing span is a second, cheaper
/// "the node never ran" signal complementing the pipeline-error scan.
///
/// Checks `elapsed_cpu` (recorded unconditionally — `RenderDiagnosticsPlugin`
/// records it even on backends without timestamp queries).
pub fn assert_nodes_dispatched(
    diagnostics: &DiagnosticsStore,
    expected_spans: &[&str],
) -> Result<(), String> {
    let mut missing: Vec<String> = Vec::new();
    for span in expected_spans {
        let cpu_path = format!("render/{span}/elapsed_cpu");
        let has_measurement = diagnostics
            .get(&DiagnosticPath::new(cpu_path.clone()))
            .map(|d| d.measurement().is_some())
            .unwrap_or(false);
        if !has_measurement {
            missing.push(format!("  {span} (no `{cpu_path}` measurement)"));
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} expected render-graph node(s) never dispatched:\n{}",
            missing.len(),
            missing.join("\n")
        ))
    }
}
