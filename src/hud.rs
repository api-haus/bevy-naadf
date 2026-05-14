//! On-screen diagnostics overlay: FPS, renderer mode, DLSS-RR state, and the
//! per-pass Solari / DLSS GPU timings exposed by `RenderDiagnosticsPlugin`.

use std::fmt::Write;

use bevy::{
    diagnostic::{Diagnostic, DiagnosticPath, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::anti_alias::dlss::{Dlss, DlssRayReconstructionFeature, DlssRayReconstructionSupported};

#[derive(Component)]
pub struct HudText;

pub fn setup_hud(mut commands: Commands) {
    commands.spawn((
        HudText,
        Text::default(),
        TextColor(Color::WHITE),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            left: px(12.0),
            padding: px(8.0).all(),
            ..default()
        },
    ));
}

pub fn update_hud(
    mut text: Single<&mut Text, With<HudText>>,
    diagnostics: Res<DiagnosticsStore>,
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))] dlss_rr_supported: Option<
        Res<DlssRayReconstructionSupported>,
    >,
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))] dlss_cameras: Query<
        (),
        (With<Camera3d>, With<Dlss<DlssRayReconstructionFeature>>),
    >,
) {
    let s = &mut text.0;
    s.clear();

    if let Some(fps) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(Diagnostic::smoothed)
    {
        let _ = writeln!(s, "FPS: {fps:.0}");
    }

    let _ = writeln!(s, "Renderer: NAADF (Phase A — albedo first-hit)");

    // DLSS Ray Reconstruction status line. Dormant in Phase A — the NAADF
    // render path is not wired to DLSS yet.
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    {
        if dlss_rr_supported.is_none() {
            let _ = writeln!(s, "DLSS-RR: unsupported on this GPU/driver");
        } else if dlss_cameras.is_empty() {
            let _ = writeln!(s, "DLSS-RR: OFF (dormant in Phase A)   [D] toggle");
        } else {
            let _ = writeln!(s, "DLSS-RR: ON  (dormant in Phase A)   [D] toggle");
        }
    }
    #[cfg(any(not(feature = "dlss"), feature = "force_disable_dlss"))]
    {
        let _ = writeln!(s, "DLSS-RR: not compiled in (no `dlss` feature)");
    }

    // Per-pass GPU timings. The NAADF render-node timing paths are wired in
    // Batch 2 (design §8 step 12) — nothing populates these lines yet.

    let _ = write!(s, "\nWASD / Shift / mouse: fly camera");
}

/// Append one `label  N.NN ms` line if the diagnostic at `path` has a value.
///
/// Unused until Batch 2 step 12 re-points the HUD at the NAADF render-node
/// timing paths (design §1.1 keeps this helper unchanged).
#[allow(dead_code)]
fn write_timing(s: &mut String, diagnostics: &DiagnosticsStore, label: &str, path: &'static str) {
    if let Some(ms) = diagnostics
        .get(&DiagnosticPath::new(path))
        .and_then(Diagnostic::smoothed)
    {
        let _ = writeln!(s, "  {label:<18} {ms:.2} ms");
    }
}
