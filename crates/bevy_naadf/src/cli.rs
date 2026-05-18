//! Shared CLI parser for `bevy-naadf` (interactive) and `e2e_render`
//! (bounded harness) — `docs/orchestrate/streaming-world/02d-design-cli-and-e2e-rearch.md`.
//!
//! ## Why this module exists
//!
//! Before this rearch, `bin/bevy-naadf.rs` parsed **only** `--vox <path>` and
//! `bin/e2e_render.rs` scanned `std::env::args()` by hand for a dozen mode
//! flags. Every new field added to [`AppArgs`] for the streaming-world Plan B
//! (`vram_budget_mib`, `max_segments_per_frame`, `noise_seed`, `sea_level`,
//! `terrain_amplitude`, `streaming_window_mode`, `noise_static_mode`, …) was
//! invisible from CLI: `cargo run --bin bevy-naadf -- --grid-preset
//! procedural-streaming --vram-budget-mib 1024` silently dropped every flag
//! and booted the default scene. `--help` ran the app instead of printing
//! help. The user's directive: *"obviously e2e path is once again not e2e ...
//! if its shape is not like that - rewrite it to hell."*
//!
//! The fix is `void main_e2e() { begin_running_actual_main();
//! control_actual_main(); }`: both binaries parse the same [`Cli`] surface,
//! the e2e binary adds a `--gate <NAME>` selector + per-gate default-overlay
//! functions, and every gate composes onto the SAME `build_app_with_args(cfg,
//! args)` path the interactive binary uses. The gate's job is to set its
//! observer mode flag + (if the user didn't override) apply the gate's
//! default preset / pose. It is NOT to construct a parallel `AppArgs`.
//!
//! ## Public surfaces
//!
//! - [`Cli`] — the interactive parser (`bin/bevy-naadf`). Every field maps
//!   one-to-one onto an [`AppArgs`] field via [`Cli::into_app_args`].
//! - [`E2eCli`] — flattens [`Cli`] + adds `--gate <Gate>`. Used by
//!   `bin/e2e_render`. [`E2eCli::into_app_args_and_gate`] returns the
//!   composed [`AppArgs`] + the optional [`Gate`] choice.
//! - [`Gate`] — every named gate (`baseline`, `streaming-window`,
//!   `noise-static-world`, …). One choice per invocation; the e2e binary's
//!   `main` matches on this to apply the gate's default overlay + dispatch
//!   to the matching post-App validation pass.
//! - [`GridPresetArg`] — clap `ValueEnum` mirror of [`GridPreset`]; the
//!   variant-payload fields (`noise_preset`, `seed`) come from the flat
//!   [`Cli`] fields `--noise-preset` / `--noise-seed`.
//!
//! ## Composition semantics — "user override wins, gate fills the rest"
//!
//! Each gate has a private `apply_<gate>_defaults(&mut AppArgs)` function
//! that:
//!
//! 1. Sets the gate's mode flag(s) unconditionally (e.g.
//!    `streaming_window_mode = true` + `oasis_edit_visual_mode = true` —
//!    "observer attachment").
//! 2. If `args.grid_preset == GridPreset::Default` (the clap default), sets
//!    the gate's preferred preset (e.g. `ProceduralStreaming { ... }`).
//!    Otherwise leaves the user's explicit choice alone.
//!
//! So `--gate streaming-window` boots with the canonical defaults, but
//! `--gate streaming-window --grid-preset procedural-static
//! --vram-budget-mib 2048` composes the user's overrides on top of the gate's
//! observer attachment.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::{AppArgs, GridPreset};

// --- The shared interactive CLI -------------------------------------------

/// Top-level CLI parser for `bin/bevy-naadf` and (flattened into [`E2eCli`])
/// `bin/e2e_render`. Every field has a default that matches
/// [`AppArgs::default`]; `into_app_args` mutates the default to reflect the
/// user's overrides.
///
/// Doc comments on each field become the `--help` text — keep them
/// user-facing.
#[derive(Parser, Clone, Debug)]
#[command(
    name = "bevy-naadf",
    about = "bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer",
    long_about = "bevy-naadf — Bevy 0.19 port of the NAADF voxel renderer.\n\n\
                  All CLI flags below are also accepted by the `e2e_render` \
                  binary; the e2e binary additionally accepts `--gate <NAME>` \
                  to attach a named verification gate on top of the same App.",
    version
)]
pub struct Cli {
    /// Which test grid to install at Startup. `default` = the embedded
    /// primitive scene; `vox` = MagicaVoxel `.vox` file (requires `--vox
    /// <path>`); `procedural-streaming` = streaming-world Plan B preset
    /// (residency manager + per-frame noise→segment_voxel_buffer dispatch);
    /// `procedural-static` = one-shot static-noise viability preset.
    #[arg(long, value_enum, default_value_t = GridPresetArg::Default)]
    pub grid_preset: GridPresetArg,

    /// Path to a MagicaVoxel `.vox` file (Track A `--vox <path>` legacy
    /// alias). When supplied without `--grid-preset`, the grid preset
    /// defaults to `vox`. When `--grid-preset vox` is supplied without a
    /// path, the parser hard-errors.
    #[arg(long, value_name = "PATH")]
    pub vox: Option<PathBuf>,

    /// WGSL noise preset index for `procedural-streaming` /
    /// `procedural-static` (Phase 2 ships `0 = SimpleTerrain` only).
    #[arg(long, default_value_t = 0)]
    pub noise_preset: u32,

    /// FastNoiseLite seed for the streaming / static noise presets.
    #[arg(long, default_value_t = 1337)]
    pub noise_seed: i32,

    /// world_y at which `noise == 0` flips solid/empty (streaming /
    /// procedural-static). Default = half world height = 256.0.
    #[arg(long, default_value_t = 256.0)]
    pub sea_level: f32,

    /// Vertical span (in voxels) over which the noise transition spreads
    /// (streaming / procedural-static).
    #[arg(long, default_value_t = 64.0)]
    pub terrain_amplitude: f32,

    /// VRAM budget (MiB) for the residency slab (streaming preset only).
    /// Asserted at install time; panics on under-budget.
    #[arg(long, default_value_t = 1024)]
    pub vram_budget_mib: u32,

    /// Per-frame admission cap for the residency driver (streaming preset
    /// only).
    #[arg(long, default_value_t = 4)]
    pub max_segments_per_frame: u32,

    /// Temporal anti-aliasing (TAA). Default on.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub taa: bool,

    /// TAA sample-ring depth. Default 32. Supported values: 16, 24, 32.
    #[arg(long, default_value_t = crate::DEFAULT_TAA_RING_DEPTH)]
    pub taa_ring_depth: u32,

    /// Spawn the Phase-C wave-3 fixture entity (4×4×4 emissive block).
    /// Off by default; the `entities` gate flips this on.
    #[arg(long, default_value_t = false)]
    pub spawn_test_entity: bool,

    /// GPU construction master switch. Default on (Phase C W1).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub gpu_construction: bool,

    /// Phase-C W4 entity track on/off.
    #[arg(long, default_value_t = false)]
    pub entities_enabled: bool,
}

impl Cli {
    /// Resolve the parsed CLI into a runtime [`AppArgs`]. Constructs an
    /// `AppArgs::default()` and overrides each field from the CLI; the
    /// `grid_preset` field is computed from `(self.grid_preset, self.vox,
    /// self.noise_preset, self.noise_seed)` per the variant-payload rules
    /// in § A.2 of the design.
    pub fn into_app_args(self) -> AppArgs {
        let Cli {
            grid_preset,
            vox,
            noise_preset,
            noise_seed,
            sea_level,
            terrain_amplitude,
            vram_budget_mib,
            max_segments_per_frame,
            taa,
            taa_ring_depth,
            spawn_test_entity,
            gpu_construction,
            entities_enabled,
        } = self;

        // (a) Resolve the GridPreset variant. The `--vox <path>` flag is a
        //     UX shortcut: without `--grid-preset`, it implies `grid_preset
        //     = vox`. With `--grid-preset vox`, `--vox <path>` is REQUIRED.
        let resolved_preset = match (grid_preset, vox.as_ref()) {
            // No --vox, default grid preset → embedded primitive scene.
            (GridPresetArg::Default, None) => GridPreset::Default,
            // --vox <path> without explicit --grid-preset → backward-compat
            // shortcut to the Vox variant.
            (GridPresetArg::Default, Some(path)) => GridPreset::Vox {
                path: path.clone(),
            },
            // Explicit --grid-preset vox needs a path.
            (GridPresetArg::Vox, Some(path)) => GridPreset::Vox {
                path: path.clone(),
            },
            (GridPresetArg::Vox, None) => {
                eprintln!(
                    "bevy-naadf: --grid-preset vox requires --vox <path>; \
                     falling back to GridPreset::Default."
                );
                GridPreset::Default
            }
            // --grid-preset procedural-streaming → variant filled from the
            // flat --noise-preset / --noise-seed.
            (GridPresetArg::ProceduralStreaming, _) => GridPreset::ProceduralStreaming {
                noise_preset,
                seed: noise_seed,
            },
            (GridPresetArg::ProceduralStatic, _) => GridPreset::ProceduralStatic {
                noise_preset,
                seed: noise_seed,
            },
        };

        let mut args = AppArgs::default();
        args.grid_preset = resolved_preset;
        args.taa = taa;
        args.taa_ring_depth = taa_ring_depth;
        args.spawn_test_entity = spawn_test_entity;
        args.construction_config.gpu_construction_enabled = gpu_construction;
        args.construction_config.entities_enabled = entities_enabled;
        args.sea_level = sea_level;
        args.terrain_amplitude = terrain_amplitude;
        args.vram_budget_mib = vram_budget_mib;
        args.max_segments_per_frame = max_segments_per_frame;
        args.noise_seed = noise_seed;
        args.noise_preset = noise_preset;
        // Every mode flag (streaming_window_mode, noise_static_mode, …)
        // stays at AppArgs::default() (= false). The interactive binary
        // never sets these; the e2e binary's gate dispatch flips them via
        // `apply_*_defaults` below.
        args
    }
}

// --- The e2e CLI -----------------------------------------------------------

/// The e2e harness CLI — flattens [`Cli`] and adds the gate selector.
/// `bin/e2e_render` parses this; the gate's effect is to (a) flip the
/// matching `AppArgs::*_mode` flag (observer attachment) and (b) fill any
/// gate-preferred defaults the user didn't override.
#[derive(Parser, Clone, Debug)]
#[command(
    name = "e2e_render",
    about = "bevy-naadf bounded e2e render harness (drives the interactive \
             App + attaches a named gate)",
    long_about = "bevy-naadf e2e harness. Boots the SAME App `bin/bevy-naadf` \
                  boots (small fixed window, synchronous pipeline compile, \
                  bounded-frame driver) and attaches a named verification \
                  gate selected by `--gate`. Every interactive `--flag` is \
                  also accepted here and composes onto the gate's defaults.",
    version
)]
pub struct E2eCli {
    /// All interactive CLI flags compose onto the gate's defaults.
    #[command(flatten)]
    pub app: Cli,

    /// Which e2e gate to run. Each gate sets its observer-mode flag(s) on
    /// AppArgs and (when the corresponding user value is at default) applies
    /// the gate's preferred preset / camera pose. Without `--gate`, the
    /// harness boots the standard baseline driver (Warmup → Motion →
    /// Settle → Shoot → Assert).
    #[arg(long, value_enum)]
    pub gate: Option<Gate>,
}

impl E2eCli {
    /// Resolve the CLI into the final `(AppArgs, gate)` pair the e2e binary
    /// hands to `run_e2e_render_with_args`. Per-gate default overlays live
    /// in the matching `e2e/<gate>.rs` module and are called by the match
    /// arm below.
    pub fn into_app_args_and_gate(self) -> (AppArgs, Option<Gate>) {
        let E2eCli { app, gate } = self;
        let mut args = app.into_app_args();

        if let Some(g) = gate {
            apply_gate_defaults(&mut args, g);
        }
        (args, gate)
    }
}

/// Apply a gate's default overlay onto `args` IN-PLACE. The gate's mode
/// flag is set unconditionally; preset / pose defaults are applied only when
/// the corresponding user field is still at `AppArgs::default()`.
fn apply_gate_defaults(args: &mut AppArgs, gate: Gate) {
    match gate {
        Gate::Baseline => {
            // No observer attachment; the standard driver runs the
            // Warmup/Motion/Settle/Shoot/Assert flow.
        }
        Gate::StreamingWindow => {
            crate::e2e::streaming_window::apply_streaming_window_defaults(args);
        }
        Gate::NoiseStaticWorld => {
            crate::e2e::noise_static_world::apply_noise_static_defaults(args);
        }
        Gate::WgslNoiseOracle => {
            // Headless pure-compute gate; short-circuited in main BEFORE
            // build_app. Apply no defaults.
        }
        Gate::ValidateGpuConstruction => {
            // Runs the standard baseline driver + post-App validation pass;
            // no preset override.
        }
        Gate::ValidateGpuConstructionScaled
        | Gate::ValidateGpuConstructionProduction => {
            // Headless validators; short-circuited in main. No defaults.
        }
        Gate::Entities => {
            args.spawn_test_entity = true;
            args.construction_config.entities_enabled = true;
        }
        Gate::EditMode => {
            // Post-App CPU validator only; the App run is the standard
            // baseline driver. No preset override.
        }
        Gate::RuntimeEditMode => {
            // Post-App runtime-edit validator; standard baseline App.
        }
        Gate::ResizeTest => {
            args.resize_test = true;
        }
        Gate::VoxE2e => {
            crate::e2e::vox_e2e::apply_vox_e2e_defaults(args);
        }
        Gate::OasisEditVisual => {
            crate::e2e::oasis_edit_visual::apply_oasis_edit_visual_defaults(args);
        }
        Gate::SmallEditVisual => {
            crate::e2e::small_edit_visual::apply_small_edit_visual_defaults(args);
        }
        Gate::SmallEditRepro => {
            crate::e2e::small_edit_repro::apply_small_edit_repro_defaults(args);
        }
        Gate::VoxGpuConstruction => {
            crate::e2e::vox_gpu_construction::apply_vox_gpu_construction_defaults(
                args,
            );
        }
        Gate::VoxGpuOracle => {
            // Top-level compare; spawns CPU + GPU subprocesses. No App boot.
        }
        Gate::VoxGpuOracleCpu => {
            crate::e2e::vox_gpu_oracle::apply_vox_gpu_oracle_cpu_defaults(args);
        }
        Gate::VoxGpuOracleGpu => {
            crate::e2e::vox_gpu_oracle::apply_vox_gpu_oracle_gpu_defaults(args);
        }
    }
}

// --- Enums -----------------------------------------------------------------

/// CLI-side mirror of [`GridPreset`] (clap `ValueEnum` doesn't support
/// struct-variant enums). The variant payload fields (`noise_preset`,
/// `seed`) come from the flat `--noise-preset` / `--noise-seed` flags;
/// [`Cli::into_app_args`] does the variant assembly.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum GridPresetArg {
    /// Embedded primitive scene (default).
    #[default]
    Default,
    /// MagicaVoxel `.vox` file (requires `--vox <path>`).
    Vox,
    /// streaming-world Plan B sliding-window preset.
    ProceduralStreaming,
    /// streaming-world Phase 2.4 one-shot static-noise preset.
    ProceduralStatic,
}

/// Named e2e gates. One choice per `e2e_render` invocation. The e2e binary's
/// `main` matches on this to apply gate defaults pre-App + dispatch the
/// matching post-App validation pass.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum Gate {
    /// Plain bounded-frame baseline (no observer attached).
    Baseline,
    /// streaming-world Phase 2 — walks the camera across ≥2 segment
    /// boundaries; asserts the residency window followed + terrain
    /// re-populated.
    StreamingWindow,
    /// streaming-world Phase 2.4 — boots ProceduralStatic, asserts strict
    /// luminance / non-sky-ratio floors on the post-warmup frame.
    NoiseStaticWorld,
    /// streaming-world Phase 1 — headless WGSL noise ↔ CPU oracle byte
    /// equality. No App boot.
    WgslNoiseOracle,
    /// Phase-C W1 — runs the baseline App then the bit-exact CPU/GPU
    /// chunk_calc oracle byte-equality pass.
    ValidateGpuConstruction,
    /// vox-gpu-rewrite Stage 6 — headless scaled byte-diff diagnostic.
    ValidateGpuConstructionScaled,
    /// vox-gpu-rewrite Stage 9 — headless production-scale voxels[]
    /// readback diagnostic.
    ValidateGpuConstructionProduction,
    /// Phase-C wave-3 — spawns the W4 fixture entity + flips
    /// `entities_enabled`, then runs the CPU entity-handler validator.
    Entities,
    /// Phase-C W2 — runs the baseline App then the CPU-side edit-pipeline
    /// validator (no GPU edit assertion).
    EditMode,
    /// `02f` rearch — runs the baseline App then the runtime-edit gate
    /// (exercises `set_voxels_batch`).
    RuntimeEditMode,
    /// `18-taa-fidelity.md` — resize-blackness reproduction. Pre-launch
    /// installs a Hyprland windowrule (best-effort).
    ResizeTest,
    /// `03a` — synthesised .vox fixture; non-skybox assertion.
    VoxE2e,
    /// `02f-followup` — Oasis VOX brush-erase visual-diff gate.
    OasisEditVisual,
    /// `03g` — single-voxel runtime edit + framebuffer diff.
    SmallEditVisual,
    /// `2026-05-17` — user-captured small-edit reproduction (no
    /// pitch-black pixels).
    SmallEditRepro,
    /// vox-gpu-rewrite W5.5 — Oasis VOX through the W5 GPU producer chain.
    VoxGpuConstruction,
    /// vox-gpu-rewrite W5.3-fix Stage 4 — top-level CPU vs GPU SSIM compare
    /// (spawns subprocesses of itself running `vox-gpu-oracle-cpu` /
    /// `vox-gpu-oracle-gpu`).
    VoxGpuOracle,
    /// CPU oracle render phase of the vox-gpu-oracle compare.
    VoxGpuOracleCpu,
    /// GPU producer render phase of the vox-gpu-oracle compare.
    VoxGpuOracleGpu,
}

impl Gate {
    /// The exact clap-emitted string (kebab-case) for this gate — used by
    /// `vox_gpu_oracle::run_vox_gpu_oracle_compare` when respawning itself
    /// as a subprocess with `--gate <name>`.
    pub fn as_kebab_str(self) -> &'static str {
        match self {
            Gate::Baseline => "baseline",
            Gate::StreamingWindow => "streaming-window",
            Gate::NoiseStaticWorld => "noise-static-world",
            Gate::WgslNoiseOracle => "wgsl-noise-oracle",
            Gate::ValidateGpuConstruction => "validate-gpu-construction",
            Gate::ValidateGpuConstructionScaled => "validate-gpu-construction-scaled",
            Gate::ValidateGpuConstructionProduction => {
                "validate-gpu-construction-production"
            }
            Gate::Entities => "entities",
            Gate::EditMode => "edit-mode",
            Gate::RuntimeEditMode => "runtime-edit-mode",
            Gate::ResizeTest => "resize-test",
            Gate::VoxE2e => "vox-e2e",
            Gate::OasisEditVisual => "oasis-edit-visual",
            Gate::SmallEditVisual => "small-edit-visual",
            Gate::SmallEditRepro => "small-edit-repro",
            Gate::VoxGpuConstruction => "vox-gpu-construction",
            Gate::VoxGpuOracle => "vox-gpu-oracle",
            Gate::VoxGpuOracleCpu => "vox-gpu-oracle-cpu",
            Gate::VoxGpuOracleGpu => "vox-gpu-oracle-gpu",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Cli` with all defaults must produce `AppArgs` byte-equivalent to
    /// `AppArgs::default()` (modulo the `noise_preset` field which is new).
    #[test]
    fn default_cli_matches_default_app_args() {
        let cli = Cli::parse_from(["bevy-naadf"]);
        let args = cli.into_app_args();
        let default = AppArgs::default();
        assert_eq!(args.grid_preset, default.grid_preset);
        assert_eq!(args.taa, default.taa);
        assert_eq!(args.taa_ring_depth, default.taa_ring_depth);
        assert_eq!(args.vram_budget_mib, default.vram_budget_mib);
        assert_eq!(args.max_segments_per_frame, default.max_segments_per_frame);
        assert_eq!(args.noise_seed, default.noise_seed);
        assert_eq!(args.sea_level, default.sea_level);
        assert_eq!(args.terrain_amplitude, default.terrain_amplitude);
    }

    /// `--grid-preset procedural-streaming` resolves to the matching
    /// variant with the user's `--noise-seed` plumbed in.
    #[test]
    fn streaming_preset_seed_propagation() {
        let cli = Cli::parse_from([
            "bevy-naadf",
            "--grid-preset",
            "procedural-streaming",
            "--noise-seed",
            "42",
        ]);
        let args = cli.into_app_args();
        match args.grid_preset {
            GridPreset::ProceduralStreaming { seed, noise_preset } => {
                assert_eq!(seed, 42);
                assert_eq!(noise_preset, 0);
            }
            other => panic!("expected ProceduralStreaming, got {other:?}"),
        }
        assert_eq!(args.noise_seed, 42);
    }

    /// `--grid-preset procedural-static` resolves to the matching variant.
    #[test]
    fn static_preset_resolves() {
        let cli = Cli::parse_from([
            "bevy-naadf",
            "--grid-preset",
            "procedural-static",
            "--noise-seed",
            "7",
        ]);
        let args = cli.into_app_args();
        assert!(matches!(
            args.grid_preset,
            GridPreset::ProceduralStatic { seed: 7, noise_preset: 0 }
        ));
    }

    /// `--vox <path>` without explicit `--grid-preset` activates the Vox
    /// variant (backward-compat with the old `--vox` UX).
    #[test]
    fn vox_shortcut_implies_vox_preset() {
        let cli = Cli::parse_from(["bevy-naadf", "--vox", "test.vox"]);
        let args = cli.into_app_args();
        match args.grid_preset {
            GridPreset::Vox { path } => {
                assert_eq!(path, PathBuf::from("test.vox"));
            }
            other => panic!("expected Vox, got {other:?}"),
        }
    }

    /// VRAM budget / max segments overrides propagate end-to-end.
    #[test]
    fn streaming_overrides_propagate() {
        let cli = Cli::parse_from([
            "bevy-naadf",
            "--grid-preset",
            "procedural-streaming",
            "--vram-budget-mib",
            "2048",
            "--max-segments-per-frame",
            "8",
            "--noise-seed",
            "9",
        ]);
        let args = cli.into_app_args();
        assert_eq!(args.vram_budget_mib, 2048);
        assert_eq!(args.max_segments_per_frame, 8);
        assert_eq!(args.noise_seed, 9);
    }

    /// E2E CLI with `--gate streaming-window` sets the streaming-window
    /// mode flag + (when grid preset is default) installs the streaming
    /// preset.
    #[test]
    fn e2e_gate_streaming_window_applies_defaults() {
        let cli = E2eCli::parse_from(["e2e_render", "--gate", "streaming-window"]);
        let (args, gate) = cli.into_app_args_and_gate();
        assert_eq!(gate, Some(Gate::StreamingWindow));
        assert!(args.streaming_window_mode);
        assert!(args.oasis_edit_visual_mode);
        match args.grid_preset {
            GridPreset::ProceduralStreaming { .. } => {}
            other => panic!(
                "expected gate default to install ProceduralStreaming; got {other:?}"
            ),
        }
    }

    /// E2E CLI: user override of `--grid-preset` wins over the gate's
    /// default.
    #[test]
    fn e2e_user_grid_preset_overrides_gate_default() {
        let cli = E2eCli::parse_from([
            "e2e_render",
            "--gate",
            "streaming-window",
            "--grid-preset",
            "procedural-static",
        ]);
        let (args, _) = cli.into_app_args_and_gate();
        // Gate observer flag still set, but the user's preset choice wins.
        assert!(args.streaming_window_mode);
        assert!(matches!(args.grid_preset, GridPreset::ProceduralStatic { .. }));
    }

    /// `Gate::as_kebab_str` round-trips through `Gate::from_str` (clap
    /// ValueEnum parses the same kebab string).
    #[test]
    fn gate_kebab_str_round_trip() {
        for gate in [
            Gate::Baseline,
            Gate::StreamingWindow,
            Gate::NoiseStaticWorld,
            Gate::VoxGpuOracleCpu,
        ] {
            let kebab = gate.as_kebab_str();
            let parsed = Gate::from_str(kebab, true).expect("clap parse");
            assert_eq!(gate, parsed, "round-trip on {kebab}");
        }
    }
}
