# NAADF -> Kova Port

Orchestrated port of the NAADF voxel engine into Kova. NAADF lives at `/mnt/archive4/DEV/NAADF`; Kova lives at `/mnt/archive4/DEV/Kova` and is the destination. Port outputs land **in Kova** — no edits to NAADF.

## Files

| Path | Purpose |
|---|---|
| `README.md` | This index + phase checklist. |
| `01-context.md` | Canonical brief every sub-agent reads first. |
| `00-reuse-audit.md` | Auditor's table of what Kova already has vs. what's missing. |
| `02-design.md` | Architect's plan: phased port breakdown, file-by-file diff plan. |
| `03-impl.md` | Implementer logs, one section per phase. |
| `04-review.md` | Final verification against NAADF feature parity + Kova rules. |

## Agent groups

- **audit** (`delegate-auditor`) — reuse audit. Output → `00-reuse-audit.md`. Complete.
- **design** (`delegate-architect`) — phased port design. Output → `02-design.md`.
- **impl** (`general-purpose`, sub-dispatched per phase) — code work. Output → `03-impl.md`.
- **review** (`general-purpose`) — parity + rules check. Output → `04-review.md`.

## Phase checklist (architect's final ordering — see `02-design.md` section 10)

- [x] Audit: enumerate Kova facilities vs. NAADF subsystems.
- [x] Design: phased plan with explicit reuse + extension + port + rewrite decisions.
- [x] Impl **P1** — Lift `Kova.VoxelsCore` (13 NAADF library files copied + namespaced). 29/29 tests pass.
- [x] Impl **P2** — `.vox`/`.vl32` importer wired into `Kova.AssetPipeline` + `.kvox` writer. `.cvox` stubbed pending VoxelType port. 5/5 new tests pass.
- [x] Impl **P0** — Compute primitives in `IGraphicsDevice` + WebGPU backend (compute pipelines, storage buffers, bind groups, dispatch, 3D textures). E2E compute smoke test passes (write 42 / read 42). 36/36 tests.
- [x] Impl **P3a** — `WorldData` skeleton + `BlockHashingHandler` + handler stubs. GPU-gated construct/dispose test runs end-to-end on wgpu-native. 38/38 tests.
- [x] Impl **P3b** — Wire `WorldData.GenerateWorld` to GPU world-gen path. Full chunk_calc / map_copy / generator_model / data_copy WGSL ports running end-to-end. 39/39 tests.
- [x] Impl **P4** — AADF generation ported to WGSL. 3 entry points, full Update() dispatch, distance bits advance per-frame. 40/40 tests.
- [x] Impl **P5** — Chunks buffer→texture sync pass. Entity-bearing path deferred (rg32uint blocked on adapter feature). 40/40 tests.
- [x] Impl **P6a** — Primary-ray + atmosphere + final pass (no GI). 9 unique colors rendered from voxel world; first-frame integration sentinel passes. 41/41 tests.
- [x] Impl **P6a.fix** — Perspective correctness gate. IoU = **1.0000** vs. analytical sphere. 3 compounding bugs fixed: matmul order, view-matrix translation double-count, spurious-hit epilogue. 43/43 tests.
- [ ] Impl **P6a.fix.2** — Tighter analytical-surface test suite. Single-sphere/symmetric-camera test had blind spots (rotational symmetry, X,Y,Z-symmetric basis, tiny world). New suite: multiple spheres (grid), single torus, multiple tori in different orientations; multiple non-symmetric camera angles per scene; off-origin scene to stress `PositionInt`/`PositionFrac` precision. CPU reference derived independently from first principles, not cloned from the WGSL.
- [ ] Impl **P6b** — Secondary rays (GI) + ReSTIR + denoise + TAA. **Deferred — rendering-quality polish layered on top of P6a's working pipeline.**
- [ ] Impl **P7** — CPU editing tools + entity logic. **Deferred — feature addition, not core-port-completion.**
- [ ] Impl **P8** — First-person camera + voxel-viewer scene (`Kova.Voxels.Viewer`, `IInput` in core, render-pipeline bind-group overloads, `render_final.wgsl`).
- [ ] Review: parity vs. NAADF; conformance to Kova rules (no backwards-compat, dead code deleted).
