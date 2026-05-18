//! On-screen diagnostics overlay: FPS plus per-pass NAADF render-node GPU
//! timings exposed by `RenderDiagnosticsPlugin`. The per-pass block is
//! native-only — WebGPU has no timestamp queries and the CPU-side
//! `elapsed_cpu` fallback rounds every pass to 0.00 ms.

use std::fmt::Write;

use bevy::{
    diagnostic::{Diagnostic, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

#[cfg(not(target_arch = "wasm32"))]
use bevy::diagnostic::DiagnosticPath;

use crate::DevFont;

#[cfg(not(target_arch = "wasm32"))]
use crate::render::graph::{FINAL_BLIT_SPAN, FIRST_HIT_SPAN, TAA_REPROJECT_SPAN};
#[cfg(not(target_arch = "wasm32"))]
use crate::render::graph_b::{
    ATMOSPHERE_SPAN, DENOISE_SPAN, GLOBAL_ILLUM_SPAN, SAMPLE_REFINE_SPAN,
    SPATIAL_RESAMPLING_SPAN,
};

/// Diagnostic path of the first-hit pass's GPU time. `RenderDiagnosticsPlugin`
/// names a `time_span(encoder, "<span>")` measurement
/// `render/<span>/elapsed_gpu` (and `.../elapsed_cpu` as a CPU-side fallback).
#[cfg(not(target_arch = "wasm32"))]
const FIRST_HIT_GPU_PATH: &str = "render/naadf_first_hit/elapsed_gpu";
/// Diagnostic path of the Phase-A-2/B TAA reproject pass's GPU time
/// (`06-design-a2.md` §11, `09-design-b.md` §5.8.1).
#[cfg(not(target_arch = "wasm32"))]
const TAA_REPROJECT_GPU_PATH: &str = "render/naadf_taa_reproject/elapsed_gpu";
/// Diagnostic path of the final-blit pass's GPU time.
#[cfg(not(target_arch = "wasm32"))]
const FINAL_BLIT_GPU_PATH: &str = "render/naadf_final_blit/elapsed_gpu";
/// Diagnostic paths of the Phase-B GI render-node GPU times (`09-design-b.md`
/// §4.12 — the HUD lists the *expensive* nodes only).
#[cfg(not(target_arch = "wasm32"))]
const ATMOSPHERE_GPU_PATH: &str = "render/naadf_atmosphere/elapsed_gpu";
#[cfg(not(target_arch = "wasm32"))]
const GLOBAL_ILLUM_GPU_PATH: &str = "render/naadf_global_illum/elapsed_gpu";
#[cfg(not(target_arch = "wasm32"))]
const SAMPLE_REFINE_GPU_PATH: &str = "render/naadf_sample_refine/elapsed_gpu";
#[cfg(not(target_arch = "wasm32"))]
const SPATIAL_RESAMPLING_GPU_PATH: &str = "render/naadf_spatial_resampling/elapsed_gpu";
#[cfg(not(target_arch = "wasm32"))]
const DENOISE_GPU_PATH: &str = "render/naadf_denoise/elapsed_gpu";
/// CPU-time fallback paths — used when the backend has no timestamp queries
/// (`RenderDiagnosticsPlugin` records `elapsed_cpu` unconditionally).
#[cfg(not(target_arch = "wasm32"))]
const FIRST_HIT_CPU_PATH: &str = "render/naadf_first_hit/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const TAA_REPROJECT_CPU_PATH: &str = "render/naadf_taa_reproject/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const FINAL_BLIT_CPU_PATH: &str = "render/naadf_final_blit/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const ATMOSPHERE_CPU_PATH: &str = "render/naadf_atmosphere/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const GLOBAL_ILLUM_CPU_PATH: &str = "render/naadf_global_illum/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const SAMPLE_REFINE_CPU_PATH: &str = "render/naadf_sample_refine/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const SPATIAL_RESAMPLING_CPU_PATH: &str = "render/naadf_spatial_resampling/elapsed_cpu";
#[cfg(not(target_arch = "wasm32"))]
const DENOISE_CPU_PATH: &str = "render/naadf_denoise/elapsed_cpu";

// Compile-time check that the HUD's hard-coded paths stay in step with the
// render-node span names (`render::graph` / `render::graph_b`).
// `RenderDiagnosticsPlugin` builds the path as `render/<span>/<field>`; assert
// the `<span>` part matches.
#[cfg(not(target_arch = "wasm32"))]
const _: () = {
    assert!(matches_span(FIRST_HIT_GPU_PATH, FIRST_HIT_SPAN));
    assert!(matches_span(TAA_REPROJECT_GPU_PATH, TAA_REPROJECT_SPAN));
    assert!(matches_span(FINAL_BLIT_GPU_PATH, FINAL_BLIT_SPAN));
    assert!(matches_span(ATMOSPHERE_GPU_PATH, ATMOSPHERE_SPAN));
    assert!(matches_span(GLOBAL_ILLUM_GPU_PATH, GLOBAL_ILLUM_SPAN));
    assert!(matches_span(SAMPLE_REFINE_GPU_PATH, SAMPLE_REFINE_SPAN));
    assert!(matches_span(SPATIAL_RESAMPLING_GPU_PATH, SPATIAL_RESAMPLING_SPAN));
    assert!(matches_span(DENOISE_GPU_PATH, DENOISE_SPAN));
};

/// `const`-evaluable check that `path` is `render/<span>/...`.
#[cfg(not(target_arch = "wasm32"))]
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

pub fn setup_hud(mut commands: Commands, dev_font: Res<DevFont>) {
    commands.spawn((
        HudText,
        Text::default(),
        TextColor(Color::WHITE),
        TextFont {
            font: dev_font.0.clone(),
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
) {
    let s = &mut text.0;
    s.clear();

    if let Some(fps) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(Diagnostic::smoothed)
    {
        let _ = writeln!(s, "FPS: {fps:.0}");
    }

    // Per-pass NAADF render-node GPU timings. Each render node
    // (`render::graph` / `render::graph_b`) wraps its work in a `time_span`,
    // which `RenderDiagnosticsPlugin` surfaces at `render/<span>/elapsed_gpu`.
    // Native-only: WebGPU has no timestamp queries, and the CPU-encode
    // fallback (`elapsed_cpu`) rounds every pass to 0.00 ms — not useful.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = writeln!(s, "NAADF passes:");
        write_timing(
            s,
            &diagnostics,
            "atmosphere",
            ATMOSPHERE_GPU_PATH,
            ATMOSPHERE_CPU_PATH,
        );
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
            "global-illum",
            GLOBAL_ILLUM_GPU_PATH,
            GLOBAL_ILLUM_CPU_PATH,
        );
        write_timing(
            s,
            &diagnostics,
            "sample-refine",
            SAMPLE_REFINE_GPU_PATH,
            SAMPLE_REFINE_CPU_PATH,
        );
        write_timing(
            s,
            &diagnostics,
            "spatial-resmpl",
            SPATIAL_RESAMPLING_GPU_PATH,
            SPATIAL_RESAMPLING_CPU_PATH,
        );
        write_timing(
            s,
            &diagnostics,
            "denoise",
            DENOISE_GPU_PATH,
            DENOISE_CPU_PATH,
        );
        write_timing(
            s,
            &diagnostics,
            "final-blit",
            FINAL_BLIT_GPU_PATH,
            FINAL_BLIT_CPU_PATH,
        );
    }

    let _ = write!(s, "\nWASD fly · RMB look · LMB paint · Esc settings");
}

/// Append one `label  N.NN ms` line for a render-node timing.
///
/// Prefers the GPU-timestamp diagnostic at `gpu_path`; if the backend has no
/// timestamp queries that diagnostic is absent, so it falls back to the
/// CPU-side `cpu_path` (`RenderDiagnosticsPlugin` records `elapsed_cpu`
/// unconditionally). Writes nothing if neither has a value yet.
#[cfg(not(target_arch = "wasm32"))]
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
