//! Async `.vox` parse pump — shared between native and web targets.
//!
//! Provides [`PendingVoxParse`] (Bevy `Resource`) + [`poll_pending_vox_parse`]
//! (`Update` system) for the off-main-thread parse stage. The web and native
//! paths diverge only in how the parse work is spawned:
//!
//! - **Native** ([`spawn_native_vox_parse`]) uses
//!   [`bevy::tasks::AsyncComputeTaskPool::get().spawn(...)`], which on
//!   desktop runs on a worker thread.
//! - **Web** (via `voxel::web_vox`'s `spawn_wasm_vox_parse`) uses
//!   [`rayon::spawn`] against the `wasm-bindgen-rayon` worker pool spawned
//!   by `wasmBindings.initThreadPool` in `init.js.template` /
//!   `init-wasm-rayon.mjs`. Result is delivered through a
//!   [`crossbeam_channel`] bounded(1) pair.
//!
//! Both targets share the same `Resource` type — the `inner` field is
//! cfg-gated so the polling system can drive both via a uniform API.
//!
//! See `docs/orchestrate/web-vox-async-loading/03-architecture.md` Q1 + Q2
//! for the design rationale.

use bevy::prelude::*;

use crate::voxel::grid::{install_imported_vox, parse_to_imported_vox};
use crate::voxel::vox_import::ImportedVox;

/// The async parse pump's hand-off resource. **Cfg-gated** — the
/// `inner` field carries a Bevy `Task<...>` on native (where
/// `AsyncComputeTaskPool::spawn` returns a `Task`) and a
/// `crossbeam_channel::Receiver<...>` on web (where `rayon::spawn` is
/// fire-and-forget, so we route results through a channel).
///
/// The result payload `(ImportedVox, String)` carries the parsed model +
/// the original source label (path / URL / `<dropped:...>`) so the
/// post-parse install pass can log it.
#[derive(Resource, Default)]
pub struct PendingVoxParse {
    /// Inner state. `None` means no parse is in flight.
    pub inner: Option<PendingVoxParseInner>,
}

/// Cfg-gated inner state — the actual async hand-off shape.
#[cfg(not(target_arch = "wasm32"))]
pub struct PendingVoxParseInner {
    /// The Bevy `AsyncComputeTaskPool::spawn` task. Poll via `block_on +
    /// poll_once` in [`poll_pending_vox_parse`].
    pub task: bevy::tasks::Task<ParseResult>,
    /// Wall-clock budget tracking for diagnostic-bail per
    /// `feedback-e2e-gates-must-fail-fast.md`. The polling system logs a
    /// warning if the task is still pending after `PARSE_BUDGET_SECS`.
    pub started_at: std::time::Instant,
    /// Source label for diagnostic logging.
    pub source_label: String,
}

#[cfg(target_arch = "wasm32")]
pub struct PendingVoxParseInner {
    /// crossbeam receiver wired to a `rayon::spawn` worker. Drained via
    /// `try_recv()` in [`poll_pending_vox_parse`].
    pub rx: crossbeam_channel::Receiver<ParseResult>,
    /// Source label for diagnostic logging.
    pub source_label: String,
}

/// The parse-result payload — `Ok((imp, source_label))` on success,
/// `Err(message)` on parse failure.
pub type ParseResult = Result<(ImportedVox, String), String>;

/// Wall-clock budget for the parse stage (native + web). The polling
/// system emits an `error!` log + drops the task if the parse exceeds
/// this without delivering. Per the architect's Q5 design (60s parse,
/// 30s readback). Generous because the Oasis fixture is 85 MB and the
/// sparse-walk parse is single-threaded.
pub const PARSE_BUDGET_SECS: u64 = 60;

/// `Update` system — drains the [`PendingVoxParse`] hand-off and runs
/// [`install_imported_vox`] when the parse completes. Cfg-gated body so
/// native and web each drive their target-specific async primitive
/// (`Task<...>` vs `crossbeam_channel::Receiver<...>`) through the same
/// resource type + system slot.
pub fn poll_pending_vox_parse(
    mut commands: Commands,
    mut pending: ResMut<PendingVoxParse>,
) {
    let Some(inner) = pending.inner.as_mut() else {
        return;
    };

    #[cfg(not(target_arch = "wasm32"))]
    {
        use bevy::tasks::block_on;
        use bevy::tasks::futures_lite::future;

        // Wall-clock budget check — if the task has been pending for
        // longer than PARSE_BUDGET_SECS, log + drop.
        let elapsed = inner.started_at.elapsed();
        if elapsed.as_secs() >= PARSE_BUDGET_SECS {
            error!(
                ".vox async parse exceeded {} s budget (source: {}) — dropping \
                 task; the parse worker likely deadlocked or hit an internal panic. \
                 The current scene stays as-is.",
                PARSE_BUDGET_SECS,
                inner.source_label,
            );
            pending.inner = None;
            return;
        }

        if let Some(result) = block_on(future::poll_once(&mut inner.task)) {
            let source_label = std::mem::take(&mut inner.source_label);
            pending.inner = None;
            match result {
                Ok((imp, label)) => install_imported_vox(&mut commands, imp, &label),
                Err(e) => error!(
                    ".vox async parse failed (source: {source_label}): {e}; \
                     current scene stays as-is."
                ),
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        match inner.rx.try_recv() {
            Ok(result) => {
                let source_label = std::mem::take(&mut inner.source_label);
                pending.inner = None;
                match result {
                    Ok((imp, label)) => install_imported_vox(&mut commands, imp, &label),
                    Err(e) => error!(
                        ".vox async parse failed (source: {source_label}): {e}; \
                         current scene stays as-is."
                    ),
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                // Still parsing — poll again next frame.
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                // The rayon worker dropped its sender without sending a
                // result — most likely a panic in the parse. Clear the
                // slot so future drops can retry.
                let source_label = std::mem::take(&mut inner.source_label);
                error!(
                    ".vox async parse worker disconnected (source: {source_label}) \
                     — the rayon task likely panicked. Check the JS console for \
                     details. Current scene stays as-is."
                );
                pending.inner = None;
            }
        }
    }
}

/// **Native only** — spawn a `.vox` parse off the main thread via
/// `AsyncComputeTaskPool::spawn`. The path is `read`-ed inside the task so
/// the I/O blocking also happens off-thread.
///
/// `commands.insert_resource(PendingVoxParse { inner: Some(...) })`
/// overwrites any in-flight parse, so a second startup call or a
/// drag-dropped file while a previous parse is still running replaces the
/// pending work (matches the web inbox's "last-writer-wins" semantic at
/// `web_vox.rs:60-64`).
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_native_vox_parse(commands: &mut Commands, path: std::path::PathBuf) {
    let source_label = path.display().to_string();
    let label_for_task = source_label.clone();
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let task = pool.spawn(async move {
        let bytes = std::fs::read(&path).map_err(|e| format!("read failed: {e}"))?;
        let imp = parse_to_imported_vox(&bytes)?;
        Ok((imp, label_for_task))
    });
    commands.insert_resource(PendingVoxParse {
        inner: Some(PendingVoxParseInner {
            task,
            started_at: std::time::Instant::now(),
            source_label,
        }),
    });
}

/// **Native only** — spawn a `.vox` parse off the main thread, given the
/// bytes already in memory. Used by `native_vox_drop_listener` where the
/// user drag-dropped a file and the bytes were `std::fs::read`-ed
/// synchronously (TODO: move the read off-thread too — currently the
/// drop handler blocks while reading, then dispatches the parse async).
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_native_vox_parse_from_bytes(
    commands: &mut Commands,
    bytes: Vec<u8>,
    source_label: String,
) {
    let label_for_task = source_label.clone();
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let task = pool.spawn(async move {
        let imp = parse_to_imported_vox(&bytes)?;
        Ok((imp, label_for_task))
    });
    commands.insert_resource(PendingVoxParse {
        inner: Some(PendingVoxParseInner {
            task,
            started_at: std::time::Instant::now(),
            source_label,
        }),
    });
}
