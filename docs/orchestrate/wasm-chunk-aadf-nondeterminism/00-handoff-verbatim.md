# Handoff: wasm chunk-AADF acceleration ray-termination non-determinism

## Why this handoff exists

I attempted to fix a WebGPU-specific bug where the voxel raymarcher's
rays terminate short of distant geometry, leaving "ocean visible where
the native build renders city." I rolled through several theories (Q4
storage-buffer-binding overrun → refuted; large-dispatch GPU-watchdog →
refuted; storage→indirect barrier on the bound-dispatch buffer →
hypothesis, partially mitigated with a wasm-only direct-dispatch path;
cross-pass atomic visibility on `bound_queue_info[].size` →
hypothesis, attempted per-round encoder+submit fix; convergence-rate
slowness from a 4096-cap throttle). The user has confirmed all of
these mitigations produce **non-deterministic results**: the SSIM
gate's measured value varies run-to-run (0.69, 0.79, 0.928, 0.94) on
identical inputs. The user observed ray reach grow from 50 % → 100 %
over ~1 s on ONE run; the same test config does not reproduce
afterwards. I'm out of context to continue and my theories are
contradicting each other — I did NOT solve this.

## Symptom

Cross-target SSIM gate
(`e2e/tests/vox-horizon-parity.spec.ts`) at the user-captured camera
pose:

- `translation = (3880.187, 497.332, 3514.350)` voxels
- `rotation = Quat(-0.09791362, 0.5846077, 0.07135339, 0.8022191)`
- `forward = (-0.924, -0.241, -0.297)`

Camera sits ~15 voxels under the 512-voxel world ceiling, looking
horizontally across the 4×4-tile Oasis. Native release rendering
covers the full city to the world-boundary ocean line; the WASM/WebGPU
canvas truncates distant geometry — distant city → ocean → sky. The
truncation distance varies run-to-run from ~30 % of world depth to
~100 %.

User repro:
1. `just web-build-release` followed by `just web-static`, or
2. `just test-wasm-full` (Playwright gate).

Reference captures in `target/e2e-screenshots/`:
- `vox_horizon_native.png` — native release, correct
- `vox_horizon_web.png` — WASM, variably truncated

Diagnostic probe outputs (persisted by the Playwright spec) in:
- `target/e2e-screenshots/vox_horizon_native.aadf-probe.log`
- `target/e2e-screenshots/vox_horizon_web.aadf-probe.log`

The probe dumps the chunk-AADF skip-distance bits at sample positions
along the view ray. Native consistently shows skip distances of 3–4
chunks per direction. Web shows wildly varying values per run, often
0–1.

## Where to start reading

These are where the symptom surfaces. Not "where the fix goes" — open
investigation only.

- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:342-460` —
  `naadf_bounds_compute_node` (the W3 regime-2 background loop). The
  wasm-only branch at 365-417 dispatches each round as its own
  encoder+submit; the native branch (419+) uses one encoder.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` —
  `prepare_group_bounds` (264-321) and `compute_group_bounds`
  (323-460). The cross-pass writes between them flow through
  `bound_queue_info` (atomic `size`), `bound_refined_info` (currently
  non-atomic — was atomic briefly, broke worse), and the chunks
  storage buffer.
- `crates/bevy_naadf/src/render/construction/mod.rs:1750-1860` — the
  seed dispatch (`add_initial_groups_to_bound_queue`) and the
  bound-queue family buffer allocation + initial write_buffer of
  per-axis queue sizes.
- `crates/bevy_naadf/src/render/construction/mod.rs:2090-2150` — the
  bounds_initialized gating and the regime-1 seed-once dispatch.
- `crates/bevy_naadf/src/render/construction/mod.rs:1042-1465` —
  `populate_cpu_mirror_from_gpu_producer`'s cross-frame readback +
  the inline AADF probe1 sample (1465-1565) that dumps web's
  post-init chunk word values.
- `crates/bevy_naadf/src/render/construction/mod.rs:3275-3565` — the
  delayed probe2 system that reads back chunks + bound_queue_info +
  bound_refined_info at frames 30 and 200 post-mirror. Useful for
  observing per-round state.
- `crates/bevy_naadf/src/render/construction/config.rs:194-219` —
  `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` cap and its `From<&AppArgs>`
  clamp.
- `crates/bevy_naadf/src/voxel/web_vox.rs` — wasm-only systems that
  drive the live build's `?pose=horizon` + `?ui=hide` overrides and
  the camera pin.
- `e2e/tests/vox-horizon-parity.spec.ts` — the gate; captures
  + persists the probe logs.

## Already tried (do not revisit)

- Q4 storage-buffer-binding-size overrun — REFUTED. Buffers (chunks 16
  MiB / blocks 512 MiB / voxels 1 GiB) all fit within
  `max_storage_buffer_binding_size = 2047 MiB` reported by Dawn (logged
  by Q4 instrumentation at `prepare.rs:535-571`).
- Browser GPU watchdog killing the 134M-workgroup
  `compute_voxel_bounds` dispatch — REFUTED. Split the dispatch into 8
  batches of 16M (SSIM moved 0.789 → 0.811, noise) and 128 batches of
  1M (SSIM dropped to 0.793). Reverted.
- Raising `WASM_MAX_GROUP_BOUND_DISPATCH` from 4096 → 32768 (native
  default) — REGRESSED SSIM from ~0.94 to ~0.69. Reverted to 4096.
- Converting `bound_refined_info` from `array<u32>` to
  `array<atomic<u32>>` with atomicStore/atomicLoad everywhere —
  REGRESSED: chunks state went from "Z bumped once" to "nothing
  expanded at all" (word=0x00000000). Reverted to plain u32.
- Per-round encoder+submit on wasm32 (instead of one encoder per node
  invocation, to force fence between regime-2 rounds) — neutral effect
  on the SSIM number (~0.79). Currently still in place at
  `bounds_calc.rs:365-417`.
- Increasing Playwright `CANVAS_SETTLE_MS` from 10 s to 30 s — neutral
  effect. The web canvas does NOT show progressive ray extension over
  the 30 s settle on a "broken" run; the user separately observed
  progressive extension on a single "lucky" run but it does not
  reproduce.
- Disabling the in-canvas UI via `?ui=hide` URL param — fixed an
  earlier UI-contamination false-pass; SSIM contamination by the
  brush palette overlay is no longer in play.
- Adding `apply_initial_camera_pose_changes` system + overriding the
  `install_imported_vox` camera spawn to the test pose — orthogonal to
  the bug; the test was already running with a pinned camera. The
  override does what it claims (live build now spawns at the gate's
  pose).
- The Playwright spec runs the WASM canvas in headed Chrome stable
  with the SAME `--enable-unsafe-webgpu` `--enable-webgpu-developer-features`
  flags the user runs locally. So "Playwright Chrome vs user Chrome
  divergence" is REFUTED.

## Forbidden moves

- The project's CLAUDE.md forbids `cargo run --bin bevy-naadf` /
  any "boot the binary for N seconds" as a verification step
  (`/mnt/archive4/DEV/bevy-naadf/CLAUDE.md`). Verification surface is
  the named e2e gates + `e2e/tests/vox-horizon-parity.spec.ts`.
- `HORIZON_SSIM_SIMILARITY_MIN` must NOT be lowered. User-pinned at
  0.91 (`vox_horizon_parity.rs:133` + `e2e/tests/vox-horizon-parity.spec.ts:65`).
- Do not raise `MAX_RAY_STEPS_PRIMARY`. User explicitly forbade
  bumping the per-ray step budget as a workaround — the fix must not
  hide the bug via more steps.
- Do not commit without the user's instruction.
- Do not push to remote.
- Do not run `cargo run --bin bevy-naadf` for visual verification —
  the SSIM gate IS the verification.

## Deliverable

The next session's reply MUST contain, in order:

1. **Investigation findings.** What you actually observed by reading
   code + running the gate + reading the persisted probe logs. Not
   speculation — observed facts. Re-run the gate at least once to see
   the current SSIM and capture fresh probe data, since the bug is
   non-deterministic.
2. **Diagnosis.** What you concluded the failure mechanism is, with
   evidence tying it to the observed probe data. If you cannot pin a
   single mechanism, say so explicitly and report what the evidence
   does and doesn't support.
3. **Proposed fix.** With explicit file:line refs for the changes.
4. **Verification plan.** How the next agent (or user) confirms the
   fix actually lands and doesn't regress under the non-determinism
   the prior session saw. The gate must run 3+ times to demonstrate
   stability.

## Repro / env

- Worktree: `/mnt/archive4/DEV/bevy-naadf`
- Branch: `main`
- Native gate: `cargo run --bin e2e_render -- --vox-horizon-native`
  → writes `target/e2e-screenshots/vox_horizon_native.png`
- WASM build: `just web-build-release` (trunk → `crates/bevy_naadf/dist/`)
- Cross-target gate (the SSIM compare):
  `cd e2e && npx playwright test vox-horizon-parity.spec.ts --headed`
  → writes `target/e2e-screenshots/vox_horizon_web.png` + the
  `vox_horizon_{native,web}.aadf-probe.log` diagnostic files
- Live web (for human visual verification): `just web-static` →
  opens Chrome at `http://127.0.0.1:8080` with the WebGPU dev flags
- The bug is wasm32 / WebGPU-only. Native release is the correct
  reference. The wasm build's WebGPU adapter reports
  `max_storage_buffer_binding_size = 2147483644` (2 GiB - 4).
- Fixture: `crates/bevy_naadf/assets/test/oasis.cvox` (Git LFS).
- Camera pose constants:
  `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:HORIZON_CAMERA_POS`
  / `HORIZON_CAMERA_ROT`.
