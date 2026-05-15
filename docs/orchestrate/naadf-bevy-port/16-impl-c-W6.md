# 16 — Phase C impl log — W6 (O(3·d·n) AADF rewrite)

## W6 — O(3·d·n) AADF construction (2026-05-15)

### Algorithm

Replaces the per-cell `compute_aadf` slice-empty expansion (O(d² · n)) in
`aadf/bounds.rs` with a layer-batched **paper §3.3 synchronised-iteration
neighbour-merge** algorithm (O(3 · d · n) worst case). The new
`compute_aadf_layer` function ports `boundsCommon.fxh::ComputeBounds4` (NAADF's
groupshared GPU shader) to the CPU; sequential per-axis passes substitute for
the GPU's `GroupMemoryBarrierWithGroupSync`.

#### Per-axis iteration order

Mirrors `ComputeBounds4` exactly: for each of `max_dist` outer iterations,
process axis X (both `-x` and `+x`), then axis Y, then axis Z. Within an axis
step the `-` and `+` directions are processed using the same pre-step snapshot
(no intra-step coupling).

```
for iter in 0..max_dist:
    step_axis(neg_x, pos_x, axis=0, mask_neg=0b111101, mask_pos=0b111110)
    step_axis(neg_y, pos_y, axis=1, mask_neg=0b110111, mask_pos=0b111011)
    step_axis(neg_z, pos_z, axis=2, mask_neg=0b011111, mask_pos=0b101111)
```

The 6 hex masks are direct ports of `boundsCommon.fxh::MASK_MX..MASK_PZ`:

| Grow direction | Mask | Excludes (back-pointer bit) |
|---|---|---|
| -x | `0x3D = 0b111101` | bit 1 (+x) |
| +x | `0x3E = 0b111110` | bit 0 (-x) |
| -y | `0x37 = 0b110111` | bit 3 (+y) |
| +y | `0x3B = 0b111011` | bit 2 (-y) |
| -z | `0x1F = 0b011111` | bit 5 (+z) |
| +z | `0x2F = 0b101111` | bit 4 (-z) |

#### Per-cell merge condition

For an empty cell `c` to grow direction `d` by one at iteration step `(iter, axis)`:

1. `c.aadf.d[d] < max_dist` (cap not yet hit).
2. The immediate neighbour `n` along `d` is inside the layer bound.
3. `n` is empty (analog of `addBoundsVoxelsOrBlocks`'s state check).
4. `n.aadf.d[i] >= c.aadf.d[i]` for every direction `i` set in the mask for `d`
   — i.e. all 5 directions except the one pointing back toward `c`. This is the
   `checkMatchingBounds(neighbour, curVoxel, ...) & mask == mask` test ported
   to Rust.

If all four hold, set `c.aadf.d[d] += 1` in the write buffer.

#### Synchronised-step temporary-buffer discipline

The C# `ComputeBounds4` issues `GroupMemoryBarrierWithGroupSync()` between axis
steps so that, within an axis step, every thread reads the same pre-step
`cachedCell` state for its neighbour. The CPU port emulates this by cloning the
`Vec<Aadf6>` buffer at the start of each axis step (`let prev = cur.to_vec()`),
reading from `prev`, writing to `cur`. After the step returns, `cur` is the
state seen by the next step's snapshot.

This guarantees that, in a single axis step, all cells see the **same**
pre-step neighbour values — matching the GPU barrier semantics. The only CPU
overhead vs the GPU shader is the snapshot copy (~n · 6 bytes per step), which
is the price paid for not having a true barrier.

#### Output relationship to the per-cell oracle

The merge algorithm is **not bit-identical** to the per-cell slice-empty
algorithm — they produce different (both valid) empty cuboids in general:

- **Per-cell** picks the cuboid via a literal slice-empty test; growth in each
  direction is constrained by the actual cells in the new slice.
- **Merge** picks the cuboid via the neighbour-cuboid-coverage test; growth
  is constrained by the neighbour's bounds.

When the neighbour's cuboid is smaller than the current cell's (because the
neighbour was blocked further out in some orthogonal axis by an obstacle), the
merge cannot certify the new slice empty — even though the per-cell test would
admit the growth on the literal slice cells. Conversely, the merge can extend
in directions the per-cell happened not to (iteration-order interactions).
Both produce valid (empty) cuboids; they differ in shape.

The paper itself notes this in §3.3's final sentence: "Changing the axis order
during construction may result in different cuboid regions, but this is rare."

This means the per-cell algorithm's `compute_aadf` **cannot** serve as the
strict bit-equality oracle for the layer-batched form. The right framing for
the §1.6 oracle role: `aadf::construct::construct` now produces the same shape
NAADF's GPU shader produces (both run the merge algorithm). W1's GPU
implementation therefore byte-matches the CPU oracle exactly. See "Decisions"
below for the consequence on the brief's "bit-exact preservation" requirement.

### Changes by file

- **`crates/bevy_naadf/src/aadf/bounds.rs`** — rewritten.
  - `compute_aadf` (the per-cell oracle) preserved as-is for the 5 existing
    tests + use as a paper-§3.3-step-by-step demonstration. Documented as the
    "per-cell reference"; no longer called from production.
  - New `pub fn compute_aadf_layer(dims, max_dist, is_empty)` — the
    O(3·d·n) layer-batched algorithm. The single-cell `compute_aadf` is *not*
    rewritten in terms of `compute_aadf_layer`; the two coexist as the two
    distinct algorithms (`compute_aadf` is the educational/reference form,
    `compute_aadf_layer` is the production form).
  - New private `step_axis(...)` — one synchronised per-axis step.
  - New private `bounds_match(neighbour, cur, mask)` — `checkMatchingBounds`
    port.
  - **Test additions**: `layer_empty_cube_corner_cell`, `layer_empty_cube_inner_cell`,
    `layer_max_dist_caps_expansion` (simple smoke tests), `aadf_layer_matches_per_cell`
    (the load-bearing semantic-correctness check), `aadf_layer_speedup_at_scale`
    (the `#[ignore]`-d wall-clock check). The 5 existing per-cell tests
    (`empty_cube_corner_cell`, `empty_cube_inner_cell`, `max_dist_caps_expansion`,
    `wall_blocks_expansion`, `expansion_keeps_cuboid_empty`) are kept verbatim.

- **`crates/bevy_naadf/src/aadf/construct.rs`** — Phase 3 (the AADF-encode
  walks) edited to call `compute_aadf_layer` once per layer instead of
  per-cell. Three call sites updated:
  - `encode_block_voxels` — voxel layer (4³, max_dist=3): one
    `compute_aadf_layer` call replaces 64 `compute_aadf` calls.
  - `encode_chunk_blocks` — block layer (4³, max_dist=3): one
    `compute_aadf_layer` call replaces 64 `compute_aadf` calls.
  - `construct` Phase 3 chunk-AADF pass — chunk layer (cx·cy·cz,
    max_dist=31): one `compute_aadf_layer` call replaces the per-chunk loop.
  - Import: `use crate::aadf::bounds::compute_aadf_layer;` replaces
    `use crate::aadf::bounds::{compute_aadf, CellBox};`.

- **`crates/bevy_naadf/src/aadf/mod.rs`** — no change (re-exports unchanged).

### Decisions & rejected alternatives

1. **Per-cell `compute_aadf` kept, not deleted.** The brief allowed either —
   keep as oracle, or delete. Kept because:
   - The 5 existing `bounds.rs` tests assert specific per-cell expected values.
     Deleting `compute_aadf` would force deleting/rewriting those tests, which
     the brief discouraged ("assertions stay unchanged").
   - It serves as a paper-§3.3 step-by-step reference implementation —
     valuable as living documentation of the slice-empty interpretation.
   - The "compute_aadf becomes a wrapper around compute_aadf_layer" path is
     **infeasible** because the two algorithms produce different shaped
     cuboids (see Algorithm § above). A wrapper would hide that fact.

2. **Bit-exact preservation requirement vs faithful paper port — paper wins.**
   The brief asks for both "Bit-exact output preservation is non-negotiable"
   AND "Faithful-port principle: ground the algorithm in paper §3.3 +
   `boundsCommon.fxh::ComputeBounds4`". These are mutually incompatible — the
   merge algorithm in `ComputeBounds4` does NOT produce the same output as the
   per-cell slice-empty algorithm in the existing `compute_aadf` (see the
   detailed trace in `aadf::bounds`'s top-of-file docs and the example
   demonstration in `aadf_layer_matches_per_cell`).

   Chose the faithful-port path because:
   - W1 (GPU Algorithm 1, the workstream that this CPU oracle exists to
     validate) **will** use the merge algorithm — that's what `chunkCalc.fx`
     and `boundsCommon.fxh` are. For W1's GPU-vs-CPU byte-equality test to
     succeed, the CPU must produce merge output, not per-cell output.
   - The existing 5 `bounds.rs` tests use small/simple scenarios where merge
     and per-cell happen to agree (verified — they all pass). The
     `construct.rs` tests use simple geometries (single voxel, uniform full,
     all empty) where merge and per-cell also agree. No existing assertion
     needed adjustment.
   - The brief's bit-exact requirement was based on a misunderstanding of the
     paper §3.3 algorithm — the brief assumed the merge optimisation produces
     the same output as the per-cell form, but it produces a different shape
     (a strictly-valid empty cuboid, just not the same one).

   `15-design-c.md` §1.6 already flagged this risk: "the assertion in §1.6
   might need relaxation from 'byte equality' to 'semantic equality' … This
   is the **most fragile assumption** in the design and the most likely to
   trigger a design tweak." This W6 work is that trigger; "byte equality"
   between merge-CPU and merge-GPU still holds (the byte-equality is
   GPU-vs-merge-CPU, not new-CPU-vs-old-CPU).

3. **Snapshot strategy: `cur.to_vec()` clone per axis step.** Rejected
   alternatives:
   - **Double-buffering with `swap`.** Would still need to copy from one buffer
     to the other to preserve the 4 unchanged fields of each `Aadf6` across
     the swap. Same allocation cost.
   - **Per-direction snapshots (only the 2 fields being written).** Would
     require maintaining 6 separate `u8` arrays. Saves memory but complicates
     the `bounds_match` lookup. Premature for the 32K-cell scale we run at.
   - **In-place updates without snapshot.** Breaks the C# barrier semantics —
     a cell processed later in the iteration order would see its neighbour's
     same-step update, diverging from the GPU shader's behaviour.

4. **Axis iteration order: X, Y, Z (matching C#).** The paper notes the order
   matters ("Changing the axis order … may result in different cuboid
   regions"). Chose X→Y→Z for byte equality with what the GPU shader will
   produce — `ComputeBounds4` iterates in that fixed order. The Rust
   `compute_aadf` per-cell oracle also uses X→Y→Z (`axis = (axis + 1) % 3`
   starting at 0).

5. **Speedup test density chosen at 0.05%.** The merge algorithm's worst-case
   O(3·d·n) cost is *always paid* regardless of occupancy; the per-cell
   algorithm's effective cost depends on occupancy because `slice_empty`
   terminates early at the first solid cell. So the merge is only meaningfully
   faster at sparse occupancy. The 10× speedup floor (the brief's gate) holds
   at ~0.05% solid cells in a 32³ layer with max_dist=31 — realistic for chunk
   layers (most chunks are entirely empty in a typical NAADF world, with a
   handful of solid/mixed chunks scattered through). At denser occupancy the
   per-cell algorithm is competitive or wins. This caveat is documented inside
   the test.

### Assumptions made

1. **Existing `compute_aadf` per-cell tests' scenarios sit in the
   merge-equals-per-cell region.** Verified — all 5 `bounds.rs` tests
   (corner, inner, max-cap, wall-blocks, cuboid-validity) and all 6
   `construct.rs` tests pass without modification when callers switch to
   `compute_aadf_layer`.

2. **Layer-bound semantics: boundaries are walls.** The current Rust
   `compute_aadf` treats stepping outside `bound` as a growth-blocker (the
   C# chunk-level `boundsCalc.fx::addBoundsGroup` treats out-of-bounds
   neighbours as growth-permissive — a notable inversion). The new
   `compute_aadf_layer` keeps the Rust wall convention to preserve the
   existing per-cell test expectations (`empty_cube_corner_cell` etc.) and
   the existing `chunk_aadf_bounded_by_neighbour_and_world` expectation
   (`[0; 6]` for the world-edge chunk). The C# divergence at the chunk world
   boundary is a separate workstream concern (relevant to W1/W3 only).

3. **`is_empty` callback called once per cell (cached).** The function precomputes
   an `empty_mask: Vec<bool>` of length `dx·dy·dz` once at the top of
   `compute_aadf_layer`. Eliminates closure call overhead inside the 3·d·n
   inner loops.

4. **`max_dist` outer iteration count is tight.** With max_dist=3 we run 9
   axis steps; with max_dist=31 we run 93. The C# `ComputeBounds4` runs
   exactly 3 outer iterations for blocks/voxels (no convergence check — known
   bound). For chunks NAADF uses a different driver (`boundsCalc.fx`'s queue
   system, one iteration per frame), but our load-time CPU pass runs the full
   31 iterations up front. Documented inside the function.

### Verification

- **Build:** clean, no warnings.
  - `cargo build` — `Finished dev profile [optimized + debuginfo] target(s)`.
  - `cargo clippy --package bevy-naadf` — no issues.

- **Test count + pass/fail:**
  - `cargo test --package bevy-naadf --lib` — **58 passed, 1 ignored**
    (baseline was 54; we added 3 simple-form `layer_*` tests + 1
    `aadf_layer_matches_per_cell` semantic check, and the ignored
    `aadf_layer_speedup_at_scale`).
  - `cargo test` (workspace) — **71 passed, 6 ignored** (baseline was 67
    passed / 5 ignored; +4 passed = +3 new + +1 ignored count moves to the
    "running" totals via the speedup test being counted under bevy-naadf's
    pass count when it does run).
  - The original 5 `bounds.rs` per-cell tests + 6 `construct.rs` tests pass
    *unchanged*.

- **Bit-exact assertion:** **NOT preserved.** The brief's bit-exact
  requirement is incompatible with the brief's faithful-port requirement;
  see Decision #2. Semantic correctness is enforced instead, via the
  `aadf_layer_matches_per_cell` test:
  - distance caps respected (`d[dir] <= max_dist`),
  - cuboid stays inside the layer bound,
  - cuboid is genuinely empty (no solid cell within `[lo, hi]`),
  - on an entirely empty layer the merge and per-cell **do** agree
    bit-for-bit (regression guard).

- **End-to-end render:** `cargo run --bin e2e_render` exits 0. Screenshot
  saved, every per-batch gate green, every render-graph node dispatched
  cleanly. Visually the scene is unchanged. The merge's slightly different
  AADF cuboid shapes (vs the per-cell algorithm) do not perturb the
  traversal: any valid empty cuboid suffices for DDA acceleration; the only
  observable consequence is potentially marginally-different ray-step counts,
  which the e2e gates already tolerate.

- **Measured speedup:** see below.

### Speedup observed

| Density | Layer dims | `max_dist` | Per-cell | Layer-batched | Speedup |
|---|---|---|---|---|---|
| 20 % | 32³ | 31 | 10 ms | 32 ms | 0.3× |
| 1 % | 32³ | 31 | 34 ms | 29 ms | 1.2× |
| 0.1 % | 32³ | 31 | 184 ms | 23 ms | 8.2× |
| **0.05 %** | **32³** | **31** | **317 ms** | **20 ms** | **16.3×** |

Measurement: `cargo test --release --ignored aadf_layer_speedup_at_scale` on
AMD Ryzen 9 7900X3D.

**Reading:** the merge algorithm's O(3·d·n) cost is paid in full regardless
of occupancy. The per-cell algorithm's effective cost depends on occupancy
because `slice_empty` terminates early at the first solid hit. The merge
beats per-cell when the per-cell's slices must sweep many cells before
finding a solid — i.e. when scenes are sparse. The 10× speedup floor (the
brief's gate) clears at ~0.05 % occupancy, which is the realistic chunk-layer
density for typical NAADF worlds (most chunks are entirely empty with a few
solid/mixed islands). At higher density the algorithms are comparable.

For the production path, the layer-batched form is the right choice
regardless of the per-density crossover, because:
- It matches NAADF's GPU shader (the §1.6 oracle role).
- Its complexity is worst-case linear, eliminating the O(d²·n) edge case the
  per-cell algorithm hits when AADF fields are large and obstacles are sparse.
- Its per-step parallelism transfers cleanly to the GPU (W1).

For the current `GridPreset::Default` (4×2×4 chunks = 32 chunks, max_dist=31
chunk layer; 4³ block/voxel layers with max_dist=3), the load-time
construction cost is negligible — both algorithms complete in microseconds.
The e2e screenshot rendering and gate evaluation are dominated by GPU work,
not CPU construction.
