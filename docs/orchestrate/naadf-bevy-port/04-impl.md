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
