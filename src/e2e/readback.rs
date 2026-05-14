//! Framebuffer readback — `Screenshot::primary_window()` + the
//! `ScreenshotCaptured` observer that stashes the captured `Image` into a
//! resource the driver's `DRAIN` state waits on (`e2e-render-test.md` §5).
//!
//! `Screenshot::primary_window()` reads back the **actual on-screen window
//! surface** — the exact composited output `naadf_final_blit_node` produced.
//! Capture is async (one or more frames later the renderer triggers
//! `ScreenshotCaptured`), so the driver polls [`E2eScreenshot`] over a bounded
//! `DRAIN` window (§5.2).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

/// The stashed captured framebuffer. `None` until the `ScreenshotCaptured`
/// observer fires; the driver's `DRAIN` state transitions to `ASSERT` as soon
/// as this is `Some`.
#[derive(Resource, Default)]
pub struct E2eScreenshot(pub Option<Image>);

/// Observer: store the captured `Image` into [`E2eScreenshot`].
///
/// `ScreenshotCaptured` derefs to its `Image`; the entity it fires on is the
/// screenshot entity, which Bevy despawns after the observer runs.
fn stash_screenshot(captured: On<ScreenshotCaptured>, mut stash: ResMut<E2eScreenshot>) {
    // Only stash the first capture — the driver shoots once (or, for the
    // Batch-6 consecutive-frame gate, the driver manages two shots explicitly).
    if stash.0.is_none() {
        stash.0 = Some(captured.image.clone());
    }
}

/// Spawn a `Screenshot::primary_window()` entity with the [`stash_screenshot`]
/// observer attached — the driver calls this once at the `SHOOT` state-step.
pub fn shoot_primary_window(commands: &mut Commands) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(stash_screenshot);
}
