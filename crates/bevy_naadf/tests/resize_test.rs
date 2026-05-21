//! BRP-driven e2e gate — `resize_test`, migrated from the legacy in-app
//! `e2e_render --resize-test` driver mode (the `ResizeTestState` driver phases
//! + `bin/e2e_render.rs::run_resize_test`)
//! (`e2e-ipc-rpc-restructure` Phase 3b).
//!
//! ## What this gate proves
//!
//! The GI-bounce-on-resize regression repro (`e2e/mod.rs` resize-test const
//! doc + `18-taa-fidelity.md` `## GI-bounce-on-resize fix (2026-05-16)`): boot
//! the windowed app, capture a baseline frame, resize the window twice, and
//! after each resize assert the full-frame mean luminance has NOT collapsed —
//! a ≥ 30% drop is the bug signal (the TAA/GI sample rings drained to black
//! and never refilled after the viewport changed). The fix caps
//! `sample_refine.wgsl` padded dispatch groups at 32 768 so wgpu's
//! indirect-validation pass does not zero the dispatch args at viewports
//! ≥ 1920×1080.
//!
//! ## Design D10 — what was confirmed, and the residual coupling (LOGGED LOUDLY)
//!
//! Design D10 chose to drive the resize with the `naadf/resize_window` BRP
//! verb (mutates `Window::resolution` → `bevy_winit`'s `changed_windows`
//! issues a winit `request_inner_size`), judging this exercises the same
//! resize→blackness repro a compositor-driven resize does, and aiming to
//! "drop the Hyprland dependency entirely". The migrating Phase-3b agent
//! confirmed against the gate's assertion intent — and found D10 is **half
//! right**:
//!
//! - **The repro is genuinely resize-mechanism-agnostic.** The bug is the
//!   TAA/GI ring drain on a viewport change; the gate asserts on full-frame
//!   luma. *Any* path that reconfigures the swapchain to a new size exercises
//!   it — there is nothing compositor-specific in the bug or the assertion.
//!   So D10's core judgement holds.
//!
//! - **But on a tiling Wayland compositor (Hyprland) the `naadf/resize_window`
//!   verb cannot drive a resize at all.** Verified at runtime (Phase 3b): even
//!   with a `resizable: true`, *floating* SUT window, winit's
//!   `request_inner_size` is a client *request* the compositor refuses — the
//!   `Window::resolution` mutation never propagated, all three captures stayed
//!   at the boot size. The legacy gate hit the *identical* wall and worked
//!   around it the only way that works on Hyprland: it shelled
//!   `hyprctl dispatch resizewindowpixel` — a *compositor command* that
//!   forcibly resizes the window — and used a `float on` windowrule so the
//!   window was resizable by the compositor.
//!
//! **Resolution (honest, not papering over).** This gate drives the resize
//! the same proven way the legacy gate did — `hyprctl resizewindowpixel`
//! (a compositor-driven resize) when running under Hyprland — and falls back
//! to the `naadf/resize_window` verb otherwise (correct on stacking / floating
//! WMs and on X11, where `request_inner_size` is honoured). The BRP verb is
//! kept in the codebase as the platform-neutral mechanism; it is simply not
//! sufficient on a tiling Wayland WM. D10's "drop the Hyprland dependency
//! entirely" goal is therefore **not fully met** — the resize driver still
//! needs `hyprctl` under Hyprland. This is a genuine residual coupling, NOT a
//! migration defect: it is a hard fact about how tiling Wayland compositors
//! treat client size requests, and the legacy gate carried the exact same
//! coupling. Flagged in `03-impl.md` Phase 3b side-notes for the orchestrator.
//!
//! ## Spawn flags
//!
//! - `--e2e-resizable` makes the SUT window `resizable: true` AND pins its
//!   `app_id` to `bevy_naadf_e2e` (boot-time window-creation attributes →
//!   spawn contract). `resizable` is required for the compositor to resize
//!   the surface at all; the deterministic `app_id` lets the `hyprctl`
//!   `class:` selector + the `float on` windowrule target the SUT window.
//!   This mirrors the legacy `WindowConfig::e2e_resize_test` (which set the
//!   same two fields).
//!
//! Step 8a below asserts the captures *actually* changed size, so if the
//! resize silently no-ops (windowrule failed, compositor refused, `hyprctl`
//! missing) the gate fails loudly instead of passing trivially on three
//! identical frames.
//!
//! ## Migration fidelity (Phase 3b brief — binding)
//!
//! The three window sizes (`E2E_RESIZE_BOOT_*` / `_A_*` / `_B_*`), the camera
//! pose (`e2e_resize_test_camera_transform`), and the luma-ratio floor
//! (`E2E_RESIZE_MIN_LUMA_RATIO`) are reused from the library **verbatim**.
//! The `full_frame_luma` + ratio assertion (private fns in `e2e/driver.rs`)
//! is ported verbatim into this test file; `Framebuffer::region_luminance` is
//! reused unchanged. No threshold is recalibrated.
//!
//! The per-step wait *counts* (`E2E_RESIZE_LAUNCH_SETTLE_FRAMES` /
//! `E2E_RESIZE_WAIT_FRAMES`, both 300) are reused as the `naadf/step` budgets
//! — they are settle counts, not assertion thresholds.
//!
//! ## How to run
//!
//! ```text
//! cargo test -p bevy-naadf --features e2e-brp --test resize_test
//! ```

use naadf_e2e::{scenario, Sut, SutOpts};

use bevy_naadf::e2e::framebuffer::{Framebuffer, Rect};
use bevy_naadf::e2e::gates::e2e_resize_test_camera_transform;

// ---------------------------------------------------------------------------
// Constants — ported VERBATIM from `crates/bevy_naadf/src/e2e/mod.rs`.
// ---------------------------------------------------------------------------

/// `E2E_RESIZE_BOOT_WIDTH` / `_HEIGHT` — the 800×600 boot window.
const E2E_RESIZE_BOOT_WIDTH: u32 = 800;
const E2E_RESIZE_BOOT_HEIGHT: u32 = 600;
/// `E2E_RESIZE_A_WIDTH` / `_HEIGHT` — the first resize target, 1920×1080.
const E2E_RESIZE_A_WIDTH: u32 = 1920;
const E2E_RESIZE_A_HEIGHT: u32 = 1080;
/// `E2E_RESIZE_B_WIDTH` / `_HEIGHT` — the second resize target, 2000×1000.
const E2E_RESIZE_B_WIDTH: u32 = 2000;
const E2E_RESIZE_B_HEIGHT: u32 = 1000;
/// `E2E_RESIZE_LAUNCH_SETTLE_FRAMES` — settle frames before the baseline capture.
const E2E_RESIZE_LAUNCH_SETTLE_FRAMES: u32 = 300;
/// `E2E_RESIZE_WAIT_FRAMES` — settle frames after each resize before capture.
const E2E_RESIZE_WAIT_FRAMES: u32 = 300;
/// `E2E_RESIZE_MIN_LUMA_RATIO` — the load-bearing assertion threshold: a single
/// resize step PASSes iff its post-resize / baseline full-frame luma ratio is
/// ≥ this. A ≥ 30% drop fails. **Ported verbatim — never recalibrated.**
const E2E_RESIZE_MIN_LUMA_RATIO: f32 = 0.7;

/// The SUT window's Wayland `app_id` / X11 `WM_CLASS` when spawned with
/// `--e2e-resizable` (`main.rs` pins `Window.name = Some("bevy_naadf_e2e")`).
/// The `hyprctl` `class:` selector + the `float on` windowrule target this.
const SUT_RESIZE_APP_ID: &str = "bevy_naadf_e2e";

/// Full-frame mean luminance — verbatim port of the private
/// `e2e/driver.rs::full_frame_luma`. The honest discriminator for the
/// ring-drain bug (the legacy `solid_block_rect` was pose-tuned for 256×256
/// and did not catch the bug reliably at other resolutions).
fn full_frame_luma(fb: &Framebuffer) -> f32 {
    fb.region_luminance(Rect {
        x0: 0,
        y0: 0,
        x1: fb.width(),
        y1: fb.height(),
    })
}

/// Whether the test is running under Hyprland (the resize driver branches on
/// this — see the module doc, D10).
fn under_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Install a Hyprland `float on` windowrule for the SUT window so it spawns
/// floating — `hyprctl resizewindowpixel` only resizes a *floating* window
/// (a tiled window's size is the tile slot's). Mirrors the legacy
/// `bin/e2e_render.rs::install_resize_test_windowrule`. Best-effort; no-op
/// off Hyprland.
fn install_float_windowrule() {
    if !under_hyprland() {
        println!(
            "resize_test: HYPRLAND_INSTANCE_SIGNATURE not set — skipping the \
             `float on` windowrule install; the resize driver will use the \
             `naadf/resize_window` BRP verb (correct on stacking/floating WMs \
             + X11)"
        );
        return;
    }
    let rule = format!("match:class ^({SUT_RESIZE_APP_ID})$, float on");
    match std::process::Command::new("hyprctl")
        .args(["keyword", "windowrule", &rule])
        .status()
    {
        Ok(s) => println!("resize_test: hyprctl keyword windowrule '{rule}' -> {s:?}"),
        Err(e) => eprintln!(
            "resize_test: hyprctl keyword windowrule FAILED to spawn: {e} — \
             the SUT window may stay tiled; the resize-took-effect guard \
             (step 8a) will catch it"
        ),
    }
}

/// Discard the runtime `float on` windowrule by reloading the Hyprland config
/// from disk. Mirrors the legacy `cleanup_resize_test_windowrule`. Best-effort.
fn cleanup_float_windowrule() {
    if !under_hyprland() {
        return;
    }
    match std::process::Command::new("hyprctl")
        .args(["reload"])
        .status()
    {
        Ok(s) => println!("resize_test: post-run hyprctl reload -> {s:?}"),
        Err(e) => eprintln!("resize_test: post-run hyprctl reload FAILED to spawn: {e}"),
    }
}

/// Resize the SUT window to `(width, height)` physical pixels.
///
/// Under Hyprland: `hyprctl dispatch resizewindowpixel exact W H,class:<app_id>`
/// — a compositor-driven resize, the proven mechanism (the BRP
/// `naadf/resize_window` verb's `request_inner_size` is refused by Hyprland;
/// see the module doc, D10). winit then receives the compositor's resize
/// event and reconfigures the swapchain, so the TAA/GI ring drain is
/// exercised exactly as it would be by a user-driven resize.
///
/// Off Hyprland: the `naadf/resize_window` BRP verb (`Window::resolution`
/// mutation → winit `request_inner_size`), which stacking/floating WMs + X11
/// honour.
fn resize_window(sut: &mut Sut, label: &str, width: u32, height: u32) {
    if under_hyprland() {
        let arg = format!("exact {width} {height},class:{SUT_RESIZE_APP_ID}");
        match std::process::Command::new("hyprctl")
            .args(["dispatch", "resizewindowpixel", &arg])
            .output()
        {
            Ok(o) => println!(
                "resize_test: [{label}] hyprctl dispatch resizewindowpixel '{arg}' \
                 -> exit {:?} stdout={:?}",
                o.status,
                String::from_utf8_lossy(&o.stdout).trim(),
            ),
            Err(e) => panic!(
                "resize_test: [{label}] hyprctl dispatch resizewindowpixel FAILED \
                 to spawn: {e}"
            ),
        }
    } else {
        scenario::resize_window(sut.client(), width, height)
            .unwrap_or_else(|e| panic!("resize_test: [{label}] naadf/resize_window: {e}"));
    }
}

/// Pin the resize-test camera. The legacy `pin_resize_test_camera` writes
/// `e2e_resize_test_camera_transform()` — a fixed pose for the whole sequence
/// (no orbit motion), so any luma collapse between captures is attributable
/// to the resize, not camera motion. The pose is built with `Vec3::Y` up.
fn pin_camera(sut: &mut Sut) {
    let pose = e2e_resize_test_camera_transform();
    let fwd = pose.forward();
    scenario::set_camera(
        sut.client(),
        [pose.translation.x, pose.translation.y, pose.translation.z],
        [
            pose.translation.x + fwd.x,
            pose.translation.y + fwd.y,
            pose.translation.z + fwd.z,
        ],
        Some([0.0, 1.0, 0.0]),
    )
    .expect("naadf/set_camera");
}

#[test]
fn resize_test() {
    println!(
        "resize_test: boot {E2E_RESIZE_BOOT_WIDTH}x{E2E_RESIZE_BOOT_HEIGHT} \
         -> resize A {E2E_RESIZE_A_WIDTH}x{E2E_RESIZE_A_HEIGHT} \
         -> resize B {E2E_RESIZE_B_WIDTH}x{E2E_RESIZE_B_HEIGHT}; \
         luma-ratio floor {E2E_RESIZE_MIN_LUMA_RATIO:.2}"
    );

    // 0. Install the Hyprland `float on` windowrule BEFORE spawning the SUT so
    //    the window comes up floating — `hyprctl resizewindowpixel` only
    //    resizes a floating window (module doc, D10).
    install_float_windowrule();

    // 1. Spawn the SUT at the 800×600 boot window, RESIZABLE — no `--vox` ⇒
    //    the `GridPreset::Default` embedded test scene (the legacy resize-test
    //    runs the standard scene; the resize camera frames the back wall +
    //    box A so shadowed regions fill a significant fraction of the frame).
    let mut sut = Sut::spawn(
        SutOpts::new(env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR"))
            .window(E2E_RESIZE_BOOT_WIDTH, E2E_RESIZE_BOOT_HEIGHT)
            .resizable(true),
    );

    // 2. World presence check.
    let state = scenario::get_state(sut.client()).expect("naadf/get_state");
    assert!(
        state.world_loaded,
        "resize_test: SUT reports world_loaded=false — the default test grid \
         failed to install"
    );

    // 3. Pin the fixed resize-test camera pose.
    pin_camera(&mut sut);

    // 4. Settle at the boot size, then capture the baseline frame.
    scenario::advance(sut.client(), E2E_RESIZE_LAUNCH_SETTLE_FRAMES)
        .expect("launch-settle advance");
    let initial = scenario::capture(sut.client()).expect("capture initial");
    println!(
        "resize_test: initial capture {}x{}",
        initial.width(),
        initial.height()
    );

    // 5. Resize A (1920×1080), settle, capture.
    resize_window(&mut sut, "A", E2E_RESIZE_A_WIDTH, E2E_RESIZE_A_HEIGHT);
    scenario::advance(sut.client(), E2E_RESIZE_WAIT_FRAMES).expect("post-resize-A advance");
    let after_a = scenario::capture(sut.client()).expect("capture after_a");
    println!(
        "resize_test: after_resize_a capture {}x{}",
        after_a.width(),
        after_a.height()
    );

    // 6. Resize B (2000×1000), settle, capture.
    resize_window(&mut sut, "B", E2E_RESIZE_B_WIDTH, E2E_RESIZE_B_HEIGHT);
    scenario::advance(sut.client(), E2E_RESIZE_WAIT_FRAMES).expect("post-resize-B advance");
    let after_b = scenario::capture(sut.client()).expect("capture after_b");
    println!(
        "resize_test: after_resize_b capture {}x{}",
        after_b.width(),
        after_b.height()
    );

    // 7. The captures are done — discard the runtime `float on` windowrule now
    //    (before the assertions, so an assertion panic does not leak it).
    cleanup_float_windowrule();

    // Save all three PNGs (legacy filenames) for visual inspection.
    let _ = initial.save_png("target/e2e-screenshots/resize_initial.png");
    let _ = after_a.save_png("target/e2e-screenshots/resize_a.png");
    let _ = after_b.save_png("target/e2e-screenshots/resize_b.png");

    // 8a. Resize-took-effect guard (Phase 3b migration finding). The legacy
    //     gate could not pass on three identical frames — the compositor
    //     actually resized the window. If the resize silently no-ops (the
    //     windowrule failed, the compositor refused, `hyprctl` is missing) all
    //     three captures stay at the boot size and the luma-ratio check below
    //     would pass trivially. Assert the captures actually grew so that
    //     failure mode fails the gate loudly instead.
    assert_eq!(
        (initial.width(), initial.height()),
        (E2E_RESIZE_BOOT_WIDTH, E2E_RESIZE_BOOT_HEIGHT),
        "resize_test: the baseline capture is not the {E2E_RESIZE_BOOT_WIDTH}x\
         {E2E_RESIZE_BOOT_HEIGHT} boot window"
    );
    assert_eq!(
        (after_a.width(), after_a.height()),
        (E2E_RESIZE_A_WIDTH, E2E_RESIZE_A_HEIGHT),
        "resize_test: the post-resize-A capture is {}x{}, not the requested \
         {E2E_RESIZE_A_WIDTH}x{E2E_RESIZE_A_HEIGHT} — the resize did not take \
         effect (windowrule failed? compositor refused? `hyprctl` missing?). \
         The gate cannot exercise the resize-blackness repro without a real \
         resize. See the module doc (D10).",
        after_a.width(),
        after_a.height()
    );
    assert_eq!(
        (after_b.width(), after_b.height()),
        (E2E_RESIZE_B_WIDTH, E2E_RESIZE_B_HEIGHT),
        "resize_test: the post-resize-B capture is {}x{}, not the requested \
         {E2E_RESIZE_B_WIDTH}x{E2E_RESIZE_B_HEIGHT} — the second resize did \
         not take effect.",
        after_b.width(),
        after_b.height()
    );

    // 8b. Assertion — verbatim port of `e2e/driver.rs::run_resize_test_assertions`'s
    //    luma-ratio check. Both post-resize captures' full-frame luma must hold
    //    ≥ `E2E_RESIZE_MIN_LUMA_RATIO` of the baseline.
    let luma_initial = full_frame_luma(&initial);
    let luma_a = full_frame_luma(&after_a);
    let luma_b = full_frame_luma(&after_b);
    assert!(
        luma_initial > 1.0e-3,
        "resize_test: initial full-frame luminance is essentially zero \
         ({luma_initial:.3}); the harness never produced a lit image"
    );
    let ratio_a = luma_a / luma_initial;
    let ratio_b = luma_b / luma_initial;
    println!(
        "resize_test: luma — initial {luma_initial:.2}, \
         after_a {luma_a:.2} (ratio {ratio_a:.4}), \
         after_b {luma_b:.2} (ratio {ratio_b:.4}); threshold {E2E_RESIZE_MIN_LUMA_RATIO:.2}"
    );

    let fail_a = ratio_a < E2E_RESIZE_MIN_LUMA_RATIO;
    let fail_b = ratio_b < E2E_RESIZE_MIN_LUMA_RATIO;
    assert!(
        !(fail_a || fail_b),
        "resize_test gate FAIL — GI bounce light went black after window resize.\n  \
         initial  ({}x{}) full-frame luma = {luma_initial:.2}\n  \
         resize_a ({}x{}) full-frame luma = {luma_a:.2}, ratio = {ratio_a:.4} [{}]\n  \
         resize_b ({}x{}) full-frame luma = {luma_b:.2}, ratio = {ratio_b:.4} [{}]\n  \
         threshold = {E2E_RESIZE_MIN_LUMA_RATIO:.2}\n  \
         Regression of the GI-bounce-on-resize fix — see\n  \
         `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`\n  \
         `## GI-bounce-on-resize fix (2026-05-16)`. The fix caps\n  \
         `sample_refine.wgsl` padded dispatch groups at 32 768; if this gate\n  \
         trips again, that cap is the first thing to check. Inspect\n  \
         crates/bevy_naadf/target/e2e-screenshots/resize_{{initial,a,b}}.png.",
        initial.width(),
        initial.height(),
        after_a.width(),
        after_a.height(),
        if fail_a { "FAIL" } else { "pass" },
        after_b.width(),
        after_b.height(),
        if fail_b { "FAIL" } else { "pass" },
    );

    // 9. Pipeline-error scan.
    scenario::pipeline_scan(sut.client()).expect("naadf/pipeline_scan reported failures");

    println!(
        "resize_test: PASS — both post-resize captures held luma ratio \
         >= {E2E_RESIZE_MIN_LUMA_RATIO:.2} (after_a {ratio_a:.4}, after_b {ratio_b:.4})"
    );
}
