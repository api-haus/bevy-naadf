# vox-gpu-rewrite — residual speckle + brush-clears diagnostic (2026-05-18)

## TL;DR

Two distinct bugs were diagnosed:

1. **`--small-edit-repro` brush-clears (411,196 dark pixels = 19.85% of
   frame)** — `WorldData::seed_block_hashing`
   (`crates/bevy_naadf/src/world/data.rs:130-182`) appends a duplicate copy
   of every unique mixed-block voxel slot to the END of `voxels_cpu` AND
   registers the hash entry with the **appended-end pointer** instead of
   the original pointer that the GPU's `voxels[]` buffer actually has
   data for. Every subsequent edit's hash-dedup lookup in
   `set_voxels_batch` therefore returns the **appended-end pointer**
   for unchanged blocks; the GPU's `apply_block_change` then writes
   those wrong pointers into `blocks[]` for the edited chunk; the
   renderer descends into `voxels[appended-end-ptr ..]` where the GPU
   never wrote and reads **zero half-words**, treating every voxel in
   the chunk as empty with AADF=0 → the entire chunk renders as a
   16-voxel-wide void. Visible as the chunk-sized dark pits surrounding
   the small bright brush voxels.

2. **`--vox-gpu-oracle` per-pixel ceiling (4040 pixels Δ>16 = 6.16%)** —
   a separate, smaller class of CPU-vs-GPU rendering divergence at
   horizon-grazing rays where the GPU phase's fixed-world tiling
   produces extra geometry beyond the CPU oracle's natural-bound world
   (CPU oracle: 1488×544×1344 voxels with empty space past those
   bounds; GPU phase: 4096×512×4096 voxels with `voxelPos % modelSize`
   tiling that adds 2-3 copies of the Oasis architecture along each
   horizontal axis + clips Oasis Y=512..544 because the fixed world is
   2 chunks shorter than the model). Rays that stray outside the natural
   model bounds in CPU oracle hit void → render sky; in GPU phase hit
   tiled geometry → render that geometry. The mean diff is small
   (3.254 < 8.0 floor) because most pixels frame the same primary content;
   the ceiling is exceeded because the 6% of pixels where rays leave
   natural bounds diverge significantly.

Bug 1 is a single-place fix in CPU code. Bug 2 is a design artifact of
the GPU tiling vs CPU non-tiling — needs either an oracle redesign
(GPU-vs-GPU compare with two different install paths) or a tighter
test viewport that excludes horizon-grazing geometry.

**Confidence: HIGH for bug 1. MEDIUM for bug 2** (other contributing
causes plausible — see hypothesis ranking).

## Symptom recap

### `--small-edit-repro` (REGRESSED Stage 2)

- Brush: `cube_brush(pos=Vec3(872.76, 341.0, 507.80), radius=1.0, ty=41,
  is_erase=false)` — places 8 voxels of type 41 (a yellow material) in
  a single 2×2×2 region at world (872..873, 340..341, 507..508). All
  8 voxels fall in chunk (54, 21, 31), spanning 2 blocks of that chunk
  (blocks (2, 1, 2) and (2, 1, 3)).
- CPU verification (`8/8 affected voxels correctly encoded`): the
  CPU mirror has the new voxel types at the expected positions.
- Edit batch: `1 changed_chunks, 1 changed_blocks (= 65 u32s = 1
  chunk × 64 blocks), 2 changed_voxels (= 66 u32s = 2 blocks × 32
  u32-pairs), 0 edited_groups` — the chunk was Mixed before AND after
  so the SetChunk AddChangedChunk gate at
  `world/data.rs:1036-1051` correctly skipped enqueuing the group.
- Render: 411,196 pixels (19.85% of 1920×1080 frame) have RGB sum <
  30 (pitch black). Pre-edit had 0 dark pixels. Δ = +411,196.

### `--vox-gpu-oracle` (failing at per-pixel ceiling since Stage 11)

- Mean per-pixel diff: 3.254 (well under 8.0 floor). ✓ This metric
  passes; Stage 11 dropped it from 127.84.
- Per-pixel ceiling: 4040 pixels (6.16%) with per-channel Δ > 16, vs
  ceiling of 655 (1.0%). ✗
- Spatial pattern (per Stage 11 narrative + gate output text):
  "scattered speckles indicate the W5 GPU producer chain corrupts
  mixed-block dedup / hashing".
- Both oracle PNGs (`oracle_cpu.png` + `oracle_gpu.png`) at 256×256
  show the SAME Oasis architecture visually — cream walls, palm
  trees, sky. The diff is in detail / texture.

## Visual evidence

### `target/e2e-screenshots/small_edit_repro_before.png`

Uniform cream/sand ground filling 1920×1080. No dark pixels. The
camera frames the Oasis ground texture; pre-edit content is
architectural Oasis voxels with diffuse cream coloring at this Y
range.

### `target/e2e-screenshots/small_edit_repro_after.png`

The dominant visual artifacts:

1. **Bright cube near frame center** — the 2×2×2 brush voxels of type
   41 (yellow). Correctly placed, correctly textured. Small (the
   brush is only 2 voxels wide).
2. **Multiple large rectangular dark pits surrounding the cube** — at
   least 4-5 distinct dark regions visible. Each has sharp vertical
   walls and horizontal floors aligned to 4-voxel (block) boundaries.
   The pits' interior surfaces are absolutely black (RGB sum < 30 per
   the gate's threshold).
3. **Sharp 90° edges** at the pit boundaries — characteristic of
   block-aligned axis-aligned voids.

Spatial scale: each pit appears 100-300 pixels wide at the 1920×1080
viewport. The camera is at world Y=345.2, looking down toward world
Y=341 (the ground). The brush chunk (54, 21, 31) is at world
(864..879, 336..351, 496..511) — directly under the camera. A 4×4×4
voxel block at ~5 voxels camera distance projects to roughly
~150-250 pixels per axis — matching the visible pit scale.

The pits are NOT shadows (they would not dip to absolute black). They
are NOT outside-the-world rays (the sky would render lighter). They
are positions where the renderer's ray hit the chunk, descended into
its blocks, found voxels reading as zero (empty with AADF=0), and
marched through to whatever lies beyond — which at that camera angle
is the world's empty void below the ground layer (rendered black by
the GI pass when no geometry returns a contribution).

### `target/e2e-screenshots/oracle_cpu.png` vs `oracle_gpu.png`

Both 256×256, both render the same architectural shape (cream walls,
palm trees at top corners, central sand floor with darker accent
voxels). Side-by-side they look almost identical to the eye. The
diff (4040 / 65536 pixels = 6.16% Δ>16) is concentrated at
**high-frequency texture edges** — sand-vs-wall boundaries, palm
frond outlines, accent-voxel edges. The diff is NOT in large
solid-coloured regions.

This pattern is consistent with a **slight geometric divergence**
(rays hitting slightly different positions on the same surface, due
to either different AADF skip distances or different tiled-vs-void
secondary-ray behaviour) rather than a wholesale type-encoding
mismatch (which would produce solid coloured patches differing
between the two frames).

## Hypothesis investigation

### Brief's four candidate hypotheses

#### H1 — Edit shaders re-introduce AADF-bearing empty voxels (Stage 11 incomplete)

**RANK: REFUTED.** The edit shader `apply_voxel_change`
(`assets/shaders/world_change.wgsl:520-572`) explicitly resets empty
voxels' AADF bits to 0 before the additive `compute_bounds_4`:

```wgsl
let raw_voxel = select(pair_u32 & 0xFFFFu, pair_u32 >> 16u, is_high);
var cur_voxel = select(0u, raw_voxel, (raw_voxel >> 15u) != 0u);
```

(line 543 — `cur_voxel = 0` when bit 15 is clear, i.e., empty.)

The shader then runs `compute_bounds_4(local_index,
voxel_pos_in_block, 15u, 0x1u, cur_voxel)` (line 555) to compute fresh
local 4³ AADFs additively from zero — idempotent with respect to input
AADFs. Output is written via line 568-570:

```wgsl
voxels[change_pointer + local_index] = (lo & 0xFFFFu) | ((hi & 0xFFFFu) << 16u);
```

For full voxels (bit 15 set), `cur_voxel` is restored from
`original_pair` via the `if ((cur_voxel >> 15u) != 0u)` branch (line
558-561). Full voxels pass through unchanged.

**The edit-shader output is byte-correct.** It does not re-introduce
the Stage-10 AADF-leak.

Similarly `apply_block_change` (line 468-507) zeroes empty blocks'
input (line 486) before the additive block-layer compute. Safe.

#### H2 — Stage 11 fix is incomplete (other paths emit AADF-bearing empties)

**RANK: REFUTED for the small_edit_repro case.** Stage 11 strips
AADF bits from empty voxels in `ModelData.data_voxel` at
`crates/bevy_naadf/src/voxel/grid.rs:368-379`. This is the ONLY
ModelData consumed by `generator_model.wgsl::get_voxel_data_in_model`.
No other ModelData encoder path exists in the W5 install path.

For the `compose_default_scene_into_fixed_world` path (Default
grid_preset), there is no ModelData (CPU upload only); the W5
generator chain is skipped for that preset.

#### H3 — W5 producer has edge cases the byte-equality test missed

**RANK: PLAUSIBLE for the vox-gpu-oracle speckle, REFUTED for the
brush-clears.** See bug 2 hypothesis investigation below.

#### H4 — `compute_voxel_bounds` writeback path corrupts voxels[]

**RANK: REFUTED for the brush-clears, PLAUSIBLE for the speckle.**

`compute_voxel_bounds` (`chunk_calc.wgsl:455-500`) reads each voxel
half-word from `voxels[]`, runs `compute_bounds_4` to add AADF bits,
restores full voxels via the `if (state == 1u)` branch (line 488-490),
and writes back. The input voxels at construction time have NO AADF
bits in empty cells (just the type | full-flag for full, or 0 for
empty — written by `calc_block_from_raw_data` from
`segment_voxel_buffer` which is the generator's output). So
`compute_bounds_4` starts from zero and produces correct AADFs.

The write-back at line 495-499 packs two voxels into one u32 per even
thread. No race within or across workgroups. ✓

For the brush-clears, `compute_voxel_bounds` does NOT run after the
brush — only at construction time. The brush flows through
`apply_voxel_change` (which is correct per H1) and `apply_block_change`
(also correct).

### New hypothesis H5 — `seed_block_hashing` appends-and-mispoints duplicate slots — **THE BRUSH-CLEARS BUG**

#### Evidence chain

1. After the GPU readback (`populate_cpu_mirror_from_gpu_producer` at
   `render/construction/mod.rs:897-1041`), the CPU mirror is correctly
   populated from the GPU: `voxels_cpu.len() = 10479424` (= GPU's
   `block_voxel_count[0]/2` cursor).

2. `populate_cpu_mirror_from_gpu_producer` then re-seeds the
   `BlockHashingHandler` via `seed_block_hashing()` (line 1021-1022).
   This is the load-bearing step that needs `block_hashing` to
   reflect which voxel slots are in use (refcounts and pointers) so
   that subsequent edits' `add_block` / `delete_block` calls produce
   the correct slot graph.

3. **`seed_block_hashing`** (`world/data.rs:130-182`) iterates every
   Mixed chunk, every Mixed block in that chunk, and for each calls:

   ```rust
   let hash = self.block_hashing.compute_hash(&pairs);
   let (registered_ptr, is_new) =
       self.block_hashing.add_block(hash, &pairs, &mut self.voxels_cpu);
   if !is_new && registered_ptr != voxel_ptr {
       self.blocks_cpu[block_idx] = (block_raw & !0x3FFF_FFFF) | registered_ptr;
   }
   ```

4. `add_block` (`aadf/block_hash.rs:154-193`), when finding no
   existing entry, calls `alloc_voxel_slot`:

   ```rust
   fn alloc_voxel_slot(&mut self, voxel_pairs: &[u32], voxels_cpu: &mut Vec<u32>) -> u32 {
       if let Some(reuse) = self.free_voxel_slots.pop_front() {
           let base = reuse as usize;
           voxels_cpu[base..base + BLOCK_VOXEL_PAIRS].copy_from_slice(voxel_pairs);
           return reuse;
       }
       let ptr = voxels_cpu.len() as u32;
       voxels_cpu.extend_from_slice(voxel_pairs);  // ← APPENDS A COPY
       ptr
   }
   ```

   `free_voxel_slots` is empty during seeding (nothing has been
   `delete_block`-ed yet), so every first-occurrence `add_block` call
   APPENDS a duplicate copy of the voxel data to the end of
   `voxels_cpu` and returns the END pointer. The hash table's entry is
   registered with `voxels_pointer = END_PTR` (not the ORIGINAL ptr
   the block actually points at in `voxels_cpu`).

5. The `if !is_new && registered_ptr != voxel_ptr` patch at line
   177-179 in `seed_block_hashing` only fires for `is_new = false`
   (subsequent occurrences of the same content). For first
   occurrences (`is_new = true`), `blocks_cpu[block_idx]` is NOT
   patched and keeps the ORIGINAL voxel_ptr.

   **Effect on `blocks_cpu`**:
   - First-occurrence Mixed blocks: `blocks_cpu[i]` still points at
     the original (correct) voxel_ptr; the hash entry says END_PTR.
   - Subsequent-occurrence (deduped) blocks: `blocks_cpu[i]` patched
     to point at END_PTR (matching the hash entry, but END_PTR has a
     duplicate copy in CPU voxels_cpu).

   **Effect on GPU `voxels[]`**: the GPU's `voxels[]` was only
   written up to the construction cursor (~10.5 M u32s = `cursor[0]/2`).
   The END_PTR slots (offset > 10.5 M) on GPU contain implementation-
   defined contents (usually zero on wgpu+NVIDIA Vulkan).

6. **Sanity numbers**. Oasis has 265,608 chunks total; per the run
   logs ~200 K mixed blocks. seed_block_hashing therefore appends ~200 K
   × 32 = ~6.4 M u32s to `voxels_cpu`, growing it from ~10.5 M to
   ~17 M u32s. **All 200K hash-table entries register `voxels_pointer
   = END_PTR` in the 10.5M–17M range**, while the CPU's
   `blocks_cpu` still has the FIRST occurrence pointing at the 0–10.5M
   range.

7. **At edit time**, `set_voxels_batch` calls Stage A's
   `add_block(hash, payload, &mut voxels_cpu)` for each mixed block in
   the edited chunk (`world/data.rs:911-921`). Hash lookup matches the
   existing entry → returns `(END_PTR, false)`. **Including for blocks
   whose CONTENT did not change** (unmodified blocks in the brush's
   chunk).

   ```rust
   new_blocks[b] = voxel_ptr | (2u32 << 30); // ← END_PTR
   ```

   For the brush's chunk, all ~64 Mixed blocks (including the 2
   modified + the 60-ish unmodified) get `new_blocks[b]` pointing at
   END_PTR addresses.

8. **Stage C** (line 988-1008) uploads these `new_blocks` to the GPU
   via `changed_blocks_dynamic`. The GPU's `apply_block_change` writes
   them to `blocks[change_pointer + local_index]` for the chunk's
   64-block slot (`world_change.wgsl:506`). For all unmodified blocks,
   the GPU's `blocks[]` now points at END_PTR voxel addresses.

9. **At render time**, the ray tracer descends the brush's chunk's
   blocks. For the ~60 unmodified blocks pointing at END_PTR, the
   renderer reads `voxels[END_PTR/2 + ...]` which on GPU is **zero**.
   `cur_voxel_pair = 0` → `cur_node = 0` → `(cur_node >> 15) == 0` →
   treated as empty voxel → `bounds_in_dir = 0` (no AADF skip) → DDA
   marches 1 voxel at a time through the entire block.

   The result: the chunk's interior renders as 16×16×16 voxels of
   empty space — a 16-voxel-wide void. The 2 modified blocks (whose
   `add_block` calls in Stage A returned `is_new=true` and pushed
   `voxel_ptr = voxels_cpu.len()` AFTER the seed-appended slots) DO
   get written by `apply_voxel_change` — those render as the brush
   voxels (the bright cube in the screenshot).

10. **Why other chunks are not affected**: only the EDITED chunk's
    `blocks[]` is overwritten by `apply_block_change`. Other chunks'
    `blocks[]` still point at the ORIGINAL voxel_ptrs (in the
    0–10.5M range, where GPU has the correct data). Hence the dark
    pits are LOCALIZED to the brush chunk — exactly matching the
    screenshot's visible artifact.

#### Byte-level evidence

- `voxels_cpu.len()` after readback = 10,479,424 (matches GPU cursor
  `block_voxel_count[0]/2`).
- After `seed_block_hashing` (rough estimate): grows by ~6.4M u32s
  (200K mixed blocks × 32 u32s/block). Final ~16.9M u32s.
- GPU `voxels[]` buffer size: 268,435,456 u32s (1 GiB allocated, per
  `prepare.rs:374`); only 0–10.5M populated by construction; offsets
  10.5M+ are zero-init / implementation-defined.
- Brush's chunk (54, 21, 31): chunk_idx = 54 + 21·256 + 31·256·32 =
  259382. Has a block_ptr to a 64-block slot in `blocks[]`.
- The 2 modified blocks (2, 1, 2) and (2, 1, 3) in this chunk get NEW
  voxel_ptrs from `add_block` with `is_new = true` (because their
  hash differs post-edit). These ptrs are at the current
  `voxels_cpu.len()` (~16.9M after seed, +0 and +32 for the two
  blocks). GPU's `apply_voxel_change` writes the correct data at
  those offsets. ✓ These two blocks render correctly.
- The 62 unmodified Mixed blocks get `voxel_ptr` from
  `add_block` with `is_new = false` (hash matches existing entry).
  The returned ptr is the entry's stored ptr — which is in the
  10.5M–17M END_PTR range (the appended-during-seed range). GPU's
  `voxels[]` at those offsets is zero. ✗ These 62 blocks render as
  empty.

### Bug 2: `--vox-gpu-oracle` speckle (Δ>16 ceiling overshoot)

The brush-clears bug does not apply (no edits in this gate). The
speckle is a separate class.

#### H6 — Fixed-world tiling vs natural-bound oracle world (PLAUSIBLE, RANK 1)

The CPU oracle phase routes through `install_vox_sized_to_model`
(`grid.rs:224-262`) which builds a world sized to the natural Oasis
extent: `1488×544×1344` voxels. Areas beyond those bounds are NOT in
the world — primary and secondary rays that stray there hit void.

The GPU phase routes through `install_vox_in_fixed_world`
(`grid.rs:281-416`) which builds a `4096×512×4096` fixed world.
`generator_model.wgsl::get_voxel_data_in_model` (line 68-72) does
`vpim = voxel_pos % model_extent_v`, which tiles the Oasis model
horizontally (~3 X-tiles × 3 Z-tiles) and clips Y to the first 32
chunks (= 512 voxels) of the 34-chunk-tall (= 544-voxel) model.

**Two divergence sources**:

a. **Horizontal tiling**: at world (1488, *, *), CPU sees void; GPU
   sees a tile-1 copy of the Oasis architecture (= Oasis at
   (0, *, *)). Camera at world (744, 800, 672) framing world
   (744, 100, 672) — primary rays stay within the natural Oasis
   tile, but **secondary GI bounce rays** can stray beyond X=1488 or
   Z=1344. CPU's GI rays hit void → contribute sky / zero; GPU's GI
   rays hit tiled geometry → contribute diffuse bounce.

b. **Y-clipping**: CPU's world Y=544 voxels covers all of Oasis;
   GPU's world Y=512 voxels clips the top 2 chunks (32 voxels).
   If the Oasis Y=512..544 region has any architecture (palm-tree
   tips, wall tops), CPU renders them, GPU does not. Primary rays
   from camera Y=800 going down would hit those CPU-only top
   surfaces FIRST in the CPU phase, but pass through them in the
   GPU phase.

Both divergence sources produce per-pixel differences at the
specific positions affected, totaling ~6% of frame. The mean diff
stays small (most pixels are unaffected by tiling/clipping); only
the affected fraction shows Δ>16.

#### H7 — Block_ptr / voxel_ptr layout divergence (PLAUSIBLE, RANK 2)

The CPU oracle builds `voxels_cpu` via `aadf::construct::construct`
which assigns block_ptrs / voxel_ptrs in a specific deterministic
order (a content-addressable allocator that dedups by hash). The GPU
producer assigns ptrs via `chunk_calc::calc_block_from_raw_data`'s
`get_voxel_pointer` (lines 262-340) which uses `atomicAdd` cursors
+ `atomicCompareExchangeWeak` on `hash_map.voxel_pointer` — ptr
ordering depends on workgroup scheduling, which is non-deterministic
on GPU.

For a 1M-slot hash map with ~200K Oasis mixed blocks, hash collisions
are infrequent but possible. When two threads with different content
collide at the same hash bin, the SECOND thread's spin-wait reads
the first's voxel_ptr, compares content (differs → continue probing
linearly), eventually claims a fresh bin further along the probe
chain. The CPU oracle has no such race (single-threaded
deterministic).

Stage 9's byte-equality at 25 sample voxel positions verifies the
LEAF DATA is correct, but does NOT verify that `chunks[ci].block_ptr
== oracle_chunks[ci].block_ptr` or that `blocks[bi].voxel_ptr ==
oracle_blocks[bi].voxel_ptr`. The renderer's behavior is the same
either way (it dereferences whatever the ptrs say), so this should
not cause visible diffs — UNLESS combined with another bug (e.g., a
ptr falls into uninitialized GPU buffer territory, which would
produce zero-filled "empty" voxel reads, similar to bug 1).

#### H8 — Subtle AADF differences from `compute_voxel_bounds` (UNLIKELY, RANK 3)

Both CPU oracle's `compute_aadf_layer` and GPU's `compute_voxel_bounds`
implement the same iterative 3-pass additive bounds-growth algorithm
over a 4³ extent. With identical input topology, output should be
byte-identical. Stage 9's byte-equality (at the full-voxel positions
sampled) implies leaf voxel data agrees; if AADFs in adjacent empty
voxels also agreed, the renderer's DDA skip distances would match.

The byte-equality test did not specifically verify AADF bits in
empty voxels, so a minor mismatch (e.g., off-by-one in a corner case
of `compute_bounds_4`) could in principle exist.

#### H9 — Tonemapping / GI noise (UNLIKELY, RANK 4)

The renderer includes TAA + GI accumulation; small-frame stochastic
differences (random noise patterns) could account for ~6% of pixels
crossing the Δ>16 threshold. Both phases use the same camera pose
and the same TAA settings; their noise patterns may diverge due to
slight initial-frame timing differences (TAA's hash function reads
frame index + camera position). But the diff is concentrated at
high-frequency texture edges (per visual inspection of the PNGs), not
in uniform regions — pointing to geometric divergence (H6/H7) over
noise.

## Identified bug(s)

### Bug 1 — `seed_block_hashing` mispoints duplicate-allocated slots

**Symptom**: `--small-edit-repro` fails (411,196 dark pixels = 19.85%
of frame).

**File**: `crates/bevy_naadf/src/world/data.rs`

**Lines**: `seed_block_hashing` body (130-182), specifically the
interaction between:

- Line 171: `let hash = self.block_hashing.compute_hash(&pairs);`
- Line 172-173: `let (registered_ptr, is_new) =
  self.block_hashing.add_block(hash, &pairs, &mut self.voxels_cpu);`

And `crates/bevy_naadf/src/aadf/block_hash.rs:246-259`
(`alloc_voxel_slot`):

- Line 256-258: `let ptr = voxels_cpu.len() as u32;
  voxels_cpu.extend_from_slice(voxel_pairs); ptr`

The mechanism: `seed_block_hashing` is intended to register existing
voxel slots WITHOUT appending duplicates, but `add_block` for a
first-occurrence call unconditionally appends a copy. The hash entry
stores the appended END_PTR, not the original voxel_ptr. Subsequent
edits' hash lookups return END_PTR, the GPU writes END_PTR into
`blocks[]` for edited chunks, and the renderer reads zero data at
END_PTR (GPU never wrote there).

**Byte-level evidence**:

- `voxels_cpu.len()` post-readback = 10,479,424.
- After seed: grows by ~6.4M (200K Oasis mixed blocks × 32 u32s).
- Hash table after seed: all entries' `voxels_pointer` field is in
  the 10.5M–17M range (the END_PTR range), not the 0–10.5M range
  where `blocks_cpu` and GPU's `blocks[]` still point.
- Edit-time `add_block(hash, payload, &mut voxels_cpu)` for an
  unmodified block: hash matches existing entry → returns
  `(END_PTR, false)` → CPU sets `new_blocks[b] = END_PTR | (2 <<
  30)` → uploaded to GPU → GPU `apply_block_change` writes
  `blocks[chunk_block_ptr + b] = END_PTR | (2 << 30)`.
- Renderer descends to `voxels[END_PTR/2 + ...]` on GPU → reads 0 →
  decodes as empty voxel with AADF=0 → DDA marches through entire
  block as empty → block renders as a 4³ void → chunk renders as
  ~63 voids + ~1 brush block = a 16-voxel-wide dark cube with the
  brush voxels inside.

**Spatial verification (visual)**: the dark pits in
`small_edit_repro_after.png` are aligned to 4-voxel block boundaries
(visible as sharp 90° corners) and confined to roughly one
16-voxel-wide chunk worth of space around the brush. Matches the
"brush's chunk turned mostly void" prediction.

### Bug 2 — Fixed-world tiling vs natural-bound oracle world

**Symptom**: `--vox-gpu-oracle` fails per-pixel ceiling (4040 pixels
Δ>16 = 6.16%) while passing mean per-pixel floor (3.254 < 8.0).

**File**: `crates/bevy_naadf/src/voxel/grid.rs`

**Lines**:

- `install_vox_sized_to_model` (224-262) — CPU oracle phase install
  path; world sized to Oasis natural extent (1488×544×1344 voxels).
- `install_vox_in_fixed_world` (281-416) — GPU phase install path;
  world sized to `WORLD_SIZE_IN_CHUNKS` = 256×32×256 chunks = 4096×512×4096
  voxels with `voxelPos % modelSize` tiling in
  `generator_model.wgsl:68-72`.

The two install paths produce semantically DIFFERENT worlds at the
same camera position: tiled Oasis with truncated Y in GPU, untiled
Oasis with full Y in CPU. The two are not byte-equal world states.
The renderer is identical for both, so the diff at the few-percent
pixel-count level reflects the world-state divergence (secondary GI
rays hitting tiled-vs-void geometry, primary rays hitting Y-clipped
top chunks).

**Byte-level evidence**: not testable as-stated — this is a
design-level divergence, not a bug in a specific line of code. The
oracle is comparing two fundamentally different world configurations.

## Recommended fix (NOT to be implemented)

### Bug 1 — Stop seed_block_hashing from duplicating voxel slots

**Surface 1 — preferred**: introduce a dedicated `seed_block` method
on `BlockHashingHandler` that registers an EXISTING (already-in-
`voxels_cpu`) slot WITHOUT calling `alloc_voxel_slot`. Use it from
`seed_block_hashing` instead of `add_block`.

The method signature:

```rust
/// Register an existing voxel slot in the hash table without
/// allocating new storage. Used by `seed_block_hashing` after a
/// fresh world load to teach the handler about pre-existing block
/// slots so subsequent edit-time `add_block` / `delete_block` calls
/// see correct refcounts and dedup against the right pointers.
///
/// If the hash already has an entry for this content (different
/// chunk slots happen to share content), increments use_count and
/// returns the existing ptr; the caller patches `blocks_cpu` to
/// point at it. If no existing entry, registers `existing_ptr`
/// directly (no allocation, no append).
pub fn seed_block(
    &mut self,
    hash: u32,
    voxel_pairs: &[u32],
    existing_ptr: u32,
    voxels_cpu: &[u32],
) -> (u32, bool) {
    // ... mirror of `add_block` except the allocation branch uses
    //     existing_ptr directly:
    //     self.map[idx] = BlockHashEntry { voxels_pointer: existing_ptr, ... };
}
```

Then `seed_block_hashing` becomes:

```rust
let (registered_ptr, is_new) =
    self.block_hashing.seed_block(hash, &pairs, voxel_ptr, &self.voxels_cpu);
```

`voxels_cpu` is NOT mutated. `voxels_cpu.len()` stays at the
readback size. Hash entries' `voxels_pointer` matches the original
`blocks_cpu` ptrs. Edit-time `add_block` returns the ORIGINAL ptrs
for unchanged blocks — GPU has correct data there.

**Surface 2 — alternative**: in `seed_block_hashing`, when
`add_block` returns `is_new = true`, patch `blocks_cpu` to point at
the new (appended) ptr instead of leaving it at the original ptr.
This makes the CPU mirror's blocks_cpu consistent with the hash
table, but then the CPU mirror diverges from GPU's `blocks[]` (which
still has original ptrs). Subsequent edits' Stage C writes would
overwrite GPU's `blocks[]` to point at the appended ptrs — same
ultimate failure mode (GPU has no data at appended ptrs).

**Surface 3 — sledgehammer**: skip `seed_block_hashing` for the W5
install path entirely. The first edit on any chunk would then see
empty hash table → `add_block` allocates fresh slots, copies in
voxel data → CPU mirror and GPU `voxels[]` agree at the new slots
(since `set_voxels_batch` Stage A's data goes into BOTH CPU mirror
via `add_block` AND GPU via `changed_voxels` upload).

But this loses the dedup intent — the first edit on each chunk would
allocate fresh slots for ALL 64 of its blocks (no dedup against
already-loaded content), bloating both buffers and missing the
content-addressable storage benefit.

**Recommendation**: implement Surface 1 (the dedicated `seed_block`
method). The 1-place fix preserves the dedup intent and matches the
C# `WorldData.cs:131-132`'s post-`GetData()` editor state where the
hash handler reflects the actual on-GPU voxel slot layout.

### Bug 2 — Redesign the vox-gpu-oracle gate

Two options:

**Option A — Tighter test viewport**: shrink the camera framing so
secondary GI rays don't stray beyond the natural Oasis bounds and
primary rays don't hit the Y-clipped top chunks. Re-baseline the
gate's per-pixel ceiling against the tighter viewport.

**Option B — Replace with GPU-vs-GPU compare**: instead of comparing
CPU-uploaded `voxels_cpu` (natural-bound world) vs GPU-produced
`voxels[]` (fixed-world tiled), compare two GPU phases with the same
install path but different determinism inputs (e.g., toggle GPU
hash dedup nondeterminism by seeding `hash_coefficients` differently).
This removes the install-path divergence from the oracle.

**Recommendation**: Option B for long-term correctness, Option A for
quick remediation. The bug is not a runtime correctness defect (both
phases render plausibly Oasis-looking architecture) but a test-gate
calibration problem — the assertion compares apples (natural-bound
world) to oranges (fixed-world tiled world).

## Confidence level

**Bug 1 (seed_block_hashing): HIGH.**

- **Static code evidence**: `alloc_voxel_slot` unambiguously appends
  to `voxels_cpu` and returns the END_PTR. The patch at line
  177-179 of `seed_block_hashing` only fires for `is_new = false`.
  First-occurrence blocks WILL have hash entries pointing at
  END_PTR; subsequent edits WILL get END_PTR from `add_block`.
- **Mechanism aligns with symptom**: GPU has no data at END_PTR →
  renderer reads zero → block reads as empty → chunk renders as
  void. Spatial confinement to brush's chunk matches `apply_block_change`
  only writing to one chunk.
- **Visual evidence**: dark pits in `small_edit_repro_after.png`
  are 4-voxel-block aligned, confined to ~one chunk's worth of
  space, with sharp 90° corners — consistent with the
  empty-block-due-to-zero-voxel-data prediction.
- **Comparative evidence**: `--oasis-edit-visual` passes because its
  erase brush turns most touched chunks into uniform-empty (no hash
  path); `--small-edit-repro`'s PLACE brush goes through the hash
  path for unmodified blocks in the brush's chunk, triggering the
  END_PTR misdirection.
- **Why the bug was not caught at construction time**: at
  construction time, NO edits have run; the brush hasn't touched
  any chunk; the GPU's `blocks[]` still points at original ptrs;
  rendering is correct. The bug only manifests AFTER the first edit.
- **Why CPU verification at edit time passes**: `world_data.get_voxel_type`
  reads from CPU `voxels_cpu` (which has correct data via the
  appended copies). It does not check GPU's view. ✓ — the test
  reports `8/8 affected voxels correctly encoded` but the GPU view
  is broken.

**Bug 2 (vox-gpu-oracle speckle): MEDIUM.**

- H6 (tiling vs natural-bound) is structurally inevitable given the
  two install paths' world shapes, but the EXACT contribution to the
  6.16% pixel diff cannot be precisely attributed without
  per-pixel tracing.
- H7 (ptr layout divergence) is plausible if combined with another
  bug like H5 (the GPU's hash race could allocate a ptr that's
  technically valid but points at a slot with subtly different
  AADFs from CPU's deterministic layout).
- H8 (AADF micro-differences) is unlikely but not ruled out without
  per-empty-voxel byte-equality testing.
- H9 (TAA/GI noise) is unlikely given the diff is at high-frequency
  texture edges, not uniform regions.

A targeted diagnostic that tightens the viewport (eliminate H6) and
re-runs the gate would discriminate H6 from H7/H8/H9. If the diff
drops dramatically with tighter viewport, H6 is dominant. If it
stays at ~6%, H7/H8/H9 are dominant.

## Cross-references

- Stage 11 fix (`16-diagnostic-renderer-wiring.md`): patched
  `crates/bevy_naadf/src/voxel/grid.rs:368-379` to strip AADF bits
  from empty voxels in `ModelData.data_voxel`. Dropped mean Δ from
  127.84 to 3.241. Did NOT touch `seed_block_hashing` (bug 1) or
  the install-path world-shape divergence (bug 2).
- Stage 9 readback (`15-diagnostic-production-scale-readback.md`):
  verified `voxels[]` byte-equality at 25 full voxel positions post-
  producer + post-bounds. Does NOT verify block_ptr or voxel_ptr
  layout equivalence, nor empty-voxel AADF byte-equality. Bug 1
  is outside its sampling scope (it samples FULL voxels for type
  correctness; the brush bug is about EMPTY voxels in END_PTR
  territory).
- Stage 2 consolidation (`03-impl.md:3219+`): both flagged gates
  (`--vox-gpu-oracle` per-pixel ceiling, `--small-edit-repro`
  regression) were tracked as Stage-2 follow-ups, with the analysis
  noting the regressions were pre-existing W5 path bugs the legacy
  CPU path sidestepped. The hypothesis at the time ("same residual
  ~6% speckle class as `--vox-gpu-oracle` ... a localised W3 AADF
  region that re-converges with the residual bug present") is
  REFUTED by this diagnostic — the brush bug is `seed_block_hashing`
  pointer mispointing, not an AADF/W3 issue.
- `WorldData.cs:131-132` (NAADF C# reference): the C# code shares
  one `BlockHashingHandler` instance between the GPU producer and
  the editor — no separate "seed from GPU readback" step exists in
  C# because the same handler reference flows through both paths.
  The Rust port had to add `seed_block_hashing` to recreate this
  state after the GPU→CPU readback; the bug is in how that
  recreation interacts with `add_block`'s "always allocate" branch.
- Block-hashing methods:
  - `add_block` at `aadf/block_hash.rs:154-193`
  - `alloc_voxel_slot` at `aadf/block_hash.rs:246-259`
  - `seed_block_hashing` at `world/data.rs:130-182`
- Edit-pipeline call site for `add_block` at edit time:
  `world/data.rs:911-921` (Stage A of `set_voxels_batch`).
- GPU shaders that write into `blocks[]` / `voxels[]` during edits:
  - `apply_block_change` at `assets/shaders/world_change.wgsl:468-507`
  - `apply_voxel_change` at `assets/shaders/world_change.wgsl:520-572`
- Brush dispatch path:
  - `cube_brush` at `editor/tools.rs:168-224`
  - `set_voxels_batch` at `world/data.rs:698-1057`
- Renderer's chunk → block → voxel descent:
  `assets/shaders/ray_tracing.wgsl:283-401`
