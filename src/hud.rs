//! On-screen diagnostics overlay: FPS, renderer mode, DLSS-RR state, and the
//! per-pass NAADF render-node GPU timings exposed by `RenderDiagnosticsPlugin`.

use std::fmt::Write;

use bevy::{
    diagnostic::{Diagnostic, DiagnosticPath, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

#[cfg(all(feature = "dlss", not(feature = "force_disable_dlss")))]
use bevy::anti_alias::dlss::{Dlss, DlssRayReconstructionFeature, DlssRayReconstructionSupported};

use crate::render::graph::{FINAL_BLIT_SPAN, FIRST_HIT_SPAN, TAA_REPROJECT_SPAN};

/// Diagnostic path of the first-hit pass's GPU time. `RenderDiagnosticsPlugin`
/// names a `time_span(encoder, "<span>")` measurement
/// `render/<span>/elapsed_gpu` (and `.../elapsed_cpu` as a CPU-side fallback).
const FIRST_HIT_GPU_PATH: &str = "render/naadf_first_hit/elapsed_gpu";
/// Diagnostic path of the Phase-A-2 TAA reproject pass's GPU time
/// (`06-design-a2.md` §11).
const TAA_REPROJECT_GPU_PATH: &str = "render/naadf_taa_reproject/elapsed_gpu";
/// Diagnostic path of the final-blit pass's GPU time.
const FINAL_BLIT_GPU_PATH: &str = "render/naadf_final_blit/elapsed_gpu";
/// CPU-time fallback paths — used when the backend has no timestamp queries
/// (`RenderDiagnosticsPlugin` records `elapsed_cpu` unconditionally).
const FIRST_HIT_CPU_PATH: &str = "render/naadf_first_hit/elapsed_cpu";
const TAA_REPROJECT_CPU_PATH: &str = "render/naadf_taa_reproject/elapsed_cpu";
const FINAL_BLIT_CPU_PATH: &str = "render/naadf_final_blit/elapsed_cpu";

// Compile-time check that the HUD's hard-coded paths stay in step with the
// render-node span names (`render::graph`). `RenderDiagnosticsPlugin` builds
// the path as `render/<span>/<field>`; assert the `<span>` part matches.
const _: () = {
    assert!(matches_span(FIRST_HIT_GPU_PATH, FIRST_HIT_SPAN));
    assert!(matches_span(TAA_REPROJECT_GPU_PATH, TAA_REPROJECT_SPAN));
    assert!(matches_span(FINAL_BLIT_GPU_PATH, FINAL_BLIT_SPAN));
};

/// `const`-evaluable check that `path` is `render/<span>/...`.
const fn matches_span(path: &str, span: &str) -> bool {
    let path = path.as_bytes();
    let span = span.as_bytes();
    // path must start with "render/"
    let prefix = b"render/";
    let mut i = 0;
    while i < prefix.len() {
        if path[i] != prefix[i] {
            return false;
        }
        i += 1;
    }
    // ...followed by `span`...
    let mut j = 0;
    while j < span.len() {
        if path[prefix.len() + j] != span[j] {
            return false;
        }
        j += 1;
    }
    // ...followed by '/'.
    path[prefix.len() + span.len()] == b'/'
}

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

    let _ = writeln!(
        s,
        "Renderer: NAADF (Phase A — albedo first-hit + AADF DDA)"
    );

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

    // Per-pass NAADF render-node GPU timings. The three render nodes
    // (`render::graph`) wrap their work in a `time_span`, which
    // `RenderDiagnosticsPlugin` surfaces at `render/<span>/elapsed_gpu`. On a
    // backend with timestamp queries (Vulkan / DX12) the GPU path populates;
    // elsewhere `write_timing` falls back to the CPU-side `elapsed_cpu`. The
    // `taa-reproject` line sits between `first-hit` and `final-blit`, matching
    // the render order (`06-design-a2.md` §11).
    let _ = writeln!(s, "NAADF passes:");
    write_timing(
        s,
        &diagnostics,
        "first-hit",
        FIRST_HIT_GPU_PATH,
        FIRST_HIT_CPU_PATH,
    );
    write_timing(
        s,
        &diagnostics,
        "taa-reproject",
        TAA_REPROJECT_GPU_PATH,
        TAA_REPROJECT_CPU_PATH,
    );
    write_timing(
        s,
        &diagnostics,
        "final-blit",
        FINAL_BLIT_GPU_PATH,
        FINAL_BLIT_CPU_PATH,
    );

    let _ = write!(s, "\nWASD / Shift / mouse: fly camera");
}

/// Append one `label  N.NN ms` line for a render-node timing.
///
/// Prefers the GPU-timestamp diagnostic at `gpu_path`; if the backend has no
/// timestamp queries that diagnostic is absent, so it falls back to the
/// CPU-side `cpu_path` (`RenderDiagnosticsPlugin` records `elapsed_cpu`
/// unconditionally). Writes nothing if neither has a value yet.
fn write_timing(
    s: &mut String,
    diagnostics: &DiagnosticsStore,
    label: &str,
    gpu_path: &'static str,
    cpu_path: &'static str,
) {
    let gpu = diagnostics
        .get(&DiagnosticPath::new(gpu_path))
        .and_then(Diagnostic::smoothed);
    if let Some(ms) = gpu {
        let _ = writeln!(s, "  {label:<12} {ms:.2} ms (gpu)");
        return;
    }
    if let Some(ms) = diagnostics
        .get(&DiagnosticPath::new(cpu_path))
        .and_then(Diagnostic::smoothed)
    {
        let _ = writeln!(s, "  {label:<12} {ms:.2} ms (cpu)");
    }
}
