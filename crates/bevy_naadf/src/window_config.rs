//! Window sizing/title knobs that `build_app` threads into the `WindowPlugin`
//! (`e2e-render-test.md` §9). The production config takes the platform
//! default; the e2e config pins a small fixed non-resizable window so the
//! framebuffer readback is fast and every `pixel_count`-sized buffer is
//! identical run-to-run (§4.2 determinism row).
//!
//! This module also owns the mode→window-config mapping used by
//! [`crate::run_e2e_render_with_args`] (the lib-root entry point that boots
//! the e2e binary). Each e2e mode constructor lives next to its peers here so
//! adding a mode is a one-file edit.
//!
//! ## Imports from `crate::e2e` — legitimate consumer, not a dep-arrow bug
//!
//! Each `WindowConfig::e2e_*` constructor reads its dimensions from the e2e
//! gate that boots through it (`crate::e2e::{E2E_WIDTH, E2E_HEIGHT,
//! E2E_RESIZE_BOOT_WIDTH, E2E_RESIZE_BOOT_HEIGHT,
//! vox_horizon_parity::HORIZON_WIDTH, vox_horizon_parity::HORIZON_HEIGHT,
//! small_edit_repro::SMALL_EDIT_REPRO_WIDTH,
//! small_edit_repro::SMALL_EDIT_REPRO_HEIGHT}`). These are legitimate consumer
//! imports: each constant's value is pinned by the gate's own contract
//! (Playwright viewport for the horizon gate, user-reported bug-report screen
//! size for `small-edit-repro`, fast-readback 256×256 framebuffer choice for
//! the standard e2e). Relocating them out of `e2e/` would orphan the constants
//! from the gate logic that defines them. The standalone
//! `WindowConfig::windowed()` constructor — the only one production calls —
//! reads zero e2e constants.

use crate::AppArgs;

/// Window sizing/title.
#[derive(Clone, Copy, Debug)]
pub struct WindowConfig {
    /// Logical resolution. `None` → the Bevy default (`Window::default`).
    pub resolution: Option<(f32, f32)>,
    /// Whether the window is user-resizable.
    pub resizable: bool,
    /// Window title.
    pub title: &'static str,
    /// Wayland `app_id` / X11 `WM_CLASS` (Bevy `Window.name`). `None` lets
    /// winit pick a default (usually the binary name). The resize-test config
    /// sets this explicitly so the hyprctl `class:...` selector is
    /// deterministic.
    pub name: Option<&'static str>,
}

impl WindowConfig {
    /// The production window — platform default size, resizable.
    pub fn windowed() -> Self {
        Self {
            resolution: None,
            resizable: true,
            title: "bevy-naadf",
            name: None,
        }
    }

    /// The e2e window — a small fixed 256×256 non-resizable window
    /// (`e2e-render-test.md` §4.2 / §9). 256² is large enough for stable
    /// region gates, small enough for a fast readback + cheap GI dispatch.
    pub fn e2e() -> Self {
        Self {
            resolution: Some((
                crate::e2e::E2E_WIDTH as f32,
                crate::e2e::E2E_HEIGHT as f32,
            )),
            // Production e2e config — non-resizable for determinism (every
            // `pixel_count`-sized buffer identical run-to-run). The
            // resize-blackness reproduction test forks into
            // [`WindowConfig::e2e_resize_test`] (resizable: true) instead.
            resizable: false,
            title: "bevy-naadf e2e_render",
            name: None,
        }
    }

    /// The e2e window for the horizon-parity gate (2026-05-19). 1280×720 —
    /// large enough that the long-distance raymarch covers the full
    /// framebuffer (the standard 256×256 e2e window is too small to make
    /// horizon-line ray-termination regressions visible), matched to the
    /// Playwright spec's `viewport: { width: 1280, height: 720 }` so the
    /// cross-target PNGs SSIM-compare without resize.
    pub fn e2e_horizon() -> Self {
        Self {
            resolution: Some((
                crate::e2e::vox_horizon_parity::HORIZON_WIDTH as f32,
                crate::e2e::vox_horizon_parity::HORIZON_HEIGHT as f32,
            )),
            resizable: false,
            title: "bevy-naadf e2e_render vox-horizon-native",
            name: None,
        }
    }

    /// The e2e window for the resize-blackness reproduction test
    /// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
    /// `## GI-bounce-on-resize fix (2026-05-16)`).
    ///
    /// Same 256×256 starting size as [`WindowConfig::e2e`] but with
    /// `resizable: true` — must be true for hyprctl-driven resize to
    /// propagate through winit; resize-test mode only.
    ///
    /// Without this flag the Hyprland compositor refuses pixel-precise resize
    /// requests on the surface (winit advertises the surface as fixed-size to
    /// the compositor when `resizable: false`). The standard e2e harness
    /// continues to use [`WindowConfig::e2e`] — only the `--resize-test`
    /// branch picks this up.
    pub fn e2e_resize_test() -> Self {
        Self {
            // User spec for the three-step resize test (boot → 1920×1080 →
            // 2000×1000): the *initial* screenshot is taken at 800×600, so
            // the window boots at exactly that size. Larger than the
            // standard e2e 256×256 because the user wants visual coverage of
            // shadow regions across resolution changes.
            resolution: Some((
                crate::e2e::E2E_RESIZE_BOOT_WIDTH as f32,
                crate::e2e::E2E_RESIZE_BOOT_HEIGHT as f32,
            )),
            // test-only: must be true for hyprctl-driven resize to propagate
            // through winit; resize-test mode only.
            resizable: true,
            title: "bevy-naadf e2e_render",
            // test-only: pin Wayland app_id to "e2e_render" so the hyprctl
            // `class:e2e_render` selector matches deterministically. Without
            // this, winit picks a default app_id that varies by build and
            // the hyprctl dispatcher prints "resizeWindow: no window".
            name: Some("e2e_render"),
        }
    }

    /// The e2e window for the `--small-edit-repro` gate (2026-05-17). Runs at
    /// the user's screen size (1920×1080) so the bug-or-fix signal matches
    /// what the user observes in the live binary. The pitch-black-pixel
    /// assertion is resolution-independent in principle, but the user's
    /// report specifies this size; we reproduce verbatim.
    pub fn e2e_small_edit_repro() -> Self {
        Self {
            resolution: Some((
                crate::e2e::small_edit_repro::SMALL_EDIT_REPRO_WIDTH as f32,
                crate::e2e::small_edit_repro::SMALL_EDIT_REPRO_HEIGHT as f32,
            )),
            resizable: false,
            title: "bevy-naadf e2e_render small-edit-repro",
            name: None,
        }
    }
}

/// Map the active e2e mode (as expressed by the boolean fields on
/// [`AppArgs`]) to its [`WindowConfig`]. The mapping is mode-disjoint (at
/// most one e2e flag is true at a time); the order below matches the
/// historical precedence of the inline if-ladder in
/// `run_e2e_render_with_args`.
pub fn window_for_e2e_args(args: &AppArgs) -> WindowConfig {
    if args.resize_test {
        WindowConfig::e2e_resize_test()
    } else if args.small_edit_repro_mode {
        WindowConfig::e2e_small_edit_repro()
    } else if args.vox_horizon_native_phase {
        WindowConfig::e2e_horizon()
    } else {
        WindowConfig::e2e()
    }
}
