# 01 — Canonical context

Every non-review agent reads this file first. **Review agents read only
`04-review.md`** (deliberately denied the design rationale — see `README.md`).

---

## Goal (verbatim from user)

> lets implement a way to open or stream large .vox files (2.1G - upper limit
> of vox file) … we have to be able to demo large voxel worlds … yeah i guess
> we should go in a direction of our own streamable sparse voxel world format
> — we actually intend to generate worlds with procedural noise, but loading
> premade worlds allows for greater showcase potential — so we would
> implement a world import process for that. this would also require a
> sliding window approach, for streaming in worlds larger than VRAM allows
> under specified VRAM budget, generating infinite worlds, a large
> coordinate system. lets focus this session on implementing procedural
> world generation with a sliding window with fixed VRAM budget

### Session scope

- **In scope:** procedural-noise generation (via `voxel_noise` / fastnoise2)
  feeding a sliding-window residency layer that holds at most a fixed-VRAM
  budget worth of voxel data at once; new `--streaming-window` e2e gate.
- **Groundwork (must be enabled, may not be wired end-to-end):** large
  coordinate system (residency manager keys chunks on `i32` chunk-coords,
  giving ±2 billion chunks); clean seam for a future streamable sparse-voxel
  world format that can ingest `.vox` and Minecraft conversions later.
- **Out of scope (but design must not preclude):** pre-made world import,
  cross-segment chunk eviction, web-worker pool re-architecture (the existing
  `voxel_noise` JS bridge already covers web).

---

## Q&A decisions (load-bearing — design must reflect these)

The user picked these from a 4-question Q&A. Cited verbatim:

1. **Coordinate widening — "Residency-only i32 widening".** Residency manager
   tracks chunks at `i32` world-chunk-coords. The GPU bind layout stays
   `(cx:11, cy:10, cz:11)` packed (matches existing `world_change.wgsl` +
   `aadf/edit.rs:330` packing). Chunks are re-indexed into the resident window
   before upload. Camera uses the existing `PositionSplit` (`IVec3 pos_int` +
   `Vec3 pos_frac`). **No shader-side packing changes.** **No `i64` / `f64`
   widening anywhere.**

2. **Residency unit — "Per-segment (16×16×16 chunks)".** One residency slot =
   one segment = the existing W5 `SEGMENT_CHUNKS = 16` shape. Reuses the
   `segment_voxel_buffer` (~128 MiB) and the `generator_model` GPU dispatch
   shape. Window shifts by segments, not chunks.

3. **Block dedup — "Per-resident-chunk-local dedup".** Each newly-generated
   chunk dedups its own 64 blocks against itself only. **No global / no
   per-window dedup state.** Cross-chunk dedup savings on procedural-noise
   content are small in practice and the eviction story is dramatically
   simpler this way.

4. **Noise backend — `voxel_noise` (cross-platform proven).** The user noted:
   *"voxel_noise is a PROVEN working cross-platform noise crate that we
   tested to work on web with webworkers AND natively"*. So no new Rust-side
   wasm-worker-pool crate is needed this session — the existing
   `voxel_noise` (native fastnoise2 + emscripten WASM + JS bridge with
   per-worker init at `voxel_noise_bridge.js:20`) is the cross-platform
   story.

---

## Reuse audit summary

Full audit at `00-reuse-audit.md` (14 candidates, 8 gaps, 4 borderline). Top 3
candidates and the one-line summary:

| # | Candidate | Why it's load-bearing |
|---|---|---|
| 1 | `voxel_noise` crate — `crates/voxel_noise/{src/lib.rs:47-65, src/native.rs:14-126, src/presets.rs:6-100}` | fastnoise2 wrapper with `NoiseNode::{from_encoded, from_preset, gen_uniform_grid_3d, gen_uniform_grid_2d, gen_single_3d}`. Already has `gen_uniform_grid_3d(x_off,y_off,z_off, x_cnt × x_step, seed)` — exactly the per-chunk-segment shape a sliding-window generator wants. **NOT yet wired into `crates/bevy_naadf/Cargo.toml` or `src/`.** |
| 2 | W5 segment generator — `crates/bevy_naadf/src/aadf/generator.rs:74-335` + `crates/bevy_naadf/src/render/construction/generator_model.rs:121-281` + driver loop `render/construction/mod.rs:2454-2566` | `(group_offset_in_chunks, group_size_in_chunks)` shape is already the "produce one chunk-window's worth of voxels" seam. The startup driver does the 512-segment iteration once at boot. A sliding-window driver inverts the gate: per-frame, dispatch only the segments that newly entered the residency window. |
| 3 | W2 delta-upload chain — `world_change` GPU passes + `pending_edits` in `world/data.rs:80`, `aadf/edit.rs:1-100`, `render/construction/world_change.rs`, `render/construction/mod.rs:149-158` | Existing "write `(chunk_pos_packed, new_state)` deltas into GPU buffers via 4 compute passes". Residency layer can synthesise the same record shape brushes emit today — **no new upload pipeline needed**. Pos packing `(cx:11, cy:10, cz:11)` is window-local; matches Q1 choice. |

**One-line summary from auditor:**

> The `voxel_noise` crate, the W5 `ModelData`+`generate_segment_cpu`+`generator_model`
> segment-iteration machinery, and the W2 delta-upload chain together already
> cover ~70% of what a "noise → per-chunk encode → upload to a fixed segment
> buffer" pipeline needs; the load-bearing greenfield piece is the
> **residency manager** (which chunks are resident, an `IVec3 chunk-pos →
> slot-index` indirection table, eviction on window-shift, a VRAM-budgeted
> slab carved out of `WorldGpu`'s existing fixed buffers).

---

## Reference project — `bevy_voxel_world` (read this!)

`/mnt/archive4/DEV/bevy_voxel_world/bevy_voxel_world/` contains the **prior
art** for wiring `voxel_noise` into a Bevy app. The user explicitly pointed at
this as the reference for how voxel_noise integration should look.

Relevant crates in that project:

- `crates/voxel_noise/` — the same `voxel_noise` source the bevy-naadf
  workspace has (carried-over). Same API.
- `crates/voxel_plugin/` — Bevy plugin layer; likely contains the Cargo dep on
  `voxel_noise` and the system that drives noise sampling per chunk.
- `crates/voxel_bevy/` — the Bevy-side glue.
- `crates/voxel_game/` — the consumer game (the actual example).
- `crates/texture_baker/`, `crates/voxel_unity/` — orthogonal, ignore.

The architect MUST Read enough of these (`Cargo.toml` deps, `lib.rs` plugin
registration, any `noise` / `chunk_generator` / `world_gen` modules) to
understand the integration pattern before designing the bevy-naadf-side wire-
up. Adapt that pattern to the bevy-naadf renderer (W5 / W2 chain), don't
reinvent the noise wiring.

---

## Required reading (in order, for design + impl agents)

1. `docs/orchestrate/streaming-world/00-reuse-audit.md` — full audit; the
   `## Top reuse candidates` table is the design's starting point.
2. `crates/bevy_naadf/src/aadf/generator.rs` (full) — W5 `ModelData` shape,
   `generate_segment_cpu` oracle; the `(group_offset_in_chunks,
   group_size_in_chunks)` API is the per-chunk-window seam.
3. `crates/bevy_naadf/src/render/construction/generator_model.rs` (full) — GPU
   port; bind-group layout, dispatch shape.
4. `crates/bevy_naadf/src/render/construction/mod.rs:1115-1610` — the
   `prepare_construction` build-once gate. The per-segment 128 MiB
   `segment_voxel_buffer` allocation and the comments at `:1242-1264` /
   `:1515-1535` document why per-segment cubic was chosen over full-world
   cubic. This is the closest existing thing to a "fixed-VRAM budget enforced
   at allocate time".
5. `crates/bevy_naadf/src/render/construction/mod.rs:2427-2566` — the
   once-at-startup 512-segment generator loop. **Gate to invert.** The
   per-segment-submit ordering bug at `:2427-2453` is load-bearing — note it.
6. `crates/bevy_naadf/src/render/construction/world_change.rs` (full) +
   `crates/bevy_naadf/src/aadf/edit.rs:1-100, 330-332` — W2 delta surface +
   the `(cx:11, cy:10, cz:11)` pos packing.
7. `crates/bevy_naadf/src/world/data.rs:80` — `pending_edits` field shape.
8. `crates/bevy_naadf/src/world/buffer.rs:30-200+` — `GrowableBuffer<T>`;
   reusable for non-slab buffers, **not** the residency slab itself (fixed-
   VRAM budget = no growth).
9. `crates/bevy_naadf/src/camera/position_split.rs:13-119` — `PositionSplit`
   shape; the world-origin offset for a sliding window uses the same split.
10. `crates/bevy_naadf/src/lib.rs:209-250` + `:920-946` — `WORLD_SIZE_IN_*`
    constants + the drift-guard unit test. These constants stay; the
    residency window has these dimensions.
11. `crates/voxel_noise/src/{lib.rs, native.rs, presets.rs}` — full noise API
    (`NoiseNode`, `gen_uniform_grid_3d`, three preset graphs).
12. `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:1-200` +
    `crates/bevy_naadf/bin/e2e_render.rs:71-130` — the canonical named-gate
    pattern. The new `--streaming-window` gate slots in here.
13. `/mnt/archive4/DEV/bevy_voxel_world/bevy_voxel_world/crates/{voxel_noise,
    voxel_plugin,voxel_bevy,voxel_game}/` — prior-art reference for
    voxel_noise→Bevy integration. Adapt, don't reinvent.

Implementation-only:

14. `crates/bevy_naadf/Cargo.toml` — needs `voxel_noise = { path =
    "../voxel_noise" }` (or workspace member); confirm workspace layout
    before adding.
15. `crates/bevy_naadf/src/voxel/grid.rs::setup_test_grid` — where worlds get
    installed at `Startup`. The streaming-window mode is a new
    `GridPreset` variant (or replaces the install for that preset).

---

## Forbidden moves / project rules

### From `CLAUDE.md` (project root)

> **Never run `cargo run --bin bevy-naadf` as a "verification" step.** … The
> named e2e gates (`baseline`, `--validate-gpu-construction`, `--edit-mode`,
> `--entities`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`)
> prove runtime correctness with framebuffer captures or oracle byte-
> equality. **These are the verification surface.**

Implication: the impl agent MUST ship a new `--streaming-window` e2e gate. No
"boot the binary for N seconds and confirm clean exit" smokes count as
verification.

### From `MEMORY.md` (`bevy-naadf-faithful-port-rule`)

> no Bevy-only microoptimizations or behaviors not in C# NAADF; default =
> match C#, even when C# has the bug. Deliberate divergences require
> explicit user approval + docs entry.

Sliding-window residency **is** a divergence from C# NAADF (which runs a
one-shot `GenerateWorld` at startup, `WorldData.cs:120-156`). The brief
explicitly motivates the divergence (worlds larger than VRAM) — that
constitutes the user approval. The design MUST include a `Δ-StreamingResidency`
docs note explaining the divergence and pointing at the C# surface it
replaces (`WorldGenerator.cs:11-22`'s `CopyToChunkData` is the per-segment
analogue).

### From the audit's "Borderline calls"

- **`GrowableBuffer<T>` is NOT applicable for the residency slab** — fixed-
  VRAM means no growth. `GrowableBuffer` is fine for ancillary buffers
  (palette, residency metadata) but **the residency slab itself is a
  one-shot-at-startup allocation sized from a `--vram-budget-mib` knob**.
- **Per-chunk `aadf::construct` extraction loses cross-chunk dedup.** Q3
  chose per-chunk-local dedup → this is acceptable; the existing whole-world
  `construct()` function should NOT be called per-chunk (it carries a shared
  HashMap that would silently still dedup across calls if extracted naively).
  Build a fresh `encode_one_chunk(&[VoxelTypeId; 16^3]) -> EncodedChunk`
  helper that owns its own per-chunk dedup HashMap.

### Other

- **`world_change` packing is `(cx:11, cy:10, cz:11)`.** This caps in-window
  chunk indices at `2048×1024×2048`. Per Q1 (residency-only widening), the
  residency window is much smaller than this — fine. **Do NOT change this
  packing.**
- **The W5 segment loop currently submits per-segment** because of a wgpu
  writes-vs-dispatches ordering bug (`mod.rs:2427-2453`). The streaming
  driver inherits this constraint — per-segment submission is expected, not
  a bug to fix in this work.

---

## Success criteria for the impl phase

(These are extracted near-verbatim into `04-review.md` at review time. Listed
here so the design + impl agents know what the bar is.)

1. **`cargo build --workspace` clean** (CLAUDE.md verification surface).
2. **`cargo test --workspace --lib` green** — including any new unit tests
   the impl adds.
3. **`cargo run --bin e2e_render -- baseline`** still passes (no regression
   on the existing baseline).
4. **New `cargo run --bin e2e_render -- --streaming-window` gate passes** —
   the gate boots a procedurally-generated world with a fixed VRAM budget,
   walks the camera through ≥2 segment boundaries, captures framebuffers
   before and after each window shift, and asserts (a) terrain renders at
   the new camera position (luminance check), (b) the old position is no
   longer in the resident window (a "should be empty" luminance check on
   the evicted region), (c) VRAM usage of the residency slab matches the
   configured budget within a stated tolerance.
5. **VRAM budget is a runtime knob** — a `--vram-budget-mib N` CLI arg on
   `AppArgs` with a default (architect picks a value justified in
   `02-design.md`).
6. **No regressions in other named gates** — `--validate-gpu-construction`,
   `--edit-mode`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`
   all still pass.
7. **No `cargo run --bin bevy-naadf` smokes** in `03-impl.md` as "proof of
   work" (CLAUDE.md ban).
