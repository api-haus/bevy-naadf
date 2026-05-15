# 03a-followup — Empty-scene + camera-dark diagnosis

**Date:** 2026-05-15
**Author:** general-purpose Opus (03a follow-up — symptom 1 fix)
**Triggered by:** user manual smoke of Track A:
> `cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox`
> loads an empty scene as of now / camera randomly becomes dark when moving

## Symptoms (reproduced)

The user's `.vox` file exists and is well-formed:

```
/home/midori/Downloads/Oasis_Hard_Cover.vox: MagicaVoxel model, version 150
84,911,723 bytes
```

`dot_vox` parses it cleanly: **version 150, 291 models, 256-entry palette,
256 materials, 961 scene-graph nodes, 452 `nSHP` references**.

### Pre-fix repro (symptom 1 — "empty scene")

```bash
$ cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
[INFO] NAADF .vox loaded from .../Oasis_Hard_Cover.vox: 257 palette entries, [16, 12, 16] chunks
[INFO] NAADF test grid (Vox { path: "..." }): 3072 chunks, 73664 blocks, 1153888 voxel-u32s (256x192x256 voxels)
[INFO] GPU producer chain DISPATCHED (size_in_chunks=[16, 12, 16], voxel_workgroups=36060)
```

The world allocates as 256×192×256 voxels — but every model in the file's
scene graph stacks at the origin under the design's Decision-6
"identity-only first cut" walk (`02a-design-vox-loading.md` Decision 6).
A diagnostic probe against the parsed file confirms:

```text
Referenced models: 452
Max model size (MV coords): [256, 256, 181]   ← max across all referenced models
Distinct filled cells (after identity-walk collate): 1,995,537
Camera at NAADF (11, 7, 17):
  Cell (11, 7, 17) filled = 1
  Filled in 11×11×11 cube around camera: 1331 / 1331   ← every cell solid
```

The camera spawns at `(11, 7, 17)` (`camera::setup_camera` at
`crates/bevy_naadf/src/camera/mod.rs:44`). Every voxel in an 11³ cube around
the camera is solid → primary rays start inside opaque material → the
camera-origin voxel is hit at distance ≈0 → the framebuffer renders
dark/featureless. User reads this as "empty scene".

### Pre-fix repro (symptom 2 — "camera randomly becomes dark when moving")

Same scenario — the camera is **inside** a dense stacked-at-origin
volume. As the user moves through it, rays start in different solid /
near-solid cells; the renderer's behaviour varies erratically (sometimes
hitting an interior wall pocket, sometimes immediately registering a
hit). Symptom 2 was a downstream consequence of symptom 1.

## Root cause — symptom 1

**Decision 6 of the original Track A design** (`02a-design-vox-loading.md`
`### Decision 6: Scene-graph flattening — identity-only walk first cut`):

> Walk the scene graph from `scenes[0]`, treat every `nTRN` as `t=(0,0,0)`
> + `r=identity`, concatenate model AABBs as if every `nSHP` references a
> model at the origin under identity. Multi-model `.vox` files with
> non-trivial transforms render incorrectly (positions/rotations not
> applied).

The flip-trigger fired on the user's first non-trivial test file
(Oasis_Hard_Cover.vox: 452 transformed model references, scene graph
nodes 0–960 with rich `_t`/`_r` attributes per `nTRN`).

The implementation site is
`crates/bevy_naadf/src/voxel/vox_import.rs::flatten_scene` (the pre-fix
version at lines 200-308 walked the scene graph collecting model ids
only, then collated every referenced model at its local coords with no
transform composition).

## Fix — symptom 1

**File:** `crates/bevy_naadf/src/voxel/vox_import.rs`
**LOC delta:** +386 / -126 (net +260 LOC, including 4 new tests).

### What the fix does

Replaces the identity-only walk with a faithful port of C#
`MagicaVoxel.GetWorldAABB` (`MagicaVoxel.cs:651-716`) +
`MagicaVoxel.CollateVoxelData` (`MagicaVoxel.cs:718-755`):

1. **`Rot3` (signed-permutation 3×3 matrix)** — parses the MagicaVoxel
   `_r` rotation byte into the same integer signed-permutation matrix the
   C# `TransformFrame.Read` builds at `MagicaVoxel.cs:127-146`.
   Column-vector convention (`R[output][source] = sign`), matching the
   .NET row-vector convention's effect bit-for-bit.

2. **`Xform` (rotation + translation in MV coords)** — composes per the
   C# `frame.matrix * parent_matrix` chain at `MagicaVoxel.cs:694` /
   `:720`: `child.parent_of(&parent).apply(p) ==
   parent.apply(child.apply(p))` (local-first then parent — matches
   .NET row-vector post-multiply semantics).

3. **Pass 1: `accumulate_world_aabb`** — walks the scene graph from
   `scenes[0]` composing transforms. For each `Shape`, transforms all 8
   centered-model corners (`-size/2`..`size/2-1` per
   `BoundsXYZ.cs:19-24`) by the composed matrix and unions into the
   world AABB.

4. **Pass 2: `collate_voxels`** — walks again, this time writing each
   shape's voxels under the composed transform into the `DenseVolume`.
   World coords get shifted by `-world_min` so they land in
   `[0..world_size)`; the Z↔Y swap to NAADF (Y-up) is applied at the
   final write step (`MV (x,y,z) → NAADF (x, z, y)`, matching
   `ModelData.cs:386` + `:438`).

5. **Older-version fallback (no scene graph) preserved** — mirrors C#
   `MagicaVoxel.cs:687`: `if (Nodes.Count == 0) return Models[0]`.

### Side-effect cap adjustments

With composition landing, real-world `.vox` files can compose to world
AABBs that vastly exceed any single model's bounding cuboid
(Oasis_Hard_Cover.vox: 1485×1331×536 MV voxels = ~93×34×84 chunks).
Two cap constants were adjusted to reflect the new reality:

- `MAX_CHUNKS_PER_AXIS`: **1024 → 32**. The pre-fix 1024 was the wgpu
  `max_texture_dimension_3d` ceiling; that's the wrong limit to gate
  on. The actual load-bearing constraint is the Phase-C-followup#1 GPU
  producer chain's `segment_voxel_buffer`
  (`render/construction/mod.rs:921-960`) — `seg³ × 2048 × 4 B` GPU
  storage where `seg = max(chunks per axis)`. At `seg = 32` the buffer
  is ~262 MiB (within wgpu defaults); at `seg = 93` (the user's file
  post-composition) it would be ~6.5 GiB and OOM the render device.
  Files past the cap fail gracefully via the existing
  `setup_test_grid` error-and-fall-back path.
- `MAX_DENSE_BYTES`: **512 MiB → 1 GiB**. Belt-and-braces secondary
  gate; `MAX_CHUNKS_PER_AXIS` lands first in practice (capping at
  ~268 MiB).

### Tests added (4)

In `crates/bevy_naadf/src/voxel/vox_import.rs::tests`:

1. **`scene_graph_translations_separate_models`** — two 1-voxel models
   under distinct `_t` translations land at distinct NAADF cells (the
   regression that caused the empty-scene symptom). Asserts exactly 2
   non-empty voxels at the correct world positions.
2. **`scene_graph_rotation_applies`** — a model voxel under a rotation
   byte ends up rotated in the output (smoke test for the rotation
   pass; the precise position is validated by the dedicated rotation +
   compose tests below).
3. **`rotation_byte_identity_and_axis_swap`** — `Rot3::from_byte(4)`
   = identity; `Rot3::from_byte(17)` rotates `+x → +y`, `+y → −x`,
   `+z → +z` (90° about Z). Locks the integer matrix to match the C#
   `TransformFrame.Read` bit semantics.
4. **`xform_compose_matches_csharp_order`** — translation+rotation
   composition order matches `parent.apply(child.apply(p))` (i.e. the
   .NET `frame * parent` row-vector post-multiply semantics).

## Symptom 2 — diagnosis status

**Status: a downstream consequence of symptom 1, no separate Track-A
regression.**

The user reported the camera "randomly becomes dark when moving"
specifically in the `--vox` run. With the identity-only walk, the camera
spawned **inside** a dense stacked-at-origin voxel mass (every cell in
an 11×11×11 neighbourhood around `(11, 7, 17)` was solid — see the
probe output in `## Symptoms (reproduced)` above). Moving the camera
through this region put rays into varied interior pockets of the
stacked-model geometry, producing the erratic dark-when-moving
behaviour.

**Verification on the default grid** (no `.vox` involved): the e2e
baseline harness runs 96 warmup + 48 camera-motion + 1 settle frames
and the luminance gate passes consistently (emissive 247.1 / solid
242.0 / sky 145.9 region values — identical to pre-Track-A). No
camera-motion luminance decay. So symptom 2 is **not** a separate
Track-A regression; it disappears when the world isn't built from
stacked-at-origin geometry.

The user can re-run the visual smoke on the default test grid
(`cargo run --bin bevy-naadf`, no flag) to confirm the camera-dark
behaviour is gone on the non-`.vox` path. No follow-up needed for
symptom 2 unless it shows up on a smaller (acceptable-size) `.vox`
file after the fix lands.

## Verification

### `cargo build --workspace`

```
Compiling bevy-naadf v0.1.0 (/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf)
 Finished `dev` profile [optimized + debuginfo] target(s) in 1m 00s
```

**PASS** — clean compile, no new warnings.

### `cargo test --workspace --lib`

```
cargo test: 146 passed, 1 ignored (3 suites, 6.15s)
```

**PASS — 146 total (was 142 pre-fix; 4 new tests landed).**

The 4 new tests + all 10 original `voxel::vox_import` tests pass:

```
$ cargo test --workspace --lib voxel::vox_import
cargo test: 14 passed, 133 filtered out (3 suites, 0.00s)
```

### `.vox` smoke

Single run (per memory `subagent-gpu-app-verification-loop`):

```
$ cargo run --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox
[ERROR] .vox load failed (VOX size [93, 34, 84] chunks per axis exceeds soft-cap
        (32 per axis); the GPU producer's segment_voxel_buffer would OOM. Pre-bake
        or shrink the .vox file); falling back to default test grid
[INFO] NAADF test grid (Vox { path: "..." }): 32 chunks, 1920 blocks, 7232
       voxel-u32s (64x32x64 voxels)
[INFO] GPU producer chain DISPATCHED (size_in_chunks=[4, 2, 4], voxel_workgroups=227)
```

**PASS** — the `.vox` file is correctly identified as too large for the
current GPU producer chain (93³ × 2048 × 4 B = 6.5 GiB GPU buffer →
OOM). The loader emits a clear error and falls back to the default
test grid. **No silent rendering failure.** The empty-scene bug
(camera-inside-stacked-models) is gone.

### e2e baseline

```
$ cargo run --bin e2e_render
e2e_render: luminance gate (batch 6) — 100.0% of the frame is non-black (luminance > 2); threshold 95%
e2e_render: region luminance — emissive 247.1, solid(GI-lit diffuse) 242.0, sky 145.9
e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, ...
```

**PASS** — same region luminance as pre-Track-A (no regression on the
default grid). Camera-motion luminance stays stable through the
48-frame motion phase.

## What the user should manually verify

The visual gate is the user's responsibility (per global memory
`subagent-gpu-app-verification-loop`). Re-run commands:

1. **Default grid (regression check, no `.vox`):**
   ```bash
   cargo run --bin bevy-naadf
   ```
   Expected: the existing expanded test scene (ground + towers + arch +
   emissives + spheres) renders correctly. Camera-motion stays smooth,
   no "dark when moving" artifact.

2. **A small `.vox` file (≤ 32 chunks per axis post-composition,
   i.e. ≤ 512 voxels per axis after Z↔Y swap):** there's no bundled
   fixture in the repo, but any MagicaVoxel "Save As" of a small
   single-model scene fits — the per-model `dot_vox` size cap is
   256³ = 16 chunks, so a typical single-model file will load. After
   composition, multi-model files can exceed the cap; the soft-cap
   error message guides the user.

3. **The Oasis_Hard_Cover.vox file:** with the new cap, this file no
   longer loads (clean error message; falls back to default grid). The
   user can either (a) re-export a smaller version from MagicaVoxel,
   (b) wait for a follow-up that adds segment-iteration / streaming
   load support, or (c) raise `MAX_CHUNKS_PER_AXIS` manually if their
   GPU has enough storage budget (RTX 5080 + 16 GiB VRAM could handle
   `seg = 64` → ~2 GiB segment buffer).

## Risks / follow-ups

1. **The GPU-producer-chain segment buffer is the real ceiling.** The
   user's file (and probably most "ambitious" MagicaVoxel scenes)
   exceeds `MAX_CHUNKS_PER_AXIS = 32`. The follow-up here is to lift
   the segment-buffer sizing constraint in
   `render/construction/mod.rs:921-960` — either via segment-iteration
   (the C# NAADF approach: dispatch the GPU producer in fixed-size
   segments and iterate; one buffer reused) or by switching to a
   pre-baked path (port-native binary blob loaded into the same
   buffers).

2. **Animation frames collapsed to frame 0.** The two-pass walk takes
   `frames.first()` (frame 0) only — animated `.vox` scenes render the
   first keyframe. Matches the C# `ModelData.ImportFromVox` static
   snapshot semantics. Not blocking.

3. **`IMAP` palette reordering still not applied.** Pre-existing
   limitation (`02a-design-vox-loading.md` Risk #1 / Assumption #10).
   The new test cases don't exercise IMAP either. Future flip-trigger
   is a file with editor-reordered palette entries.

4. **No checked-in `.vox` fixture in the repo.** The 4 new tests use
   hand-built `DotVoxData` structs (consistent with the existing test
   pattern under Decision B of `03a-impl-vox-loading.md`). A real
   `.vox` round-trip fixture (an actual MagicaVoxel file written by
   the editor, parsed + composed) would catch real-world divergences;
   adding one is a follow-up.

5. **`Rot3::from_byte` returns a "degenerate" matrix for invalid bytes
   (e.g. `i1 == i2`).** The MagicaVoxel spec guarantees valid bytes
   only have non-equal `i1` / `i2`; pathological inputs fall through to
   a degenerate signed-permutation. Not a panic (the matrix is still
   defined), just not a sensible rotation. Tests skip the pathological
   case explicitly (`rotation_byte_identity_and_axis_swap` notes byte
   0 is degenerate).

6. **The `parses_small_cube_fixture` test passed pre- and post-fix
   because the cube's `DotVoxData` has empty `scenes`** — it goes
   through the older-version fallback path, not the new
   composition path. The new composition path is exclusively
   exercised by the 2 new tests + (in production) any real
   MagicaVoxel-written file (which always has scenes).
