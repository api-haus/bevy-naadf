# 01 — Canonical context bundle

**Every non-review agent reads this file in full before doing anything else.**
Reviewers read `04*-review-*.md` only.

---

## 1. Goal (verbatim user words)

> "lets focus on feature completeness first:
>
> 1. GPU algorithmics
> 2. loading large VOX worlds
> 3. editor with paiting
>
> then we can get to esotheric stuff like barier hazards and entity taa reprojection"

**Track 1 (GPU algorithmics) is dropped** — user confirmed in Q&A ("yeah i
forgot it already landed"). Phase C delivered the four GPU compute chains
(`chunk_calc`, `bounds_calc`, `generator_model`, `entity_update`) per
`docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` rows 17–21. This
orchestration covers Tracks 2 and 3 only.

### Track A — large VOX world loading

- **Format scope:** MagicaVoxel `.vox` ONLY. `obj2voxel` is **deferred entirely** (Q&A locked).
- **Reference (C#, read-only):**
  - `/mnt/archive4/DEV/NAADF/NAADF/IO/MagicaVoxel.cs` — `.vox` chunk-tagged binary parse.
  - `/mnt/archive4/DEV/NAADF/NAADF/IO/VoxFile.cs` — entry wrapper.
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526` — `ImportFromVox` glue (`.vox` parsed shape → `ModelData`). **This is the bridge.**
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:528-560` — K-means palette mapping (Accord.NET MiniBatchKMeans, `maxColors = 254`, `tolerance = 0.1`).
- **End state:** the port can load `.vox` assets into a `WorldData` instance (either via runtime `AssetLoader` or pre-bake), replacing or supplementing the hard-coded test grid (D2 in `01-context.md` original scope). The audit recommends supporting BOTH paths (runtime default).

### Track B — editor with paint/cube/sphere

- **Tool scope:** paint + cube + sphere. **Skip** flood-fill tool, model-paste tool.
- **UI:** fresh `bevy_ui` 0.19, gamified styling — **explicit sanctioned divergence** from the C# ImGui tree.
- **Reference (C#, read-only):**
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingTools/Paint.cs`
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingTools/Cube.cs`
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingTools/Sphere.cs`
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473` — CPU `RayTraversal` (the canonical, ~80-line naive-DDA reference; the load-bearing missing piece in the port).

**Track order.** VOX (Track A) **before** editor (Track B) — user-confirmed.

---

## 2. User-confirmed decisions (Q&A binding)

| Q | answer | implication |
|---|---|---|
| Track 1 status | "yeah i forgot it already landed" | GPU algorithmics dropped from this orchestration. |
| Track A formats | "MagicaVoxel .vox, obj2voxel intermediate format" → then refined to "Defer obj2voxel entirely" in the borderline Q&A | Only `.vox` parsing in scope. `obj2voxel` documented as future work. |
| Track B tools | "paint cube + sphere tools, but we'll rewrite ui as bevy_ui more-like gamified it" | Paint + Cube + Sphere only. `bevy_ui` 0.19 fresh, gamified styling. |
| Track order | "VOX → editor → GPU algorithmics" | VOX lands first; editor builds on it. |
| K-means impl | "we dont really care about deps or bit parity, but we do care about delivering a simplest working port first. hand-rolling lloyd's according to C# OR using kmeans crate is way to go and should be rather probable to achieve in one shot. optimisation is a future concern." | **Architect chooses** between hand-rolled Lloyd's (~50 LOC) and the `kmeans` crate based on simplicity-in-one-shot. Bit-exact match to C# Accord.NET is NOT required. |
| obj2voxel posture | "Defer obj2voxel entirely" | Track A is `.vox` only. |
| Edit batching | "Add set_voxels_batch up-front" | Track B lands a `WorldData::set_voxels_batch(&[(IVec3, VoxelTypeId)])` method that groups by chunk + does one `process_edit_batch` per affected chunk (~625µs/click on a sphere r=16, vs ~85ms per-voxel). **This is a sanctioned divergence** from strict faithful-port (C# sets per-voxel) — recorded explicitly in the design's `## Decisions` section. Motivation: noticeable input lag is user-visible, fix pre-emptively. |

### Faithful-port rule (modulated for this orchestration)

The general project rule remains (memory: `bevy-naadf-faithful-port-rule`): no
Bevy-only microoptimisations or behaviours not in C# NAADF; default = match C#
even where C# has the bug. Sanctioned divergences require explicit user
approval + a divergences-doc entry.

For this orchestration, the user explicitly relaxed it on three axes (record
all three under `## Decisions & rejected alternatives` in the design files):

1. **K-means bit-exact parity NOT required.** "we don't really care about deps or bit parity"; "optimisation is a future concern."
2. **`bevy_ui` UI is the explicit sanctioned divergence** from the C# ImGui tree.
3. **`set_voxels_batch` is a sanctioned perf divergence** from C#'s per-voxel `setVoxelData`.

The rest of the faithful-port rule holds. In particular:
- Tool algorithms (paint brush footprint, cube Chebyshev test, sphere `r²` test) must match C# semantics. Cite `Paint.cs:69-79`, `Cube.cs:76-90`, `Sphere.cs:76-89` in the design.
- `WorldData::ray_traversal` must match C# `WorldData.RayTraversal:396-473` semantics — naive DDA with 3-layer descend (no AADF-skipping; the C# CPU traversal doesn't use AADFs even though the GPU one does).
- `.vox` → palette colors → NAADF `VoxelType` table must follow the C# `ImportFromVox` → K-means pipeline (the K-means *algorithm* may diverge per #1 above, but the *role* it plays in the pipeline stays).
- All `WorldData` mutations route through `set_voxel` / `set_voxels_batch` → `process_edit_batch` → `change_handler.rs` flood-fill. The editor must NOT bypass the W2 chain.

---

## 3. Reuse audit summary

Full table in `00-reuse-audit.md`. Top reuse per track:

### Track A (~75% reuse + ~25% new ≈ 400 LOC + 1–2 deps)

| What | Where | Reuse mode |
|---|---|---|
| `aadf/generator.rs::ModelData` + `generate_segment_cpu` | `crates/bevy_naadf/src/aadf/generator.rs` + `src/render/construction/generator_model.rs` (~600 LOC asleep) | **structural target** — `.vox` parsed shape ingests into `ModelData`, then W5 dispatch runs |
| `aadf/construct.rs::construct(&DenseVolume)` | `crates/bevy_naadf/src/aadf/construct.rs` (referenced from `voxel/grid.rs:29`) | **alternative path** for small `.vox` (≤256³): parse → `DenseVolume` → `construct()` |
| `world/buffer.rs::GrowableBuffer<T>` | `crates/bevy_naadf/src/world/buffer.rs:45-200+` | **reuse** — auto-handles `.vox`-driven buffer growth up to wgpu `max_buffer_size` |
| World-size ceilings | chunks 3D texture sized at `prepare.rs:206-280`; chunk-pos packing `(x:11, y:10, z:11) bits` at `aadf/edit.rs:67-69` | **constraint** — port world max is `2048×1024×2048` chunks = 32k×16k×32k voxels; large-VOX loads bound by `wgpu::Limits::max_texture_dimension_3d` (~1024 on Vulkan minimums) AND `max_buffer_size` (2 GiB) |

### Track B (~70% reuse + ~30% new ≈ 400 LOC, no new deps)

| What | Where | Reuse mode |
|---|---|---|
| `panel.rs` (`Knob`/`KnobKind` machinery, F1-toggle, mouse-drag, keyboard nav, hi-DPI) | `crates/bevy_naadf/src/panel.rs:1-1293` | **extend** — add `KnobKind::Enum` variant + new `KNOBS` rows |
| `WorldData::set_voxel(IVec3, VoxelTypeId)` | `crates/bevy_naadf/src/world/data.rs:98-210` | **call** from tools; extend with `set_voxels_batch` |
| `aadf/edit.rs::process_edit_batch` | `crates/bevy_naadf/src/aadf/edit.rs` | **reuse — bedrock** |
| `change_handler.rs::compute_change_groups` (flood-fill BFS) | `crates/bevy_naadf/src/render/construction/change_handler.rs:127+` | **reuse — don't reimplement** |
| `voxel/grid.rs::fill_sphere` + `fill_box` | `crates/bevy_naadf/src/voxel/grid.rs` | **extract** as `pub` helpers taking a closure `|p| world_data.set_voxel(...)` |
| `hud.rs` (`Node`+`Text` overlay pattern) | `crates/bevy_naadf/src/hud.rs:1-255` | **style template** for tool-state HUD |
| `FreeCamera` (from `bevy_camera_controller`) | `crates/bevy_naadf/src/camera/mod.rs:63-67` | **gate** — add edit-active-mode resource that disables `FreeCamera` movement when LMB is held with a tool selected |

### Borderline calls (audit) and their resolutions

| call | resolution (audit + Q&A) |
|---|---|
| External crate vs. transliterate `MagicaVoxel.cs` | use `dot_vox` crate (audit recommendation; user is dep-tolerant) |
| Pre-bake vs runtime `AssetLoader` | support BOTH; runtime default for ≤256³, pre-bake recommended for ≥1024³. **Architect decides minimal first cut** — runtime-only is acceptable for the simplest one-shot if user re-confirms during design |
| obj2voxel | DEFER entirely (user) |
| Extend `panel.rs` vs sibling | EXTEND (audit; ~30 LOC `KnobKind::Enum` + ~50 LOC `KNOBS` rows) |
| CPU ray-traversal naive vs AADF-skipping | NAIVE (matches C# `WorldData.RayTraversal` faithfully) |
| K-means impl choice | hand-roll Lloyd's OR `kmeans` crate (architect picks simplest-one-shot) |
| `set_voxel` per-voxel vs batched | `set_voxels_batch` UP-FRONT (user) |

---

## 4. Required reading

Order matters; do not skip.

1. **`docs/orchestrate/feature-completeness/00-reuse-audit.md`** — full audit table, per-track overview, borderline calls. **Why**: every reuse choice in the design must trace to a candidate row here.
2. **`docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` §1 (Scope) + §3 (Divergences)** — read once. **Why**: this orchestration explicitly re-opens scope that the original `01-context.md` deferred; the divergences table is the format the design's `## Decisions` section follows.
3. **`docs/orchestrate/naadf-bevy-port/21-design-quality-panel.md`** — the Bevy-UI panel design that produced `panel.rs`. **Why**: Track B extends this panel; the design's gamified-Bevy-UI vocabulary lives here.
4. **`crates/bevy_naadf/src/panel.rs`** (full file, 1293 lines) — **Why**: Track B sees this as the foundation. Track A skips.
5. **`crates/bevy_naadf/src/world/data.rs`** (Track A: full file; Track B: focus on `set_voxel` at `:98-210` + the docstring at `:153-155`) — **Why**: both tracks mutate `WorldData`. The `set_voxels_batch` extension lives here.
6. **Track A only** — `crates/bevy_naadf/src/aadf/generator.rs` (`ModelData` + `generate_segment_cpu`), `crates/bevy_naadf/src/render/construction/generator_model.rs` (the W5 GPU dispatch). **Why**: the parsed `.vox` ingestion target.
7. **Track A only** — `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526` (the `ImportFromVox` glue) + `:528-560` (K-means stage). **Why**: canonical reference for the bridge from parsed `.vox` to `ModelData`.
8. **Track A only** — `/mnt/archive4/DEV/NAADF/NAADF/IO/MagicaVoxel.cs` (skim) + `/mnt/archive4/DEV/NAADF/NAADF/IO/VoxFile.cs` (read in full). **Why**: confirm what `.vox` chunks the C# consumes / ignores; `dot_vox` produces a superset.
9. **Track B only** — `/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingTools/Paint.cs`, `Cube.cs`, `Sphere.cs` (~100 lines each). **Why**: brush footprint reference. Tool algorithms match these exactly.
10. **Track B only** — `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473` (`RayTraversal`). **Why**: canonical CPU ray traversal; the port's `WorldData::ray_traversal` ports this.
11. **Track B only** — `crates/bevy_naadf/src/hud.rs` (~255 lines) + `crates/bevy_naadf/src/render/construction/change_handler.rs:127-220` (`compute_change_groups`). **Why**: HUD style template + the flood-fill BFS the editor's bulk edits hand off to.
12. **`crates/bevy_naadf/Cargo.toml`** (the `[dependencies]` section, ~lines 34-72). **Why**: confirm what's already pulled in before adding `dot_vox` / `kmeans` (Track A) or before assuming an editor dep is missing (Track B).
13. Memory file: `instamat-bake-to-disk.md` (`/home/midori/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/instamat-bake-to-disk.md`). **Why**: explains the project's offline-pre-bake pattern (`bin/bake.rs` + `justfile`, `AssetMode::Unprocessed`). Track A's pre-bake option follows this pattern if the design picks pre-bake.

---

## 5. Forbidden moves

- **Do NOT reimplement the W2 edit chain.** Track B tools call into `set_voxel` / `set_voxels_batch` → `process_edit_batch` → `compute_change_groups` → `WorldEditEvent` → `naadf_world_change_node`. The flood-fill BFS over the 63³ affected volume is in place and bit-faithful to `ChangeHandler.cs:73-174`.
- **Do NOT reinvent the AADF chain.** The Phase-C GPU dispatch (regime-3) handles invalidation. Brushes just emit `set_voxel*` calls.
- **Do NOT delete `MAX_RAY_STEPS_*` consts at `ray_tracing.wgsl:122-126`.** They are intentionally retained per `21-design-quality-panel.md` §6. (This is a Track-B-adjacent note from the deferred Phase-D scope.)
- **Do NOT add a uniform field to `GpuGiParams` without matching the post-Dispatch-A layout discipline.** `GpuGiParams` grew from 288 → 304 (Dispatch A) → 336 bytes (panel). New fields land after offset 332 on a fresh 16-byte row, with `offset_of!` guards. Track B should not need to touch `GpuGiParams`; if it does, surface the layout impact in the design.
- **Do NOT touch the render pipeline or the GI shader chain.** Both tracks are CPU-side (parsing, traversal, edit batching) + UI side. Render-graph nodes stay unchanged.
- **Do NOT touch the `naadf_gpu_producer_node` or `gpu_producer_skip_upload` lever.** Those are part of the deferred Phase-D residuals (`docs/orchestrate/phase-d-completion/`).
- **Do NOT port `obj2voxel`** in any form for this orchestration. Deferred entirely.
- **Do NOT port the C# `Gui/` ImGui tree.** UI is fresh `bevy_ui`; gamified.
- **Do NOT port flood-fill or model-paste editor tools.** Track B is paint + cube + sphere only.
- **Do NOT add `bevy_egui` or any ImGui-shaped UI crate.** UI extends `panel.rs`.
- **Do NOT silently break the e2e harness.** The test grid (`voxel/grid.rs::setup_test_grid`) stays as the e2e baseline content; `.vox` loading is additive. The e2e modes (baseline · `--validate-gpu-construction` · `--edit-mode` · `--entities`) all continue to pass.

---

## 6. Working tree & branch state

- Branch: `main`. HEAD: `b4c47a1 docs(phase-d-completion): record deferred-work reuse audit + wgpu barrier research`.
- Recent Phase-D-shadow + panel commits: `1c35c7f` (`sun_shadow_taps`, `GpuGiParams` 288→304), `4211910` (panel commit, `GpuGiParams` 304→336, added `panel.rs`), `a602508` / `1009c08` (panel UX iterations), `32e8846` (embedded Roboto font), `3fd96fb` / `1c5610c` / `6de2335` / `6f55174` (TAA-resize-blackness fix + docs fold).
- Working tree was clean before this orchestration's audit + the `01-context.md` write.

---

## 7. Exit criteria

This orchestration ends when:

1. Track A `.vox` loader is implemented + reviewed PASS — a `.vox` file at a fixed path loads through the runtime `AssetLoader` and renders correctly via the existing GI pipeline. Test: substitute a small reference `.vox` for the test grid; visual + the existing e2e gates still pass.
2. Track B paint + cube + sphere tools are implemented + reviewed PASS — LMB-with-tool-active edits the world, `set_voxels_batch` propagates through the W2 chain, the panel exposes tool selector + brush radius + erase + continuous + voxel-type-palette controls, the editor-mode gates `FreeCamera`.
3. `cargo build` clean; all `#[test]` pass; existing e2e modes (baseline, `--validate-gpu-construction`, `--edit-mode`, `--entities`) all pass.
4. `README.md`'s phase checklist is fully `[x]`.

Deliverables on disk: the design docs (`02a-*`, `02b-*`), the impl logs (`03a-*`, `03b-*`), the review verdicts (`04a-*`, `04b-*`). Phase-D residuals (`docs/orchestrate/phase-d-completion/`) stay intact for a future circle-back.
