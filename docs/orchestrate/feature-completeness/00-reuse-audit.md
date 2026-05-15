# 00 — Reuse Audit: Feature Completeness (Track A — VOX loading + Track B — Editor with paint/cube/sphere)

**Date:** 2026-05-15
**Author:** delegate-auditor (read-only on code)
**Branch:** `main` at HEAD (post `1c35c7f`, pre-handoff for the feature-completeness push)
**Brief:** orchestrator-supplied; covers the user's "feature completeness" request — Track 1 (GPU algorithmics) **dropped** (landed in Phase C); Tracks A and B in scope.

**Scope-shift note (binding):** both tracks were explicitly OUT-of-scope in `docs/orchestrate/naadf-bevy-port/01-context.md` Q1 ("Core engine, no editor"). The user has re-opened them. The faithful-port rule remains binding — port tool algorithms + `.vox` parsing semantics from the C# reference; the editor UI is the explicit sanctioned divergence (fresh Bevy-native, gamified, not an ImGui transliteration).

---

## 1. Audit table

| # | candidate (port-side asset) | location (file:line) | what it does | reuse / extend / not-applicable | one-line justification |
|---|---|---|---|---|---|
| 1 | `panel.rs` — Bevy-UI dev panel | `crates/bevy_naadf/src/panel.rs:1-1293` | Full F1-toggled `bevy_ui` 0.19 panel with `Knob` table (P/C/D/B classes), `PanelState` + `PanelDrag` resources, keyboard navigator (↑↓←→ / PgUp/PgDn / Shift / R / Shift+R), mouse drag-slider state machine (`Idle`/`Pressed`/`Dragging`), `Interaction` + `RelativeCursorPosition` hit-test, hi-DPI sensitivity scaling. Uses `DevFont` + Roboto. | **extend** | The exact widget vocabulary the brief asks for: every editor knob (tool select, brush radius, voxel-type palette, erase/continuous toggles) maps directly onto the existing `KnobKind::U32 / F32 / Bool / Action` rows; add a new `KnobKind::Enum` for tool selector + a voxel-type swatch row, hook up new sections at the top of `KNOBS`. Already proves Bevy 0.19 native UI works without `bevy_egui` (which is Bevy-0.18-blocked per `21-design-quality-panel.md` §3.1). |
| 2 | `WorldData::set_voxel(IVec3, VoxelTypeId)` | `crates/bevy_naadf/src/world/data.rs:98-210` | The CPU-side bulk-write entry point: decode-into-edit-window → mutate one voxel → re-encode via `process_edit_batch` → push to `pending_edits` (drained next frame by `extract_world_changes` into the GPU regime-3 dispatch chain). | **reuse** (call from tools) | Already implements the full Phase-C W2 round-trip from a programmatic single-voxel edit to the GPU flood-fill AADF invalidation chain (`change_handler.rs`, `world_change.wgsl`). The cube/sphere/paint tools each issue many `set_voxel` calls per click — exactly the API the C# `editingHandler.setVoxelData(...)` exposes (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingHandler.cs:228+`). Caveat: the docstring at `data.rs:153-155` flags an unbounded-CPU-mirror growth issue across many edits; the editor must either batch or the issue must be addressed before sphere-of-radius-50 lands. |
| 3 | `aadf/edit.rs::process_edit_batch` + 3 GPU oracles | `crates/bevy_naadf/src/aadf/edit.rs:1-100+` | Bit-exact ports of `world_change.wgsl::{apply_chunk,apply_block,apply_voxel}_change` + `EditingHandler.processChunks`. Take a "chunk edit window" of 2048 u32s, re-hash, free old slots, fill `changed_chunks` / `changed_blocks` / `changed_voxels` in NAADF on-wire format. | **reuse** (bedrock) | The faithful-port rule says editor tools must call into this chain unchanged. The pixel-pick → voxel-list-to-edit → `set_voxel` → `process_edit_batch` → `compute_change_groups` → `WorldEditEvent` → render-graph node chain is the binding pipeline; tools assemble inputs to step 1 only. |
| 4 | `change_handler.rs::compute_change_groups` | `crates/bevy_naadf/src/render/construction/change_handler.rs:127+` | Bit-exact port of `ChangeHandler.UpdateWorld` — the two-loop BFS + 7-round addBounds flood-fill propagation across the 63³-chunk affected volume. Returns `changedGroupsWithDist[]` (`Uint2[]` of group pos packed + flood-fill 5-bit AADFs). | **reuse** (don't reimplement) | Already-faithful flood-fill: a sphere of radius 16 touches ~5 chunks per axis → 125 chunks → this is exactly what the chain handles. The cube/sphere/paint tools must NOT re-derive their own AADF invalidation logic; they call the W2 chain and it propagates correctly. |
| 5 | `hud.rs` — diagnostics overlay | `crates/bevy_naadf/src/hud.rs:1-255` | Always-on top-left overlay: FPS, renderer mode, DLSS-RR state, per-pass NAADF GPU timings via `RenderDiagnosticsPlugin`, `BackgroundColor` + `Node` + `Text` pattern, embedded Roboto via `DevFont`, gated on `AppConfig::add_hud`. | **reuse** (style template) | The "gamified" Bevy-native presentation pattern the user wants: same `Node`-with-`BackgroundColor`-overlay + `Text`-child layout (`hud.rs:92-110`). Editor tool-state HUD overlay (current tool, brush radius, hover-voxel position, hover-voxel type) can be a sibling component with the same chrome at top-right (HUD top-left, panel bottom-left, tool-HUD top-right — see `21-design-quality-panel.md` §7). |
| 6 | `voxel/grid.rs::setup_test_grid` + `build_palette` + `fill_box` / `fill_sphere` | `crates/bevy_naadf/src/voxel/grid.rs:66-339` | Hard-coded Phase-A test grid (D2): 4×2×4 chunks = 64×32×64 voxels, palette of 12 voxel types (5 emissive), `DenseVolume` + `construct()` build path, `IAabb3` bounding box. `fill_sphere` is integer-radius `d[0]² + d[1]² + d[2]² ≤ r²` with clamp-to-volume. `fill_box` is an inclusive `[min..=max]` triple loop. | **reuse** (sphere/cube tool primitives) + **extend** (palette) | `fill_sphere(v, center, radius, ty)` and `fill_box(v, min, max, ty)` are exactly the brush-footprint math the sphere + cube tools need (compare to `EditingToolSphere.cs:65-89` `distToPosSqr < radiusSqr` and `EditingToolCube.cs:65-90` `distToPosMax < radius`). The palette (`build_palette` at `:114-216`) is the data the tool's voxel-type selector picks from; extend `VoxelTypes::types` at runtime if `.vox` imports add new types. **However:** the existing fns write to a `DenseVolume` (pre-construction), NOT to `WorldData` directly; the tool versions must call `WorldData::set_voxel` per-voxel instead of mutating a `DenseVolume`. |
| 7 | `aadf/generator.rs` + `ModelData` + `generate_segment_cpu` | `crates/bevy_naadf/src/aadf/generator.rs:1-200+` + `render/construction/generator_model.rs` | Bit-exact CPU port + GPU dispatch of `generatorModel.fx::fillChunkDataWithModelData16` (the C# `WorldGeneratorModel`). Reads NAADF's three-byte-array model encoding (`data_chunk`/`data_block`/`data_voxel`), packs two voxel `u16`s into each `u32`, runs the same 4³-workgroup × 32-iteration × 2-voxels-per-iter shape. Bit-exact oracle. | **extend** | The natural `.vox` integration shape: a `.vox` parse output → a `ModelData` (the same three-byte-array encoding) → `generate_segment_cpu(model, segment_size_in_chunks, segment_offset_in_chunks)` runs the existing GPU/CPU oracle path. Faithful to the C# (`ModelData.ImportFromVox` in `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526` does exactly this). The .vox importer's output target is `ModelData`, NOT raw voxel writes. |
| 8 | `world/buffer.rs::GrowableBuffer<T>` | `crates/bevy_naadf/src/world/buffer.rs:45-200+` | The wgpu equivalent of C# `DynamicStructuredBuffer`: `GROWTH_FACTOR = 2`, `reserve` + `copy_buffer_to_buffer`-on-grow, `write` w/ `queue.write_buffer`, `upload_all` discard-and-reupload, capacity/length tracking. `debug_assert!(size <= max_buffer_size)` is the only hard ceiling. | **reuse** | The buffer that grows under large `.vox` loads — when a 256³-voxel `.vox` lands, `blocks`/`voxels`/`voxel_types` grow via this. No edit needed for Track A; the buffer already handles arbitrary growth up to wgpu's `max_buffer_size` (typically 2 GiB on Vulkan). |
| 9 | `prepare.rs::prepare_world_gpu` — chunks 3D texture | `crates/bevy_naadf/src/render/prepare.rs:206-280` | Allocates the `Rg32Uint` 3D chunks texture sized `size_in_chunks.x × y × z`; `wgpu::TextureFormat::Rg32Uint`, 8 B/texel; usage `TEXTURE_BINDING | COPY_DST | STORAGE_BINDING`. CPU-built today (`extracted.chunks` upload); GPU-writable for W1/W2/W3. | **reuse with caveat** | The **hard ceiling for "large worlds"**: a 3D texture is bounded by `wgpu::Limits::max_texture_dimension_3d` (default `2048`, often `1024` on Vulkan minimums). `chunks_cpu` ≤ 2048³ chunks = 32768³ voxels = ~36 TiB encoded — far past any practical world. **The real ceiling** is the chunk-pos packing `(x: 11, y: 10, z: 11) bits` at `aadf/edit.rs:67-69` and `world_change.wgsl` — that caps the world at **2048 × 1024 × 2048** chunks = 32768 × 16384 × 32768 voxels. Large-VOX loads are NOT limited by buffer growth; they're limited by `max_texture_dimension_3d` (typically the binding constraint at ~1024) or by `max_buffer_size` (2 GiB on the `blocks`/`voxels` buffers — that hits first on a fully-mixed-no-dedup world at ~16k chunks). |
| 10 | `aadf/construct.rs::construct(&DenseVolume) -> ConstructedWorld` (referenced from `voxel/grid.rs:72`) | `crates/bevy_naadf/src/aadf/construct.rs` (referenced at `voxel/grid.rs:29`) | CPU-side 3-phase AADF build: takes a `DenseVolume` (dense voxel-type stream) + returns `(chunks, blocks, voxels)` u32 buffers, retained as the Phase-C **bit-exact validation oracle** + fallback per E4 (`docs/orchestrate/naadf-bevy-port/12-alignment-gap.md` row 2). | **reuse** | The natural target for `.vox` ingestion when **not** going through `ModelData`: parse `.vox` → fill a `DenseVolume` (same shape `voxel/grid.rs::build_default_volume` builds) → `construct(&volume)` → install into `WorldData`. The simplest possible code path; no GPU dispatch coordination needed. |
| 11 | `camera/position_split.rs::sync_position_split` + `FreeCamera` filter fix | `crates/bevy_naadf/src/camera/mod.rs:43-76` + `src/camera/position_split.rs` | The `Camera3d` is spawned with `FreeCamera { walk_speed, run_speed }` from `bevy_camera_controller` (a Bevy-upstream crate) + `PositionSplit` (D1) + `Tonemapping::default()`. `FreeCamera` is the input-side fly camera; it captures mouse always (no toggle). | **reuse (with caveat)** | The editor's tool-active vs camera-fly mode toggle is **NOT** in scope of the existing `FreeCamera` — it captures mouse always; a tool-active mode needs either (a) an `Update`-system `if tool_active { return; }` early-out gating `FreeCamera`'s movement, or (b) a key-bound mode switch resource. The brief calls this out under Track B. The mouse-pick raycast needed by `Paint`/`Cube`/`Sphere` is **not present CPU-side**: `shoot_ray` only exists in WGSL (`assets/shaders/ray_tracing.wgsl:201`). A CPU port of C# `WorldData.RayTraversal` (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473`, ~80 lines, no AADF skipping — just DDA with the 3-layer descend) must be written. |
| 12 | Cargo.toml — no VOX / no egui deps | `crates/bevy_naadf/Cargo.toml:34-72` | Active deps: `bevy =0.19.0-rc.1`, `bytemuck`, `image` (PNG/JPEG only — no `.vox`), `bevy-instamat`, `ron`, `serde`, `thiserror`. Native-only: `basis-universal`. **No** `dot_vox`, `vox-format`, `bevy_egui`, `bevy-inspector-egui`. | **extend** (add one VOX dep) | A clean slate. The two viable Rust crates are `dot_vox` (most popular, MIT-licensed, simple `DotVoxData` struct with palette + models + scene graph; depends only on `byteorder`/`nom`) and `vox-format` (smaller, more recent). Either adapts to NAADF's `VoxelDataBytes` shape losslessly. The K-means palette mapping (`ModelData.cs:528-560`) must be ported in Rust regardless — `dot_vox` only reads the 256-entry palette, not the K-means cluster reduction. |
| 13 | `bin/bake.rs` — out-of-band asset processor (InstaMAT pattern) | `crates/bevy_naadf/src/bin/bake.rs` + `instamat-bake-to-disk.md` memory | The InstaMAT pattern referenced in user memory: a sibling binary runs before the main app to pre-process baked assets. Justfile-driven. AssetProcessor stays off in the production app (`lib.rs:476-481` keeps `AssetMode::Unprocessed`). | **template (for VOX pre-bake)** | The "borderline call" pivot the brief flags: `.vox` could be pre-baked offline to a port-native `.cvox`-like format that's loaded at startup, OR loaded via a runtime Bevy `AssetLoader`. `bake.rs` is the offline-pre-bake template; the texture-array `*.texarray.ron` loader is the runtime-`AssetLoader` template. The right answer depends on intended use: small `.vox` (≤64³ chunks) load fast at runtime; large worlds (`obj2voxel`-style 1k³+) want the offline pre-bake to a binary blob (no parse cost at runtime, no K-means re-running). |
| 14 | `aadf/entity.rs::EntityData::from_types` — per-entity AADF builder | `crates/bevy_naadf/src/aadf/entity.rs` (referenced from `lib.rs:660-674`) | Constructs an `EntityData` from a dense `[u32]` voxel-type stream + a `[w,h,d]` size: runs the 31-iteration per-axis neighbour-merge for 5-bit-per-axis AADFs (Phase-C W4). Used today by the entity fixture. | **reuse / not-applicable** | The right tool for "small placed mesh" (entity) loads, not whole-world loads. If a `.vox` model is meant to be a dynamic entity (a placeable mesh that can move), this is the constructor; if it's meant to be baked into the world geometry, use the `ModelData` path (#7) or the `DenseVolume`+`construct` path (#10). Both VOX import styles are well-supported. |
| 15 | `world_data.wgsl` + `world_layout` bind-group surface | `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` + `render/pipelines.rs:301-319, 619-818` | The render-world 8-binding world layout: chunks (Rg32Uint texture), blocks/voxels (storage buffers), voxel_types, entity_chunk_instances, entity_voxel_data, entity_instances_history. Already structured for runtime mutation via the construction subsystem. | **reuse / not-applicable** | The brief asks if the editor mouse-pick should use a CPU or GPU ray-traversal; the GPU side is already wired through this binding surface but is one-way (compute → render). Pickup needs a CPU-side traversal (#11 caveat); the GPU side is read-only from main-world's perspective without explicit readback. |
| 16 | `aadf/entity.rs::compress_quaternion` + entity instance plumbing | `crates/bevy_naadf/src/aadf/entity.rs` + `render/gpu_types.rs::EntityInstance` | Smallest-three quaternion compression for per-frame entity instance uploads; `EntityInstance` = `(position, quaternion, voxel_start, entity, size)`. Used by W4 + wave-3 for moving entities. | **not applicable** (Track A) / **reuse** (future Track A++) | Not the right shape for static `.vox` world loading; the right shape for `.vox`-as-entity (a placeable mesh). For Track A worlds, the entity track is irrelevant. Listed because the W4 dispatch path is the only currently-working "load mesh data at runtime, render it via AADF traversal" path — useful to know exists. |

---

## 2. Track A overview — VOX format parsing surface

### 2.1 — What exists in the port

**Nothing for `.vox` parsing.** Zero VOX-format code, zero `obj2voxel` integration, zero K-means palette mapping. Cargo.toml carries no relevant dep (`crates/bevy_naadf/Cargo.toml:34-72`).

### 2.2 — What exists for the **ingestion target**

Three viable target shapes for the parsed `.vox` data, all already in the tree:

1. **`DenseVolume` + `construct()`** (`voxel/grid.rs:29` + `aadf/construct.rs`) — Phase-A path; CPU builds the whole world into `chunks`/`blocks`/`voxels` u32 buffers. Simplest, works today, used by the test grid. Suitable for `.vox` sizes ≤ ~256³ voxels (a 256³ `DenseVolume` is 32 MiB of `VoxelTypeId(u16)` — fine on host).
2. **`ModelData`** (`aadf/generator.rs:73-83` + the GPU `generatorModel.fx` dispatch) — Phase-C W5 path; segments-of-chunks dispatch shape; bit-exact CPU/GPU oracle. Mirrors the C# `ModelData.ImportFromVox` shape (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526`) — this is the faithful target.
3. **`WorldData::set_voxel(...)` per-voxel** (`world/data.rs:98`) — the editor-tool path; not appropriate for whole-world bulk loads (would issue `N` `WorldEditEvent`s for an N-voxel world; the regime-3 dispatch chain isn't sized for that).

### 2.3 — What's missing

Per the C# reference (`/mnt/archive4/DEV/NAADF/NAADF/Libraries/VoxelsCore/`):

- **`.vox` binary parser** — `MagicaVoxel.cs` (~700 lines: chunk-tagged binary format, `VOX_`, `MAIN`, `PACK`, `SIZE`, `XYZI`, `RGBA`, `nTRN/nGRP/nSHP/LAYR/MATL` scene-graph chunks, `Flatten()` collation; `MagicaVoxel.cs:226-441` is the read loop). Output: per-model `VoxelDataBytes` (size + byte-per-voxel palette index) + a 256-entry `Color[]` palette + `Material[]` (emit/flux/metal/rough/ior).
- **`VoxelDataBytes` shape** — `Libraries/VoxelsCore/VoxelDataBytes.cs` (~30 lines wrapping a base `VoxelData<byte>` with `Colors` + `Materials` side-tables). The C# `VoxelImport.cs` is 4 lines: dispatch on `.vox` to `VoxFile.Read` (`Libraries/VoxelsCore/VoxFile.cs:8-17`).
- **The `ImportFromVox` → `ModelData` glue** (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526`) — runs the same 4³-block 64-voxel hash dedup the GPU path uses (lines 433-485 are the open-addressing CAS loop, mirroring `chunk_calc.wgsl`'s W1 hash). **This is the bridge.** The output is a `ModelData` that goes straight into the W5 generator dispatch (or the CPU `generate_segment_cpu` oracle).
- **K-means palette mapping** (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:528-560`) — uses C# `KMeans` / `MiniBatchKMeans` from `Accord.NET`. **Out-of-the-box NOT trivially portable** — needs a Rust `kmeans` crate or a hand-written Lloyd's-algorithm impl. The faithful-port rule says this is part of correctness; the C# clusters per-model palette colors down to `maxColors = 254` clusters via Lloyd's (`tolerance = 0.1f`), and clusters become the `VoxelType` palette entries.
- **`obj2voxel` integration** — `/mnt/archive4/DEV/NAADF/NAADF/obj2voxel.exe` is an **external Windows-only binary** (a 3rd-party tool, ~MB-sized, NOT C# source). C# shells out to it (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:773-820`: `Process.Start(obj2voxel.exe, "<obj-path> <out-vox-path>")` → reads its `.vox` output via `ImportFromVox`). **There is no C# source to transliterate.** Options: (a) shell out to `obj2voxel.exe` (Windows-only; Wine on Linux); (b) shell out to the `obj2voxel` Rust crate (exists on crates.io, MIT/Apache, ~v0.4 in 2024 — but bundles its own pipeline); (c) shell out to MagicaVoxel's CLI `obj_to_voxel`; (d) re-implement OBJ→voxel tessellation in Rust (~few hundred lines: parse OBJ, voxelise via triangle-AABB intersection at chosen resolution).

### 2.4 — External-crate vs. transliteration recommendation per format

| format | C# size (lines) | Rust crate(s) | recommendation |
|---|---|---|---|
| `.vox` | `MagicaVoxel.cs` ~750 + `VoxFile.cs` ~30 + `VoxelDataBytes.cs` ~30 = ~810 lines | `dot_vox` (most popular, MIT, ~2000 weekly downloads, returns `DotVoxData { models, palette, materials, scenes }`); `vox-format` (newer, MIT) | **`dot_vox`** for the parse + **transliterate the `ImportFromVox` glue** (`ModelData.cs:356-526`, ~170 lines of well-defined Rust). The crate's `DotVoxData` is shape-compatible with what `ImportFromVox` consumes (one `Model { size, voxels }` per `SIZE`/`XYZI` pair + a `Vec<Color>` palette + a `Vec<Material>` materials list). The K-means stage must be a Rust port — use `linfa-clustering` or a 50-line hand-rolled Lloyd's. |
| `obj2voxel` (i.e., OBJ → voxel) | external binary, no source to port | `obj` crate for OBJ parse; voxelisation hand-written OR shell out to MagicaVoxel CLI / Embree | **shell out by default**: support both (a) `obj2voxel` Rust crate (zero code on port side, MIT) AND (b) the user opts out by writing `.obj → .vox` ahead of time with MagicaVoxel/Goxel/their tool. Re-implementing tessellation in Rust is ~300 LOC for a basic triangle-AABB voxeliser; the **shell-out by default** answer is friendly to large `.obj` models where third-party tools have years of optimisation. |
| Voxlap (`.vxl` etc.) | `Voxlap.cs` ~40 lines | n/a | **OUT of scope** per Step-4 Q&A. Don't touch. |

---

## 3. Track B overview — Editor with paint + cube + sphere

### 3.1 — What exists in the port

**Editor primitives ready to wire:**

- **`WorldData::set_voxel(IVec3, VoxelTypeId)`** — `world/data.rs:98-210`. The C# `editingHandler.setVoxelData` equivalent. Bulk-call shape: `for v in voxels_in_brush { world_data.set_voxel(v, ty); }`.
- **`aadf/edit.rs::process_edit_batch` + 3 GPU oracles** — `aadf/edit.rs:1-100+` & onward. Already invoked from inside `set_voxel`; nothing for the editor to do here.
- **`change_handler::compute_change_groups`** — `render/construction/change_handler.rs:127+`. The flood-fill BFS over the 63³ affected volume.
- **`render::construction` GPU dispatch chain** — fires automatically on `WorldEditEvent`s via the existing per-frame `extract_world_changes` + `naadf_world_change_node` (`12-alignment-gap.md` row 19).

**Bevy-native UI foundation:**

- **`panel.rs`** (`crates/bevy_naadf/src/panel.rs:1-1293`, 1293 lines). The dev panel with the full F1-toggled `bevy_ui` machinery. Has:
  - `PanelState { open, cursor }` resource (`:104-120`).
  - `PanelDrag { state: Idle/Pressed/Dragging }` mouse drag state machine (`:122-146`).
  - `Knob` table with `KnobKind::U32/F32/Bool/Readonly/Action` (`:177-228`).
  - Keyboard nav (`adjust_panel`, `:734-836`) + mouse-drag sliders (`mouse_interact_panel`, `:844-944`).
  - Hi-DPI sensitivity scaling via `Window::scale_factor()` (`:949-951`).
  - Click-vs-drag threshold detection (`DRAG_THRESHOLD_PX = 2.0`, `:84`).
  - Roboto via `DevFont` resource (`lib.rs:33`).
- **`hud.rs`** — `Node`+`Text`-child layout pattern (`:92-110`).

**Existing input + camera:**

- **`FreeCamera`** (from `bevy_camera_controller` upstream) — `camera/mod.rs:63-67`. Mouse-captured fly-camera. No tool-active mode toggle today; **no left-mouse handler exists** for tool dispatch.
- **No CPU ray-traversal exists.** `shoot_ray` is WGSL-only (`assets/shaders/ray_tracing.wgsl:201`). A port of C# `WorldData.RayTraversal` (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473`, ~80 lines: simple DDA with the 3-layer descend; AADF skipping is optional for picking) is **the missing piece** for screen-space pixel-pick.

### 3.2 — Tool algorithms in C# (binding reference — port faithfully)

All three tools share an identical input pattern (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/EditingTools/`):

1. On LMB-pressed: `rayDir = camera.getRayDir(mouse.Position)`.
2. `hitType = worldData.RayTraversal(camPos, rayDir, out hitLength, out voxelPos, out normal)`.
3. If `hitType != 0`: `hitPos = camPos + rayDir * hitLength`.
4. First-press: snap `pos = hitPos`. Drag: `pos = lerp(hitPos, pos, ...)` (motion smoothing — `Paint.cs:36-40`, `Cube.cs:43-47`, `Sphere.cs:43-47`).
5. Compute affected-chunk AABB (`[pos - radius .. pos + radius]`).
6. **Per-tool brush footprint** (the only divergence — see table):

| tool | C# brush footprint | port primitive |
|---|---|---|
| Paint (`Paint.cs:69-79`) | Per-voxel-in-affected-chunks: `(voxelPos - pos).LengthSquared() < radius²`. **Only mutates voxels that are already non-empty** (`if (curType != 0) setVoxel(...)`). | `(p_voxel.as_vec3() - pos).length_squared() < r²` + check `WorldData::get_voxel(p) != EMPTY` before set. |
| Cube (`Cube.cs:76-90`) | Per-voxel-in-affected-chunks: `max(|dx|, |dy|, |dz|) < radius`. Solid; also has `isErase` and `isContinuous` knobs. | Same Chebyshev-distance check. `voxel/grid.rs::fill_box` is the closest port primitive but operates on `DenseVolume`, not `WorldData`. |
| Sphere (`Sphere.cs:76-89`) | Per-voxel-in-affected-chunks: `(voxelPos - pos).LengthSquared() < radius²`. Solid (writes all voxels in radius, not just non-empty ones — unlike Paint). | Same r² check. `voxel/grid.rs::fill_sphere` is the closest port primitive but operates on `DenseVolume`. |

### 3.3 — Tool UI surface (Bevy-native sanctioned divergence)

The user has approved a **fresh Bevy-native** UI as a deliberate deviation from the C# ImGui tree. The C# ImGui shape (`EditingToolPaint.cs:90-94` etc.) is:
```
ImGui.SliderFloat("Radius", ref radius, 1, 400, "%2.f", LOG);
ImGui.Checkbox("Erase", ref isErase);
ImGui.Checkbox("Continuous", ref isContinuous);
```

These three controls map **directly onto `panel.rs`'s existing `KnobKind`s** with zero structural extension:
- `radius` → `KnobKind::F32 { nudge: 1.0, big_step: 10.0, min: 1.0, max: 400.0, ... }`
- `isErase` → `KnobKind::Bool`
- `isContinuous` → `KnobKind::Bool`

The missing knob shape is **tool selection** (a 3-way enum: Paint/Cube/Sphere). That's a new `KnobKind::Enum { variants: &[&str], getter, setter }` — ~20 LOC. The voxel-type palette swatch (selecting which color to paint with) is a `KnobKind::U32` with a `getter` that pulls from `VoxelTypes::types.len()` — already feasible without a new kind.

### 3.4 — The seam decision: extend `panel.rs` vs. sibling module

`panel.rs` already has the "quality knobs" panel; the editor would need a **separate section group** for tool state. Two designs are plausible:

(a) **Extend `panel.rs`**: add a new top-level `EDITOR` section in `KNOBS` with tool selector + radius + erase/continuous + voxel-type swatch. One panel, two modes (F1 toggles "all knobs"; the editor tool selector is just another row).

(b) **Sibling `editor_panel.rs`**: separate F2-toggled panel using the same `Knob`/`KnobKind` machinery (copy ~300 LOC of structural code, or refactor into a `panel_core.rs` shared base).

Verdict: **(a) extend, do not duplicate.** The Knob machinery is already general; the panel is already toggle-gated; adding tool-state rows costs ~30 LOC of new `KNOBS` entries + a `KnobKind::Enum` variant + a `KnobKind::Action` for "spawn brush at hit point" if a click target is wanted. The user already has `R` to reset; the workflow stays one-key.

### 3.5 — User-visible API of each tool (target shape, citing C# equivalents)

```rust
// (sketch — for the design phase, NOT a directive)

#[derive(Resource, Default)]
pub struct EditorState {
    pub tool: EditTool,             // Paint | Cube | Sphere
    pub selected_type: VoxelTypeId, // C# `editingHandler.selectedTypeRenderIndex`
    pub radius: f32,                // C# `radius` (1..400)
    pub is_erase: bool,             // C# `isErase` (Cube/Sphere only; Paint always-paint)
    pub is_continuous: bool,        // C# `isContinuous` (Cube/Sphere only)
    pub pos: Vec3,                  // smoothed hit position (C# `pos`)
    pub edit_active: bool,          // gates camera vs. tool
}

// Per-frame Update system:
fn apply_edit_tool(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Transform, &PositionSplit), With<Camera3d>>,
    mut world_data: ResMut<WorldData>,
    voxel_types: Res<VoxelTypes>,
    state: Res<EditorState>,
    time: Res<Time>,
) {
    if !state.edit_active || !mouse.pressed(MouseButton::Left) { return; }
    let Some(cursor_pos) = window.cursor_position() else { return; };
    let ray = screen_to_ray(cursor_pos, *window, *camera);              // new helper
    let Some(hit) = world_data.ray_traversal(ray.origin, ray.dir) else { return; };
    let pos = smooth_lerp(state.pos, hit.world_pos, time, state.radius); // C# Paint/Cube/Sphere :38-40
    match state.tool {
        EditTool::Paint  => paint_brush(&mut world_data, pos, state.radius, state.selected_type),
        EditTool::Cube   => cube_brush(&mut world_data, pos, state.radius, state.selected_type, state.is_erase),
        EditTool::Sphere => sphere_brush(&mut world_data, pos, state.radius, state.selected_type, state.is_erase),
    }
}
```

The `ray_traversal` method is the missing piece (#11). `paint_brush`/`cube_brush`/`sphere_brush` each emit `WorldData::set_voxel` calls in the brush footprint; the existing chain takes it from there.

---

## 4. Borderline calls

| call | the verdict | what made it borderline | what would flip it |
|---|---|---|---|
| **External VOX crate (`dot_vox`) vs. transliterate `MagicaVoxel.cs`** | recommend external crate | `dot_vox` is a stable, MIT-licensed, well-maintained Rust crate with the same shape the C# parser produces; the C# parse is ~750 LOC of straightforward chunk-tagged binary; the `MagicaVoxel.cs` data model (Nodes/Layers/Materials) is already what `dot_vox::DotVoxData` provides. Borderline because **faithful-port rule** says match C# bit-exactly — and a 3rd-party crate's internal state machine differs from `MagicaVoxel.cs`'s. **Flip-trigger:** if any test scene relies on a `MagicaVoxel.cs`-specific quirk (e.g., the `IMAP` 1-based-vs-0-based palette-index off-by-one at `MagicaVoxel.cs:415-424`), or if `dot_vox` lacks scene-graph support (`nTRN/nGRP/nSHP`), fall back to a hand-written port. The K-means stage is **separate** and must be a Rust port regardless. |
| **Pre-bake `.vox`→`.cvox` vs. runtime `AssetLoader`** | recommend **both, default to runtime** | The InstaMAT memory says the project pattern is offline pre-bake (avoids `AssetProcessor`); a `bake.rs`-style sibling could pre-K-means a `.vox` to a port-native binary. **But:** `.vox` files are small (<10 MiB typical), K-means is fast (~250ms for 254 clusters at 50k unique colors), and runtime load through Bevy's `AssetServer::load` lets the user drop new `.vox` files in `assets/` without re-baking. Borderline because **large `obj2voxel` outputs** (1k³ chunks, ~1 GiB encoded) DO want offline pre-bake — parse + K-means + AADF construction at 1k³ scale is ~10 s, which is too slow for app startup. **Flip-trigger:** target world size. ≤256³ voxels: runtime `AssetLoader`. ≥1024³: pre-bake. The right answer is **support both**: an `AssetLoader` for `.vox` directly + a `bake.rs --vox <path>` mode that outputs a `.cvox`-equivalent that the `AssetLoader` also handles. Single-format simplicity loses; dual-path simplicity wins. |
| **`obj2voxel`: shell-out vs. re-implement** | recommend **shell-out by default, document re-impl as future work** | The C# tree ships `obj2voxel.exe` as an opaque Windows-only binary and shells out to it (`ModelData.cs:773-820`). **There is no C# source to transliterate.** Faithful-port rule says match C# behaviour — which means shell-out. **Borderline:** the binary is Windows-only; on Linux, options are (a) Wine, (b) the `obj2voxel` Rust crate (independent reimpl on crates.io ~v0.4), (c) MagicaVoxel CLI, (d) hand-write triangle-AABB voxelisation (~300 LOC). The shell-out keeps fidelity; (b)/(d) ship a Rust-native binary path that doesn't depend on `obj2voxel.exe` being installed. **Flip-trigger:** if `obj2voxel` Rust crate's behaviour can be byte-compared against `obj2voxel.exe` on a fixed test OBJ, use the Rust crate (zero shell-out, faithful). Otherwise document the dependency on the external binary in `README.md`. |
| **Extend `panel.rs` vs. sibling `editor_panel.rs`** | recommend **extend `panel.rs`** | `panel.rs`'s `Knob`/`KnobKind` machinery is already general enough for editor controls — radius slider, erase toggle, continuous toggle, tool selector — with at most one new `KnobKind::Enum` variant. The user explicitly said "panel with all the knobs and whistles" for the quality panel; the editor is structurally the same. **Borderline** because the quality panel already has 28 rows; adding ~5 editor rows pushes it past 30 and the screen real estate gets tight (~30% of screen). **Flip-trigger:** if the editor needs visual surface beyond what `KnobKind` supports (a 3D gizmo widget, a voxel-type-palette grid swatch, a 2D color picker), fall back to a sibling module. For paint+cube+sphere as specified (radius slider, erase toggle, continuous toggle, tool selector, palette index) — the existing machinery suffices. |
| **CPU ray-traversal scope: AADF-skipping or naive DDA?** | recommend **naive DDA, ~80 LOC** | The C# `WorldData.RayTraversal` (`/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldData.cs:396-473`) is naive DDA — it descends the 3 layers per cell, but it does NOT use AADF empty-space skipping. (The GPU `shoot_ray` in WGSL DOES use AADFs.) This is the canonical reference; matching it is faithful and simpler. **Borderline** because someone might think "AADF-skipping is faster, why not?" — but a CPU pick-ray on a 1024³ world hits typically 50-200 cells with naive DDA (single-threaded, sub-millisecond); the AADF path is ~5× faster but pulls in much more bit-packing code. **Flip-trigger:** if pick latency is measurably bad on large worlds (>10ms on a release build), port the AADF path; otherwise stay naive. |
| **`set_voxel` per-voxel vs. batched bulk-write API** | recommend **batched API as extension of `WorldData`** | A sphere of radius 16 ≈ 17k voxels; today's `set_voxel` builds + processes one edit batch per call (`data.rs:130-209`), which means 17k individual `process_edit_batch` invocations (~17k allocations, ~17k re-encode-window passes) per click — at ~5µs each that's 85 ms per click of input lag. **Borderline:** the existing code path is correct, just slow at brush scale. **Flip-trigger:** the design phase decides this. Options: (a) `WorldData::set_voxels_batch(&[(IVec3, VoxelTypeId)])` that groups by chunk + does one `process_edit_batch` per affected chunk (~125 chunks for r=16, ~125 batches → ~625µs → fast); (b) leave `set_voxel` alone, the tool just calls it 17k times and we measure (might be fine on release builds with allocator caching). The user note at `data.rs:153-155` flags this. |

---

## 5. Reuse-vs-new recommendation per track

### Track A — VOX world loading

**Recommendation: ~75% reuse + ~25% new.**

- **Reuse (no edits):** `ModelData` / `aadf/generator.rs::generate_segment_cpu` / `aadf/construct.rs::construct` / `WorldData` / `GrowableBuffer` / chunks-texture allocation / W1 hashing dispatch / the bit-exact GPU/CPU oracle gates. All ingestion paths are in place.
- **New:** (a) `dot_vox` Cargo dep (1 line); (b) a `voxel/vox_import.rs` module that turns a `dot_vox::DotVoxData` into either a `DenseVolume` (simple path) or a `ModelData` (faithful path) — ~150 LOC; (c) a Rust K-means port (~50 LOC hand-rolled or one crate dep); (d) `obj2voxel` shell-out helper (~50 LOC) OR `obj2voxel` Rust crate dep (1 line); (e) an `AssetLoader<DotVoxAsset>` impl for runtime `.vox` loading (~80 LOC) OR a `bake.rs` extension for pre-baking (~100 LOC); (f) `voxel/grid.rs::setup_test_grid` extension to optionally load a `.vox` instead of building the hard-coded grid (~30 LOC).
- **Total new code: ~400 LOC + 1-2 deps.** No edits to the construction subsystem or render path.

### Track B — Editor with paint/cube/sphere

**Recommendation: ~70% reuse + ~30% new.**

- **Reuse (no edits):** `WorldData::set_voxel` / `aadf/edit.rs` / `change_handler.rs` / GPU regime-3 dispatch / the rendering chain (no shader edits). `panel.rs` machinery (Knob table, `PanelDrag` mouse state machine, `PanelState`, keyboard navigator). `hud.rs` style pattern. `voxel/grid.rs::{fill_box, fill_sphere}` math (extract as `pub` helpers operating on closures `|p| world_data.set_voxel(p, ty)` instead of `DenseVolume`).
- **New:** (a) `WorldData::ray_traversal(origin, dir) -> Option<RayHit>` (~80 LOC port of C# `WorldData.RayTraversal`); (b) `EditorState` resource + `apply_edit_tool` Update system (~200 LOC); (c) `KnobKind::Enum` variant for the tool selector (~30 LOC); (d) `KNOBS` table additions in `panel.rs` for editor rows (~50 LOC); (e) editor-active-mode gating on `FreeCamera` movement (~10 LOC); (f) screen-to-ray helper (~30 LOC: viewport+camera unprojection).
- **Total new code: ~400 LOC, no new deps.** Possible perf-tuning extension `WorldData::set_voxels_batch` (~100 LOC) if `set_voxel` per-voxel is too slow at brush scale.

---

## 6. Top reuse recommendation

**The single largest body of asleep code that addresses these tracks is `panel.rs` (1293 lines)** — the F1-toggled `bevy_ui` 0.19 dev panel with the full `Knob`/`KnobKind` + keyboard + mouse-drag-slider + hi-DPI infrastructure already built for the quality-panel dispatch. The user's "gamified Bevy-native UI" requirement for Track B is exactly what `panel.rs` ships, and the editor's control vocabulary (tool selector, brush radius, erase toggle, continuous toggle, voxel-type palette index) is a strict subset of what `KnobKind::U32/F32/Bool/Action` already handles — one new `KnobKind::Enum` variant and ~80 additional LOC of `KNOBS` entries cover the entire Track B UI surface. For Track A, the parallel call-out is **`aadf/generator.rs::ModelData` + `generate_segment_cpu` (~600 LOC across `generator.rs` + `render/construction/generator_model.rs`)** — the W5 worldgen path is structurally the C# `ModelData.ImportFromVox` (`/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:356-526`) ingestion target, ready to consume a parsed `.vox` directly. No greenfield is justified for either track's load-bearing surfaces.
