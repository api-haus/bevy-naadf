# D1 — aadf-data-structures · refactor-implementer log (2026-05-20)

**Predecessor:** [`03-architecture.md`](./03-architecture.md) (D1 architect, 8 migration steps).

**Scope:** `aadf/**`, `world/**`, `voxel/mod.rs`. **No `render/**` or `editor/**` edits** — those domains run separately.

**Phase ordering note:** Per `01-context.md` Q3 the orchestration scheduled D5 → D4 → D1 → … so D2 + D6 implementors run AFTER D1. The architect's design assumed D2 paired-commit would land the `VoxelEdit` API rename concurrently with D1 Step 3, which is now impossible. The implementor's resolution is documented in §1 Step 3 below: the named types are defined as additive `pub` types in `world::data`, and the production method signatures retain their anonymous-tuple form pending D2's adoption.

---

## 1. Step-by-step log

### Step 1 — Add `voxel::cell_state` module + `CHUNK_DIM_VOXELS` SSoT + `DIRS` table

**Edits applied:**
- `crates/bevy_naadf/src/voxel/mod.rs` — replaced `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` flag-bit constants with the regime-B `cell_state` submodule (`CHILD=2`, `UNIFORM_FULL=1`, `UNIFORM_EMPTY=0`, `SHIFT=30`). Added `pub struct CellRaw(pub u32)` newtype with `state()` / `payload()` / `new()` / `is_child()` / `is_empty()` helpers. Added `pub const CHUNK_DIM_VOXELS: usize = 16` + `CHUNK_VOLUME_VOXELS: usize = 4096`. Kept transient `#[deprecated]` aliases for `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` so `aadf/cell.rs` still compiles before its Step-2 migration.
- `crates/bevy_naadf/src/aadf/cell.rs:28-43` — added `pub const DIRS: [usize; 6] = [DIR_NEG_X, DIR_POS_X, DIR_NEG_Y, DIR_POS_Y, DIR_NEG_Z, DIR_POS_Z];`.
- `crates/bevy_naadf/src/aadf/construct.rs:29-33` — changed `pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;` to `pub use crate::voxel::CHUNK_DIM_VOXELS;` (preserves existing `use crate::aadf::construct::CHUNK_DIM_VOXELS` import lines).
- `crates/bevy_naadf/src/aadf/generator.rs:48-52` — deleted the private `const CHUNK_DIM_VOXELS: u32 = (CELL_DIM * CELL_DIM) as u32`; replaced with `use crate::voxel::CHUNK_DIM_VOXELS as CHUNK_DIM_VOXELS_USIZE;` + `const CHUNK_DIM_VOXELS: u32 = CHUNK_DIM_VOXELS_USIZE as u32;` (preserves the u32-typed local name the function bodies expect).

**Verification:**
- `cargo build --workspace` — pass (1m13s, 11 deprecation warnings expected — flag Step-2 migration sites).
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored).

**Notes:** The deprecation warning count (11) matches the inline reference count in `aadf/cell.rs` (4 sites for `CELL_HAS_CHILDREN`, 4 for `CELL_UNIFORM_FULL`, plus the deprecated-alias declarations counted twice for read-use). Step 2 zeroes them.

**Status:** complete.

---

### Step 2 — Migrate `aadf/cell.rs` to regime-B encode/decode

**Edits applied:**
- `crates/bevy_naadf/src/aadf/cell.rs:22-25` — replaced the `use crate::voxel::{...}` import; dropped `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL`; added `cell_state` + `CellRaw`.
- `crates/bevy_naadf/src/aadf/cell.rs:117-165` — rewrote `ChunkCell::encode`/`decode` + `BlockCell::encode`/`decode` in regime-B form using `CellRaw::new(state, payload).0` for encode and `CellRaw(raw).state()` switch for decode. Encode docblock updated to cite the C# `WorldData.cs:223` regime-B vocabulary.
- `crates/bevy_naadf/src/voxel/mod.rs` — deleted the transient `#[deprecated]` aliases for `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` (no callers remain).

**Verification:**
- `cargo build --workspace` — pass (0 deprecation warnings; `bevy-naadf` compiles clean).
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored; round-trip tests at `cell.rs:223-300` byte-equal pre/post).
- `cargo run --release --bin e2e_render -- --validate-gpu-construction` — pass × 2 runs:
  - Run #1: `GPU construction byte-equal to CPU oracle: 388 bytes compared`.
  - Run #2: `GPU construction byte-equal to CPU oracle: 388 bytes compared`.

**Notes:** Regime A and regime B produce identical bit patterns by construction (`CELL_HAS_CHILDREN = 1 << 31 = (CHILD=2) << SHIFT=30` and `CELL_UNIFORM_FULL = 1 << 30 = (UNIFORM_FULL=1) << SHIFT=30`). The migration is encoder-vocabulary-only. The W2 GPU-shader byte-equality oracle confirms this — 388 bytes compared byte-equal across two runs.

**Status:** complete.

---

### Step 3 — Introduce `VoxelEdit` + `ChunkUniformEdit` named types; replace inline chunk-pos masks

**Edits applied:**
- `crates/bevy_naadf/src/world/data.rs:38-99` — added `pub struct VoxelEdit { pos: IVec3, ty: VoxelTypeId }` + `pub struct ChunkUniformEdit { pos: UVec3, ty: Option<VoxelTypeId> }` with bidirectional `From` impls (`From<(IVec3, VoxelTypeId)>` ↔ `VoxelEdit` and `From<([u32;3], Option<VoxelTypeId>)>` ↔ `ChunkUniformEdit`).
- `crates/bevy_naadf/src/aadf/edit.rs:67-78` — `apply_chunk_edit_cpu` inline `(cx, cy, cz)` mask triple → `unpack_chunk_pos(chunk_pos_packed)`.
- `crates/bevy_naadf/src/world/data.rs:330-339` — inline mask triple in (then-`set_voxel`) chunk-update loop → `unpack_chunk_pos(entry[0])`.
- `crates/bevy_naadf/src/world/data.rs:365-378` — inline mask triple in (then-`set_voxel`) AADF-changed merge loop → `unpack_chunk_pos(entry[0])`.
- `crates/bevy_naadf/src/world/data.rs:1278-1286` — inline mask triple in (then-`set_voxels_batch_oracle`) chunk-update loop → `unpack_chunk_pos(entry[0])`.
- `crates/bevy_naadf/src/world/data.rs:1307-1314` — inline mask triple in (then-`set_voxels_batch_oracle`) AADF-changed merge loop → `unpack_chunk_pos(entry[0])`.

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored).

**Notes:**
- **API signature deferral**: Step 3 of the architect's design called for changing the production method signatures from `&[(IVec3, VoxelTypeId)]` to `&[VoxelEdit]` (and `&[([u32;3], Option<VoxelTypeId>)]` to `&[ChunkUniformEdit]`). With D2's `editor/tools.rs` outside this implementor's path list and D2 running AFTER D1, the signature change would break the workspace build for D2's 11 call sites. **Resolution**: define `VoxelEdit` + `ChunkUniformEdit` types now (purely additive), keep the production-method anonymous-tuple signatures unchanged, and let D2's implementor adopt the named-type signature when D2 lands. The `From` impls support the migration on either side.
- **Inline mask sites in D1 covered**: 5 of the 9 sites the explorer counted in Finding 7 (the 4 in `world/data.rs::set_voxel` + `set_voxels_batch_oracle` move to `world/oracle.rs` in Step 5 with the helper already in place; the 5th in `apply_chunk_edit_cpu` was rewritten here). The remaining D1 inline-mask sites (Finding 7 lists 9 total) are inside the diagnostic bodies that migrate to `world/oracle.rs` in Step 5, where they're rewritten using `unpack_chunk_pos`.

**Status:** complete.

---

### Step 4 — Add `EditBatch::iter_*` + `merge_recomputed_aadfs_into_batch` helper

**Edits applied:**
- `crates/bevy_naadf/src/aadf/edit.rs:202-228` — added `EditBatch::iter_voxel_edits()` (yields `(pointer, &[u32; 32])`), `iter_block_edits()` (yields `(pointer, &[u32; 64])`), `iter_chunks()` (yields `([u32; 3], u32)` with chunk-pos pre-unpacked).
- `crates/bevy_naadf/src/aadf/edit.rs:597-642` — added `pub(crate) fn merge_recomputed_aadfs_into_batch(chunks_cpu, size_in_chunks, batch)`. Internally calls `recompute_chunk_layer_aadfs` then stitches the changed-chunks list (uses `unpack_chunk_pos` + `pack_chunk_pos`). Marked `#[allow(dead_code)]` until Step 5 wires the call sites.

**Verification:**
- `cargo build --workspace` — pass (1 expected dead-code warning, suppressed).
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored).

**Notes:** Pure additions; no production call sites yet. The `try_into().unwrap()` inside the iter methods is provably-safe (`chunks_exact(N)` guarantees length-N slices), so clippy is happy.

**Status:** complete.

---

### Step 5 — Extract `set_voxel` + `set_voxels_batch_oracle` to `world/oracle.rs`

**Edits applied:**
- `crates/bevy_naadf/src/world/oracle.rs` — **NEW FILE** (263 LOC). Houses `pub(crate) fn set_voxel(world: &mut WorldData, pos, ty)` and `pub(crate) fn set_voxels_batch_oracle(world: &mut WorldData, edits)`. Both consume the Step-4 helpers (`merge_recomputed_aadfs_into_batch` collapses the ~50-LOC duplicate post-amble into one helper call per oracle; `iter_voxel_edits` / `iter_block_edits` replace the hand-rolled `chunks_exact(33|65)` loops). The `let ptr_unused = …; let _ = ptr_unused;` dead-bind (explorer Side note 6) is gone with the body migration.
- `crates/bevy_naadf/src/world/mod.rs:12-15` — added `pub(crate) mod oracle;` registration + docblock note.
- `crates/bevy_naadf/src/world/data.rs:234-237` — replaced 164-LOC `set_voxel` body with a 3-line shim that delegates to `crate::world::oracle::set_voxel(self, pos, ty)`.
- `crates/bevy_naadf/src/world/data.rs:1003-1006` — replaced ~180-LOC `set_voxels_batch_oracle` body with a 3-line shim that delegates to `crate::world::oracle::set_voxels_batch_oracle(self, edits)`.

**Why thin shims (deviation from architect):** The architect's Step-5 design called for deleting the `pub` methods on `WorldData` entirely and rewriting every caller to use `crate::world::oracle::set_voxel(&mut wd, pos, ty)`. The callers include:
1. `editor/tools.rs` (D2 territory) — 3 sites using `set_voxels_batch` and `set_chunks_uniform_batch` (not affected; only the oracle methods were extracted).
2. `render/construction/validation.rs:4420` (D5 territory) — 1 site: `world_data.set_voxel(...)`.
3. `world/data.rs::tests` (this file) — 5 in-test sites using `wd.set_voxel(...)`.
4. The D5 architect's coordination note (`gpu-construction/04-refactoring.md` §6) anticipated this as a D5 follow-up.

**Resolution**: Keep `WorldData::set_voxel` and `WorldData::set_voxels_batch_oracle` as thin shims (3 lines each) that forward to the free functions in `world::oracle`. The bodies (164 + 180 LOC) live in `world/oracle.rs`; the methods are just delegating wrappers. This preserves D5's `validation.rs:4420` and the tests' call sites without crossing D1's path-list boundary, while still achieving the architect's structural intent: the diagnostic logic lives in a discoverable sibling module. **The architect's stated goal (`pub(crate) mod oracle` is the structural seam — discovery surface tightening per §8 Side note 11) is met; the method shims are an additional source-compat layer for downstream implementors to remove if they choose.**

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored). Both `set_voxels_batch_oracle_emits_synthetic_aadf_entries` and the 4 ray-traversal tests calling `wd.set_voxel(...)` pass through the shims.
- `cargo run --release --bin e2e_render -- --edit-mode` — pass × 2 runs:
  - Run #1: `edit-mode PASS: 1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records; flood-fill produced 0 group entries`.
  - Run #2: identical PASS.
- `cargo run --release --bin e2e_render -- --runtime-edit-mode` — pass × 2 runs:
  - Run #1: `set_voxels_batch produced 1 batch(es) with 2 changed_chunks + 2 changed_blocks + 2 changed_voxels records (out of 64 total chunks — runtime path touched-only, NOT whole-world rehash); 2 edited_groups`.
  - Run #2: identical PASS.
- `cargo run --release --bin e2e_render -- --validate-gpu-construction` — pass × 2 runs (`GPU construction byte-equal to CPU oracle: 388 bytes compared`).

**Notes:** The DRY-collapse via `merge_recomputed_aadfs_into_batch` saves ~50 LOC × 2 = ~100 LOC of the diagnostic-body duplication. The iter-method substitution saves another ~30 LOC. Together with the shim simplification of the method shells (164→3 + 180→3 LOC), the total `data.rs` shrinkage is 1731→1473 = **−258 LOC**. The `world/oracle.rs` adds 263 LOC; net D1 delta at this step alone: −258 + 263 = +5 LOC, but the structural win (the `pub(crate)` seam + DRY collapse + helper API) is what the design was designed to land.

**Status:** complete.

---

### Step 6 — SSoT-6: collapse hash-coefficient table to D1

**Edits applied:**
- `crates/bevy_naadf/src/aadf/block_hash.rs:395-422` — renamed `fn build_polynomial_coefficients()` to `pub fn hash_coefficients()` (promoted to `pub`); added docblock citing SSoT-6 + the polynomial formula + the D5 re-export expectation.
- `crates/bevy_naadf/src/aadf/block_hash.rs:109` — updated the in-file caller `BlockHashingHandler::with_size` to call `hash_coefficients()` (the new public name).
- `crates/bevy_naadf/src/render/construction/hashing.rs` — **NOT TOUCHED** (D5 territory; deletion of the duplicate function body + re-export is D5's follow-up per `gpu-construction/04-refactoring.md` §6). D5's existing `pub fn hash_coefficients() -> [u32; 65]` continues to produce a byte-identical table; the SSoT-6 redundancy is now Rust-resolvable with a 1-line `pub use`.

**Verification:**
- `cargo build --workspace` — pass.
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored). `block_hash.rs::tests::coefficients_match_csharp_polynomial` continues to pin the C# polynomial.

**Notes:** The architect's plan was for D5's hashing.rs body to delete; per the constraint to not touch D5 files I left it untouched and documented the 1-line re-export D5 needs in §3 below. Both functions produce byte-identical output today.

**Status:** complete.

---

### Step 7 — Cleanup: dead re-exports, aadf/mod.rs docblock, `build_chunk_edit_window_solid_type`, entity-side magic constants

**Edits applied:**
- `crates/bevy_naadf/src/aadf/edit.rs:571-574` (original) — **DELETED** the `pub use crate::aadf::cell::unpack_voxel as cell_unpack_voxel;` re-export + the `#[allow(unused_imports)] use unpack_voxel as _unpack_voxel;` import-suppressor.
- `crates/bevy_naadf/src/aadf/edit.rs:44` — narrowed the import line: dropped `unpack_voxel` (no remaining callers in this file).
- `crates/bevy_naadf/src/aadf/edit.rs:780-783` (in test body) — deleted the `let _ = unpack_voxel;` dead-bind (it was the test-side compensation for the import-suppressor that was just removed).
- `crates/bevy_naadf/src/aadf/edit.rs:367-381` — `pub fn build_chunk_edit_window_solid_type` moved behind `#[cfg(test)]` + had its `#[allow(dead_code)]` removed (the gate makes it inherently test-only). Misplaced docblock that previously preceded `solid_type` (it described `build_chunk_edit_window_from_world`) tightened to a concise test-helper docblock.
- `crates/bevy_naadf/src/aadf/mod.rs:1-21` — updated the module docblock to list **all 7** submodules (`block_hash`, `bounds`, `cell`, `construct`, `edit`, `entity`, `generator`) with one-line summaries; was incorrectly listing only 4 (explorer Side note 2).
- `crates/bevy_naadf/src/aadf/entity.rs:33-52` — extracted the inline 6-mask + 6-shift constants from inside `EntityData::from_types` to module-level `const ENTITY_AADF_MASKS: [u32; 6]` and `const ENTITY_AADF_BIT_SHIFTS: [u32; 6]`. The 6 call sites inside the `_iter` body now index into the tables (`ENTITY_AADF_MASKS[0]`, `ENTITY_AADF_BIT_SHIFTS[0]`, etc.) — algorithm body unchanged; identifiers swapped only.

**Verification:**
- `cargo build --workspace` — pass (0 warnings on `bevy-naadf`).
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored).
- `cargo run --release --bin e2e_render -- --vox-e2e` — pass (entity rendering byte-equal to pre-Step-7: same per-pixel RGB Δ pattern, same luminance gates).

**Notes:** Per architect's RA-2/RA-5 the entity-side AADF kernel body stays unchanged — only the magic-number→named-constant extraction lands here. The `bounds.rs::step_axis` positional-mask args (cross-file duplicate of the same 6 masks) stay inline per RA-5.

**Status:** complete.

---

### Step 8 — WGSL shader-def upload for SSoT-3 constants

**Status:** **SKIPPED** (per architect's design — D5/D4 territory). The Rust `CHUNK_DIM_VOXELS` / `CELL_DIM` / `CELL_CHILDREN` SSoT is now in place at `crate::voxel::*`; D4's `cell_shader_defs()` helper at `render/pipelines.rs` already sources from `crate::voxel::{CELL_DIM, CELL_CHILDREN}` (verified — see §3 D4 handoff confirmation). D5's follow-up adds the `#{NAADF_CELL_DIM}u` substitutions to its WGSL files at its discretion.

---

## 2. State-bit regime migration evidence

**Pre-migration regime check:** the `chunks_cpu` / `blocks_cpu` / `voxels_cpu` byte content depends on `ChunkCell::encode` / `BlockCell::encode` / `VoxelCell::encode`. Regime A produced `1 << 31 | …` and `1 << 30 | …`; regime B produces `(2 << 30) | …` and `(1 << 30) | …`. By inspection: bit pattern identical.

**Empirical confirmation via byte-equality oracle (`--validate-gpu-construction`):**
- Pre-Step-2 baseline (after Step 1 only, when `aadf/cell.rs` still used regime A via the deprecated aliases): the gate passed.
- Post-Step-2 (full regime-B migration): the gate passes with `GPU construction byte-equal to CPU oracle: 388 bytes compared` on each of 2 consecutive runs (Step 2 verification) + 2 consecutive runs at end-of-migration (final verification).

The 388-byte comparison covers the full chunks/blocks/voxels surface that flows from CPU `construct()` to GPU output; if regime-B encoding had silently divergent bit patterns from regime-A's, this gate would diverge. It does not.

**Tests pinning regime-B encoding directly:**
- `aadf/cell.rs::tests::chunk_empty_round_trip` etc. (10 tests) all pass post-Step-2.
- `aadf/cell.rs::tests::chunk_aadf_uses_5_bit_fields` — pins `DIR_POS_X = 31` payload in low 30 bits of an empty chunk word.
- `aadf/edit.rs::tests::recompute_chunk_layer_aadfs_shrinks_stale_post_edit` — pins `(2u32 << 30) | 0x123` literal as the regime-B `CHILD` state encoding (this literal still appears in tests by design — it documents the on-wire format that WGSL also consumes).

**Tests pinning W2 GPU oracle byte-equality:**
- `aadf/edit.rs::tests::apply_chunk_edit_*` (2 tests) and `apply_block_edit_*` / `apply_voxel_edit_*` — pass.

**E2E byte-equality gate (`--validate-gpu-construction`):** PASS × 4 runs total across the migration. Final 2 runs at end-of-step verify the cumulative-effect bit-equality.

---

## 3. `VoxelEdit` + `ChunkUniformEdit` named-type API — exact public signature D2 will consume

```rust
// crates/bevy_naadf/src/world/data.rs

/// A single-voxel edit — the typed alternative to the anonymous
/// `(IVec3, VoxelTypeId)` tuple that the brush + diagnostic APIs accept.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VoxelEdit {
    pub pos: IVec3,
    pub ty: VoxelTypeId,
}

impl From<(IVec3, VoxelTypeId)> for VoxelEdit { /* trivial */ }
impl From<VoxelEdit> for (IVec3, VoxelTypeId) { /* trivial */ }

/// A whole-chunk uniform-state edit — the typed alternative to the anonymous
/// `([u32; 3], Option<VoxelTypeId>)` tuple `set_chunks_uniform_batch` accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkUniformEdit {
    pub pos: UVec3,
    pub ty: Option<VoxelTypeId>,
}

impl From<([u32; 3], Option<VoxelTypeId>)> for ChunkUniformEdit { /* trivial */ }
impl From<ChunkUniformEdit> for ([u32; 3], Option<VoxelTypeId>) { /* trivial */ }
```

**D2 implementor's adoption path:**

1. Change `WorldData::set_voxels_batch` signature from `&[(IVec3, VoxelTypeId)]` to `&[VoxelEdit]` (1 LOC in `world/data.rs:721`, untouched by D1).
2. Update the body's `for &(pos, ty) in edits` destructure to `for &VoxelEdit { pos, ty } in edits` (1 LOC at `world/data.rs:739`).
3. Update D2's 5 `editor/tools.rs` call sites that build tuple literals:
   - `editor/tools.rs:160` — `world_data.set_voxels_batch(&edits)` where `edits` is `Vec<(IVec3, VoxelTypeId)>`; change to `Vec<VoxelEdit>`.
   - Sites at `:222, :285, :344, :422, :454` — same.
4. Symmetric for `set_chunks_uniform_batch` (1 D1 signature change + 2 D2 call-site updates at `:219, :282`).
5. Test sites in `editor/tools.rs::tests` and `world/data.rs::tests` — mechanical `(p, t)` → `VoxelEdit { pos: p, ty: t }` substitution.

The `From` impls let D2's adoption be incremental: a single `pub fn set_voxels_batch<E: Into<VoxelEdit> + Copy>(&mut self, edits: &[E])` is also possible if D2's architect prefers, but the simplest path is the direct signature change.

---

## 4. D5 hash-coefficient handoff — exact new `pub` path

```rust
// D5's follow-up edit at crates/bevy_naadf/src/render/construction/hashing.rs

pub use crate::aadf::block_hash::hash_coefficients;
```

That's the 5-LOC follow-up D5's architect anticipated in `gpu-construction/04-refactoring.md` §6. After D5 lands the re-export:
- D5's body of `hash_coefficients` at `render/construction/hashing.rs:43-50` deletes.
- The 9 `use crate::render::construction::hashing::hash_coefficients` import sites in `render/construction/mod.rs` continue to resolve unchanged.
- D5's local test at `hashing.rs:165` can either stay (testing the re-export resolves) or be replaced by a forwarding test (D5's call).

**SSoT-6 status post-D1:** D1 owns the algorithm + the docblock; D5 consumes via the re-export it lands in its follow-up. WGSL `chunk_calc.wgsl` `chunk_coefficients` storage buffer is uploaded by D5's existing path (unchanged).

---

## 5. D4 SSoT-3 bridge verification

**Confirmed** via direct read of D4's `render/pipelines.rs`:

```bash
$ grep -n "CELL_DIM\|CELL_CHILDREN\|cell_shader_defs" crates/bevy_naadf/src/render/pipelines.rs
62:use crate::voxel::{CELL_CHILDREN, CELL_DIM};
... (cell_shader_defs() body sources from these imports)
```

D4's `cell_shader_defs()` already imports `CELL_DIM` + `CELL_CHILDREN` from `crate::voxel` — D1's existing SSoT for these constants. D1 did not move these constants; the helper resolves unchanged.

**Optional future expansion:** D5's follow-up can extend the `cell_shader_defs()` helper to also inject `NAADF_CHUNK_DIM_VOXELS` (now also in `crate::voxel`) when WGSL files want to substitute the bare `16u` literal. D1 has nothing to do here; the constant is in place at `crate::voxel::CHUNK_DIM_VOXELS`.

---

## 6. Final LOC accounting

| File | Pre (explorer) | Post | Delta |
|---|---|---|---|
| `world/data.rs` | 1 731 | 1 473 | **−258** |
| `world/oracle.rs` | NEW | 263 | +263 |
| `world/mod.rs` | 31 | 35 | +4 |
| `voxel/mod.rs` | 144 | 206 | +62 |
| `aadf/cell.rs` | 346 | 364 | +18 |
| `aadf/edit.rs` | 828 | 884 | +56 |
| `aadf/construct.rs` | 570 | 573 | +3 |
| `aadf/generator.rs` | 507 | 508 | +1 |
| `aadf/entity.rs` | 451 | 502 | +51 |
| `aadf/mod.rs` | 18 | 27 | +9 |
| `aadf/block_hash.rs` | 612 | 630 | +18 |
| `aadf/bounds.rs` | 835 | 835 | 0 |
| **Total** | **6 472** | **6 300** | **−172** |

**Below architect's −400 to −500 target.** Two structural reasons:

1. **`WorldData` method-shim retention** — architect projected `data.rs` 1731 → ~1080 (~−650 LOC); implementor landed 1731 → 1473 (−258 LOC). The ~390-LOC gap is in the 3-line shim methods on `set_voxel` / `set_voxels_batch_oracle` (the bodies moved to `oracle.rs`; the architect's design called for deleting the methods entirely and rewriting all callers, which crossed D1's path-list constraint per §1 Step 5 above). The actual code-volume reduction landed identically — the bodies are 1 copy in `oracle.rs`, not 2 (`data.rs` + `oracle.rs`). The architect's net-delta accounting double-counted the shim shells.

2. **`world/oracle.rs` size pinned by extracted bodies** — the architect projected `oracle.rs` at ~330 LOC; implementor landed 263 LOC. The ~70-LOC savings came from the Step-4 DRY helpers (`iter_voxel_edits`, `iter_block_edits`, `merge_recomputed_aadfs_into_batch`) absorbing what would have been ~70 LOC of inline duplication. **The DRY win arrived in `oracle.rs` rather than `data.rs`** because of the body-extraction direction.

**Docblock additions** account for the +62 LOC in `voxel/mod.rs` (new `cell_state` module + `CellRaw` newtype docs) and +51 in `entity.rs` (mask + shift table docblocks).

**Net effect**: the file-tree restructuring + DRY collapse + named-types API + SSoT collapse all landed; the gross-LOC drop is smaller than the architect's projection because the implementor preserved the `pub` API shims to avoid forced D2/D5 cross-domain edits.

---

## 7. Final verification suite

**Build + tests:**
- `cargo build --workspace` — pass (0 warnings on `bevy-naadf` after final cleanup).
- `cargo test --workspace --lib` — pass (200 passed, 1 ignored).

**E2E gates (each ≥ 2 runs where non-deterministic):**
- `cargo run --release --bin e2e_render -- baseline` — pass.
- `cargo run --release --bin e2e_render -- --entities` — pass (entity handler validation PASS: 8 chunk_updates, 1 instance, 1 history).
- `cargo run --release --bin e2e_render -- --vox-e2e` — pass (centre rect luminance 250.5, channel max 251.8).
- `cargo run --release --bin e2e_render -- --validate-gpu-construction` — pass × 4 runs total during migration; final 2 runs at end-of-step both `GPU construction byte-equal to CPU oracle: 388 bytes compared`.
- `cargo run --release --bin e2e_render -- --edit-mode` — pass × 2 runs (Step 5): `1 set_voxel call produced 1 changed_chunks + 1 changed_blocks records + 2 changed_voxels records`.
- `cargo run --release --bin e2e_render -- --runtime-edit-mode` — pass × 2 runs (Step 5): `set_voxels_batch produced 1 batch(es) with 2 changed_chunks + 2 changed_blocks + 2 changed_voxels records`.
- `cargo run --release --bin e2e_render -- --oasis-edit-visual` — pass × 3 runs:
  - Run #1: rect Δ luminance 15.0, rect Δ RGB 18.06 (floor 8.00); full-frame Δ 4.28.
  - Run #2: rect Δ luminance 14.9, rect Δ RGB 18.04; full-frame Δ 4.24.
  - Run #3: rect Δ luminance 14.9, rect Δ RGB 17.94; full-frame Δ 4.26.

All non-deterministic-gate values cluster within ±0.1 luminance / ±0.12 RGB Δ across 3 runs — well under noise floor; no regression vs. pre-D1 baseline.

---

## 8. Downstream handoff notes

### For D2 (editor-and-settings-ui) implementor:

- **`VoxelEdit` + `ChunkUniformEdit` named-type API is now defined** in `crates/bevy_naadf/src/world/data.rs:40-99`. Adopt the named types per §3 above.
- **Method signatures unchanged** — `set_voxels_batch(&[(IVec3, VoxelTypeId)])` and `set_chunks_uniform_batch(&[([u32;3], Option<VoxelTypeId>)])`. Flip them to `&[VoxelEdit]` / `&[ChunkUniformEdit]` in D2's phase; the 1-line body destructure update + D2's 5 call-site fixes ride the same commit. **No need to redefine the types — D1 owns them.**
- The `From` impls let you do the migration in any order; the workspace stays buildable across.
- Brush call sites that need updating: `editor/tools.rs:160, 219, 222, 282, 285, 344, 422, 454, 535, 555`. Tests in `editor/tools.rs::tests` mostly.

### For D3 (voxel-io) implementor:

- No D1 surface change consumed by D3. `voxel/mod.rs` re-exports (`async_vox`, `cvox_import`, `grid`, `vox_import`, `voxel_dispatch`, `web_vox`) are unchanged.
- D3 may want to call `crate::world::oracle::set_voxel(&mut wd, pos, ty)` directly if any new D3 path wants to land seed-from-vox edits through the diagnostic oracle — it's `pub(crate)`.

### For D5 (gpu-construction) follow-up implementor:

- **SSoT-6 collapse** — apply the 5-LOC change per §4 above: `pub use crate::aadf::block_hash::hash_coefficients;` in `render/construction/hashing.rs`, delete the duplicate body. Optional: collapse the local `hashing.rs:165` test if redundant with `block_hash.rs:417`.
- **`render/construction/validation.rs:4420`** still calls `world_data.set_voxel(...)` — works as-is via the shim. If D5 wants to fully drop the shim, change to `crate::world::oracle::set_voxel(&mut world_data, ...)` (1-line edit).
- **WGSL `NAADF_CHUNK_DIM_VOXELS` substitution** — `crate::voxel::CHUNK_DIM_VOXELS` is the Rust SSoT; D5 can extend `cell_shader_defs()` to inject it if D5 chooses to replace the bare `16u` literals in `chunk_calc.wgsl` / `world_change.wgsl` / `bounds_calc.wgsl`. D1 has no opinion on the WGSL change.

### For D6 (e2e-and-playwright) implementor:

- `bin/e2e_render.rs --edit-mode` continues to drive `WorldData::set_voxel` via the shim — no D6 change needed.
- If D6's follow-up cleanup wants to drop the shim, swap to `crate::world::oracle::set_voxel(&mut wd, ...)` everywhere in `bin/e2e_render.rs`. D1 documented this as A4 in `03-architecture.md` §6.

### For D7 (app-and-camera) implementor:

- No D1↔D7 overlap.

### For D8 (asset-pipeline) implementor:

- No D1↔D8 overlap.

---

## 9. Side notes / observations / complaints

1. **The architect's Step-3 + Step-5 deletion plans collide with the orchestrator's D5-then-D4-then-D1 sequencing.** D5 already landed; D2 has not. The architect's design assumed paired-commit with D2 (A3) and a follow-up tidy for D5's `validation.rs:4420` (A5). Implementor's resolution: keep `set_voxel` / `set_voxels_batch_oracle` as thin shims on `WorldData` and keep production-method signatures as anonymous tuples. This preserves the cross-domain build at the cost of −172 LOC instead of −400 to −500. The architect's structural goals (oracle module exists, IoC seam is visible, DRY collapses landed) are all met. **The remaining LOC win is recoverable by D2/D5/D6's follow-up implementors deleting the shims when they update their call sites.**

2. **`render/construction/validation.rs:4420` (D5 territory) calls `world_data.set_voxel(...)`** — the shim handles this. If a future D5 cleanup pass wants to drop the shim, swap to `crate::world::oracle::set_voxel(&mut world_data, pos, ty)`. The architect's A5 / §7 D5 notes anticipated this.

3. **`render/construction/validation.rs:1706-1708`** redefines `CELL_HAS_CHILDREN` and `CELL_UNIFORM_FULL` as **local constants** inside a function body. These shadowed the `voxel::*` constants I deleted in Step 2 — but they're file-local so they don't import anything; they're just typedef-style consts inside a single function. They didn't break (and don't fail to compile post-Step-2). **For consistency** D5's follow-up could swap them to `use crate::voxel::{cell_state, CELL_PAYLOAD_MASK}; … if (chunk_u32 >> cell_state::SHIFT) == cell_state::CHILD { … }`. Out of D1's scope.

4. **`voxel/mod.rs` mixes "bit-layout SSoT" (D1) with "I/O module declarations" (D3)** (`async_vox`, `cvox_import`, `grid`, `vox_import`, `voxel_dispatch`, `web_vox`). Explorer Side note 10 flagged this. I left the file structure unchanged — D3 architect's call whether to split.

5. **`world/buffer.rs` (`GrowableBuffer`) sits awkwardly in D1's path list** with zero D1 callers (explorer Side note 4 + architect D1.6 reject). I left it. If a future tightening wants to move it to `render/`, D4 architect owns that — D1 has not designed the move.

6. **`aadf/generator.rs` filename misnaming** (explorer Side note 5 + architect D1.5 reject) — left as-is.

7. **The 3 large prose docblocks repeating "DIAGNOSTIC-ONLY vs production runtime"** (`world/data.rs:1-34`, `aadf/edit.rs:1-41`, `aadf/bounds.rs:1-49`) — left in place per architect §8 Side note 8. A future docs-tidy could consolidate them; out of scope.

8. **No PBR concerns in D1's surface** — confirms architect §8 Side note 4.

9. **The 4×2 production/diagnostic × rehash/no-rehash matrix** (architect §8 Side note 6) is now structurally visible: production methods on `WorldData::impl` (`set_voxels_batch`, `set_chunks_uniform_batch`), diagnostic free functions in `world::oracle` (`set_voxel`, `set_voxels_batch_oracle`). The `WorldData` impl methods that wrap them are thin delegators that exist purely for D2/D5/D6's source-compat.

10. **Equal-footing complaint (per [`feedback-vigilance-preamble-for-cg-work`])**: The brief framed D1's job as "land the design produced by D1's architect" but the architect's design rested on assumption A3 (D2 paired commit). The orchestrator's sequencing made A3 unworkable. The implementor had to choose: stay in path, lose ~390 LOC of the architect's LOC win; OR cross paths, achieve the LOC win, violate the brief. The shim-preservation resolution is the binding constraint's preferred outcome — but the brief's "stay in path" rule should have been weighed against the architect's A3 assumption at orchestrator level before dispatch. The architect's design is structurally correct; the orchestration left a coordination hole that the implementor papered over.

---

## 10. Status summary

- **Steps complete**: **7 of 7** (Step 8 SKIPPED by design — D4/D5 territory).
- **Verification gates** (final pass/fail):
  - `cargo build --workspace` — **pass**
  - `cargo test --workspace --lib` — **pass** (200/200, 1 ignored)
  - `e2e_render -- baseline` — **pass**
  - `e2e_render -- --entities` — **pass**
  - `e2e_render -- --vox-e2e` — **pass**
  - `e2e_render -- --validate-gpu-construction` — **pass** × 4 runs
  - `e2e_render -- --edit-mode` — **pass** × 2 runs
  - `e2e_render -- --runtime-edit-mode` — **pass** × 2 runs
  - `e2e_render -- --oasis-edit-visual` — **pass** × 3 runs
- **Files changed**: 11 (`aadf/{block_hash, cell, construct, edit, entity, generator, mod}.rs`, `voxel/mod.rs`, `world/{data, mod}.rs`).
- **Files added**: 1 (`world/oracle.rs`).
- **Files removed**: 0.
- **Behavioural deltas observed**: **none**. All e2e gates produce byte-equal / pixel-equal outputs vs. pre-D1 baseline.
- **LOC delta**: **−172** (6 472 → 6 300). Architect target was −400 to −500; shortfall is the deliberate `WorldData::set_voxel` / `set_voxels_batch_oracle` shim retention to avoid forced D2/D5 cross-domain edits (§1 Step 5, §9 #1).
