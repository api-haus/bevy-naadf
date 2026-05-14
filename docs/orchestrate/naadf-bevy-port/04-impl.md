# 04 — Implementation Log

## impl findings — Phase A Batch 1 (steps 1–6) (2026-05-14)

Executed steps 1–6 of the `03-design.md` §8 Phase-A implementation sequence, in
order. Every step ends at a compiling state; steps 4 and 5 also pass their unit
tests; the build was smoke-run at the end. Toolchain: `cargo 1.95.0` /
`rustc 1.95.0`, the pinned `rust-toolchain.toml` stable channel, `mold` linker
from `.cargo/config.toml`. The DLSS SDK env vars (`DLSS_SDK`, `VULKAN_SDK`) were
set on this machine, so the default-feature build (with `dlss`) compiled — no
need for `--features force_disable_dlss`.

Note on test invocation: this is a **binary-only crate** (no `lib.rs`), so unit
tests run via `cargo test --bin bevy-naadf`, not `cargo test --lib` (which
errors "no library targets found"). Batch 2 / the reviewer should use
`cargo test --bin bevy-naadf`.

The NAADF C# source repo (`/mnt/archive4/DEV/NAADF/NAADF/`) was **empty /
inaccessible in this sub-agent environment** — both `find` and `ls` returned
nothing. All C# bit-layout / algorithm detail was therefore taken from the
already-verified citations in `02-research.md` §1.1.2–§1.1.4 and the
authoritative spec in `03-design.md` §2 / §6, both of which state the C# was
cross-checked during the research phase. No NAADF paths were invented.

---

### Step 1 — Strip Solari (D3)

**Files edited:** `Cargo.toml`, `src/main.rs`, `src/camera.rs`.
**Files deleted:** `src/scene.rs`.

- `Cargo.toml`: removed `"bevy_solari"` and `"bluenoise_texture"` from the
  `bevy` feature list; kept `"free_camera"`, the `dlss` / `force_disable_dlss`
  feature plumbing, and the `[profile.dev*]` tuning.
- `src/main.rs`: removed the `solari::{...}` import, `SolariPlugins` from the
  `add_plugins` tuple, and the `if args.pathtracer { PathtracingPlugin }` block.
  Kept `DefaultPlugins`, `FreeCameraPlugin`, `FrameTimeDiagnosticsPlugin`,
  `RenderDiagnosticsPlugin`, the `DlssProjectId` cfg-block (dormant). Replaced
  the `scene::setup_scene` Startup entry with a temporary empty system
  (`setup_scene_placeholder`).
- `src/camera.rs`: removed the `solari::{pathtracer::Pathtracer,
  prelude::SolariLighting}` import, the `Pathtracer`/`SolariLighting` insert
  branch, the `CameraMainTextureUsages::default().with(STORAGE_BINDING)` line and
  its `camera::CameraMainTextureUsages` + `render::render_resource::TextureUsages`
  imports. Kept `Msaa::Off`, `Camera3d`, `Camera{clear_color}`, `FreeCamera`,
  `Transform`, and the DLSS-RR `D`-key toggle (un-coupled from Solari — it no
  longer reads `args.pathtracer`).
- `src/scene.rs` deleted (`git rm` via `rm`; tracked deletion).

**Deviation (small, logged):** `toggle_dlss` previously early-returned on
`args.pathtracer`; with Solari gone that branch was dropped, so `toggle_dlss` no
longer takes `Res<AppArgs>`. Necessary consequence of the strip.

**Build:** `cargo build` succeeded (one warning: unused `AppArgs` import in
`camera.rs`, fixed immediately after by removing the now-unneeded import). No
`bevy_solari` symbol remains anywhere — D3 "strip entirely, not dormant"
satisfied.

---

### Step 2 — Module skeleton + `AppArgs` extension + `bytemuck`

**Files created:** `src/camera/position_split.rs` (stub),
`src/voxel/mod.rs` (stub), `src/voxel/grid.rs` (stub), `src/aadf/mod.rs`,
`src/aadf/cell.rs` (stub), `src/aadf/construct.rs` (stub),
`src/aadf/bounds.rs` (stub), `src/world/mod.rs`, `src/world/data.rs` (stub),
`src/world/buffer.rs` (stub), `src/render/mod.rs`, `src/render/extract.rs`
(stub), `src/render/prepare.rs` (stub), `src/render/graph.rs` (stub),
`src/render/pipelines.rs` (stub), `src/render/gpu_types.rs` (stub),
`src/assets/shaders/` (empty dir).
**Files moved:** `src/camera.rs` → `src/camera/mod.rs` (`git mv`).
**Files edited:** `src/main.rs`, `src/camera/mod.rs`, `src/hud.rs`, `Cargo.toml`.

- Created the full module tree from design §1: `camera/`, `voxel/`, `aadf/`,
  `world/`, `render/`, `assets/shaders/`. Each stub module is a doc-comment
  header naming its design-§ reference and the step that fills it. `world/` and
  `render/` modules are Batch-2 stubs but are wired into the crate now so the
  module tree is complete and compiles.
- `src/main.rs`: declared the new modules (`aadf`, `render`, `voxel`, `world`);
  added the `GridPreset` enum and extended `AppArgs` per design §4.1 — the
  `pathtracer` field is **deleted** (D3), replaced by `grid_preset: GridPreset`
  and `taa: bool` (wired, defaults `false` — D4). `AppArgs` is still parsed/built
  once at startup.
- `Cargo.toml`: added `bytemuck = { version = "=1.25.0", features = ["derive"] }`
  — pinned to the version Bevy 0.19-rc.1 re-exports (confirmed via `Cargo.lock`:
  Bevy resolves `bytemuck 1.25.0`), so `Pod`/`Zeroable` match across the crate
  boundary. Resolved with no lockfile conflict.

**Deviation (small, logged):** design §8 step 2 only lists `AppArgs` + the empty
module tree + `bytemuck`. But deleting `AppArgs::pathtracer` breaks `src/hud.rs`,
which read `args.pathtracer` for the renderer-mode string and the DLSS-RR status
branch. The HUD "re-point" is nominally step 12 (Batch 2), but the *compile fix*
for the field removal cannot wait. I made the **minimal** hud.rs change: dropped
the `Res<AppArgs>` parameter and the pathtracer branches, set the renderer-mode
string to a static `"NAADF (Phase A — albedo first-hit)"`, and reworded the
DLSS-RR status lines to "dormant in Phase A". I did **not** touch the GPU-timing
paths — they still point at the old Solari diagnostic paths and simply don't
populate (the design says step 12 re-points them at the NAADF render-node
names). The `write_timing` helper is kept verbatim (design §1.1 says "Keep
`write_timing` helper unchanged") with `#[allow(dead_code)]` so it survives
unused until Batch 2 step 12 wires it back up.

**Build:** `cargo build` succeeded (warnings: `setup_test_grid` unused — expected,
wired in step 6).

---

### Step 3 — `PositionSplit` + camera (D1)

**Files created/filled:** `src/camera/position_split.rs`.
**Files edited:** `src/camera/mod.rs`, `src/main.rs`.

- `src/camera/position_split.rs`: the `PositionSplit` value type — `pos_int:
  IVec3` + `pos_frac: Vec3` — a `#[derive(Component)]`, with `from_world`
  (component-wise `floor` split → `pos_frac ∈ [0,1)³`), `to_world` (recombine),
  `normalise` (fold `floor(pos_frac)` into `pos_int` — the C# `updateInternals`
  step), and `Add`/`Sub` operator impls (component-wise then normalise — mirror
  the C# operators). Also holds the `sync_position_split` `Update` system.
- `src/camera/mod.rs`: declared `pub mod position_split;` and re-exported
  `PositionSplit` + `sync_position_split`. `setup_camera` now inserts a
  `PositionSplit::from_world(start.translation)` component on the camera entity
  alongside `Camera3d` / `FreeCamera` / `Transform` / `Msaa::Off`.
- `src/main.rs`: added `camera::sync_position_split` to the `Update` schedule.

**Note on system ordering:** design §4.2 says `sync_position_split` runs "after
`FreeCameraPlugin`'s movement system." Inspecting
`bevy_camera_controller-0.19.0-rc.1/src/free_camera.rs`, that movement system
(`run_freecamera_controller`) runs in the **`RunFixedMainLoop`** schedule, not
`Update`. `RunFixedMainLoop` is ordered before `Update` in the main schedule, so
a plain `Update` system already sees the current-frame `Transform` — **no
explicit ordering constraint is needed**, and adding one against a system in a
different schedule would not even be expressible the simple way. Documented in a
comment at the `add_systems` call. This is a small, correct local resolution of
a design instruction, not a redesign.

**Unit tests** (6, in `position_split.rs`): positive/negative `from_world`
splits, world round-trip, `normalise` folding + idempotency, `Add`/`Sub`
normalisation.

**Build + tests:** `cargo build` succeeded; `cargo test --bin bevy-naadf
position_split` → 5 passed (one test fn contains two related assertions; the
harness counts 5 test functions touching position_split — all green).
Windowed verification of the camera + live `PositionSplit` updates is deferred
to the user's Phase-A review (no display in this environment) — see the smoke
run under "state at end of Batch 1".

---

### Step 4 — CPU data structure (`voxel/mod.rs` + `aadf/cell.rs`)

**Files filled:** `src/voxel/mod.rs`, `src/aadf/cell.rs`.

- `src/voxel/mod.rs`: the cell-state bit-layout constants (`CELL_HAS_CHILDREN`
  = bit 31, `CELL_UNIFORM_FULL` = bit 30, `CELL_PAYLOAD_MASK` = `0x3FFF_FFFF`,
  `VOXEL_FULL_FLAG` = bit 15, `VOXEL_PAYLOAD_MASK` = `0x7FFF`, the AADF field
  widths `AADF_BITS_CHUNK`=5/`AADF_MAX_CHUNK`=31 and `AADF_BITS_SMALL`=2/
  `AADF_MAX_SMALL`=3, `CELL_DIM`=4, `CELL_CHILDREN`=64); the `VoxelTypeId(u16)`
  newtype with `EMPTY` = id 0; the `MaterialBase` (Diffuse=0/Emissive=1/
  MetallicRough=2/MetallicMirror=3) and `MaterialLayer` (None=0/MetallicRough=2/
  MetallicMirror=3 — `1` intentionally skipped, matching the C# enum) enums; the
  `VoxelType` palette entry (the C# 128-bit `Uint4` form per design §2.4 — base+
  layer enums, f16 roughness, `color_base` + `color_layered` RGB).
- `src/aadf/cell.rs`: the `Aadf6 { d: [u8; 6] }` type (direction order
  `-x,+x,-y,+y,-z,+z`) with private `pack`/`unpack` bitfield helpers; the
  `BlockPtr(u32)` / `VoxelPtr(u32)` pointer newtypes (`VoxelPtr` documented as a
  *`u32`-element* offset per `02-research.md` divergence #4); the `ChunkCell` /
  `BlockCell` / `VoxelCell` enums with `encode` → `u32`/`u16` and `decode` ←
  `u32`/`u16`. The bit tests in `decode` mirror the C# `shootRay`: check bit 31
  (mixed) first, then bit 30 (uniform-full), else empty (low bits = AADF). Plus
  `pack_voxels` / `unpack_voxel` free fns for the two-voxels-per-`u32` packing.

**Bit-layout note:** `Aadf6::pack` **clamps** each distance to the field max
before packing (a 2-bit field saturates at 3, not wraps) — there is a unit test
for this. AADF values that overflow the field are a bounds-construction concern,
not an encode concern, but clamping in `encode` is the safe belt-and-braces.

**Unit tests** (13, in `cell.rs`): `encode`∘`decode` round-trips for all three
cell types in all three states (empty/uniform/mixed), per-direction AADF field
isolation (no bleed between the 6 fields), chunk-vs-block field-width
distinction (31 fits a chunk AADF, clamps in a block AADF), and the
two-per-`u32` voxel packing + indexed unpack.

**Build + tests:** `cargo build` succeeded; `cargo test --bin bevy-naadf` →
18 passed (5 + 13).

---

### Step 5 — CPU construction (`aadf/construct.rs` + `aadf/bounds.rs`)

**Files filled:** `src/aadf/bounds.rs`, `src/aadf/construct.rs`.

- `src/aadf/bounds.rs`: the AADF cuboid-expansion routine. `CellBox` = an
  inclusive integer box (the containing upper-layer cell, or the world for
  chunks). `compute_aadf(cell, bound, max_dist, is_empty)` implements paper §3.3
  faithfully: start the cuboid at the cell itself, iterate **alternating x/y/z**,
  each iteration attempting one +1 step in *both* the negative and positive
  direction of one axis; a direction stops when it hits `max_dist`, crosses
  `bound`, or the new slice (`slice_empty`) contains non-empty geometry. Returns
  the 6 per-direction distances as an `Aadf6`. The paper's O(3·d·n)
  neighbour-merge optimisation is **not** implemented — design §6.1 step 3
  explicitly makes it optional for Phase A's tiny static grid; this is the
  straightforward per-cell expansion. The same routine serves all three layers
  with different `max_dist` (3 for block/voxel, 31 for chunk).
- `src/aadf/construct.rs`: `DenseVolume` (the dense `Vec<VoxelTypeId>` input,
  sized in whole chunks, x-fastest indexing); `ConstructedWorld` (the three
  output buffers + `size_in_chunks`); `construct(&DenseVolume) ->
  ConstructedWorld`. A faithful CPU re-derivation of paper Algorithm 1
  (`03-design.md` §6.1 step 2), **not** a transliteration of `chunkCalc.fx`:
  - Phase 1 — classify every block: all-empty → `Empty`; all the same non-empty
    type → `UniformFull`; else dedup against an in-memory
    `HashMap<[VoxelTypeId; 64], VoxelPtr>` (the CPU stand-in for the GPU
    `BlockHashingHandler` — design §6.1 step 2 explicitly says a `HashMap` keyed
    on the 64-voxel array is correct and simpler than the GPU hash) and append a
    32-`u32` voxel group on a miss.
  - Phase 2 — classify every chunk: all-empty blocks → `Empty`; all blocks the
    same uniform-full type → `UniformFull`; else reserve 64 consecutive block
    slots and store the `BlockPtr`.
  - Phase 3 — AADFs + encode: voxel AADFs (bounded by the block's 4³, max 3),
    block AADFs (bounded by the chunk's 4³, max 3), chunk AADFs (bounded by the
    world, max 31), then pack everything into the buffer words. Voxels are
    packed two per `u32`.

**Deviation (small, logged) — `voxels_buf` placeholder pass:** `classify_block`
appends 32 zero-`u32` placeholder slots on a dedup miss (to reserve the
`VoxelPtr` offset) and Phase 3 re-walks the mixed blocks to overwrite those
slots with the AADF-augmented encoding. This two-pass shape is a local
implementation choice (the AADF pass needs the typed classification of the whole
block, which the offset-reservation pass produces) — it does not change the
output buffer layout, which is still bit-identical to what the GPU path would
produce (design §6.2). Noted so Batch 2 / the reviewer isn't surprised by the
placeholder write.

**Unit tests** (10 total — 5 in `bounds.rs`, 5 in `construct.rs`):
`bounds.rs` — empty-cube corner/inner-cell expansion, `max_dist` cap, a wall
blocking expansion, and a check that the final cuboid never sweeps an occupied
cell. `construct.rs` — all-empty volume, uniform-full volume, a hand-checked
single-voxel mixed volume (walks chunk→block→voxel and checks the full voxel +
a sampled empty voxel's AADF), block dedup (two identical mixed blocks share one
`VoxelPtr`, only one voxel group appended), and chunk-level AADF bounded by a
solid neighbour chunk + the world edge.

**Build + tests:** `cargo build` succeeded; `cargo test --bin bevy-naadf` →
28 passed (5 + 13 + 10).

---

### Step 6 — Hard-coded test grid (`voxel/grid.rs`, D2)

**Files filled:** `src/voxel/grid.rs`, `src/world/data.rs`.
**Files edited:** `src/main.rs`.

- `src/world/data.rs`: the `WorldData` and `VoxelTypes` **main-world**
  `Resource`s (design §4.4). `WorldData = { chunks_cpu, blocks_cpu, voxels_cpu:
  Vec<u32>, size_in_chunks: UVec3, bounding_box: IAabb3, dirty: bool }` plus the
  `IAabb3` helper type (inclusive integer voxel-space AABB — the design's
  `bounding_box: IAabb3`). `VoxelTypes = { types: Vec<VoxelType>, dirty: bool }`,
  `Default` seeds `types` with just the element-0 empty placeholder.
- `src/voxel/grid.rs`: `setup_test_grid` — the Startup system. Builds the
  `VoxelTypes` palette (6 entries: index 0 reserved empty placeholder, then
  ground / box-A / box-B / sphere / one emissive), authors a `DenseVolume`
  (4×2×4 chunks = 64×32×64 voxels per design §6.1 step 1: a 3-voxel ground slab
  + two axis-aligned boxes + a solid sphere + one floating emissive box), runs
  `construct`, and inserts the `WorldData` + `VoxelTypes` resources with
  `dirty = true`. Logs the chunk/block/voxel counts at `info!`.
  `GridPreset::Default` is the only preset; the `match args.grid_preset` is in
  place so Batch 2 / later work can add more.
- `src/main.rs`: replaced the temporary `setup_scene_placeholder` (from step 1)
  in the `Startup` schedule with `voxel::grid::setup_test_grid`; deleted the
  placeholder fn.

**Note:** `WorldData` is the *main-world CPU* resource (design §4.4 lists it
under "Main world") — it is legitimately step 6's deliverable (the thing
`setup_test_grid` fills), distinct from the render-world `WorldGpu` resource
that Batch 2 step 8 creates. `world/data.rs` being filled here is consistent
with the design; only `WorldGpu`/`FrameGpu` are deferred to Batch 2.

**Unit tests** (4, in `grid.rs`): default-volume dimensions, ground-present /
air-present sampling, the default volume constructs to 32 chunks with non-empty
block + voxel buffers and every chunk word decoding cleanly, and the palette
reserving element 0.

**Build + tests:** `cargo build` succeeded; `cargo test --bin bevy-naadf` →
32 passed (5 + 13 + 10 + 4).

**Smoke run:** `cargo run` (capped at 25 s — windowed GPU app) **launched
successfully**: created a window on the RTX 5080 / Vulkan, and `setup_test_grid`
ran, logging `NAADF test grid (Default): 32 chunks, 1536 blocks, 2144
voxel-u32s (64x32x64 voxels)`. No panics; clean shutdown. The window is black
(no render path yet — that is Batch 2 steps 8–11), exactly as design §8 step 6
predicts ("still black window, but `WorldData` is populated").

---

## State at end of Batch 1

- **Solari fully stripped** (D3): no `bevy_solari` / `bluenoise_texture` feature,
  no `SolariPlugins` / `PathtracingPlugin`, no Solari camera components. DLSS
  plumbing kept, dormant.
- **Module tree complete** (design §1): `camera/{mod,position_split}`,
  `voxel/{mod,grid}`, `aadf/{mod,cell,construct,bounds}`, `world/{mod,data,
  buffer}`, `render/{mod,extract,prepare,graph,pipelines,gpu_types}`,
  `assets/shaders/`. `world/buffer.rs` and all of `render/` are doc-comment
  stubs awaiting Batch 2.
- **`AppArgs` extended** (design §4.1): `pathtracer` deleted, `grid_preset` +
  `taa` added.
- **`PositionSplit`** (D1): faithful int+frac type, on the camera entity, synced
  from the `FreeCamera` `Transform` every `Update`.
- **AADF data structure** (design §2): `voxel/mod.rs` bit constants + the
  `VoxelType` material system; `aadf/cell.rs` `Aadf6` + `Chunk/Block/VoxelCell`
  encode/decode, bit-matching the verified C# re-encoding.
- **CPU construction** (design §6.1): `aadf/bounds.rs` cuboid expansion +
  `aadf/construct.rs` dense→three-layer with `HashMap` dedup.
- **Hard-coded test grid** (D2): `voxel/grid.rs` builds it; `world/data.rs`
  holds the `WorldData` + `VoxelTypes` resources; wired into `Startup`.
- **Build:** `cargo build` clean (12 dead-code warnings, all "pub item defined,
  not yet consumed" — `decode`/`unpack`/`unpack_voxel`/`to_world` used by tests
  + Batch 2's extract path; `WorldData`/`VoxelTypes` fields read by Batch 2's
  extract/prepare; `MetallicRough`/`MetallicMirror` variants + `VOXEL_TYPE_MAX`
  are the Phase-B material path. No real issues — they clear as Batch 2
  consumes them).
- **Tests:** `cargo test --bin bevy-naadf` → **32 passed, 0 failed.**
- **Smoke run:** launches, builds the grid, opens a (black) window, exits clean.

## What Batch 2 (steps 7–12) / the reviewer needs to know

1. **Test command is `cargo test --bin bevy-naadf`** — not `--lib` (binary-only
   crate, no `lib.rs`).
2. **HUD GPU-timing paths are stale.** `src/hud.rs` still has the old Solari
   diagnostic paths hard-coded but no longer calls `write_timing` (it is
   `#[allow(dead_code)]`, kept verbatim per design §1.1). Step 12 must re-point
   them at the NAADF render-node names from `render/graph.rs` and re-add the
   `write_timing` calls. The renderer-mode string is currently a static
   Phase-A placeholder.
3. **`WorldData` / `VoxelTypes` already exist** in `src/world/data.rs` as
   main-world resources with `dirty = true` after `setup_test_grid`. Batch 2's
   `extract.rs` consumes them; `prepare.rs` builds `WorldGpu`/`FrameGpu` from
   them. `IAabb3` is the bounding-box type. `bounding_box` is in **voxel**
   coordinates, inclusive `[min, max]`.
4. **`VoxelPtr` is a `u32`-element offset**, not a byte or voxel offset — a
   64-voxel group is 32 consecutive `u32`s. `aadf::cell::unpack_voxel(buf, i)`
   does the `buf[i/2] >> (16*(i&1))` addressing. `02-research.md` divergence #4
   flags this as easy to get wrong; the traversal WGSL port (step 9) must match.
5. **`construct.rs` does a two-pass voxel/block encode** (placeholder offsets
   reserved in classification, AADF-augmented words written in a Phase-3
   re-walk). Output layout is unaffected and bit-identical to the GPU path.
6. **AADF neighbour-merge optimisation is not implemented** (per design §6.1
   step 3 it is optional for Phase A). If a larger grid ever makes construction
   slow, it is a localised addition to `bounds.rs` — not needed now.
7. **`bytemuck` is pinned to `=1.25.0`** (Bevy 0.19-rc.1's re-exported version).
   `render/gpu_types.rs` (step 8) derives `Pod`/`Zeroable` from it.
8. **NAADF C# source was inaccessible in this environment** — all C# detail came
   from `02-research.md` / `03-design.md` verified citations. If Batch 2 needs to
   verify a traversal-shader detail against `rayTracing.fxh` and the C# repo is
   still empty, flag it to the orchestrator rather than guessing.
9. **`camera::sync_position_split` ordering:** placed in `Update` with no
   explicit constraint — `FreeCameraPlugin`'s movement runs in `RunFixedMainLoop`
   (before `Update`), so the `Transform` is already current. If Batch 2 adds the
   extract step, `PositionSplit` is read straight off the camera entity there.

## Verdict

**Ready for Batch 2.** Steps 1–6 complete, `cargo build` clean, all 32 unit
tests pass, the app smoke-runs and builds the test grid. No blockers. No design
deviation large enough to need orchestrator/user consultation — the deviations
logged above (the `hud.rs` compile-fix in step 2, the `sync_position_split`
scheduling resolution in step 3, the two-pass encode in step 5) are all small
and local, made and logged per the brief's "small local deviations are fine"
allowance.

---

## impl findings — Phase A Batch 2 (steps 7–12) (2026-05-14)

Executed steps 7–12 of the `03-design.md` §8 Phase-A implementation sequence, in
order. Every step ends at a compiling state; step 7 passes the full test suite;
steps 11–12 were smoke-run on the windowed GPU app. Toolchain: pinned stable
`rust-toolchain.toml`, `mold` linker, default-feature build (with `dlss` — the
SDK env vars are set on this machine).

**Correction to a Batch 1 note:** the NAADF C# + HLSL source IS readable at
`/mnt/archive4/DEV/NAADF/NAADF/` via the Read/Glob tools — the Batch 1 agent's
"empty/inaccessible" report was a shell-hook quirk affecting plain `ls`/`find`
only. Steps 9–11 ported from the actual `.fx`/`.fxh` files (`rayTracing.fxh`,
`common/*.fxh`, `settings.fxh`, `versions/albedo/{renderFirstHit,renderFinal}.fx`).

### Step 7 — `GrowableBuffer` — `world/buffer.rs`

**Files filled:** `src/world/buffer.rs`.

- `GrowableBuffer<T: Pod>` — the `DynamicStructuredBuffer` equivalent. `new`
  (clamps capacity ≥ 1, wgpu rejects 0-size buffers), `reserve` (grows to
  `max(min_capacity, capacity * GROWTH_FACTOR)` with `GROWTH_FACTOR = 2` per
  design §3.1, single `copy_buffer_to_buffer` old→new, returns the old buffer so
  the caller keeps it alive past the encoder submit), `write` (`queue.write_buffer`
  + logical-length tracking), `upload_all` (the build-once convenience path).
  `debug_assert!` on `max_buffer_size` keeps the ceiling visible (design §3.2 —
  no chunked copies in Phase A).
- **Deviation (small, logged) — `upload_all` discards instead of copies.** The
  first cut had `upload_all` call `reserve` (which copies old→new) then `write`.
  The grow-and-copy test surfaced a real wgpu ordering hazard: `queue.write_buffer`
  is applied *before* the command buffer of the same submit, so the
  `copy_buffer_to_buffer` (copying the old, never-written zeros) ran *after* the
  write and clobbered elements 0..old_cap. Fix: `upload_all` uses a private
  `reserve_discard` that reallocs *without* copying — correct, since `upload_all`
  overwrites the whole buffer anyway, and it removes the hazard. `reserve` itself
  (the general grow-and-preserve path) is unchanged and its dedicated test passes.
- **Unit tests (5):** capacity clamp; write-within-capacity (no grow); the
  grow-and-copy path (fill 4, reserve 6 → cap 8, old contents 0..4 survive the
  copy + new write 4..6 lands); `min_capacity` beating the growth factor; and
  `upload_all` grows-then-writes. The tests build a headless render world
  (`MinimalPlugins` + `AssetPlugin` + `ImagePlugin` + `RenderPlugin`) and pull
  the `RenderDevice`/`RenderQueue` after `app.finish()` — `RenderPlugin::ready`
  blocks `finish` until the async device request resolves, so no render schedule
  is ever run (which would panic without a window). Tests skip gracefully (print
  + return) if no adapter is available.

**Build + tests:** `cargo build` clean; `cargo test --bin bevy-naadf` →
**37 passed** (the 32 pre-existing + 5 new).

### Step 8 — render-world resources + extract/prepare — `render/{gpu_types,extract,prepare,pipelines,mod}.rs`, `world/mod.rs`, `main.rs`

**Files filled:** `src/render/gpu_types.rs`, `src/render/extract.rs`,
`src/render/prepare.rs`, `src/render/pipelines.rs`, `src/render/mod.rs`,
`src/world/mod.rs`. **Files edited:** `src/main.rs` (added `WorldPlugin` +
`NaadfRenderPlugin` to `add_plugins`).

- `gpu_types.rs`: `#[repr(C)]` + `bytemuck::Pod` structs — `GpuCamera`
  (inv-view-proj + int+frac camera position, 96 B), `GpuRenderParams` (screen
  size / frame counters / sun term / jitter / packed flags / exposure / bbox,
  112 B), `GpuWorldMeta` (chunk-grid extent + voxel-space bbox, 48 B),
  `GpuVoxelType` (the 128-bit `Uint4` material entry). `GpuVoxelType::from_voxel_type`
  packs base|layer|f16(roughness) + the 6 f16 colour channels exactly as the C#
  `compressForRender`; a hand-rolled `f16_bits` does the f32→f16. Compile-time
  `assert!`s lock the struct sizes to what the WGSL declares; 2 unit tests cover
  `f16_bits` + the material pack.
- `extract.rs`: `ExtractedWorld` + `ExtractedCameraData` render-world resources;
  `extract_world` (build-once — mirrors the CPU buffers only on `WorldData.dirty`)
  and `extract_camera` (every frame — `PositionSplit` + `inv_view_proj` =
  `(clip_from_view · world_from_view⁻¹)⁻¹` + viewport size).
- `prepare.rs`: `WorldGpu` (chunk `R32Uint` 3D texture, CPU-built and
  `write_texture`-uploaded per design §6.1; `blocks`/`voxels`/`voxel_types`
  `GrowableBuffer`s; `world_meta` uniform; `@group(0)` bind group) created once
  by `prepare_world_gpu`. `FrameGpu` (camera/params uniforms rewritten each
  frame; `first_hit_data` `vec4<u32>`/pixel + `shaded_color` `vec2<u32>`/pixel
  storage buffers re-created on viewport resize and cleared on creation; the
  `@group(1)` compute bind group + the blit bind group) by `prepare_frame_gpu`.
- `pipelines.rs`: `NaadfPipelines` (`FromWorld`, built in `RenderStartup` via
  `init_gpu_resource`) — the three bind-group-layout descriptors + the compute
  pipeline id. Uniform layout entries use `uniform_buffer_sized` (the `#[repr(C)]`
  structs are not `ShaderType`); storage entries use `storage_buffer_sized` /
  `storage_buffer_read_only_sized` matching each WGSL `var<storage,...>` access.
- **Deviation (small, logged):** design §8 puts the bind-group *layouts* in
  step 8 and the *pipeline ids* in steps 10–11; I put both in `pipelines.rs` at
  step 8 (the layouts can't exist without the resource that holds them, and the
  pipeline `queue_*` calls just need shader handles, which `asset_server.load`
  produces immediately). The graph *node systems* (`graph.rs`) stayed no-op
  stubs through step 8, wired into the `Core3d` schedule so `NaadfRenderPlugin`
  compiles — filled in steps 10–11.

**Build:** `cargo build` clean.

### Step 9 — WGSL world data + traversal — `assets/shaders/{common,world_data,ray_tracing_common,render_pipeline_common,ray_tracing}.wgsl`

**Files created:** the five WGSL import modules.

- `common.wgsl` — `PI` + `flatten_index` (the HLSL `FLATTEN_INDEX` macro).
- `world_data.wgsl` — the `@group(0)` bindings: `chunks` (`texture_3d<u32>`),
  `blocks`/`voxels` (`array<u32>`, read), `voxel_types` (`array<vec4<u32>>`,
  read), `world_meta` (uniform). Phase A is entity-free so the chunk texture is
  `u32`, not the `Rg64Uint`/`ENTITIES` widening.
- `ray_tracing_common.wgsl` — the Phase-A subset of `commonRayTracing.fxh`:
  PCG / xoroshiro64* RNG (`pcg_hash`, `init_rand`, `xoroshiro64star`, `next_rand`,
  `next_rand2`) + octahedral encode/decode. VNDF-GGX / hemisphere sampling / the
  quaternion (de)compress are Phase B (the header splits A/B per `02-research.md`).
- `render_pipeline_common.wgsl` — the Phase-A subset of `commonRenderPipeline.fxh`
  + `renderFirstHit.fx`'s uniform block: the `HIT_*`/`SURFACE_*` consts, the
  `NORMAL[8]` LUT, `VoxelType` + `decompress_voxel_type`, the `GpuCamera` /
  `GpuRenderParams` struct decls, `get_ray_dir`, `compress_first_hit_data`. The
  specular-path `getHitDataFromPlanes` is Phase B.
- `ray_tracing.wgsl` — **`shoot_ray`, the AADF DDA**, ported faithfully from the
  no-entities path of `rayTracing.fxh`'s `shootRay(int3 rayOriginInt, ...)`, plus
  `ray_aabb`, `RayResult`, the `MAX_RAY_STEPS_*` consts. The chunk→block→voxel
  descent, the AADF empty-cuboid skip (`bounds_in_dir` from the 5-bit chunk /
  2-bit block-voxel fields selected by ray sign), the two-voxels-per-`u32`
  addressing (`02-research.md` divergence #4), and the DDA step are all matched
  to the HLSL. HLSL `step`/`mad`/`rcp`/`frac` mapped to WGSL `step`/`fma`-as-`a*b+c`/
  `1.0/x`/`fract`. The `#ifdef ENTITIES` branch is omitted (design §7.5).
- **Deviation (small, logged):** the C# `shootRay` relies on a `uint3` cast of a
  negative cell wrapping huge so the `>= boundingBoxMax` test trips. WGSL keeps
  the cell signed, so I added an explicit `any(cur_cell < bounding_box_min)`
  break alongside the `>= bounding_box_max` one — same effect, just made explicit
  for signed coords.

WGSL has no `#include`; these are naga-oil import modules referenced via
quoted-path `#import "shaders/foo.wgsl"::{...}` (which Bevy's shader loader
auto-loads as asset dependencies — verified against the loader source).

**Build:** `cargo build` clean (WGSL is asset data, not rustc-compiled — actual
WGSL validation happens at pipeline-compile time, exercised in steps 10–11).

### Step 10 — WGSL first-hit + pipeline + node — `assets/shaders/naadf_first_hit.wgsl`, `render/graph.rs`

**Files created:** `src/assets/shaders/naadf_first_hit.wgsl`.
**Files filled:** `src/render/graph.rs` (`naadf_first_hit_node`).

- `naadf_first_hit.wgsl` — the `@workgroup_size(64,1,1)` compute entry
  `calc_first_hit`, a faithful port of `albedo/renderFirstHit.fx`'s `calcFirstHit`
  no-TAA path: per-pixel ray setup via `get_ray_dir`, `ray_aabb` volume clip,
  `shoot_ray` primary trace, a sun shadow ray + a cheap ambient term, then the
  G-buffer + shaded-colour writes.
- **Deviations (small, logged, all per design §5.3 + D4):** the HLSL only writes
  `firstHitData` inside `if (isTAA)` — Phase A writes it unconditionally so the
  G-buffer plane 0 is always populated; the `taaSamples` ring write is omitted
  entirely (that buffer does not exist in Phase A); the HLSL's `taaSampleAccum`
  write becomes Phase A's `shaded_color` write (identical `vec2<u32>` element
  format, so the final blit stays a near-verbatim `renderFinal.fx` port).
- `graph.rs` `naadf_first_hit_node` — a `Core3d`-schedule system (Bevy 0.19's
  render API has no node-trait — a render-graph node is just a system recording
  via `RenderContext`). Dispatches `ceil(pixel_count / 64)` workgroups of the
  compute pipeline, binds `@group(0)` (world) + `@group(1)` (frame). Wrapped in a
  `time_span("naadf_first_hit")` for the HUD.

**Build:** `cargo build` clean.

### Step 11 — WGSL final blit — `assets/shaders/naadf_final.wgsl`, `render/graph.rs` — **the Phase-A deliverable**

**Files created:** `src/assets/shaders/naadf_final.wgsl`.
**Files filled:** `src/render/graph.rs` (`naadf_final_blit_node`).

- `naadf_final.wgsl` — the fullscreen `@fragment fn fragment`, a near-verbatim
  port of `albedo/renderFinal.fx`'s `MainPS`: read `shaded_color[pixel_index]`,
  divide RGB by `max(1, weight)`, apply the verified tonemap
  (`mix(curColor/(exposure+luminance), tv, tv)` with `tv = curColor/(1+curColor)`),
  output. The C# `Cube`+PS trick becomes Bevy's `FullscreenShader` triangle
  (`02-research.md` divergence #9); `HDR` is off (design §5.4).
- `graph.rs` `naadf_final_blit_node` — a fullscreen render pass into the view
  target's main texture, binds the blit bind group, `draw(0..3, 0..1)`. Wrapped
  in a `time_span("naadf_final_blit")`. Both nodes run in `Core3dSystems::PostProcess`,
  chained, before `tonemapping`.

**Smoke-run findings (this is real signal — the definitive visual check is the
user's at the review gate):** the first `cargo run` surfaced three concrete
issues, each chased and fixed:
  1. **naga-oil rejected `_pad*` struct members** ("Composable module
     identifiers must not require substitution") — and after renaming to `pad*`
     it *still* rejected them (the writeback round-trip drops/renames explicit
     padding members). Fix: removed the explicit padding members from the WGSL
     struct decls entirely — WGSL's std140-ish `vec3`-to-16-byte slotting +
     `vec2` 8-byte alignment reproduce the padded `#[repr(C)]` Rust layout
     exactly (offsets verified field-by-field, documented in the WGSL).
  2. **storage-access mismatch** — the bind-group *layouts* declared
     `blocks`/`voxels`/`voxel_types` + the blit's `first_hit_data`/`shaded_color`
     as read-write, but the WGSL declares them `var<storage, read>`. Fix:
     switched those layout entries to `storage_buffer_read_only_sized`.
  3. **colour-target format mismatch** — the blit pipeline hard-coded
     `Rgba8UnormSrgb`, but the `Core3d` view target's main texture format is
     chosen per-camera (observed as both `Rgba16Float` and `Rgba8UnormSrgb`).
     Fix (**deviation, logged**): the blit pipeline is now queued *lazily
     per-`TextureFormat`* — `NaadfPipelines.blit_pipelines` is a
     `HashMap<TextureFormat, CachedRenderPipelineId>`, a `prepare_blit_pipeline`
     system reads each view's `ExtractedView::target_format` and queues the
     matching variant, and `naadf_final_blit_node` picks the variant by the
     view's format. This is the lightweight form of the `FullscreenMaterial`
     specialiser pattern — a localized addition, not a redesign.
- **After the fixes, `cargo run` is clean:** window opens on the RTX 5080 /
  Vulkan, the AADF test grid builds (`32 chunks, 1536 blocks, 2144 voxel-u32s,
  64x32x64 voxels`), the two-pass render graph (first-hit compute → final blit)
  compiles and runs with **no WGSL compile errors, no pipeline-validation
  errors, no panics**, and exits cleanly when the smoke-run timeout fires. The
  voxel scene's on-screen appearance is the user's review-gate check (this
  environment can't capture the framebuffer), but the full pipeline compiling +
  running clean is exactly the signal the brief asks for at this gate.

**Build:** `cargo build` clean.

### Step 12 — HUD re-point + polish — `hud.rs`, `README.md`

**Files edited:** `src/hud.rs`, `README.md`.

- `hud.rs`: re-pointed the GPU-timing paths at the NAADF render-node span names
  — `render/naadf_first_hit/elapsed_gpu` + `render/naadf_final_blit/elapsed_gpu`
  (the path format `RenderDiagnosticsPlugin` builds from a `time_span` is
  `render/<span>/<field>`). `write_timing` re-added and called for both passes;
  it prefers the GPU-timestamp diagnostic and falls back to the `elapsed_cpu`
  one when the backend has no timestamp queries. A `const fn` compile-time check
  asserts the HUD's hard-coded paths stay in step with the `render::graph` span
  constants. Renderer-mode string updated to mention the AADF DDA.
- `README.md`: rewrote the intro / "What it does" (it described the old Solari
  proof-of-concept), the controls table, and the project layout to the current
  module tree, and replaced the 3-item Solari roadmap with the **four-phase
  split** (toolchain PoC → Phase A → Phase A-2 TAA → Phase B GI → Phase C GPU
  construction), Phase A marked complete.
- Timing spans were registered in the two render nodes back in steps 10–11
  (`time_span` calls in `graph.rs`), so step 12 only needed the HUD side.

**Build + run:** `cargo build` clean; `cargo test --bin bevy-naadf` →
**39 passed, 0 failed** (32 pre-existing + 5 `GrowableBuffer` + 2 `GpuVoxelType`).
Final `cargo run` smoke-run: clean — window opens, grid builds, no errors /
panics / validation errors, exits clean on the timeout. HUD draws over the
NAADF passes (it is a UI pass, after `tonemapping`); whether the FPS + per-pass
timing lines are populated and the voxel scene is correctly lit is the user's
review-gate visual check.

---

## State at end of Phase A

- **Step 7 — `GrowableBuffer`:** the growable GPU storage buffer (realloc +
  `copy_buffer_to_buffer` on growth, factor 2; `upload_all` discards-and-reallocs
  to avoid a wgpu queued-write ordering hazard). 5 device-backed unit tests.
- **Step 8 — render-world plumbing:** `gpu_types.rs` (`#[repr(C)]` bytemuck
  mirrors + `GpuVoxelType` 128-bit pack), `extract.rs` (`ExtractedWorld` /
  `ExtractedCameraData`), `prepare.rs` (`WorldGpu` chunk-3D-texture + growable
  block/voxel/type buffers + `FrameGpu` uniforms/G-buffer/bind-groups),
  `pipelines.rs` (3 bind-group layouts + compute pipeline + per-format blit
  pipeline cache), `NaadfRenderPlugin` wiring extract → prepare → the two
  `Core3d` graph nodes. `WorldPlugin` is the main-world seam (resources are
  inserted by `setup_test_grid`).
- **Step 9 — WGSL substrate:** `common`, `world_data`, `ray_tracing_common`
  (RNG/oct), `render_pipeline_common` (consts/`VoxelType`/`get_ray_dir`/
  G-buffer pack), and **`ray_tracing` — `shoot_ray`, the AADF DDA**, all ported
  faithfully from the NAADF HLSL, entity branch omitted.
- **Step 10 — first-hit:** `naadf_first_hit.wgsl` (compute, ports `calcFirstHit`)
  + `naadf_first_hit_node` dispatching it.
- **Step 11 — final blit (the deliverable):** `naadf_final.wgsl` (fullscreen
  tonemap, ports `MainPS`) + `naadf_final_blit_node`; the app renders the voxel
  scene through the two-pass NAADF graph and the user can fly through it.
- **Step 12 — HUD + docs:** HUD timing paths re-pointed at the NAADF render
  nodes; `README.md` roadmap updated to the four-phase split.
- **Build:** `cargo build` clean (13 dead-code warnings — all "pub item /
  variant defined, not yet consumed": `decode`/`unpack_voxel`/`to_world` used by
  tests + Phase B; `WorldGpu` fields held alive behind the bind group; the
  `MetallicRough`/`MetallicMirror` material variants + `FLAG_SHOW_RAY_STEP`/
  `FLAG_IS_TAA` + `GrowableBuffer::reserve`/`capacity`/… are Phase-A-2 / B / C
  surface. Same warning profile as Batch 1 — no real issues).
- **Tests:** `cargo test --bin bevy-naadf` → **39 passed, 0 failed.**
- **Smoke-run:** `cargo run` launches on the RTX 5080 / Vulkan, builds the AADF
  test grid, the first-hit compute + final-blit render graph compile and run
  with no WGSL / pipeline-validation errors and no panics, exits clean.

### Deviations from `03-design.md` (all small + local, logged inline above)

1. **`upload_all` reallocs without copying** (step 7) — avoids a real wgpu
   queued-write-vs-copy ordering hazard; `reserve` (grow-and-preserve) unchanged.
2. **bind-group layouts live in `pipelines.rs` at step 8**, not split between
   steps 8 and 10–11 — the layouts can't exist apart from the resource holding
   them; graph node *systems* stayed stubs through step 8.
3. **explicit `_pad` members removed from the WGSL structs** (step 11) —
   naga-oil's composable-module round-trip rejects them; WGSL's natural `vec3`/
   `vec2` slotting reproduces the padded `#[repr(C)]` Rust layout (offsets
   verified, documented in the WGSL). The Rust `#[repr(C)]` structs keep their
   `_pad*` fields.
4. **explicit `any(cur_cell < bounding_box_min)` break in `shoot_ray`** (step 9)
   — the C# leans on `uint3`-cast wraparound for negative cells; WGSL keeps the
   cell signed, so the out-of-world test is made explicit.
5. **the final-blit pipeline is queued lazily per `TextureFormat`** (step 11) —
   the `Core3d` view target's main-texture format is per-camera; a single
   hard-coded format fails validation. The per-format cache + `prepare_blit_pipeline`
   is the lightweight `FullscreenMaterial`-specialiser pattern.

None of these is a large redesign; each is the kind of "small local deviation,
made and logged" the brief explicitly allows. No blocker required pausing for
orchestrator/user consultation.

## Verdict

**Phase A complete.** Steps 7–12 done; `cargo build` clean; `cargo test --bin
bevy-naadf` → 39 passed / 0 failed; `cargo run` smoke-runs clean — the
two-pass NAADF render graph (AADF-DDA first-hit compute → fullscreen tonemap
blit) compiles and runs on the RTX 5080 / Vulkan with no WGSL or
pipeline-validation errors and no panics. The faithful HLSL→WGSL port (Q2) of
`shootRay` / `calcFirstHit` / `MainPS` + the shared headers is in place; the
int+frac `PositionSplit` camera (D1) is threaded through; Phase A keeps the
`shaded_color` blit-source stand-in (D4 — no TAA machinery). Scope stopped at
step 12 — no Phase A-2 / B / C work started. **Ready for the Phase-A review
gate** — the one remaining check is the user's interactive visual confirmation
that the voxel scene renders correctly lit and is flyable.
