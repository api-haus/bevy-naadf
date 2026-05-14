//! On-screen diagnostics overlay: FPS, renderer mode, DLSS-RR state, and the
//! per-pass Solari / DLSS GPU timings exposed by `RenderDiagnosticsPlugin`.

use std::fmt::Write;

use bevy::{
    diagnostic::{Diagnostic, DiagnosticPath, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

use crate::AppArgs;

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
    args: Res<AppArgs>,
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

    let _ = writeln!(
        s,
        "Renderer: {}",
        if args.pathtracer {
            "Solari pathtracer (reference)"
        } else {
            "Solari realtime lighting"
        }
    );

    // DLSS Ray Reconstruction status line.
    #[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
    {
        if args.pathtracer {
            let _ = writeln!(s, "DLSS-RR: n/a in pathtracer mode");
        } else if dlss_rr_supported.is_none() {
            let _ = writeln!(s, "DLSS-RR: unsupported on this GPU/driver");
        } else if dlss_cameras.is_empty() {
            let _ = writeln!(s, "DLSS-RR: OFF — raw Solari output   [D] toggle");
        } else {
            let _ = writeln!(s, "DLSS-RR: ON  — denoised + upscaled  [D] toggle");
        }
    }
    #[cfg(any(not(feature = "dlss"), feature = "force_disable_dlss"))]
    {
        let _ = writeln!(s, "DLSS-RR: not compiled in (no `dlss` feature)");
    }

    // Per-pass GPU timings (populated once RenderDiagnosticsPlugin has data).
    write_timing(s, &diagnostics, "direct lighting", "render/solari_lighting/direct_lighting/elapsed_gpu");
    write_timing(s, &diagnostics, "diffuse indirect", "render/solari_lighting/diffuse_indirect_lighting/elapsed_gpu");
    write_timing(s, &diagnostics, "specular indirect", "render/solari_lighting/specular_indirect_lighting/elapsed_gpu");
    write_timing(s, &diagnostics, "world cache", "render/solari_lighting/world_cache/elapsed_gpu");
    write_timing(s, &diagnostics, "DLSS-RR", "render/dlss_ray_reconstruction/elapsed_gpu");

    let _ = write!(s, "\nWASD / Shift / mouse: fly camera");
}

/// Append one `label  N.NN ms` line if the diagnostic at `path` has a value.
fn write_timing(s: &mut String, diagnostics: &DiagnosticsStore, label: &str, path: &'static str) {
    if let Some(ms) = diagnostics
        .get(&DiagnosticPath::new(path))
        .and_then(Diagnostic::smoothed)
    {
        let _ = writeln!(s, "  {label:<18} {ms:.2} ms");
    }
}
