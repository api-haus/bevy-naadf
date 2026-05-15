//! Comprehensive raymarching-quality dev panel (`21-design-quality-panel.md`).
//!
//! A keyboard-driven in-app panel that exposes every meaningful runtime knob
//! affecting raymarching / GI / reservoir / TAA quality, so the user can tune
//! live without rebuilds. Toggled by `F1`; closed by default.
//!
//! ## Why keyboard-driven (not `bevy_egui`)
//!
//! `bevy_egui` 0.39.1 declares `bevy_app = 0.18.0` — no Bevy 0.19 release
//! exists, no open PR. `bevy-inspector-egui` rides on the same dep. The brief's
//! sanctioned fallback path is "bare egui + manual integration", but that is
//! ~500 lines of wgpu / winit / clipboard glue (= reimplementing `bevy_egui`).
//! The cleanest path that works on Bevy 0.19-rc.1 is **`bevy_ui` 0.19 native**
//! — zero new deps, same render-graph the HUD uses. `bevy_ui` has no slider
//! widget, so the panel is keyboard-driven (Up/Down navigate; Left/Right
//! adjust; PageUp/PageDn big-step; R reset; Shift+R reset-all).
//!
//! ## Architecture
//!
//! - [`PanelState`] — main-world resource holding `open: bool` + the selected
//!   row index. Default = closed (`open: false`).
//! - [`PanelRoot`] — marker component on the panel `Node` entity (the
//!   container Bevy UI element).
//! - [`PanelText`] — marker component on the panel's single `Text` entity that
//!   is rewritten every frame.
//! - [`setup_panel`] — `Startup` system that spawns the root + text entities
//!   with `Display::None` (hidden) and the panel chrome layout.
//! - [`toggle_panel`] — `Update` system: F1 flips `PanelState.open` + toggles
//!   the root's `Display` between `None` ↔ `Flex`.
//! - [`adjust_panel`] — `Update` system: while open, reads input + mutates
//!   the selected knob on `AppArgs.gi`. Closed → no-op.
//! - [`update_panel_text`] — `Update` system: rewrites the panel text content
//!   every frame from current `AppArgs.gi` + the read-only diagnostics.
//!
//! ## Plumbing
//!
//! `AppArgs` is the single source of truth. Mutations land on
//! `AppArgs.gi: GiSettings`; the render-side `extract_gi_config` mirrors the
//! whole struct into `ExtractedGiConfig` every frame, so panel changes
//! propagate to the GPU uniform on the next frame with no extra wiring.
//!
//! ## Test-mode gate
//!
//! The panel is opt-in via `AppConfig.add_hud` (same gate as the HUD —
//! `01-context.md` §2.2). E2E config (`AppConfig::e2e`) has `add_hud = false`,
//! so the panel never spawns in the harness, and e2e luminance gates are
//! unaffected.

use std::fmt::Write;

use bevy::input::ButtonInput;
use bevy::prelude::*;

use crate::{AppArgs, GiSettings, DEFAULT_TAA_RING_DEPTH};
use crate::render::gi::{
    BUCKET_STORAGE_COUNT, INVALID_SAMPLE_STORAGE_COUNT, REFINED_BUCKET_STORAGE_COUNT,
    VALID_SAMPLE_STORAGE_COUNT,
};
use crate::render::taa::CAMERA_HISTORY_DEPTH;

/// Main-world resource: is the panel open + what knob is selected.
///
/// `open` is flipped by [`toggle_panel`] on F1; the panel UI's `Display`
/// follows. `cursor` is a 0-based index into the [`KNOBS`] table (only
/// non-readonly rows step the cursor — the navigator skips readonly rows).
#[derive(Resource, Debug, Clone, Copy)]
pub struct PanelState {
    /// Panel visibility — toggled by F1.
    pub open: bool,
    /// Currently-selected knob index into [`KNOBS`]. Range 0..[`KNOBS`].len().
    pub cursor: usize,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            open: false,
            // Start on the first non-readonly knob (`max_ray_steps_primary`).
            cursor: 0,
        }
    }
}

/// Marker for the panel root `Node` (the container).
#[derive(Component)]
pub struct PanelRoot;

/// Marker for the panel `Text` (the single text entity whose content is
/// rewritten every frame).
#[derive(Component)]
pub struct PanelText;

/// One row in the panel — a knob descriptor.
///
/// `kind` determines mutation behaviour (`nudge` / `big_step` / a `bool` flip
/// / readonly). `getter` / `setter` operate on `AppArgs.gi` for `GiKnob` rows;
/// `readonly_value` produces a display string for `Readonly` / `Section` rows.
struct Knob {
    /// Display label (left-aligned in the panel row).
    label: &'static str,
    /// Knob class indicator: 'P' = promoted runtime knob, 'C' = already config,
    /// 'D' = read-only diagnostic, ' ' = section header.
    class: char,
    /// Mutation kind.
    kind: KnobKind,
}

#[allow(clippy::type_complexity)] // function pointers carry their own arity
enum KnobKind {
    /// Section-header row — no value, no interaction. The cursor skips it.
    Section,
    /// A `u32` knob on `AppArgs.gi`. `getter` reads, `setter` writes (clamped).
    U32 {
        getter: fn(&GiSettings) -> u32,
        setter: fn(&mut GiSettings, u32),
        nudge: u32,
        big_step: u32,
        min: u32,
        max: u32,
        default: u32,
    },
    /// An `f32` knob on `AppArgs.gi`.
    F32 {
        getter: fn(&GiSettings) -> f32,
        setter: fn(&mut GiSettings, f32),
        nudge: f32,
        big_step: f32,
        min: f32,
        max: f32,
        default: f32,
    },
    /// A `bool` knob on `AppArgs.gi` (Left/Right both flip).
    Bool {
        getter: fn(&GiSettings) -> bool,
        setter: fn(&mut GiSettings, bool),
        default: bool,
    },
    /// A read-only diagnostic — display only, cursor skips.
    Readonly {
        value: fn(&AppArgs) -> String,
    },
}

impl KnobKind {
    /// `true` if the cursor should land on this row (i.e. it can be adjusted).
    fn is_interactive(&self) -> bool {
        matches!(self, KnobKind::U32 { .. } | KnobKind::F32 { .. } | KnobKind::Bool { .. })
    }
}

/// The full knob table — one row per panel line, in display order. Sections
/// land as `Section` kinds (cursor skips); readonly rows as `Readonly`.
///
/// `21-design-quality-panel.md` §5 panel layout maps directly to this array.
const KNOBS: &[Knob] = &[
    Knob {
        label: "RAY STEP CAPS",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  primary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_primary,
            setter: |g, v| g.max_ray_steps_primary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 120,
        },
    },
    Knob {
        label: "  secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_secondary,
            setter: |g, v| g.max_ray_steps_secondary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 100,
        },
    },
    Knob {
        label: "  sun",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun,
            setter: |g, v| g.max_ray_steps_sun = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 120,
        },
    },
    Knob {
        label: "  sun-secondary",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_sun_secondary,
            setter: |g, v| g.max_ray_steps_sun_secondary = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 80,
        },
    },
    Knob {
        label: "  visibility",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.max_ray_steps_visibility,
            setter: |g, v| g.max_ray_steps_visibility = v,
            nudge: 8,
            big_step: 32,
            min: 1,
            max: 512,
            default: 60,
        },
    },
    Knob {
        label: "SPATIAL RESAMPLING",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  iter count",
        class: 'P',
        kind: KnobKind::U32 {
            getter: |g| g.spatial_iter_count,
            setter: |g, v| g.spatial_iter_count = v,
            nudge: 1,
            big_step: 4,
            min: 1,
            max: 32,
            default: 12,
        },
    },
    Knob {
        label: "  sun_shadow_taps",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.sun_shadow_taps,
            setter: |g, v| g.sun_shadow_taps = v,
            nudge: 1,
            big_step: 4,
            min: 1,
            max: 32,
            default: 4,
        },
    },
    Knob {
        label: "  resample_size",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.spatial_resample_size,
            setter: |g, v| g.spatial_resample_size = v,
            nudge: 50.0,
            big_step: 200.0,
            min: 32.0,
            max: 2000.0,
            default: 500.0,
        },
    },
    Knob {
        label: "  radius_lit_factor",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.radius_lit_factor,
            setter: |g, v| g.radius_lit_factor = v,
            nudge: 0.5,
            big_step: 3.0,
            min: 0.0,
            max: 1000.0,
            default: 3.0,
        },
    },
    Knob {
        label: "  noise_suppress",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.noise_suppression_factor,
            setter: |g, v| g.noise_suppression_factor = v,
            nudge: 0.05,
            big_step: 0.5,
            min: 0.01,
            max: 100.0,
            default: 0.4,
        },
    },
    Knob {
        label: "GI",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  bounce_count",
        class: 'C',
        kind: KnobKind::U32 {
            getter: |g| g.bounce_count,
            setter: |g, v| g.bounce_count = v,
            nudge: 1,
            big_step: 1,
            min: 1,
            max: 3,
            default: 3,
        },
    },
    Knob {
        label: "  denoise_thresh",
        class: 'C',
        kind: KnobKind::F32 {
            getter: |g| g.denoise_thresh,
            setter: |g, v| g.denoise_thresh = v,
            nudge: 50.0,
            big_step: 200.0,
            min: 0.0,
            max: 2000.0,
            default: 400.0,
        },
    },
    Knob {
        label: "  is_denoise",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_denoise,
            setter: |g, v| g.is_denoise = v,
            default: true,
        },
    },
    Knob {
        label: "  is_sample_leveling",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_sample_leveling,
            setter: |g, v| g.is_sample_leveling = v,
            default: true,
        },
    },
    Knob {
        label: "  is_varying_radius",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_varying_resampling_radius,
            setter: |g, v| g.is_varying_resampling_radius = v,
            default: true,
        },
    },
    Knob {
        label: "  is_atmosphere_int",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.is_atmosphere_interaction,
            setter: |g, v| g.is_atmosphere_interaction = v,
            default: true,
        },
    },
    Knob {
        label: "  skip_samples",
        class: 'C',
        kind: KnobKind::Bool {
            getter: |g| g.skip_samples,
            setter: |g, v| g.skip_samples = v,
            default: true,
        },
    },
    Knob {
        label: "DIAGNOSTICS (read-only)",
        class: ' ',
        kind: KnobKind::Section,
    },
    Knob {
        label: "  taa_ring_depth",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |a| format!("{} [restart-required]", a.taa_ring_depth),
        },
    },
    Knob {
        label: "  camera_history_depth",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [const]", CAMERA_HISTORY_DEPTH),
        },
    },
    Knob {
        label: "  valid_sample_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", VALID_SAMPLE_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  invalid_sample_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", INVALID_SAMPLE_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  bucket_storage",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", BUCKET_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  refined_bucket",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |_| format!("{} [storage-tied]", REFINED_BUCKET_STORAGE_COUNT),
        },
    },
    Knob {
        label: "  global_illum_max_accum",
        class: 'D',
        kind: KnobKind::Readonly {
            value: |a| format!("{} [const]", a.gi.global_illum_max_accum),
        },
    },
];

/// First interactive-row index (the cursor lands here on startup) — the first
/// `KnobKind::U32` / `F32` / `Bool` past any leading `Section`s.
fn first_interactive() -> usize {
    KNOBS
        .iter()
        .position(|k| k.kind.is_interactive())
        .unwrap_or(0)
}

/// `Startup` system: spawn the panel root + text entity, `Display::None` (the
/// panel is closed by default — F1 reveals it).
pub fn setup_panel(mut commands: Commands) {
    // The chrome layout: bottom-left, slightly smaller font than the HUD.
    let root = commands
        .spawn((
            PanelRoot,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(12.0),
                left: px(12.0),
                padding: px(10.0).all(),
                width: px(360.0),
                // Hidden until F1 toggle.
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
        ))
        .id();

    let text = commands
        .spawn((
            PanelText,
            Text::default(),
            TextColor(Color::WHITE),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
        ))
        .id();
    commands.entity(root).add_child(text);
}

/// `Update` system: F1 toggles `PanelState.open` and the root `Node`'s
/// `Display` between `None` (hidden) and `Flex` (visible). Just-pressed only —
/// holding F1 does not retoggle.
pub fn toggle_panel(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PanelState>,
    mut root: Query<&mut Node, With<PanelRoot>>,
) {
    if !keys.just_pressed(KeyCode::F1) {
        return;
    }
    state.open = !state.open;
    if let Ok(mut node) = root.single_mut() {
        node.display = if state.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    // On first-open, anchor the cursor on the first interactive row in case
    // the default was somehow skipped.
    if state.open && !KNOBS.get(state.cursor).map(|k| k.kind.is_interactive()).unwrap_or(false) {
        state.cursor = first_interactive();
    }
}

/// `Update` system: while the panel is open, read input + mutate the selected
/// knob on `AppArgs.gi`. Closed → no-op (camera input flows through normally).
///
/// Bindings:
/// - Up / Down — move cursor (skips Section / Readonly rows).
/// - Left / Right — adjust selected knob by `nudge` (bool flips).
/// - PageUp / PageDown — adjust by `big_step`.
/// - Shift + Left/Right — fine adjust (`nudge / 4`, rounded for u32).
/// - R — reset selected knob to its default.
/// - Shift + R — reset every knob to defaults.
pub fn adjust_panel(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PanelState>,
    mut args: ResMut<AppArgs>,
) {
    if !state.open {
        return;
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    // Cursor navigation — skip non-interactive rows in the appropriate dir.
    if keys.just_pressed(KeyCode::ArrowUp) {
        state.cursor = step_cursor(state.cursor, -1);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        state.cursor = step_cursor(state.cursor, 1);
    }

    // Adjust the selected knob.
    let left = keys.just_pressed(KeyCode::ArrowLeft);
    let right = keys.just_pressed(KeyCode::ArrowRight);
    let big_left = keys.just_pressed(KeyCode::PageUp);
    let big_right = keys.just_pressed(KeyCode::PageDown);
    let reset_one = keys.just_pressed(KeyCode::KeyR) && !shift;
    let reset_all = keys.just_pressed(KeyCode::KeyR) && shift;

    if reset_all {
        // Reset every knob to its default — preserves field identity by
        // calling each row's setter with its declared default.
        for row in KNOBS {
            match row.kind {
                KnobKind::U32 { setter, default, .. } => setter(&mut args.gi, default),
                KnobKind::F32 { setter, default, .. } => setter(&mut args.gi, default),
                KnobKind::Bool { setter, default, .. } => setter(&mut args.gi, default),
                _ => {}
            }
        }
        return;
    }

    if let Some(row) = KNOBS.get(state.cursor) {
        match row.kind {
            KnobKind::U32 { getter, setter, nudge, big_step, min, max, default } => {
                let mut v = getter(&args.gi);
                let n_step = if shift { (nudge / 4).max(1) } else { nudge };
                let b_step = big_step;
                if left {
                    v = v.saturating_sub(n_step);
                }
                if right {
                    v = v.saturating_add(n_step);
                }
                if big_left {
                    v = v.saturating_sub(b_step);
                }
                if big_right {
                    v = v.saturating_add(b_step);
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::F32 { getter, setter, nudge, big_step, min, max, default } => {
                let mut v = getter(&args.gi);
                let n_step = if shift { nudge / 4.0 } else { nudge };
                let b_step = big_step;
                if left {
                    v -= n_step;
                }
                if right {
                    v += n_step;
                }
                if big_left {
                    v -= b_step;
                }
                if big_right {
                    v += b_step;
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v.clamp(min, max));
            }
            KnobKind::Bool { getter, setter, default } => {
                let mut v = getter(&args.gi);
                if left || right {
                    v = !v;
                }
                if reset_one {
                    v = default;
                }
                setter(&mut args.gi, v);
            }
            _ => {}
        }
    }
}

/// `Update` system: rewrite the panel text content from `AppArgs.gi` + the
/// read-only diagnostics. Runs every frame *only when the panel is open* (cheap
/// `state.open` guard).
pub fn update_panel_text(
    state: Res<PanelState>,
    args: Res<AppArgs>,
    mut text: Query<&mut Text, With<PanelText>>,
) {
    if !state.open {
        return;
    }
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    let s = &mut text.0;
    s.clear();

    let _ = writeln!(s, "[F1] Raymarching Quality");
    let _ = writeln!(s, "─────────────────────────────");

    for (i, row) in KNOBS.iter().enumerate() {
        let marker = if i == state.cursor && row.kind.is_interactive() {
            "> "
        } else {
            "  "
        };
        match &row.kind {
            KnobKind::Section => {
                let _ = writeln!(s, "  {}", row.label);
            }
            KnobKind::U32 { getter, .. } => {
                let _ = writeln!(
                    s,
                    "{}{:<22} {:>6} [{}]",
                    marker,
                    row.label,
                    getter(&args.gi),
                    row.class,
                );
            }
            KnobKind::F32 { getter, .. } => {
                let _ = writeln!(
                    s,
                    "{}{:<22} {:>6.2} [{}]",
                    marker,
                    row.label,
                    getter(&args.gi),
                    row.class,
                );
            }
            KnobKind::Bool { getter, .. } => {
                let _ = writeln!(
                    s,
                    "{}{:<22} {:>6} [{}]",
                    marker,
                    row.label,
                    if getter(&args.gi) { "true" } else { "false" },
                    row.class,
                );
            }
            KnobKind::Readonly { value } => {
                let _ = writeln!(s, "{}{:<22} {} [{}]", marker, row.label, value(&args), row.class);
            }
        }
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "[↑↓] navigate  [←→] adjust  [PgUp/PgDn] big");
    let _ = writeln!(s, "[Shift+←→] fine  [R] reset row  [Shift+R] reset all");
    // The `DEFAULT_TAA_RING_DEPTH` const reference is intentional — silences
    // an unused-import warning if `taa_ring_depth` is removed in a future
    // edit, and documents the source-of-truth.
    let _ = DEFAULT_TAA_RING_DEPTH;
}

/// Move the cursor by `delta` (±1), skipping non-interactive rows. Wraps at
/// both ends. Returns the new cursor index.
fn step_cursor(cur: usize, delta: i32) -> usize {
    let n = KNOBS.len();
    if n == 0 {
        return 0;
    }
    let mut i = cur as i32;
    for _ in 0..n {
        i += delta;
        if i < 0 {
            i = (n as i32) - 1;
        }
        if i >= n as i32 {
            i = 0;
        }
        let ui = i as usize;
        if KNOBS[ui].kind.is_interactive() {
            return ui;
        }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cursor must land on an interactive row after a single `step` from
    /// any starting position. Verifies the section / readonly skip logic.
    #[test]
    fn cursor_skips_non_interactive_rows() {
        for start in 0..KNOBS.len() {
            let next = step_cursor(start, 1);
            assert!(
                KNOBS[next].kind.is_interactive(),
                "cursor landed on a non-interactive row from start={start}, next={next}",
            );
            let prev = step_cursor(start, -1);
            assert!(
                KNOBS[prev].kind.is_interactive(),
                "cursor landed on a non-interactive row from start={start}, prev={prev}",
            );
        }
    }

    /// All defaults in the knob table MUST match `GiSettings::default()` —
    /// `21-design-quality-panel.md` §6 bit-equivalence promise. A drift here
    /// is a panel-default-vs-`GiSettings::default()` mismatch that would
    /// silently change behaviour on `R` (reset row).
    #[test]
    fn defaults_match_gi_settings_default() {
        let g = GiSettings::default();
        for row in KNOBS {
            match row.kind {
                KnobKind::U32 { getter, default, .. } => {
                    assert_eq!(
                        getter(&g),
                        default,
                        "u32 knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                KnobKind::F32 { getter, default, .. } => {
                    assert!(
                        (getter(&g) - default).abs() < f32::EPSILON,
                        "f32 knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                KnobKind::Bool { getter, default, .. } => {
                    assert_eq!(
                        getter(&g),
                        default,
                        "bool knob {:?} default ({}) != GiSettings::default ({})",
                        row.label,
                        default,
                        getter(&g),
                    );
                }
                _ => {}
            }
        }
    }

    /// Class-P ray-step-cap defaults must equal the WGSL `MAX_RAY_STEPS_*`
    /// consts the promotions replaced — the bit-equivalence promise of
    /// `21-design-quality-panel.md` §6.
    #[test]
    fn promoted_defaults_match_canonical_consts() {
        let g = GiSettings::default();
        // Mirror of `ray_tracing.wgsl:122-126`.
        assert_eq!(g.max_ray_steps_primary, 120);
        assert_eq!(g.max_ray_steps_secondary, 100);
        assert_eq!(g.max_ray_steps_sun, 120);
        assert_eq!(g.max_ray_steps_sun_secondary, 80);
        assert_eq!(g.max_ray_steps_visibility, 60);
        // Mirror of `spatial_resampling.wgsl::sample_neighbors` argument.
        assert_eq!(g.spatial_iter_count, 12);
    }

    /// At least one interactive knob exists (otherwise `first_interactive`
    /// returns 0 which would land on a section header — broken UX).
    #[test]
    fn at_least_one_interactive_knob() {
        let count = KNOBS.iter().filter(|k| k.kind.is_interactive()).count();
        assert!(count > 0, "no interactive knobs in the panel table");
        assert!(KNOBS[first_interactive()].kind.is_interactive());
    }
}
