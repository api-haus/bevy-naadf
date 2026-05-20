## refactor-explorer findings (2026-05-20) — D1 aadf-data-structures

**Domain LOC** (verified `wc -l`): 6 472 across 12 files.

| file | LOC |
|---|---|
| `world/data.rs` | 1 731 |
| `aadf/bounds.rs` | 835 |
| `aadf/edit.rs` | 828 |
| `aadf/block_hash.rs` | 612 |
| `aadf/construct.rs` | 570 |
| `aadf/generator.rs` | 507 |
| `aadf/entity.rs` | 451 |
| `world/buffer.rs` | 399 |
| `aadf/cell.rs` | 346 |
| `voxel/mod.rs` | 144 |
| `world/mod.rs` | 31 |
| `aadf/mod.rs` | 18 |

---

## Findings (priority order)

### Finding 1 — `WorldData` API conflates four set-voxel entry points across two completely different semantic regimes (HIGH)

**Locations:**
- `world/data.rs:235-398` — `pub fn set_voxel` (DIAGNOSTIC-ONLY, marked `#[doc(hidden)]`)
- `world/data.rs:721-1080` — `pub fn set_voxels_batch` (production runtime fast path)
- `world/data.rs:1099-1161` — `pub fn set_chunks_uniform_batch` (production brush inside-chunk fast path)
- `world/data.rs:1181-1342` — `pub fn set_voxels_batch_oracle` (DIAGNOSTIC-ONLY, `#[doc(hidden)]`)

**What's wrong:** `WorldData` is a render-world handoff resource. Its `impl` block (1 731 LOC, 7 `pub fn`s) mixes **two production runtime paths** (`set_voxels_batch`, `set_chunks_uniform_batch`) with **two diagnostic-only paths** (`set_voxel`, `set_voxels_batch_oracle`) that the docblock at `world/data.rs:19-34` itself flags ("Production code paths NEVER call these methods"). The diagnostic-only paths run the whole-world AADF rehash via `crate::aadf::edit::recompute_chunk_layer_aadfs` — O(N_chunks × 31 × 3) per call — and exist solely to drive the `--edit-mode` e2e gate and unit tests. Their public surface bloats every IDE auto-complete on `WorldData` and tempts new callers down the slow path.

`set_voxel` body alone is 164 lines (`:235-398`) and contains 4 near-identical inline chunk-pos packing/unpacking blocks (`:330-339`, `:363-376`, `:383-388`) plus a full duplicate of the recompute logic that `set_voxels_batch_oracle` (`:1181-1342`) repeats verbatim.

**Why it matters:** This is the canonical IoC violation called out in `00-reuse-audit.md §3.2 DUP-1` and `01-context.md §Q2`. The user's verbatim directive is "cpu oracle stays" — but the oracle's CALL SITES on `WorldData` (the diagnostic set-voxel methods) bloat the resource's API surface. They should remain reachable from the `--edit-mode` gate and tests, but not appear on every render-world consumer's view of `WorldData`.

The four entry points form a 2×2 matrix that the file structure should mirror but doesn't:

| | production runtime | diagnostic oracle |
|---|---|---|
| **single voxel** | (none — brushes always batch) | `set_voxel` |
| **batch** | `set_voxels_batch`, `set_chunks_uniform_batch` | `set_voxels_batch_oracle` |

**Suggested direction (NOT a design):** Move the diagnostic-only methods into a `pub(crate) mod oracle` (or `mod diagnostic`) sibling inside `world/`, keep them callable from `--edit-mode`/`--runtime-edit-mode` gates and unit tests via the crate-internal seam, drop them from `WorldData`'s `pub` surface. Collapse the two duplicate `recompute_chunk_layer_aadfs` + synthetic-`changed_chunks` emit blocks into one helper. Leave `set_voxels_batch` and `set_chunks_uniform_batch` as the only production-facing methods.

**Out-of-scope ripple:** `render/construction/mod.rs:9207, :9350, :9416` call `set_voxel` / `set_voxels_batch` from D5's e2e gate bodies — the seam needs to be visible to those callers. `bin/e2e_render.rs` dispatch entries (D6) consume the gates. Both are crate-internal so a `pub(crate)` move is non-breaking.

---

### Finding 2 — State-bit encoding is implemented in two parallel regimes (HIGH)

**Locations:**
- `voxel/mod.rs:27-37` — `CELL_HAS_CHILDREN = 1 << 31`, `CELL_UNIFORM_FULL = 1 << 30`, `CELL_PAYLOAD_MASK = 0x3FFF_FFFF` (single-bit-flag regime)
- `aadf/cell.rs:122-139, :147-164, :172-188` — encode/decode uses the flag-bit regime (`raw & CELL_HAS_CHILDREN`)
- `world/data.rs:134, :145, :517-518, :542-543, :637-665, :887-890, :918, :944, :1031, :1129, :1143-1144` — uses the 2-bit-state-nibble regime (`raw >> 30 == 2`, `(2u32 << 30) | ptr`, `(1u32 << 30) | ty`)
- `aadf/edit.rs:101, :116, :291, :306, :328, :381-415, :540, :555, :706, :777` — same 2-bit-state-nibble regime
- `aadf/generator.rs:105, :164, :174, :193, :197` — same
- WGSL: `assets/shaders/chunk_calc.wgsl:246-248`, `world_change.wgsl:161-163`, `bounds_calc.wgsl:180-182` — defines `BLOCK_STATE_CHILD = 2u, BLOCK_STATE_UNIFORM_EMPTY = 0u, BLOCK_STATE_UNIFORM_FULL = 1u` constants

**What's wrong:** Two encoding regimes coexist for the same bit layout:

- **Regime A** (`voxel/mod.rs`): bit 31 = has-children, bit 30 = uniform-full. Used only inside `aadf/cell.rs` encode/decode methods.
- **Regime B** (everywhere else, including the WGSL shaders): a 2-bit state nibble at bits 30-31 — `0` empty, `1` uniform-full, `2` mixed (a.k.a. `BLOCK_STATE_CHILD`). State `3` is unused.

Both happen to produce the same bit pattern because `state == 2` (`0b10`) sets bit 31 and clears bit 30, but the two regimes are semantically different — regime A is "two independent flags", regime B is "a 2-bit discriminator". Reading the code one regime at a time you cannot tell which mental model applies. Worse, the project's WGSL shaders define exactly the regime-B constants in three files — they're the SSoT — but Rust uses raw literals `0`/`1`/`2` and bare `>> 30` everywhere. `aadf/edit.rs:306, :328` has a hand-written comment `// BLOCK_STATE_CHILD` next to `(2u32 << 30)` because the author wanted the constant; it doesn't exist on the Rust side.

I count **~30 inline `>> 30` / `& 0x3FFF_FFFF` / `(N << 30)` sites in D1 alone** that would benefit from a named constant or a `ChunkRaw::state()` / `ChunkRaw::payload()` helper.

**Why it matters:** Foundation rot. This is paper §3.1's load-bearing encoding (`02-research.md §1.1.2` flags it as "easy to get wrong"), and the AADF orchestration history (`wasm-chunk-aadf-nondeterminism`) literally cites encoding-regime confusion as a root cause class. Pinning a single named-constant regime in Rust matching the WGSL one is a Rust-idiom alignment + a paper-faithful clarity win.

Misalignment with the architectural anchor (`00-reuse-audit.md §3.1 SSoT-3` calls out `CELL_DIM`/`CELL_CHILDREN` as the WGSL-vs-Rust SSoT case — the state-bit constants are a strictly worse case: they exist named on the WGSL side, anonymous on the Rust side, in three regimes).

**Suggested direction (NOT a design):** Add a `voxel::CellState` enum or `pub mod state` with the three named state values `EMPTY = 0`, `UNIFORM_FULL = 1`, `MIXED = 2` (or `CHILD = 2` to match WGSL) plus a `CELL_STATE_SHIFT: u32 = 30`. Optionally a tiny `RawCell(u32)` newtype with `.state()`, `.payload()`, `.with_state(...)`, `.with_payload(...)` helpers — same idiom as the existing `pack_chunk_pos` / `unpack_chunk_pos` (`aadf/edit.rs:203-214`). Keep `CELL_HAS_CHILDREN`/`CELL_UNIFORM_FULL` as deprecated aliases if needed for `aadf/cell.rs`'s encode/decode methods — or migrate those to the state-nibble regime so the file aligns with the rest of the codebase.

**Out-of-scope ripple:** WGSL is D5's territory. The proposal is "match WGSL's existing constant set in Rust"; WGSL doesn't change.

---

### Finding 3 — `recompute_chunk_layer_aadfs` body is duplicated verbatim in two `WorldData` methods (HIGH)

**Locations:**
- `world/data.rs:340-397` — `set_voxel` post-amble: `recompute_chunk_layer_aadfs` call + AADF-changed-chunk merge + `pending_edits` push
- `world/data.rs:1294-1342` — `set_voxels_batch_oracle` post-amble: same recompute call + same merge logic + same `pending_edits` push

**What's wrong:** ~50 lines of identical logic — packed-pos decode loop, `already_in_batch` set construction, `aadf_changed` iteration, `pack_chunk_pos(...)` reconstruction, `pending_edits.edited_groups.push(...)`. The only meaningful difference between the two is that `set_voxel` builds its `edited_groups` push once from the single chunk position, while `set_voxels_batch_oracle` loops over `edited_chunks`. Everything else (the recompute call body) is byte-for-byte identical.

**Why it matters:** Maintenance hazard — any fix to the AADF-changed-chunk merge logic has to land in two places. The diagnostic methods being "rarely run" means the duplication has historically diverged silently. This is also the same logic that lives GPU-side in `world_change.wgsl::apply_group_change` (referenced at `world/data.rs:1056`) — three copies total (two CPU, one GPU). Rust ↔ Rust DRY is the lower bar; even that's failing.

**Suggested direction (NOT a design):** Extract a `pub(crate) fn merge_recomputed_aadfs_into_batch(...)` helper in `aadf/edit.rs` (where `recompute_chunk_layer_aadfs` already lives) that takes `&mut WorldData` + `batch: &mut EditBatch` + `edited_chunk_positions: &[[u32;3]]` and does the recompute + AADF-changed-merge + edited-groups-push as one block. Both diagnostic methods call it.

**Out-of-scope ripple:** None. Both call sites and the helper destination are in D1.

---

### Finding 4 — Three CPU AADF computation paths with different APIs but compatible cores (MEDIUM)

**Locations:**
- `aadf/bounds.rs:247-335` — `compute_aadf_layer` (closure-driven `is_empty`, layer-of-`Aadf6` output, used by `construct.rs`, `edit.rs`, generic over layer dims)
- `aadf/entity.rs:144-222` — `EntityData::from_types` body (inlines the same 31-iteration synchronised-iteration neighbour-merge over a packed-`u32` voxel buffer — `0x80000000` flag, 5-bit-per-axis AADFs at shifts `0,5,10,15,20,25`). 75 lines of XYZ-axis-loop body that re-implements `bounds::step_axis` + `bounds::bounds_match` against a different storage layout.
- `aadf/edit.rs:509-568` — `recompute_chunk_layer_aadfs` (calls `compute_aadf_layer` but adds whole-world snapshot + per-empty-cell `ChunkCell::Empty(...).encode()` write — already on top of `compute_aadf_layer`, OK)

**What's wrong:** `aadf/entity.rs` declaratively says (lines 9-15) "Faithful port of the C# inline loop — not delegated to `crate::aadf::bounds::compute_aadf`, because the C# `EntityData` AADF kernel runs on a packed-32-bit voxel buffer (the AADFs in the low 30 bits, the `0x80000000` full-cell flag in the top bit) that does not match the `aadf::bounds` 5-bit-AADF chunk format. Both kernels implement paper §3.3 synchronised-iteration neighbour-merge, just over different bit layouts."

Reading the bodies: this is *correct on the surface* — `bounds::compute_aadf_layer` outputs `Vec<Aadf6>`, while `EntityData::from_types` writes back into the same `Vec<u32>` it reads, with AADFs in-place at shift offsets `0,5,10,15,20,25`. The mask constants `MASK_MX..MASK_PZ` at `entity.rs:159-164` are byte-equal to the masks `compute_aadf_layer` passes to `step_axis` at `bounds.rs:298-328` (`0x3D, 0x3E, 0x37, 0x3B, 0x1F, 0x2F`). The merge predicate `entity.rs::check_matching_bound_cell` (lines 249-270) is the inline-expanded equivalent of `bounds.rs::bounds_match` (lines 418-428), specialised for the 5-bit-per-axis packing.

So the two share the *algorithm* but diverge in *storage layout*. The third path (`recompute_chunk_layer_aadfs`) properly reuses `compute_aadf_layer`. Whether entity-side can also reuse is a real question — the entity buffer's in-place pack is load-bearing for the GPU mirror.

**Why it matters:** Paper §3.3 has one algorithm. The Rust port has 1.5 implementations of it. Faithful-port rule (`bevy-naadf-faithful-port-rule`) says match C# — and C# has the inline loop in `EntityData.cs:64-105`. So this divergence is C#-faithful. But it costs **170 LOC of inline algorithm body** in `entity.rs:170-218` that mirrors `bounds.rs:291-329` semantically.

**Suggested direction (NOT a design):** `compute_aadf_layer` already takes an `is_empty` closure. Architect to evaluate whether a sibling `compute_aadf_layer_packed_in_place(voxels: &mut [u32], dims: [usize; 3], max_dist: u8, bit_shifts: [u32; 6])` could absorb the entity-side body without breaking the C#-faithful per-iteration synchronisation pattern. Alternative: leave entity-side alone and only extract the shared 6-mask + 6-shift constant table (which is ~10 LOC) — the lower-impact option that doesn't risk breaking the bit-for-bit GPU match. Either way: document the relationship more tersely at `aadf/entity.rs:9-15` and `aadf/bounds.rs:32-49` (today both have ~40 lines of prose explaining why the algorithms are or aren't related).

**Out-of-scope ripple:** None — entity-side is D1.

---

### Finding 5 — Anonymous `(IVec3, VoxelTypeId)` and `([u32; 3], Option<VoxelTypeId>)` tuples across the bulk-edit API (MEDIUM)

**Locations:**
- `world/data.rs:721` — `pub fn set_voxels_batch(&mut self, edits: &[(IVec3, VoxelTypeId)])`
- `world/data.rs:1099-1101` — `pub fn set_chunks_uniform_batch(&mut self, chunks: &[([u32; 3], Option<VoxelTypeId>)])`
- `world/data.rs:1181` — `pub fn set_voxels_batch_oracle(&mut self, edits: &[(IVec3, VoxelTypeId)])`
- `world/data.rs:737-738, :793` — internal `HashMap<[u32; 3], Vec<([u32; 3], u16)>>` ("`(voxel_in_chunk, type-as-u16)`")
- `world/data.rs:771, :774` — `Vec<([u32; 3], u32)>` ("`(chunk_pos, edit_data_offset)`"), `Vec<(usize, u32)>` ("`(chunk_idx, old_state)`")
- Callers: `editor/tools.rs:139, :153, :160, :208, :222, :282, :285, :344, :422, :454, :535` (paint/cube/sphere brushes — D2)
- `aadf/edit.rs:252` — `pub fn process_edit_batch(..., edited_chunks: &[([u32; 3], u32)], ...)` — the `(chunk_pos, edit_data_offset)` tuple again

**What's wrong:** Every "voxel edit" or "chunk + maybe-type" parameter is an anonymous tuple. At the call site you cannot tell `(IVec3, VoxelTypeId)` from `([u32; 3], u32)` without looking at the function signature; reading the body of `set_voxels_batch` (the largest production method, 360 LOC), the same `(IVec3, VoxelTypeId)` value is destructured as `&(pos, ty)` at `:739`, then `(voxel_in_chunk, ty)` after conversion, then re-flattened into `Vec<([u32; 3], u16)>` at `:737-738`. Five different anonymous-tuple shapes carry "voxel-edit-with-context" through this one function.

**Why it matters:** Direct hit on the user's brief ("anonymous tuples carrying meaning"). The audit explicitly calls this out in `00-reuse-audit.md §3.5 UA-1`. Self-documenting structs at the API boundary (`VoxelEdit { pos: IVec3, ty: VoxelTypeId }`, `ChunkEdit { pos: [u32;3], ty: Option<VoxelTypeId> }`) ripple naturally into the internal `HashMap<...>` keys without altering wire formats.

**Suggested direction (NOT a design):** Introduce `pub struct VoxelEdit { pub pos: IVec3, pub ty: VoxelTypeId }` in `world::data` (or `aadf::edit` — wherever the architect lands). `pub struct ChunkEdit { pub pos: UVec3, pub ty: Option<VoxelTypeId> }`. Internal tuples can stay anonymous (low blast-radius). Callers across `editor/tools.rs` get the readability win at the API surface.

**Out-of-scope ripple:** `editor/tools.rs` (D2) is the heaviest caller — about 11 call sites construct the tuple. Coordinated rename, not a design break.

---

### Finding 6 — `CHUNK_DIM_VOXELS` (= 16) and `CELL_DIM` (= 4) are defined twice and inlined repeatedly (MEDIUM)

**Locations:**
- `aadf/construct.rs:29` — `pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;`
- `aadf/generator.rs:51` — `const CHUNK_DIM_VOXELS: u32 = (CELL_DIM * CELL_DIM) as u32;` (private re-definition with different type!)
- `voxel/mod.rs:63-65` — `pub const CELL_DIM: usize = 4; pub const CELL_CHILDREN: usize = CELL_DIM * CELL_DIM * CELL_DIM;`
- Inline `16` literals (chunk side in voxels):
  - `world/data.rs:493-497, :509-511, :520, :523-525, :535-538, :646-650, :654, :667-670, :674`
  - `aadf/edit.rs:444, :446, :612`
- Inline `4` literals (block side, in cells of layer below):
  - `world/data.rs:520, :522-525, :531-533, :536-538, :651, :654, :670, :674`
  - `aadf/edit.rs:444, :446`
  - `aadf/generator.rs:167, :169, :177, :179, :269, :276, :425, :428`

**What's wrong:** `CELL_DIM = 4` is the SSoT (paper §3.1, the only number that should appear). `CHUNK_DIM_VOXELS = CELL_DIM * CELL_DIM = 16` is derived once correctly in `aadf/construct.rs`. Then it's redefined privately in `aadf/generator.rs:51` with a different type (`u32` vs `usize`), and elsewhere written as the bare integer `16` (~25 sites in `world/data.rs` alone). The block-layer constant (4 — both the block's side in voxels AND the chunk's side in blocks; they happen to share `CELL_DIM`) is inlined in another ~20 sites.

The C# reference (`WorldData.cs:421-440, MagicaVoxel.cs`) does the same — it's a faithful port — but C# at least has the constants in one file. Rust has them in two crates worth of files and uses raw `16`s next to them.

**Why it matters:** `00-reuse-audit.md §3.1 SSoT-3` flags this for D1+D4+D5 — D1 owns the Rust SSoT. The hazard is real: if `CELL_DIM` ever needs to be revisited (the paper hardcodes 4 forever, but a re-encoded variant might not), the change ripples through 50+ inline literals across the Rust source. Today's risk is low; the readability cost is constant.

**Suggested direction (NOT a design):** Promote `CHUNK_DIM_VOXELS` (or call it `CHUNK_SIDE_VOXELS`) to a single `pub const` in `voxel/mod.rs` alongside `CELL_DIM` and `CELL_CHILDREN`. Delete the private re-definition in `aadf/generator.rs`. Pass-through inline grep + replace of bare `16` / `4` literals to `CHUNK_DIM_VOXELS as i32` / `CELL_DIM as i32` (or i64 for ray-traversal — match call-site type).

**Out-of-scope ripple:** WGSL hard-codes `16u` / `4u` in many shader files (D4/D5). That's the SSoT-3 crosscutting story — D1's job is just to expose the constants for D5 to consume via `#{...}` shader-defs or a planned uniform when the architect lands SSoT-3.

---

### Finding 7 — `set_voxel` body has 4 inline chunk-position pack/unpack blocks instead of using `pack_chunk_pos` / `unpack_chunk_pos` (MEDIUM)

**Locations:**
- Helpers: `aadf/edit.rs:203-214` — `pack_chunk_pos`, `unpack_chunk_pos`
- Inline duplicates:
  - `world/data.rs:330-335` — `let cx = (pos_packed & 0x7FF)` etc. (inline unpack)
  - `world/data.rs:363-368` — same inline unpack
  - `world/data.rs:383-388` — `let pos_packed = crate::aadf::edit::pack_chunk_pos([cx, cy, cz]);` (correctly uses helper — but the surrounding context re-builds `cx, cy, cz` from a flat index by hand)
  - `world/data.rs:1278-1285` — inline unpack (`set_voxels_batch_oracle`)
  - `world/data.rs:1307-1314` — same
  - `world/data.rs:1326-1331` — same plus `pack_chunk_pos` (correct)
  - `aadf/edit.rs:67-69` — inline unpack inside `apply_chunk_edit_cpu` (load-bearing — has comment cite to `worldChange.fx:122` — could call helper but doesn't)
- WGSL uses the same `& 0x7FF | (>> 11 & 0x3FF) | (>> 21)` packing — `assets/shaders/world_change.wgsl` etc. (D5 owns)

**What's wrong:** A `pub fn pack_chunk_pos` and `pub fn unpack_chunk_pos` exist (`aadf/edit.rs:203-214`) and are the docblock-cited SSoT for the layout (`aadf/edit.rs:26-29`). But the bodies of `set_voxel`, `set_voxels_batch_oracle`, `apply_chunk_edit_cpu`, and `recompute_chunk_layer_aadfs` all use inline `pos_packed & 0x7FF` / `>> 11 & 0x3FF` / `>> 21` instead. ~9 inline call sites in D1. Each one is a chance to typo the shifts (which are arch-dependent: 11/21 not 10/20).

**Why it matters:** `00-reuse-audit.md §3.5 UA-3` calls this out for D1+D5. UA-3 in the brief lists "~10 inline call sites". Verified: 9 sites in D1 alone (plus more in render/construction — D5's count). The mask literal `0x7FF` for x, `0x3FF` for y, no mask for z is asymmetric — `pack_chunk_pos` writes `p[0] | (p[1] << 11) | (p[2] << 21)` (no truncation of x at 11 bits, no truncation of y at 10 bits before shift). If a caller hand-rolls `(pos.x & 0x7FF) | (pos.y << 11) | ...` for a position whose y exceeds 1023 chunks, behaviour diverges. Today's world is 32 chunks tall, so it doesn't bite — but the asymmetry is a real footgun.

**Suggested direction (NOT a design):** Replace every inline `& 0x7FF` / `>> 11 & 0x3FF` / `>> 21` triple in D1 with `unpack_chunk_pos(...)`. Replace every inline `cx | (cy << 11) | (cz << 21)` with `pack_chunk_pos(...)`. The helpers already exist; this is rote substitution.

**Out-of-scope ripple:** ~5 more sites in `render/construction/world_change.rs` etc. (D5 owns; flag for architect).

---

### Finding 8 — Hash-coefficient table is implemented twice in Rust (MEDIUM)

**Locations:**
- `aadf/block_hash.rs:395-404` — `fn build_polynomial_coefficients() -> [u32; 65]` (D1 owns — used by `BlockHashingHandler::with_size`)
- `render/construction/hashing.rs:43-50` — `pub fn hash_coefficients() -> [u32; 65]` (D5 owns)
- Both implement the same algorithm: `c[64] = 1; for i in (0..64).rev() { c[i] = c[i+1].wrapping_mul(31); }`
- WGSL: `chunk_calc.wgsl` (D5 owns) — verified hardcoded literal table referenced as "chunk_coefficients" per the audit, not re-checked here.

**What's wrong:** Verified by direct read: both Rust functions are byte-equivalent. They differ in visibility (`pub` vs file-private) and name only. `aadf/block_hash.rs:395-404` is private; `render/construction/hashing.rs:43-50` is `pub`. The latter is consumed by W1's GPU upload path; the former is consumed by `BlockHashingHandler::new` for the edit-time CPU dedup. Both produce identical 65-entry tables.

**Why it matters:** `00-reuse-audit.md §3.1 SSoT-6` calls this out. Single Rust SSoT, single test that pins it, makes the Rust ↔ WGSL agreement audit (which is D5's job for the WGSL side) cleaner. Today the audit has to chase two Rust implementations.

**Suggested direction (NOT a design):** Promote `aadf::block_hash::build_polynomial_coefficients` to `pub` (or move it into `voxel/mod.rs` as a generic SSoT), delete `render/construction/hashing.rs::hash_coefficients`, have D5 re-export or directly call the D1 one. The architect should decide whether the Rust SSoT lives in D1 or in a shared `aadf::hash` module.

**Out-of-scope ripple:** Touches `render/construction/hashing.rs` (D5). Pure deletion + import swap.

---

### Finding 9 — `set_voxel` and `set_voxels_batch` re-pack edit-batch payloads with hand-rolled loops that `EditBatch` could encapsulate (MEDIUM)

**Locations:**
- `world/data.rs:294-312` — `set_voxel` appends voxels/blocks from `batch.changed_voxels.chunks_exact(33)` and `batch.changed_blocks.chunks_exact(65)` with hand-rolled `for &v in &chunk_vox[1..33]` slicing
- `world/data.rs:315-324` — `set_voxel` iterates `batch.changed_blocks.chunks_exact(65)` AGAIN immediately afterwards to call `apply_block_edit_cpu`
- `world/data.rs:1261-1276` — `set_voxels_batch_oracle` repeats the same pattern verbatim
- `aadf/edit.rs:189-200` — `EditBatch` struct definition (`changed_chunks`, `changed_blocks`, `changed_voxels` as flat `Vec<u32>`s with documented "33-u32-per-edit" / "65-u32-per-edit" stride)

**What's wrong:** The 33-u32 (1 pointer + 32 voxel pairs) and 65-u32 (1 pointer + 64 block words) on-wire packing format is documented in the docblock at `aadf/edit.rs:25-34` but the **iteration logic to walk it is rolled by hand at every call site**. `EditBatch` could expose `.iter_voxel_edits() -> impl Iterator<Item = (u32, &[u32; 32])>` and `.iter_block_edits() -> impl Iterator<Item = (u32, &[u32; 64])>` and the call sites would be 3 lines each instead of 15.

**Why it matters:** Primitive obsession — `Vec<u32>` is treated as an opaque blob the caller has to remember the stride of. This is the exact "anonymous tuples carrying meaning" pattern at the byte level. Rust-idiom alignment: types own their iteration.

**Suggested direction (NOT a design):** Add iter methods on `EditBatch` (~15 LOC). Replace the four inline `chunks_exact(33|65)` loops in `world/data.rs` with the iter methods. Pure additive — old callers continue to work.

**Out-of-scope ripple:** None — `EditBatch` only consumed in D1.

---

### Finding 10 — `DIR_NEG_X..DIR_POS_Z` are raw `usize` constants instead of a `Dir6` enum (LOW)

**Locations:**
- `aadf/cell.rs:28-33` — `pub const DIR_NEG_X: usize = 0; DIR_POS_X: usize = 1; ...` (six constants)
- Callers in D1: `aadf/bounds.rs:51, :145-150, :298-323, :481-501, :614-621`, `aadf/construct.rs:423, :497-499, :564`, `aadf/cell.rs:306-321` (tests only)
- C# equivalent: 6-bit masks computed at WGSL/HLSL bit positions — no named enum either.

**What's wrong:** `Aadf6.d[DIR_POS_X]` is an array index; the type system can't catch `Aadf6.d[3]` (DIR_POS_Y) being passed where DIR_POS_X was meant. The bare `usize` constants give index correctness only by convention.

**Why it matters:** The audit's `00-reuse-audit.md §3.5 UA-4` flags this with a real caveat — "Low severity because it's used in tight inner loops where the indirection cost matters". Verified: `aadf/bounds.rs::step_axis` and `bounds_match` are hot. A `#[repr(u8)] enum Dir6 { NegX = 0, PosX = 1, ... }` with `#[inline] fn as_idx(self) -> usize` doesn't change codegen but the function-pointer-discipline of "you can iterate `Dir6::ALL` but you can't pass `usize` where `Dir6` is expected" is the win.

**Suggested direction (NOT a design):** Architect to judge. If kept as constants, at least add a `const DIRS: [usize; 6] = [DIR_NEG_X, DIR_POS_X, ..., DIR_POS_Z]` for the `for &dir in DIRS.iter()` pattern that the test code at `aadf/cell.rs:284` already wants. If promoted to an enum, ensure inlining keeps the hot-loop codegen unchanged (sanity-check `compute_aadf_layer`'s assembly before/after).

**Out-of-scope ripple:** None — all D1 callers.

---

## Confirmed / refuted audit suspicions

### Suspicion 1 — `world/data.rs` 3+ near-parallel set-voxel entry points

**CONFIRMED with expansion to 4.** Counted 4 `pub` set-voxel methods: `set_voxel`, `set_voxels_batch`, `set_chunks_uniform_batch`, `set_voxels_batch_oracle`. Two are diagnostic-only (docblock at lines 19-34 says so, and marks them `#[doc(hidden)]`). See Finding 1 + Finding 3.

### Suspicion 2 — `aadf/edit.rs` 828 LOC is the CPU oracle — public surface honest?

**CONFIRMED honest, modulo one test-helper.** Read every `pub fn`:

- `apply_chunk_edit_cpu`, `apply_block_edit_cpu`, `apply_voxel_edit_cpu` — the W2 GPU-shader oracle trio. **Sacred per user directive.** Used from production diagnostic paths (`world/data.rs::set_voxel` ← `--edit-mode` gate) and from `--validate-gpu-construction*` gates in D5.
- `process_edit_batch` — production runtime helper (called from `world/data.rs::set_voxel` and `set_voxels_batch_oracle`). Comment at `edit.rs:218-224` describes it as "runtime path", but only the diagnostic-only `set_voxel` / `set_voxels_batch_oracle` call it now — the production runtime `set_voxels_batch` does its own inline hash-dedup encode. Worth a docblock update; the public surface is genuinely used by tests + the diagnostic gate.
- `pack_chunk_pos`, `unpack_chunk_pos` — utility, correctly `pub`.
- `EditBatch` (struct + `Default` impl) — wire-format type, correctly `pub`.
- `build_chunk_edit_window_solid_type` — **#[allow(dead_code)] test helper masquerading as `pub`**. Only the doc-comment says "Test-helper only". Should be `#[cfg(test)]` or moved to a `pub(crate)` test fixture module.
- `build_chunk_edit_window_from_world`, `set_voxel_in_window` — used from `world/data.rs::set_voxel` and `set_voxels_batch`. Correctly `pub`.
- `recompute_chunk_layer_aadfs` — `#[doc(hidden)]`, diagnostic-only. Correctly tagged.
- `cell_unpack_voxel` re-export at `edit.rs:571` — looks like vestigial test-bridge code (also has an `#[allow(unused_imports)] use unpack_voxel as _unpack_voxel;` at line 574 that has no functional purpose). Can probably be removed; not blocking.

Sacred public surface stays. Only `build_chunk_edit_window_solid_type` and the dead-import at `edit.rs:571-574` are spurious.

### Suspicion 3 — `bounds.rs` + `construct.rs` + `generator.rs` compute AADF over 3 different layout shapes — can they share a core?

**CONFIRMED PARTIALLY.** Read all three:
- `bounds::compute_aadf_layer` is the core. **Already called from** `aadf/construct.rs:239, :365, :403` (3 sites) AND from `aadf/edit.rs:103, :145, :542` (3 sites). Construction-time AND edit-time AADF all flow through this one function.
- `aadf/entity.rs::EntityData::from_types` (`:144-222`) is a 4th caller-equivalent but doesn't actually call — it inlines the same algorithm against a packed `u32` voxel layout (5-bit AADFs at shifts 0/5/10/15/20/25 + 0x80000000 flag bit). Already addressed in Finding 4.
- `aadf/generator.rs` does **NOT** compute AADF — it generates packed voxel `u32`s from a `ModelData` (no AADF involved). The brief's suspicion that `generator` overlaps with `construct`/`bounds` is **REFUTED** at the algorithmic level — `generator_segment_cpu` is the CPU oracle for the W5 GPU **world generator** (input to Algorithm 1), not a re-implementation of AADF computation. Confusingly named in the audit but the file body is unambiguous (`aadf/generator.rs:1-44` docblock).

So: the construct/bounds/edit triad **already shares** `compute_aadf_layer`. The remaining duplication is just `aadf/entity.rs` (Finding 4). Suspicion is refined to a single-target audit, not three-way.

### Crosscutting SSoT-3 (CELL_DIM/CELL_CHILDREN exposure)

**CONFIRMED with broader scope.** Original audit cited `voxel/mod.rs:63-65` as the SSoT — verified. The bare `16` literal (`= CELL_DIM * CELL_DIM`) and bare `4` literal occur 40+ times in D1 alone (Finding 6). `CHUNK_DIM_VOXELS` is **defined twice** (`construct.rs:29`, `generator.rs:51` private — different types). D1 should consolidate the Rust SSoT before D5 wires the WGSL shader-def upload (D5's responsibility per `00-reuse-audit.md §3.1`).

### Crosscutting SSoT-6 (hash coefficients — 3 implementations agree?)

**CONFIRMED, agree.** Verified `aadf/block_hash.rs::build_polynomial_coefficients` (lines 395-404) and `render/construction/hashing.rs::hash_coefficients` (lines 43-50). Both implement `c[64] = 1; c[i] = c[i+1].wrapping_mul(31) for i in (0..64).rev()`. Byte-equal output. WGSL side (`chunk_calc.wgsl` `chunk_coefficients`) not re-verified — D5's territory — but the audit asserts agreement. Recommended single Rust SSoT: see Finding 8.

### Crosscutting DUP-1 (3+ set-voxel entry points)

**CONFIRMED — 4 entry points, not 3.** Verified above (Suspicion 1). See Finding 1.

### Crosscutting UA-1 (anonymous tuples in set_voxels_batch)

**CONFIRMED.** ~11 call sites of `(IVec3, VoxelTypeId)` in `editor/tools.rs` (D2 — already in their domain), 3 in D1's tests, 4 in `world/data.rs` body destructures, 1 internal `HashMap<[u32; 3], Vec<([u32; 3], u16)>>` (5+ tuple shapes total in one function — see Finding 5).

### Crosscutting UA-3 (raw u32 chunk-pos masks bypass pack/unpack helpers)

**CONFIRMED — 9 sites in D1.** Verified locations in Finding 7. Brief estimated ~10; actual count in D1 is 9. Plus more sites in `render/construction/world_change.rs` (D5).

### Crosscutting UA-4 (raw DIR_* indices)

**CONFIRMED as low-priority.** All callers verified inside D1. Hot-loop codegen sensitivity is real per the brief. See Finding 10.

---

## Side notes / observations / complaints

1. **`world/data.rs` is 1 731 LOC but only ~1 030 LOC of it is non-test code** — the embedded `#[cfg(test)] mod tests` is 326 LOC (`:1405-1731`), and the `set_voxel` + `set_voxels_batch_oracle` diagnostic methods together are another ~330 LOC. The "production runtime surface" of `WorldData` is closer to 700 LOC including `Default`/`seed_block_hashing`/`ray_traversal`/`get_voxel_type`/`set_voxels_batch`/`set_chunks_uniform_batch`. The headline 1 731 number conflates three populations. After Finding 1's diagnostic extraction, the file would land at ~1 100 LOC — substantial but not pathological for a load-bearing resource.

2. **`aadf/mod.rs` re-exports `block_hash`, `bounds`, `cell`, `construct`, `edit`, `entity`, `generator` — but the docblock at lines 1-10 only describes `cell`, `construct`, `bounds`, `generator`. `edit`, `entity`, `block_hash` are silently `pub mod`.** Minor docblock-vs-code mismatch; pure documentation rot — fix while touching the file.

3. **The audit's domain card describes `aadf/edit.rs` as "now that the GPU path is the production producer (per E4), this file is *only* test infrastructure"** — that framing is contradicted by what I read. `edit.rs::process_edit_batch` is called from `world/data.rs::set_voxel` (diagnostic-only, true) AND from `set_voxels_batch_oracle` (also diagnostic-only). But `edit.rs::set_voxel_in_window` and `edit.rs::build_chunk_edit_window_from_world` are called from BOTH diagnostic AND production paths (`world/data.rs:267, :273` from `set_voxel`, `:818-832, :825` from `set_voxels_batch` production). And `apply_block_edit_cpu` is called from `set_voxels_batch` production path too (`:1026`). So edit.rs's public surface is genuinely mixed-use; only `recompute_chunk_layer_aadfs` is purely diagnostic. The user directive ("cpu oracle stays") is what matters; the brief's "this file is only test infrastructure" framing is wrong.

4. **`world/buffer.rs` (`GrowableBuffer<T: Pod>`) is in the domain path list but has zero overlap with the rest of D1's findings.** It's a GPU-buffer helper consumed by D4/D5; it lives under `world/` because the file tree is organised by build-time ownership, not consumer-time. The architect should consider whether `world/buffer.rs` belongs in D1 at all or whether it should move to `render/` (D4) where its 100% callers live. Out of scope to act on, but worth flagging — the file does not appear in any D1 finding above.

5. **`aadf/mod.rs` docblock at line 9-10 calls `aadf/generator.rs` the "Phase-C W5 CPU oracle for `generatorModel.fx`" — confirmed accurate.** But the file name `generator.rs` is misleading next to `bounds.rs` / `construct.rs` / `edit.rs` — a reader naturally expects it to be a generator of AADF data structures. It generates *initial voxel content* from `ModelData`. Rename to `generator_model.rs` or `world_generator.rs` would improve discoverability. Out of scope unless the architect agrees.

6. **`world/data.rs::set_voxel` body has a `let ptr_unused = edit_block[0]; let _ = ptr_unused;` pattern at lines 316-317.** This is a dead bind. The pointer is intentionally not used because the call site computes a different pointer from `b_cursor + idx * 64` at line 319. The dead bind plus the explicit `let _ = ptr_unused;` is a code smell that someone removed a use and didn't clean up. Trivial deletion; flag for the architect since touching `set_voxel` is in scope of Finding 1.

7. **Several large prose docblocks (`world/data.rs:1-34`, `aadf/edit.rs:1-41`, `aadf/bounds.rs:1-49`) explain the project's diagnostic-vs-runtime architecture in nearly identical terms.** Information is correct but redundant — the same "DIAGNOSTIC-ONLY vs production runtime" story is retold three times. Could collapse to one canonical statement in `world/mod.rs` (the natural plug-in seam) and shorter file-level docblocks referring to it. Out of scope; pure documentation tidy.

8. **The faithful-port directive bites hard here.** `set_voxels_batch` (`world/data.rs:721-1080`) is a 360-line port of C# `EditingHandler.processChunks` with extensive C# line references in the comments — every block is anchored to a C# line range. Any refactor that *moves* logic around (instead of preserving the 1:1 stanza-with-C# structure) damages the doc-link traceability that several `wasm-chunk-aadf-nondeterminism` / `vox-gpu-rewrite` orchestrations relied on. Recommend the architect's Finding 1 extraction preserves the C# line citations even when methods change names/file locations.

9. **`set_voxel` (single-voxel path) does the AADF rehash; `set_voxels_batch` (production path) explicitly does NOT (per `02c` Decision 3, comment at `world/data.rs:1052-1056`).** So the two diagnostic methods AGREE on the rehash but disagree on which call sites get it (`set_voxel` always; `set_voxels_batch_oracle` always; `set_voxels_batch` never; `set_chunks_uniform_batch` never). This 4×2 production-vs-diagnostic / rehash-vs-no-rehash matrix is the actual code shape that the unified-diagnostic-mod (Finding 1) needs to preserve. Naming `rehash_oracle()` vs `runtime_batch()` would make the semantic split visible at the call site; today both spellings just say "set voxels".

10. **The `bevy_naadf` crate's `voxel/mod.rs:14-19` re-exports `pub mod async_vox, cvox_import, grid, vox_import, voxel_dispatch, web_vox` — modules that are entirely D3's domain (voxel I/O).** D1's interest in `voxel/mod.rs` is just the bit-layout constants + the `VoxelType` / `VoxelTypeId` / `MaterialBase` / `MaterialLayer` types at lines 21-144. The file mixes "bit-layout SSoT" (D1) with "I/O module declarations" (D3) in one 144-LOC stub. Could split if a D3 architect agrees, but the file is small enough that it's also fine as-is. Flag for orchestrator awareness.

---

## Open questions for the architect

- **Finding 1's diagnostic extraction:** does the user accept hiding the diagnostic methods behind `pub(crate) mod oracle`, or do they want stronger `#[cfg(any(test, feature = "diagnostic"))]` gating? The orchestration's faithful-port rule + the e2e gates calling `set_voxel` make `pub(crate)` the minimally-invasive option. If feature-gated, every `--edit-mode` e2e CI run needs the feature on — possible but introduces a build-matrix axis.
- **Finding 2's state-bit constants:** the architect must pick a Rust naming convention compatible with WGSL's `BLOCK_STATE_CHILD / BLOCK_STATE_UNIFORM_EMPTY / BLOCK_STATE_UNIFORM_FULL` (D5 owns WGSL). Direct mirror (`pub const CELL_STATE_CHILD: u32 = 2`) is the lowest-friction option but the existing `voxel/mod.rs` constants (`CELL_HAS_CHILDREN = 1 << 31` etc.) are public API used by `aadf/cell.rs`. Backward-compatible aliases? Or migrate `aadf/cell.rs::encode/decode` to the state-nibble regime?
- **Finding 4's entity AADF kernel:** is bit-for-bit GPU match more important than DRY? The current C#-faithful inline loop in `EntityData::from_types` matches the C# GPU shader's bit-for-bit output. Sharing the kernel with `compute_aadf_layer` (which is also bit-for-bit-equivalent per `02-research.md` §1.6) is theoretically clean but adds risk to W4. Architect to judge whether the shared kernel passes the W4 oracle tests cleanly.
- **Finding 8's hash-coefficient SSoT:** does the unified location live in D1 (`aadf::block_hash`), in `voxel::hash` (new module), or in D5 (where the GPU upload happens)? D1 owns the algorithm; D5 owns the GPU consumer. Architect to choose.
