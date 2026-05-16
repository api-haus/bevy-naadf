# 03e — Implementation log — dirty-flag fix + --vox-grid tiling

**Date:** 2026-05-16
**Author:** general-purpose Opus 4.7 (1M context), dispatched by `/delegate` orchestrator
**Branch:** `main`
**Against:** `docs/orchestrate/feature-completeness/02e-perframe-cpu-investigation.md` (diagnosis, 2026-05-16) + the user's verbatim "load a bunch of those VOX'es on load like the C# version does" directive.

---

## Summary

Two changes in one dispatch:

1. **Dirty-flag fix (Part 1, primary).** The `WorldData.dirty` flag was set
   `true` at world load and never cleared in the main world. Every frame,
   `extract_world` re-cloned the entire ~48 MiB CPU mirror across the
   world boundary (~2.8 ms/frame on Oasis), and `prepare_world_gpu`
   re-allocated + re-uploaded ~50 MiB of GPU buffers (~16.7 ms/frame) —
   accounting for the entire 20 ms gap between 40 FPS Oasis and the
   expected ~180 FPS the named GPU passes (1.23 ms total) leave room for.
   Fix: `extract_world` now mutates the main world via `ResMut<MainWorld>`
   and clears `world_data.dirty` + `voxel_types.dirty` after the copy.
   Edit-path `dirty = true` writes (4 sites in `world/data.rs`) are also
   removed; per-edit changes flow through the W2 delta-upload chain
   (`pending_edits.batches` → `naadf_world_change_node`), the full-world
   re-extract is redundant. The `02e` diagnostic instrumentation is removed
   from `extract.rs`, `prepare.rs`, and `construction/mod.rs`.

2. **`--vox-grid N` tile feature (Part 2, additive).** New CLI flag tiles
   the loaded `.vox` content N×N in the XZ plane. The C# reference loads
   multiple `Oasis_Hard_Cover.vox` instances in a 4×4 grid at startup;
   the port surfaces this as a port-side CLI affordance (faithful in
   effect, divergent in interface — C# has no `--vox-grid` flag). The
   single-tile sparse buckets are replicated to `tiles²` positions; the
   block-dedup HashMap downstream collapses identical content across
   tiles for free. At 4×4 Oasis: world expands to 372×34×336 chunks
   (= 5952×544×5376 voxels = ~17M voxels), but `voxels_cpu` stays at
   10,498,368 u32s (= 42 MiB, identical to single-tile — full dedup).

All 180 in-crate tests pass (179 + 1 new tile test). All 5 e2e gates pass
(baseline · `--validate-gpu-construction` · `--edit-mode` · `--entities`
· `--vox-e2e`). 3 smoke scenarios boot + load cleanly.

---

## Part 1 — dirty-flag fix

### Sites edited

#### `crates/bevy_naadf/src/render/extract.rs:86-130` — `extract_world`

The system param `world_data: Extract<Option<Res<WorldData>>>` was
replaced with `main_world: ResMut<MainWorld>` so the system can mutate
the main world to clear the flag. This is the sanctioned bevy_render
pattern (see `bevy_render::erased_render_asset::extract_render_asset`
at `bevy_render-0.19.0-rc.1/src/erased_render_asset.rs:283`). `Extract<P>`
requires `ReadOnlySystemParam`, so a `ResMut`-via-`Extract` shape (the
investigation's first suggestion) doesn't compile in Bevy 0.19. The
`ResMut<MainWorld>` pattern is functionally equivalent and is the
existing precedent in `bevy_render`.

After the copy, the system clears `world_data.dirty = false` +
`voxel_types.dirty = false` directly via `world.get_resource_mut::<_>()`.

Also removed: the `info!(target: "naadf::perf", "extract_world clone: ...")`
instrumentation block + the `_t0 = Instant::now()` timer (committed as
diagnostic infrastructure in the `02e` investigation; explicitly flagged
for removal in the dispatch brief).

#### `crates/bevy_naadf/src/render/prepare.rs:151-441` — `prepare_world_gpu`

Removed:

- `let _t_prepare = std::time::Instant::now()` + `let _has_existing = ...`
  at lines 179-180.
- The trailing `info!(target: "naadf::perf", "prepare_world_gpu (re)build: ...")`
  log block at lines 446-459.

Functional behavior unchanged — the build-once gate at line 169
(`existing.is_some() && !extracted.dirty`) still drives the no-op path
on every frame after the first dirty render-world flag is consumed.

#### `crates/bevy_naadf/src/render/construction/mod.rs:670-771` — `extract_world_changes`

Removed: the `_t0 / _batch_count / _edited_groups_count` capture at
lines 670-674 + the per-120-frame `info!` block at lines 757-771. The
diagnostic confirmed this system is 0.000 ms on stationary scenes and
not a contributor — instrumentation served its purpose; removed.

#### `crates/bevy_naadf/src/render/construction/mod.rs:837-1859` — `prepare_construction`

Removed: the `_t_prepare = Instant::now()` capture at lines 837-843 +
the trailing per-120-frame `info!` log at lines 1843-1859. Steady-state
cost was confirmed at 0.038 ms (Oasis) / one-time 0.733 ms first-frame
(test grid).

#### `crates/bevy_naadf/src/world/data.rs` — 4 edit-path `dirty = true` sites

The `02e` investigation cited these 4 sites; all 4 are inside the
W2 edit-batch handlers. After the fix, **per-edit changes flow only
through the W2 delta chain** (`pending_edits.batches.changed_chunks/
blocks/voxels` → extracted by `extract_world_changes` → uploaded by
`naadf_world_change_node`); the full-world re-extract the `dirty` flag
triggered was redundant.

- `:211` — `set_voxel`. Now a docblock comment in place of `self.dirty = true`.
- `:769` (was) → now `:776` after the comment insert above —
  `set_chunks_uniform_batch`. Same treatment.
- `:880` (was) → now `:890` after the previous comment insert — inside
  `set_voxels_batch` post-batch push. Same treatment.
- `:1008` (was) → now `:1022` — inside `set_voxels_batch_oracle`. Same
  treatment.

The initial-load sets at `voxel/grid.rs:{115,122,189,195}` (test grid +
`.vox` fallback default) and `voxel/vox_import.rs:{213,224}` (sparse
`.vox` install) **stay** — they trigger the one-shot extract + GPU
upload at startup.

The `--edit-mode` validation gate at
`render/construction/mod.rs:2750-2771` asserted `world_data.dirty` after
a `set_voxel` call. This assertion was removed (the gate still asserts
`pending_edits.batches` is non-empty + `chunks_cpu` was mutated, which
together prove the edit landed in the W2 delta chain — the actual
runtime path).

### Cite-to-`02e` rationale

The investigation's headline at `02e-perframe-cpu-investigation.md:12`
identified the symptom; §"Root cause" §1-3 at `02e:90-156` identified the
mechanism; §"Proposed fix shape" at `02e:166-209` proposed the 1-LOC
fix; §"R1 — `set_voxels_batch` may over-trigger" at `02e:242-248`
identified the per-edit hardening (since the W2 delta chain already
delivers per-edit changes correctly). All four are implemented here.

---

## Part 2 — `--vox-grid` tiling

### CLI flag

#### `crates/bevy_naadf/src/main.rs`

Added a `--vox-grid <N>` parse step before the `--vox` parse. Default
`tiles = 1`. Negative / zero / non-integer values produce a clear error
(parse + exit). The `tiles` value threads into `GridPreset::Vox { path,
tiles }`. The flag is order-independent relative to `--vox`.

### `GridPreset::Vox` enum extension

#### `crates/bevy_naadf/src/lib.rs:55-78`

Extended `GridPreset::Vox` from `{ path: PathBuf }` to `{ path: PathBuf,
tiles: u32 }`. The doc-comment notes "Faithful in effect, divergent in
interface" — C# has no `--vox-grid` flag; the multi-load is menu-driven
in C#.

Constructor sites updated (all unconditional `tiles: 1`):
- `crates/bevy_naadf/src/main.rs:53-58` — sets `tiles` from `--vox-grid`.
- `crates/bevy_naadf/src/e2e/vox_e2e.rs:353` — fixture uses `tiles: 1`.

### Tile-replication implementation

#### `crates/bevy_naadf/src/voxel/vox_import.rs`

The chosen approach: **parse + compose to single-tile `ChunkBuckets`
once, then replicate the buckets to N² XZ positions in a larger
`ChunkBuckets`, then run `build_constructed_world_sparse` on the
larger buckets.** This minimizes the diff — `build_constructed_world_sparse`
already handles arbitrary-size sparse worlds + dedupes blocks across
chunks; feeding it a tiled bucket-array Just Works.

New functions:

- **`parse_dot_vox_data_tiled(data: &DotVoxData, tiles: u32) ->
  Result<ImportedVox, _>`** — the new entrypoint. Single-tile path
  (tiles == 1) is equivalent to old `parse_dot_vox_data`.
  `parse_dot_vox_data` now just calls `parse_dot_vox_data_tiled(data, 1)`.

- **`parse_vox_bytes_tiled(bytes: &[u8], tiles: u32)`** + **`load_vox_tiled(path, tiles)`** —
  the two convenience wrappers paralleling the existing untiled APIs.

- **`replicate_buckets_xz(tile_buckets: &ChunkBuckets,
  tile_size_in_chunks: [u32; 3], tiles: u32) -> Result<ChunkBuckets, _>`** —
  the core of the tiling. Builds a new `ChunkBuckets` sized at
  `(tw × N, th, td × N)` and, for each of the N² tile positions, clones
  every non-empty bucket from the source to the destination at
  chunk-offset `(tx × tw, 0, tz × td)`. The `validate_caps` pre-flight
  re-fires on the larger output dims so a tiled cap-exceeding world
  errors with `VoxImportError::SizeExceedsTextureLimit` (rather than
  silently producing an over-budget buffer).

**Per-chunk indexing**: per-tile chunk position is
`(tx × tw + cx, cy, tz × td + cz)`. The bucket data (which stores
`(local_idx, VoxelTypeId)` pairs where `local_idx` is within the chunk's
16³ voxels) is byte-identical across tiles — only the destination
chunk index differs. The voxel-local positions stay in `[0..16)` per
axis.

**Block dedup behavior**: `build_constructed_world_sparse` uses a
`HashMap<[VoxelTypeId; 64], VoxelPtr>` keyed on literal 64-voxel block
content (Decision Δ-Hash in `02a-v2`). Since every tile contributes
identical voxel content, the second-and-later tile's blocks all hit
the dedup map (zero new `voxels_cpu` allocation per additional tile).
This is verified by the new test:

> `tiled_load_expands_world_xz_and_dedups_blocks`: drives the
> `build_small_cube` fixture through `parse_dot_vox_data_tiled(_, 3)`,
> asserts `chunks_cpu.len()` scaled by 9 (3²), and `voxels_cpu.len()`
> stayed **identical** to the single-tile output.

**Camera framing**: `InitialCameraPose::from_world_voxels` already
derives from `world_voxels = size_in_chunks × 16`
(`crates/bevy_naadf/src/camera/mod.rs:54-64`) — no separate
camera-init logic for the tiled path; it Just Works because the loaded
world's chunk count drives the formula transparently. Verified in the
smoke runs: single Oasis frames at `(726.56, 850, 52.5)`; 4×4 Oasis
frames at `(2906.25, 850, 210)` — 4× the X, identical Y, 4× the Z, per
the C#-faithful rescaling.

### Install-time logging

`crates/bevy_naadf/src/voxel/grid.rs:125-150` — the `info!` log now
appends `(tiled NxN in XZ)` when `tiles > 1`. The Default-grid `info!`
is untouched.

---

## Changes by file

### Source files (10 files)

| Path | Δ-LOC | Description |
|---|---:|---|
| `crates/bevy_naadf/src/main.rs` | +35/-3 | `--vox-grid <N>` CLI flag parse + threading into `GridPreset::Vox { tiles }`. |
| `crates/bevy_naadf/src/lib.rs` | +14/-1 | `GridPreset::Vox` enum extended with `tiles: u32` field + doc-comment. |
| `crates/bevy_naadf/src/voxel/vox_import.rs` | +130/-9 | NEW `parse_dot_vox_data_tiled` / `parse_vox_bytes_tiled` / `load_vox_tiled` / `replicate_buckets_xz`. `parse_dot_vox_data` now delegates to the tiled variant with `tiles=1`. + 1 new test (`tiled_load_expands_world_xz_and_dedups_blocks`). |
| `crates/bevy_naadf/src/voxel/grid.rs` | +12/-3 | `GridPreset::Vox` arm now destructures `{ path, tiles }` + calls `load_vox_tiled(path, *tiles)` + the install-time `info!` log appends `(tiled NxN in XZ)`. |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | +1/-1 | Constructor updated to include `tiles: 1`. |
| `crates/bevy_naadf/src/render/extract.rs` | +28/-22 | `extract_world` widened to `ResMut<MainWorld>`; clears `world_data.dirty` + `voxel_types.dirty` after copy. Doc rewritten to cite `02e`. Instrumentation removed (Instant + `info!` block). |
| `crates/bevy_naadf/src/render/prepare.rs` | +0/-19 | `prepare_world_gpu` instrumentation removed (Instant + `info!` block). |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +0/-32 | `extract_world_changes` + `prepare_construction` instrumentation removed. `--edit-mode` validation assertion on `world_data.dirty` removed (W2 delta chain is now the contract; the assertion would fail post-fix because we no longer set dirty on edits). |
| `crates/bevy_naadf/src/world/data.rs` | +24/-4 | 4 edit-path `self.dirty = true` writes replaced with doc-block comments citing `02e`. |

### Documentation

| Path | Change |
|---|---|
| `docs/orchestrate/feature-completeness/03e-impl-dirty-fix-and-vox-grid.md` | NEW (this file). |
| `docs/orchestrate/feature-completeness/README.md` | +1 file-table row for `03e-*`. |

**Net diff:** ~244 LOC added, ~94 LOC removed (mostly instrumentation).
The dirty-flag fix is ~12 functional LOC + ~40 LOC of doc-comments
citing `02e`. The tile feature is ~150 LOC including the 1 new test.

---

## Test summary

| Suite | Before | After | Delta |
|---|---:|---:|---:|
| `vox_import::tests` | 16 | 17 | +1 (`tiled_load_expands_world_xz_and_dedups_blocks`) |
| Other in-crate | 163 | 163 | 0 |
| **Total** | **179** | **180** | **+1** |

`cargo test --workspace --lib` reports `180 passed, 1 ignored (3 suites,
4.49s)` post-impl.

---

## Verification

### Gate 1 — `cargo build --workspace`

**PASS**, clean build, 51.84s after the full implementation.

### Gate 2 — `cargo test --workspace --lib`

**PASS** — 180 passed, 1 ignored (pre-existing). +1 net from the new
tile test.

### Gate 3 — 5 e2e modes

All run via `cargo run --bin e2e_render -- [flag]`. All PASS:

| Mode | Verdict | Region luminance (emissive / solid / sky) |
|---|---|---|
| `baseline` | PASS | 247.0 / 242.0 / 145.9 |
| `--validate-gpu-construction` | PASS | + GPU/CPU byte-equal 388 bytes |
| `--edit-mode` | PASS | edit-mode validation green: 1 changed_chunks + 1 changed_blocks + 2 changed_voxels |
| `--entities` | PASS | entity handler 8 chunk_updates / 1 entity_chunk_instances / 1 history |
| `--vox-e2e` | PASS | central rect luminance 249.6 (> 160 threshold) |

The `--edit-mode` bit-exact oracle gate is the contract referenced by
the dispatch brief. **It continues PASSING** — confirms the W2 delta
chain handles per-edit changes faithfully without needing the
`world_data.dirty = true` write that's been removed from the 4 edit
paths.

### Gate 4 — 3 smoke runs (release build)

Per global memory `subagent-gpu-app-verification-loop`: one smoke per
scenario, no visual-iteration loop. The user verifies live FPS.

| Smoke | Verdict | World size | Boot |
|---|---|---|---|
| **Default test grid** (`cargo run --release --bin bevy-naadf`) | BOOT OK | 4×2×4 chunks (64×32×64 voxels), 32 chunks, 1920 blocks, 7232 voxel-u32s | Clean — `NAADF test grid (Default)` log line; no error fallback, no panic. |
| **Single Oasis** (`--vox /home/midori/Downloads/Oasis_Hard_Cover.vox`) | BOOT OK | 93×34×84 chunks (1488×544×1344 voxels), 265,608 chunks total, 1,617,216 blocks_cpu u32s, 10,498,368 voxels_cpu u32s | Clean load — `(sparse path, GPU producer skipped)` log line. Camera framed at `(726.56, 850.00, 52.50)`. No fallback, no panic. |
| **4×4 Oasis tile** (`--vox …Oasis_Hard_Cover.vox --vox-grid 4`) | BOOT OK | 372×34×336 chunks (5952×544×5376 voxels), 4,249,728 chunks total, 25,875,456 blocks_cpu u32s (= 16× single-tile, as expected), **voxels_cpu 10,498,368 u32s (= identical to single-tile; full block dedup)** | Clean load — `(sparse path, GPU producer skipped) (tiled 4×4 in XZ)` log line. Camera framed at `(2906.25, 850.00, 210.00)` — perfect 4× scaling from the single-Oasis pos. No fallback, no panic. |

Notes on the 4×4 smoke:
- World volume scales 16× (4 × 4 tiles in XZ); chunks_cpu scales 16×;
  blocks_cpu scales 16× (every tile's chunks contribute their own 64 block
  slots per Mixed chunk, even with content-identical blocks the block
  records themselves are 64 u32s per chunk).
- **voxels_cpu does NOT scale** — the `HashMap<[VoxelTypeId; 64], VoxelPtr>`
  block dedup in `build_constructed_world_sparse` collapses identical-content
  blocks across tiles. This is the "free win" the dispatch brief
  predicted: 4×4 Oasis is 16 copies of the same .vox content, so the
  unique-block set is identical to single-Oasis.
- `chunks_cpu` × 8 (Rg32Uint texel stride) = 33.6 MiB upload to the 3D
  chunks texture. Within wgpu Vulkan baseline `max_buffer_size = 256 MiB`.
- `blocks_cpu` × 4 = 103.5 MiB. Below the 256 MiB conservative cap.
- `voxels_cpu` × 4 = 42 MiB. Unchanged from single-Oasis.

---

## C# parity status

The dirty-flag fix unblocks:

- **Single Oasis stationary FPS** — diagnostic predicted 40 → ~180 FPS
  post-fix, bounded by the named GPU passes (1.23 ms HUD'd) + Bevy
  plugin overhead per `02d`. The user verifies live.
- **4×4 Oasis (the C# benchmark target — 130 FPS in fullscreen +
  painting)** — depends on the GI / DDA cost at the larger texture
  size + however the GI passes scale. The fix removes the
  `extract_world` + `prepare_world_gpu` per-frame ceiling; the
  remaining cost is the per-pixel GI work + the named passes, which
  are the same shape as single-Oasis. The user verifies live.

If the post-fix gap to C# 130 FPS on 4×4 Oasis persists, the next
investigation is the `02d`-identified Bevy `DefaultPlugins` overhead
+ any GI / sun-shadow cost at the larger world size.

The tile feature itself is the port-side affordance equivalent to C#'s
startup-time multi-`.vox` load behaviour. C# loads multiple Oasis
instances via menu / config; the port surfaces this as `--vox-grid N`
since the port has no menu UI. **Faithful in effect, divergent in
interface.**

---

## What the user manually verifies

Run each smoke from the project root and check live HUD FPS:

```bash
# Scenario A — default test grid, post-fix should hold ~240 FPS
cargo run --release --bin bevy-naadf

# Scenario B — single Oasis. Pre-fix: ~40 FPS. Post-fix predicted: ~180 FPS.
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox

# Scenario C — 4×4 Oasis grid. C# benchmark target: 130 FPS fullscreen + painting.
cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox --vox-grid 4
```

Expected boot output for Scenario C:
```
NAADF .vox loaded from /home/midori/Downloads/Oasis_Hard_Cover.vox: 257 palette entries,
  world bounds 372×34×336 chunks (5952×544×5376 voxels), 4249728 chunks total,
  blocks_cpu 25875456 u32s, voxels_cpu 10498368 u32s
  (sparse path, GPU producer skipped) (tiled 4×4 in XZ)
camera::setup_camera: framing loaded world — pos=(2906.25, 850.00, 210.00), look_at=(2906.25, 850.00, 211.00)
```

The user verifies the HUD FPS reading + visual world geometry. The
implementer does NOT loop on visual perf per global memory
`subagent-gpu-app-verification-loop`.

---

## Risks / follow-ups

### R1 — Post-fix per-frame profile match

The pre-fix Oasis frame time was ~25 ms (40 FPS), broken down as:
- `extract_world` clone: 2.81 ms/frame
- `prepare_world_gpu` rebuild: 16.73 ms/frame
- Named GPU passes (HUD): 1.23 ms
- Residual / Bevy overhead: ~4 ms

Post-fix prediction: `extract_world` + `prepare_world_gpu` drop to
0 ms steady-state (after the first build-once frame). Frame time:
~5-6 ms (180-200 FPS).

If the user's live HUD shows post-fix Oasis frame time materially
above ~6 ms, the next investigation is the `02d`-identified Bevy
`DefaultPlugins` curation (~5-15% gain) and any residual GI cost
not in the HUD-named passes. Not in scope for this dispatch.

### R2 — 4×4 Oasis blocks_cpu = 103.5 MiB upload cost

The post-fix first-frame upload uploads 103.5 MiB of blocks +
33.6 MiB of chunks texture + 42 MiB of voxels = ~180 MiB total
one-shot. This is paid ONCE at startup; subsequent frames are
no-ops (after the dirty-flag fix). User-visible as a startup hitch
on the first ~1 sec; not a per-frame concern.

If the 4×4 (or 5×5 / 6×6) tiled Oasis exceeds wgpu's
`max_buffer_size`, the pre-flight cap in `replicate_buckets_xz` will
fire `VoxImportError::SizeExceedsTextureLimit` (axis cap) or
`VoxImportError::SizeExceedsBudget` (buffer cap) cleanly. At 4×4
Oasis the world is 372×34×336 chunks — within 1024 per axis.

### R3 — Test grid camera pose unaffected

`GridPreset::Default` does NOT insert `InitialCameraPose`, so the
camera falls back to the hardcoded `(11, 7, 17)` test-grid pose at
`camera/mod.rs:111`. The dirty-flag fix doesn't touch this path.
Verified by Gate 3 baseline e2e: emissive 247 / solid 242 / sky 146
luminance is bit-identical to pre-impl.

### R4 — `Extract<ResMut<MainWorld>>` vs `Extract<Option<ResMut<T>>>`

The dispatch brief proposed `Extract<Option<ResMut<T>>>`. This **does
not compile** in Bevy 0.19 — `Extract<P>` requires `P:
ReadOnlySystemParam`, and `ResMut<_>` is not read-only. The chosen
pattern (`ResMut<MainWorld>` direct) is the bevy_render-internal
precedent (see `erased_render_asset::extract_render_asset` at
`bevy_render-0.19.0-rc.1/src/erased_render_asset.rs:283`) and
functionally equivalent.

### R5 — `set_voxels_batch_oracle` no longer sets dirty

The oracle path (`world/data.rs:913+`, used by `--edit-mode` validation
test and reserved for CPU-fallback) had its `dirty = true` write
removed. The W2 delta chain still carries the synthetic batch the
oracle produces. **The `--edit-mode` bit-exact gate continues
PASSING**, confirming the contract holds.

### R6 — Tile loop is single-threaded

`replicate_buckets_xz` walks N² × tile-chunks in a single loop, cloning
each `Vec<(u16, VoxelTypeId)>` bucket. For 4×4 Oasis: 16 tiles × 265K
chunks = ~4.2M bucket clones (most are `None` since most chunks in
Oasis are empty). Single smoke measured ~no observable hitch. If
larger tile counts (e.g. 8×8 Oasis = 64× world, 17M chunks) become
slow, parallelizing via `rayon::par_iter` over the N² tile positions
is the optimization path. Not in scope.

### R7 — `--vox-grid N` for non-Oasis content

The flag is general — works on any `.vox` file. For small fixtures
(8³ small_cube) at N=3 the world is 3×1×3 chunks (= 48×16×48 voxels);
camera framing rescales proportionally. The test
`tiled_load_expands_world_xz_and_dedups_blocks` covers this case.

### R8 — Y-axis not tiled

The tile factor only applies to XZ (horizontal plane). C#'s 4×4
Oasis grid is also XZ. Y-up is the world height; a Y-tile would
stack the world vertically, which is not what the C# reference does.
If a Y-tile is needed later, it's a trivial extension to
`replicate_buckets_xz` (rename + add a Y dimension to the loop). Not
in scope.

### R9 — First-frame extract still pays its cost

The fix removes per-frame `extract_world` + `prepare_world_gpu` cost
on stationary frames. The first dirty-frame still pays the full
copy + upload (2.8 ms / 16.7 ms on Oasis = ~20 ms one-shot startup
hitch). This is correct + necessary; the data has to land on the GPU
somehow. User-visible as a tiny startup pause, not a steady-state
concern.

### R10 — Post-fix follow-up: GPU-side per-frame timestamp queries

The `02d` investigation noted the HUD's GPU-timestamp-query overhead
from `RenderDiagnosticsPlugin`. Not a contributor at Oasis scale (the
CPU contributors removed here are 10× larger). Stays as a Phase-D
residual.
