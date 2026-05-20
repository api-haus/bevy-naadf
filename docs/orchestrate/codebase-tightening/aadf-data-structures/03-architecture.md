# D1 — aadf-data-structures · refactor-architect findings (2026-05-20)

**Domain:** AADF cell encoding + CPU AADF computation + the `WorldData` container.
**Scope LOC:** 6 472 across 12 files.
**Orchestration:** `/delegate` codebase-tightening, D1 of 8.
**Predecessor:** [`02-exploration.md`](./02-exploration.md) (D1 explorer, 10 findings).

Implementor reads this file directly. Orchestrator does **not** read it (per user directive). All file:line citations below were re-verified with `Read`/`Grep` after re-reading the explorer's claims — I do not trust the explorer's prose alone.

---

## 1. Findings addressed

This design covers **all 10 D1 findings** (`02-exploration.md §Findings`):

| F# | Title | Addressed in |
|---|---|---|
| F1 | `WorldData` API conflates 4 set-voxel entry points across 2 regimes | §2.3 + §3 Steps 3, 5 |
| F2 | State-bit encoding implemented in two parallel regimes — **foundation rot** | §2.2 + §3 Steps 1, 2 |
| F3 | `recompute_chunk_layer_aadfs` post-amble duplicated verbatim in 2 methods | §2.5 + §3 Step 4 |
| F4 | Entity AADF kernel inlines paper §3.3 over a different bit layout | §2.6 (no-op — sanctioned C#-faithful divergence; tighten docs only) |
| F5 | Anonymous `(IVec3, VoxelTypeId)` and `([u32;3], ...)` tuples across edit API | §2.4 + §3 Steps 3, 5 (carries `UA-1`) |
| F6 | `CHUNK_DIM_VOXELS = 16` defined twice; bare `16`/`4` literals everywhere | §2.1 + §3 Step 1 (carries `SSoT-3`) |
| F7 | 9 inline chunk-pos pack/unpack sites bypassing `pack_chunk_pos` helper | §2.4 + §3 Steps 3, 4, 5 (carries `UA-3`) |
| F8 | Hash-coefficient table implemented twice in Rust | §2.7 + §3 Step 6 (carries `SSoT-6`) |
| F9 | `EditBatch` Vec<u32> walked with hand-rolled `chunks_exact(33|65)` everywhere | §2.4 + §3 Steps 3, 4 |
| F10 | `DIR_*` raw `usize` constants instead of `Dir6` enum | §2.6 (**reject** — keep `usize` for hot-loop codegen) |

Nothing in the brief asked us to skip — all 10 findings are addressed (some with explicit reject + reason).

**Side-finding from explorer that I act on:** `aadf/edit.rs:571-574` dead re-export (`cell_unpack_voxel` + the `_unpack_voxel` import-suppressor) → deleted in Step 7. The `let ptr_unused = edit_block[0]; let _ = ptr_unused;` dead-bind at `world/data.rs:316-317` is also deleted in Step 3.

**Side-finding I propose but flag as cross-domain:** `world/buffer.rs` relocation + `aadf/generator.rs` rename are in §6 "Side notes" — they're inside D1's path list but their callers all live in D4/D5; relocation requires D4 architect sign-off, not D1's unilateral move.

---

## 2. Target-state architecture

### 2.0 New / changed file layout

```
crates/bevy_naadf/src/
├── voxel/
│   └── mod.rs                              (~165 LOC, +20)
│       ├── existing: VoxelType / VoxelTypeId / MaterialBase / MaterialLayer / VOXEL_*
│       ├── NEW: pub mod cell_state { CHILD, UNIFORM_EMPTY, UNIFORM_FULL, SHIFT }
│       ├── NEW: pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM
│       └── KEEP (deprecated-aliased): CELL_HAS_CHILDREN / CELL_UNIFORM_FULL / CELL_PAYLOAD_MASK
├── aadf/
│   ├── mod.rs                              (~25 LOC, +5)
│   │   └── docblock now lists every submodule (was wrong — see explorer Side note 2)
│   ├── cell.rs                             (~360 LOC, +15)
│   │   └── encode/decode migrated to cell_state regime; ChunkRaw helpers added
│   ├── bounds.rs                           (unchanged surface, ~830 LOC)
│   ├── construct.rs                        (~555 LOC, -15)
│   │   └── CHUNK_DIM_VOXELS re-exported from voxel/mod.rs
│   ├── edit.rs                             (~870 LOC, +40)
│   │   ├── inline mask-triples replaced by unpack_chunk_pos/pack_chunk_pos
│   │   ├── EditBatch grows .iter_voxel_edits() / .iter_block_edits()
│   │   ├── NEW: pub(crate) fn merge_recomputed_aadfs_into_batch(...) — DRY helper
│   │   ├── dead re-export at :571-574 deleted
│   │   └── build_chunk_edit_window_solid_type moved behind #[cfg(test)]
│   ├── entity.rs                           (~455 LOC, +5)
│   │   └── 6-mask constant table extracted; algorithm body unchanged (F4 reject)
│   ├── generator.rs                        (~495 LOC, -10)
│   │   └── private CHUNK_DIM_VOXELS deleted (uses voxel::CHUNK_DIM_VOXELS)
│   ├── block_hash.rs                       (~615 LOC, +5)
│   │   └── build_polynomial_coefficients → pub fn hash_coefficients (renamed +
│   │     promoted, SSoT-6 winner)
│   └── NEW: oracle/                        (sibling submodule)
│       └── mod.rs                          (~360 LOC, extracted from data.rs)
│           ├── pub(crate) fn set_voxel_oracle(...)   — DIAGNOSTIC-ONLY
│           └── pub(crate) fn set_voxels_batch_oracle(...) — DIAGNOSTIC-ONLY
└── world/
    ├── mod.rs                              (~50 LOC, +20)
    │   └── pub mod oracle removed from runtime API exposure but registered
    │     pub(crate) so e2e gates / tests can reach via crate::world::oracle::*
    ├── data.rs                             (~1080 LOC, -650)
    │   ├── DELETE: set_voxel pub method body (moves to oracle::set_voxel_oracle)
    │   ├── DELETE: set_voxels_batch_oracle pub method body (moves to oracle module)
    │   ├── KEEP: set_voxels_batch (production runtime), set_chunks_uniform_batch
    │   ├── KEEP: ray_traversal, get_voxel_type, seed_block_hashing
    │   └── tests inside data.rs migrate to oracle path where appropriate
    └── buffer.rs                           (unchanged, ~399 LOC — see §6 side note)
```

**Net delta:** roughly **−400 to −500 LOC** across D1 (excluding test churn). Bulk of the drop is `data.rs` post-Finding 1 extraction (1731 → ~1080); `oracle/` mod absorbs ~360 LOC of that and the F3 DRY collapse saves ~50 LOC more.

---

### 2.1 Finding 6 + SSoT-3 — Promote `CHUNK_DIM_VOXELS` to a single Rust SSoT, expose to WGSL via shader-defs

**Current shape (verified):**
- `voxel/mod.rs:63-65` — `pub const CELL_DIM: usize = 4; pub const CELL_CHILDREN: usize = 64;` (these are the existing SSoT).
- `aadf/construct.rs:29` — `pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;` (D1-internal, public).
- `aadf/generator.rs:51` — `const CHUNK_DIM_VOXELS: u32 = (CELL_DIM * CELL_DIM) as u32;` (**private, different type**).
- Bare `16` and `4` integer literals: verified 25+ sites in `world/data.rs` (e.g. `:493-497`, `:520-525`, `:646-670`); ~10 in `aadf/edit.rs` (e.g. `:444-446`); ~8 in `aadf/generator.rs` (e.g. `:167-179`, `:269-276`, `:425-428`).

**Target shape:**
```rust
// voxel/mod.rs — the SSoT-3 home
/// Side length of a cell in cells of the layer below — every NAADF layer is a
/// 4×4×4 grid of the layer beneath it (paper §3.1).
pub const CELL_DIM: usize = 4;
/// Child cells per cell (`CELL_DIM³ = 64`).
pub const CELL_CHILDREN: usize = CELL_DIM * CELL_DIM * CELL_DIM;
/// Side length of a **chunk** in voxels (`CELL_DIM² = 16`). Single Rust SSoT —
/// every D1 file derives this; WGSL receives it via shader-defs (SSoT-3).
pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;
/// Voxels per chunk (`CHUNK_DIM_VOXELS³ = 4096` — same as 64 blocks × 64 voxels/block).
pub const CHUNK_VOLUME_VOXELS: usize = CHUNK_DIM_VOXELS * CHUNK_DIM_VOXELS * CHUNK_DIM_VOXELS;
```

The `aadf/construct.rs:29` declaration becomes a one-line re-export `pub use crate::voxel::CHUNK_DIM_VOXELS;` for backward source-compat — its 9 in-crate callers don't need to change their `use` line. The private duplicate at `aadf/generator.rs:51` is **deleted**; generator.rs imports `crate::voxel::CHUNK_DIM_VOXELS` and casts to `u32` at use sites.

**WGSL coordination (D4/D5):** D5 architect is responsible for landing the shader-def upload — D1's job is to expose the Rust constant. The proposed seam is to extend `pipelines.rs:278-279`'s existing `ShaderDefVal::UInt("TAA_SAMPLE_RING_DEPTH", ...)` pattern with:

```rust
// Hypothetical addition to render/pipelines.rs (D5 architect lands this)
shader_defs: vec![
    ShaderDefVal::UInt("CELL_DIM".into(), crate::voxel::CELL_DIM as u32),
    ShaderDefVal::UInt("CHUNK_DIM_VOXELS".into(), crate::voxel::CHUNK_DIM_VOXELS as u32),
    ShaderDefVal::UInt("CELL_CHILDREN".into(), crate::voxel::CELL_CHILDREN as u32),
],
```

The shader files then use `#{CHUNK_DIM_VOXELS}` (Bevy preprocessor) or `const CHUNK_DIM_VOXELS: u32 = #{CHUNK_DIM_VOXELS}u;` at the top of each `.wgsl`. **D5 owns this**; D1 only requires the consumer side exists. Until D5 lands its half, D1's refactor is non-breaking — the bare `16u`/`4u` literals in WGSL stay correct because `CELL_DIM` is constant-by-paper-decree.

**Reuse choices:**
- The existing `CELL_DIM` / `CELL_CHILDREN` SSoT proves the pattern works. Adding `CHUNK_DIM_VOXELS` to the same file is the lowest-friction extension.
- Bevy's `ShaderDefVal::UInt` plumbing already exists at `pipelines.rs:53,278-279` — D5 reuses, doesn't invent.

**Behavioural delta:** None — pure structural refactor. All replaced `16`/`4` literals were already equal to the constants by construction.

---

### 2.2 Finding 2 — State-bit encoding regime decision (foundation rot resolution)

**Current shape (verified):**

Two regimes coexist for the chunk/block `u32`'s top 2 bits:

- **Regime A (Rust `aadf/cell.rs` encode/decode only):** Two independent flag bits.
  - `voxel/mod.rs:29` — `pub const CELL_HAS_CHILDREN: u32 = 1 << 31;`
  - `voxel/mod.rs:33` — `pub const CELL_UNIFORM_FULL: u32 = 1 << 30;`
  - `aadf/cell.rs:122-139` (`ChunkCell::encode`/`decode`), `:147-164` (`BlockCell`).

- **Regime B (everywhere else — including all WGSL shaders):** A 2-bit state nibble at bits 30-31.
  - WGSL: `assets/shaders/chunk_calc.wgsl:246-248`, `world_change.wgsl:161-163`, `bounds_calc.wgsl:180-182` define:
    ```wgsl
    const BLOCK_STATE_CHILD: u32 = 2u;
    const BLOCK_STATE_UNIFORM_EMPTY: u32 = 0u;
    const BLOCK_STATE_UNIFORM_FULL: u32 = 1u;
    ```
  - Rust regime-B sites (verified via `Grep` for `>> 30`):
    - `world/data.rs:134,145,517-518,542-543,637,659,663,888,964,1031,1061-1062,1143-1144` — ~14 sites.
    - `aadf/edit.rs:101,116,143,295-307,381-414,538-540,555,706,777` — ~10 sites.
    - `aadf/generator.rs:105,164,174,193,197` — ~5 sites.
    - Inline literal `(2u32 << 30)` / `(1u32 << 30)` / `(2u << 30u)` at `world/data.rs:944,1031,1129`, `aadf/edit.rs:306,328`, etc.

**C# canon (faithful-port verdict):** I read `/mnt/archive4/DEV/NAADF/NAADF/World/Data/{ChangeHandler,WorldData,EditingHandler,EntityHandler}.cs` end-to-end. **C# uses regime B exclusively:**

- `WorldData.cs:223` (`FillChunkData`): `uint chunkState = chunk >> 30; if (chunkState != 2) { uint type = chunkState == 1 ? ... : 0u; }` — canonical 2-bit discriminator.
- `WorldData.cs:236-238,263-275,334,388-392` — every block/chunk classification reads `>> 30` and compares to `0`/`1`/`2`.
- `EditingHandler.cs:99-100`: `newBlocks[b] = firstVoxelType | ((firstVoxelType == 0 ? 0u : 1u) << 30);` — state 0 (empty) or state 1 (uniform-full) at bits 30-31.
- `EditingHandler.cs:119`: `newBlocks[b] = (pointer) | (2u << 30);` — state 2 (mixed) at bits 30-31.
- `EditingHandler.cs:127,133`: `if ((curChunk >> 30) == 2)` / `if ((oldBlock >> 30) == 2)` — state 2 discriminator on read.

The only C# sites that use regime-A-style shortcuts are inside `WorldData.cs::RayTraversal` (lines 435, 443, 449, 456 — `>> 31`, `& 0x40000000`, `& 0x8000`). Those are read-only shortcuts that are *consistent with* regime B (state 2 = `0b10` has bit 31 set, state 1 = `0b01` has bit 30 set) — they're optimisations on top of regime B, not a separate encoding.

**Verdict — faithful-port rule decides:** **Regime B is canonical**. The Rust port's regime-A in `aadf/cell.rs` is the deviation; it produces the same bit patterns but obscures the C# vocabulary. The fix: migrate `aadf/cell.rs::encode`/`decode` to regime B; deprecate `CELL_HAS_CHILDREN`/`CELL_UNIFORM_FULL` (keep as thin aliases for one release in case downstream consumers exist outside D1, then delete in a follow-up if no callers).

**Target shape:**
```rust
// voxel/mod.rs — new submodule mirroring WGSL constants byte-for-byte
pub mod cell_state {
    //! Cell-state nibble at bits 30-31 of a `Chunk`/`Block` `u32`.
    //!
    //! Faithful port of C# `WorldData.cs:223` `chunk >> 30` discriminator.
    //! Mirrors the WGSL `BLOCK_STATE_*` constants in `chunk_calc.wgsl:246-248`,
    //! `world_change.wgsl:161-163`, `bounds_calc.wgsl:180-182`. State 3 is
    //! reserved / unused (C# never emits it).

    /// State value for **empty** cells — bits 30-31 = `0b00`. Low 30 bits carry
    /// the AADF (6-direction empty distance — 5-bit fields at chunk layer,
    /// 2-bit at block/voxel layer).
    pub const UNIFORM_EMPTY: u32 = 0;
    /// State value for **uniform-full** cells — bits 30-31 = `0b01`. Low 15
    /// bits carry the voxel type id.
    pub const UNIFORM_FULL: u32 = 1;
    /// State value for **mixed** cells — bits 30-31 = `0b10`. Low 30 bits carry
    /// the child-group pointer (block ptr at chunk layer, voxel ptr at block
    /// layer).
    pub const CHILD: u32 = 2;

    /// Bit shift applied to extract `state` from a raw cell word
    /// (`raw >> SHIFT`). C# `chunk >> 30`.
    pub const SHIFT: u32 = 30;
}

/// 30-bit payload mask (`raw & PAYLOAD_MASK`) — the AADF when empty, child-ptr
/// when mixed, or 15-bit voxel type when uniform-full. C# `& 0x3FFFFFFF`.
pub const CELL_PAYLOAD_MASK: u32 = 0x3FFF_FFFF;

// Kept for one release as deprecated aliases — only `aadf/cell.rs` consumed them
// and that file migrates in Step 2 of the migration plan. After the migration
// passes a green test cycle, delete these.
#[deprecated(note = "use crate::voxel::cell_state::CHILD with the SHIFT — regime B")]
pub const CELL_HAS_CHILDREN: u32 = (cell_state::CHILD as u32) << cell_state::SHIFT;
#[deprecated(note = "use crate::voxel::cell_state::UNIFORM_FULL with the SHIFT — regime B")]
pub const CELL_UNIFORM_FULL: u32 = (cell_state::UNIFORM_FULL as u32) << cell_state::SHIFT;
```

```rust
// voxel/mod.rs — small typed-newtype helper to collapse the ~30 inline mask sites
/// Typed view over a chunk/block cell `u32` word — the regime-B state nibble +
/// 30-bit payload. Same idiom as the existing `pack_chunk_pos`/`unpack_chunk_pos`
/// helpers in `aadf::edit`.
///
/// Used at hot decode sites that want to avoid the `Cell::decode` enum branch
/// (e.g. `WorldData::ray_traversal`'s 3-layer descent, `set_voxels_batch`'s
/// stage-A/B/C loop). Zero-cost — `repr(transparent)` over a `u32`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct CellRaw(pub u32);

impl CellRaw {
    /// Extract the 2-bit state nibble (one of [`cell_state::UNIFORM_EMPTY`],
    /// [`cell_state::UNIFORM_FULL`], [`cell_state::CHILD`]).
    #[inline]
    pub const fn state(self) -> u32 { self.0 >> cell_state::SHIFT }
    /// Extract the 30-bit payload (AADF / child-ptr / 15-bit type).
    #[inline]
    pub const fn payload(self) -> u32 { self.0 & CELL_PAYLOAD_MASK }
    /// Construct a raw word from a state value and 30-bit payload.
    #[inline]
    pub const fn new(state: u32, payload: u32) -> Self {
        Self((state << cell_state::SHIFT) | (payload & CELL_PAYLOAD_MASK))
    }
    /// True iff `state() == cell_state::CHILD`.
    #[inline]
    pub const fn is_child(self) -> bool { self.state() == cell_state::CHILD }
    /// True iff `state() == cell_state::UNIFORM_EMPTY`.
    #[inline]
    pub const fn is_empty(self) -> bool { self.state() == cell_state::UNIFORM_EMPTY }
}
```

`aadf/cell.rs::ChunkCell::encode` / `BlockCell::encode` are rewritten in regime-B form:

```rust
// aadf/cell.rs — migrated
impl ChunkCell {
    pub fn encode(self) -> u32 {
        match self {
            ChunkCell::Empty(aadf)        => CellRaw::new(cell_state::UNIFORM_EMPTY,
                                                          aadf.pack(AADF_BITS_CHUNK, AADF_MAX_CHUNK)).0,
            ChunkCell::UniformFull(ty)    => CellRaw::new(cell_state::UNIFORM_FULL, ty.raw() as u32).0,
            ChunkCell::Mixed(ptr)         => CellRaw::new(cell_state::CHILD, ptr.0).0,
        }
    }
    pub fn decode(raw: u32) -> ChunkCell {
        let cr = CellRaw(raw);
        match cr.state() {
            cell_state::CHILD          => ChunkCell::Mixed(BlockPtr(cr.payload())),
            cell_state::UNIFORM_FULL   => ChunkCell::UniformFull(VoxelTypeId((cr.payload() as u16) & VOXEL_PAYLOAD_MASK)),
            _                          => ChunkCell::Empty(Aadf6::unpack(raw, AADF_BITS_CHUNK)),
        }
    }
}
```

The hot-path callers (`world/data.rs::ray_traversal:517`, `:542`, `:637-664`, `set_voxels_batch:887-893,964`) get `CellRaw(raw).state()` / `.payload()` substitutions for the inline `>> 30` / `& 0x3FFF_FFFF` triples. Mass replace — verified via Grep that no exotic uses (rotations, signed shifts, etc) exist on these bits.

**Reuse choices:**
- WGSL's `BLOCK_STATE_*` naming is **the source vocabulary**. Rust adopts identical naming (`cell_state::CHILD`, `UNIFORM_EMPTY`, `UNIFORM_FULL`) so cross-side grep yields hits in both files.
- `CellRaw` is the same `repr(transparent)` newtype idiom as `BlockPtr`/`VoxelPtr` at `aadf/cell.rs:73-83` — no new pattern introduced.
- Deprecated aliases ride one release before deletion — gives D5 architect time to find any cross-domain consumers (`render/construction/world_change.rs` `#[cfg(test)] use crate::aadf::edit::...` callsites — see D5's territory).

**Behavioural delta:** **None — bit-identical output.** Both regimes produced the same `u32` patterns; the change is naming + access vocabulary, not bit layout. Encode/decode round-trip tests at `aadf/cell.rs:223-300` pass unchanged. The W2 GPU-shader byte-equality oracle is preserved.

---

### 2.3 Finding 1 — Extract diagnostic-only set-voxel paths from `WorldData`'s public API

**Current shape (verified):**
- `world/data.rs:235-398` — `#[doc(hidden)] pub fn set_voxel(...)`. **164 lines.** DIAGNOSTIC-ONLY (docblock at `:19-34, :206-233`).
- `world/data.rs:1163-1342` — `#[doc(hidden)] pub fn set_voxels_batch_oracle(...)`. **~180 lines.** DIAGNOSTIC-ONLY.
- `world/data.rs:721-1080` — `pub fn set_voxels_batch(...)`. **~360 lines.** Production runtime fast path.
- `world/data.rs:1099-1161` — `pub fn set_chunks_uniform_batch(...)`. **~63 lines.** Production brush inside-chunk fast path.

Callers of the diagnostic methods (verified by Grep):
- `world/data.rs::tests::*` (lines 1457, 1517, 1531, 1556, 1577) — 5 in-file tests.
- `world/data.rs::tests` calls `set_voxels_batch_oracle` only at `:1556`.
- `bin/e2e_render.rs` `--edit-mode` dispatch (D6 territory, but it imports via crate, so a `pub(crate)` API works).
- The exploration says `render/construction/mod.rs:9207,:9350,:9416` calls `set_voxel`/`set_voxels_batch` from D5 e2e gate bodies (`pub(crate)` reaches them).

**Target shape:**

Move both diagnostic methods into a new sibling module `crates/bevy_naadf/src/world/oracle.rs`, exposed as `pub(crate) mod oracle` in `crates/bevy_naadf/src/world/mod.rs`. The methods become free functions taking `&mut WorldData` instead of `&mut self`:

```rust
// world/oracle.rs — DIAGNOSTIC-ONLY edit paths
//!
//! `set_voxel` + `set_voxels_batch_oracle` extracted from `WorldData`'s public
//! API per the `/delegate` codebase-tightening D1 architect's Finding 1.
//!
//! These run the whole-world AADF rehash (`recompute_chunk_layer_aadfs`) +
//! emit synthetic chunk uploads. O(N_chunks × 31 × 3) per call. **Production
//! code paths NEVER call this module** — see `WorldData::set_voxels_batch`
//! / `set_chunks_uniform_batch` for the runtime fast paths.
//!
//! Call sites: `--edit-mode` e2e gate, unit tests in `world/data.rs::tests`,
//! `D5 render/construction/mod.rs` e2e gate fixtures.

use bevy::math::IVec3;
use crate::world::data::WorldData;
use crate::voxel::VoxelTypeId;

/// DIAGNOSTIC-ONLY single-voxel edit. Runs whole-world AADF rehash.
pub(crate) fn set_voxel(world: &mut WorldData, pos: IVec3, ty: VoxelTypeId) { /* ... */ }

/// DIAGNOSTIC-ONLY bulk-edit with whole-world AADF rehash + synthetic-entry
/// emission — the slow-but-bit-exact pre-`02c` behaviour.
pub(crate) fn set_voxels_batch(world: &mut WorldData, edits: &[VoxelEdit]) { /* ... */ }
```

The `world/mod.rs` exposes it crate-private:
```rust
// world/mod.rs — D1 wiring seam
pub mod data;
pub mod buffer;
pub(crate) mod oracle;  // DIAGNOSTIC-ONLY — see oracle/mod.rs docblock
```

Test-side callers update from `wd.set_voxel(pos, ty)` to `crate::world::oracle::set_voxel(&mut wd, pos, ty)`. D5 e2e fixture callers update identically (the `set_voxels_batch` calls at `render/construction/mod.rs:9207` etc. that hit the **production** path keep using the method — only diagnostic-call sites move).

**Reuse choices:**
- Free-function-on-`&mut WorldData` matches the existing `aadf::edit::process_edit_batch(...)` / `recompute_chunk_layer_aadfs(...)` style — sibling helpers that mutate `WorldData` without being methods on it.
- `pub(crate)` visibility is the existing seam Bevy uses for plugin-internal API (cf. `aadf/construct.rs:115-131 BlockClass/ChunkClass` already `pub(crate)`).

**Behavioural delta:** None for callers that update imports. **The `pub` surface of `WorldData` shrinks from 7 methods to 5** (`Default::default`, `seed_block_hashing`, `ray_traversal`, `get_voxel_type`, `set_voxels_batch`, `set_chunks_uniform_batch` — the diagnostic two are extracted). IDE auto-complete on `WorldData` no longer surfaces the diagnostic methods. The `#[doc(hidden)]` becomes structural rather than declarative.

**Test migration:** the 5 in-`data.rs` tests that call `set_voxel`/`set_voxels_batch_oracle` either (a) update their call sites to `crate::world::oracle::set_voxel(&mut wd, pos, ty)`, or (b) move into `crates/bevy_naadf/src/world/oracle.rs` as `#[cfg(test)] mod tests { ... }` if they're testing diagnostic-only semantics. The 4 ray-traversal tests at `data.rs:1431-1587` use `set_voxel` only to *seed* the world — those stay in `data.rs::tests` and update to `oracle::set_voxel(&mut wd, ...)`.

---

### 2.4 Findings 5, 7, 9 — Named edit types + helper substitution + EditBatch iter methods

**Current shape (verified):**
- F5 — `world/data.rs:721,1099,1181` use anonymous tuples in `pub` API:
  - `pub fn set_voxels_batch(edits: &[(IVec3, VoxelTypeId)])`
  - `pub fn set_chunks_uniform_batch(chunks: &[([u32; 3], Option<VoxelTypeId>)])`
  - `pub fn set_voxels_batch_oracle(edits: &[(IVec3, VoxelTypeId)])`
- F5 callers in D2 (`editor/tools.rs`) — explorer counts ~11 sites; tests at `world/data.rs:1447-1450,1599-1601,1638` use the same tuple shape.
- F7 — inline pack/unpack of chunk-pos at `world/data.rs:330-335,363-368,1278-1285,1307-1314`, `aadf/edit.rs:67-69`. 9 sites total in D1.
- F9 — hand-rolled `chunks_exact(33|65)` loops at `world/data.rs:295-312,304-324,1261-1276`, plus the `for entry in &batch.changed_chunks { ... unpack chunk_pos ... }` pattern at `:327-339,:1277-1289,:1305-1331`.

**Target shape:**

**Named types (F5 / UA-1):**
```rust
// world/data.rs (or a sibling world/edit_types.rs if data.rs is too crowded —
// architect leans toward keeping these where the consumers live = data.rs)

/// A single-voxel edit (replaces the anonymous `(IVec3, VoxelTypeId)` tuple).
/// One of these per voxel a brush touches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VoxelEdit {
    /// World-space voxel position. Negative components are silently dropped.
    pub pos: IVec3,
    /// Target voxel type. `VoxelTypeId::EMPTY` clears the voxel.
    pub ty: VoxelTypeId,
}

/// A whole-chunk uniform-state edit (replaces `([u32; 3], Option<VoxelTypeId>)`).
/// Used by the brush inside-chunk fast path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkUniformEdit {
    /// Chunk position in chunk-space.
    pub pos: UVec3,
    /// `Some(ty)` for `UniformFull(ty)`; `None` (or `Some(EMPTY)`) for Empty.
    pub ty: Option<VoxelTypeId>,
}

impl WorldData {
    pub fn set_voxels_batch(&mut self, edits: &[VoxelEdit]) { /* ... */ }
    pub fn set_chunks_uniform_batch(&mut self, chunks: &[ChunkUniformEdit]) { /* ... */ }
}
```

**D2 coordination notes:** D2's architect will see the brief carry `UA-1` — the design above defines the public type; D2 consumes by changing `editor/tools.rs:139,153,...` from `(pos, ty)` tuples to `VoxelEdit { pos, ty }` literals. Type definition lives in D1; type usage is D2's refactor. D2's architect should NOT redefine `VoxelEdit` in `editor/`.

**Internal tuples** (`world/data.rs:737-738,771,774` — `HashMap<[u32; 3], Vec<([u32; 3], u16)>>` etc.) stay anonymous — explorer's recommendation (low blast-radius) is the right call. The win is at the API boundary, not the implementation interior.

**Inline pack/unpack helpers (F7 / UA-3):**

Replace every inline `(pos_packed & 0x7FF, (>> 11) & 0x3FF, >> 21)` triple with `crate::aadf::edit::unpack_chunk_pos(packed)`. Replace every inline `cx | (cy << 11) | (cz << 21)` with `pack_chunk_pos([cx, cy, cz])`. Rote substitution; helpers already exist. The 9 D1 sites listed above all migrate.

For ergonomic destructure, an optional convenience:
```rust
// aadf/edit.rs — addition (~6 LOC)
impl EditBatch {
    /// Iterate `(chunk_pos, new_state)` decoded — replaces inline-unpack loops.
    pub fn iter_chunks(&self) -> impl Iterator<Item = ([u32; 3], u32)> + '_ {
        self.changed_chunks.iter().map(|entry| (unpack_chunk_pos(entry[0]), entry[1]))
    }
}
```

**EditBatch iter methods (F9):**

```rust
// aadf/edit.rs — addition (~30 LOC)
impl EditBatch {
    /// Iterate edited voxel groups in the wire format
    /// (`(pointer, &[u32; 32])` per edit — 33-u32 stride).
    pub fn iter_voxel_edits(&self) -> impl Iterator<Item = (u32, &[u32; 32])> + '_ {
        self.changed_voxels.chunks_exact(33).map(|c| {
            let arr: &[u32; 32] = (&c[1..33]).try_into().unwrap();
            (c[0], arr)
        })
    }
    /// Iterate edited block groups in the wire format
    /// (`(pointer, &[u32; 64])` per edit — 65-u32 stride).
    pub fn iter_block_edits(&self) -> impl Iterator<Item = (u32, &[u32; 64])> + '_ {
        self.changed_blocks.chunks_exact(65).map(|c| {
            let arr: &[u32; 64] = (&c[1..65]).try_into().unwrap();
            (c[0], arr)
        })
    }
}
```

The 4 hand-rolled `chunks_exact(33|65)` sites in `world/data.rs::set_voxel` and `set_voxels_batch_oracle` (which move into `world/oracle.rs` per §2.3) collapse to:
```rust
for (_ptr, voxels) in batch.iter_voxel_edits() {
    self.voxels_cpu.extend_from_slice(voxels);
}
for (idx, (_ptr, blocks)) in batch.iter_block_edits().enumerate() {
    let block_ptr = b_cursor + (idx as u32) * 64;
    crate::aadf::edit::apply_block_edit_cpu(&mut self.blocks_cpu, block_ptr, blocks);
}
```

**Reuse choices:**
- `pack_chunk_pos`/`unpack_chunk_pos` already exist at `aadf/edit.rs:203-214` — no new helper, just stop bypassing them.
- `EditBatch` iter methods are the idiomatic Rust shape (returning `&[u32; N]` array refs gives the implementor type-safe AADFs on the slice length). The implementor MUST use `try_into().unwrap()` (`chunks_exact(N)` guarantees length-N slices so the unwrap is provably safe — clippy will not flag).
- No new dependency.

**Behavioural delta:** None. Public API field names change (`VoxelEdit::pos` / `ty`) — that's a source-compat break that ripples to D2's editor module; the rename is mechanical (a `(p, t) → VoxelEdit { pos: p, ty: t }` substitution at every call site).

---

### 2.5 Finding 3 — Collapse the duplicate recompute post-amble

**Current shape (verified):**
- `world/data.rs:340-397` — `set_voxel` post-amble: `recompute_chunk_layer_aadfs` call + `already_in_batch` set construction + iteration over `aadf_changed` + `pack_chunk_pos` reconstruction + push to `batch.changed_chunks` + push to `pending_edits.edited_groups`.
- `world/data.rs:1294-1342` — `set_voxels_batch_oracle` post-amble: byte-equivalent logic. The only meaningful difference is the per-chunk loop at `:1334-1340` pushes `edited_groups` per edited chunk vs `set_voxel`'s single push at `:393-397`.

Both methods move into `world/oracle.rs` per §2.3. The DRY collapse is a free helper in `aadf/edit.rs` that **both** call.

**Target shape:**
```rust
// aadf/edit.rs — new pub(crate) helper (~50 LOC, replaces the two ~50-LOC blobs)
//
/// DIAGNOSTIC-ONLY — see `world::oracle` module docblock.
///
/// Runs `recompute_chunk_layer_aadfs` over `chunks_cpu` and stitches the
/// AADF-changed chunks into `batch.changed_chunks` (replacing entries already
/// in the batch's directly-edited-chunks list with the recomputed state, and
/// appending synthetic entries for indirectly-affected chunks).
///
/// `size_in_chunks` is the world dimensions — matches the public arg of
/// `recompute_chunk_layer_aadfs`.
#[doc(hidden)]
pub(crate) fn merge_recomputed_aadfs_into_batch(
    chunks_cpu: &mut [u32],
    size_in_chunks: [u32; 3],
    batch: &mut EditBatch,
) {
    let aadf_changed = recompute_chunk_layer_aadfs(chunks_cpu, size_in_chunks);
    let sx = size_in_chunks[0];
    let sy = size_in_chunks[1];
    let mut already_in_batch: std::collections::HashSet<usize> =
        std::collections::HashSet::with_capacity(batch.changed_chunks.len());

    for entry in batch.changed_chunks.iter_mut() {
        let pos = unpack_chunk_pos(entry[0]);
        let ci = (pos[0] + pos[1] * sx + pos[2] * sx * sy) as usize;
        if ci < chunks_cpu.len() {
            entry[1] = chunks_cpu[ci];
            already_in_batch.insert(ci);
        }
    }
    for ci in aadf_changed {
        if already_in_batch.contains(&ci) {
            continue;
        }
        let cz = (ci / (sx as usize * sy as usize)) as u32;
        let rem = ci % (sx as usize * sy as usize);
        let cy = (rem / sx as usize) as u32;
        let cx = (rem % sx as usize) as u32;
        batch.changed_chunks.push([pack_chunk_pos([cx, cy, cz]), chunks_cpu[ci]]);
    }
}
```

Both `world::oracle::set_voxel` and `world::oracle::set_voxels_batch` (post-extraction) call this single helper after producing their initial `batch`. Net code savings: ~50 LOC (one ~50-LOC copy collapses into one ~50-LOC helper plus two ~3-LOC call sites; the displaced second copy is gone).

**Reuse choices:**
- The helper lives in `aadf/edit.rs` because `recompute_chunk_layer_aadfs` lives there — same file, same module.
- Internally uses the F7-promoted `pack_chunk_pos`/`unpack_chunk_pos` helpers (drift-prevention SSoT).

**Behavioural delta:** None. Identical control flow; just one copy instead of two.

---

### 2.6 Findings 4 & 10 — Entity AADF kernel & Dir6 — explicit rejects

**Finding 4 — `EntityData::from_types` inline AADF kernel.**

The kernel at `aadf/entity.rs:144-222` (75-line per-axis loop body) inlines paper §3.3's neighbour-merge over an **in-place packed-`u32` voxel buffer** where AADFs live in the low 30 bits at shifts `0,5,10,15,20,25` and the full-cell flag is `0x80000000`. The kernel that `bounds.rs::compute_aadf_layer` ports produces a `Vec<Aadf6>` (decomposed per-axis distances) over a separate buffer with `is_empty` as a closure.

C# `EntityData.cs:64-105` keeps the inline form too (entity-specific bit layout — 5-bit AADFs to match the chunk-layer encoding, *not* the 2-bit block/voxel encoding). The faithful-port rule prefers the C# shape.

**Verdict: REJECT the kernel-share refactor**. The risk of breaking the W4 GPU oracle (which is bit-for-bit pinned against the inline kernel) outweighs the ~75-LOC duplication win. The shared-mask-table half of the proposal (extract 6 mask constants + 6 bit-shift offsets to a `pub(crate) const` table) **IS** worth doing — it's ~10 LOC of pure documentation. Land that. Leave the kernel body alone.

**Target shape (Step 8):**
```rust
// aadf/entity.rs — extract the 6-mask + 6-shift table to module-level consts
// (was lines 159-164 inside `from_types`).

/// 6-direction masks for the §3.3 neighbour-merge `addBounds` predicate
/// (`EntityData.cs:58-63`). Each bit `b` means "the bound in direction `b`
/// matches the neighbour's bound". The mask excludes the direction pointing
/// back toward us. Mirrors `boundsCommon.fxh::ComputeBounds4` masks (which
/// `aadf::bounds::compute_aadf_layer` consumes via `step_axis`'s `mask_neg`/
/// `mask_pos` args).
const ENTITY_AADF_MASKS: [u32; 6] = [
    0x3D, // -X (drop +X bit)  — same as bounds::step_axis(mask_neg) for axis=0
    0x3E, // +X (drop -X bit)
    0x37, // -Y
    0x3B, // +Y
    0x1F, // -Z
    0x2F, // +Z
];

/// Bit-shifts for the 5-bit-per-axis 6-direction AADF inside an
/// `EntityData::voxels` u32. C# `EntityData.cs:71,73,77,79,83,85` `<< 0..25`.
const ENTITY_AADF_BIT_SHIFTS: [u32; 6] = [0, 5, 10, 15, 20, 25];
```

Then the body at `:170-218` references those tables instead of the literal `0`/`5`/`10`/etc and the magic `0x3D..=0x2F`. The 31-iteration outer + 3-axis-inner loop body stays unchanged.

**Cross-check with `bounds::step_axis`:** the masks `0x3D, 0x3E, 0x37, 0x3B, 0x1F, 0x2F` are passed positionally at `aadf/bounds.rs:298-328` (`step_axis(..., 0x3D, 0x3E)` for axis 0). Those literals are good candidates for a follow-up to import `ENTITY_AADF_MASKS` and consume it from both files — but that's a `bounds.rs` change that touches its hot loop, and the kernel-share rejection logic above also applies. **Leave `bounds.rs`'s literals as inline numeric args to `step_axis` for now**; only the entity-side magic literals get extracted. Cross-file SSoT for these 6 masks would be a follow-up if the project later wants it; in scope it would be a fourth out-of-D1-scope cross-cut.

**Finding 10 — `DIR_NEG_X..DIR_POS_Z` as raw `usize`.**

The audit (`00-reuse-audit.md §3.5 UA-4`) explicitly tags this as "Low severity because it's used in tight inner loops where the indirection cost matters". Verified: `aadf/bounds.rs:298-323` (`step_axis` per-axis call) and `aadf/bounds.rs::bounds_match:417-428` are deeply hot.

I tested this in my head against `#[repr(u8)] enum Dir6 { ... }` with `#[inline] fn as_idx(self) -> usize { self as usize }` — the lowering is identical to a raw `usize` constant in release builds. **But** the explorer's note ("ensure inlining keeps the hot-loop codegen unchanged — sanity-check `compute_aadf_layer`'s assembly before/after") is real risk for a slot in the parallel /refactor where the implementor will not run cargo-asm.

**Verdict: REJECT the Dir6 enum migration**. Keep `DIR_*` constants as `pub const … : usize = …`. **Add** the `pub const DIRS: [usize; 6] = [DIR_NEG_X, DIR_POS_X, DIR_NEG_Y, DIR_POS_Y, DIR_NEG_Z, DIR_POS_Z];` array the explorer notes the test code at `aadf/cell.rs:284` already wants. ~3 LOC of typed-iteration win without touching hot loops.

```rust
// aadf/cell.rs — Step 8
pub const DIR_NEG_X: usize = 0;
pub const DIR_POS_X: usize = 1;
pub const DIR_NEG_Y: usize = 2;
pub const DIR_POS_Y: usize = 3;
pub const DIR_NEG_Z: usize = 4;
pub const DIR_POS_Z: usize = 5;

/// All 6 cardinal directions in canonical iteration order (-x,+x,-y,+y,-z,+z).
/// Use this for any `for dir in DIRS { … aadf.d[dir] … }` pattern; **inside
/// hot loops (compute_aadf_layer, bounds_match) the raw `usize` is preferred
/// to avoid potential dispatch overhead from a wrapper enum**.
pub const DIRS: [usize; 6] = [
    DIR_NEG_X, DIR_POS_X, DIR_NEG_Y, DIR_POS_Y, DIR_NEG_Z, DIR_POS_Z,
];
```

Cell.rs test at `:284` (`for dir in 0..6 { … }`) migrates to `for &dir in DIRS.iter() { … }` for the readability win that doesn't change anything.

---

### 2.7 Finding 8 — Hash-coefficient SSoT collapse

**Current shape (verified):**
- D1 owner: `aadf/block_hash.rs:395-404` — `fn build_polynomial_coefficients() -> [u32; 65]` (**file-private**). Consumed at `block_hash.rs:109`.
- D5 owner: `render/construction/hashing.rs:43-50` — `pub fn hash_coefficients() -> [u32; 65]`. Consumed at 9 sites in `render/construction/mod.rs` (`:1790, :4950, :5649, :6648, :7160, :7864, :8441, :9841` + entity-handler reference in docs at `:40`) plus a few tests.
- Both produce byte-identical output: `c[64] = 1; c[i] = c[i+1].wrapping_mul(31) for i in (0..64).rev()`.

**Target shape:**

**D1 owns the SSoT** (the AADF data layer is D1's domain; the GPU consumer is the consumer). Promote the D1 implementation:

```rust
// aadf/block_hash.rs — Step 6

/// Compute the 65-entry hash-coefficient table used by NAADF's voxel-block
/// hash (`BlockHashingHandler.cs:50-55` / `chunk_calc.wgsl:131-134`).
///
/// `c[64] = 1`; `c[i] = (c[i+1] * 31) mod 2^32` for `i = 63..0`. The hash of
/// a 64-voxel block is:
///
/// ```text
/// H = c[0] + Σᵢ c[i*2+1] * (v[i] & 0x7FFF)
///          + c[i*2+2] * ((v[i] >> 16) & 0x7FFF)
/// ```
///
/// where `v[i]` is the i-th `u32` of the 32-element packed-voxel block.
///
/// **Single Rust SSoT** (was previously implemented twice — see
/// `docs/orchestrate/codebase-tightening/00-reuse-audit.md §3.1 SSoT-6`).
/// `render::construction::hashing` re-exports this function so legacy callers
/// don't need to update imports.
pub fn hash_coefficients() -> [u32; 65] {
    let mut c = [0u32; 65];
    c[64] = 1;
    for i in (0..64).rev() {
        c[i] = c[i + 1].wrapping_mul(31);
    }
    c
}
```

The old name `build_polynomial_coefficients` is renamed (was file-private, no external callers); `BlockHashingHandler::new` at `block_hash.rs:109` calls `hash_coefficients()` directly.

**D5 architect note (coordination):** Replace the body of `render/construction/hashing.rs::hash_coefficients` (lines 43-50) with `pub use crate::aadf::block_hash::hash_coefficients;` — a thin re-export so the 9 `mod.rs` import sites continue to resolve unchanged. **D5's architect owns landing this re-export** (the file is in D5's path list); D1's design only requires that D5 agree to consume the D1 SSoT instead of duplicating. The duplicate function definition in `hashing.rs` is **deleted**.

**Reuse choices:**
- D1's `block_hash.rs` already had the algorithm; we promote the existing function rather than introducing a new home.
- The D5 file remains as a re-export shim — zero callers move.

**Behavioural delta:** None — byte-equal output, byte-equal caller behaviour.

---

## 3. Migration steps

Ordered, granular, each step independently buildable + green-on-tests. Verification gate per step: `cargo build --workspace` + `cargo test --workspace --lib` + the relevant `e2e_render` gate. **Always run gates twice** (per `feedback-multiple-runs-rule-out-false-positives`) for the non-deterministic ones (`--edit-mode`, `--oasis-edit-visual`).

---

#### Step 1 — Add `voxel::cell_state` module + `CHUNK_DIM_VOXELS` SSoT + `DIRS` table

**Edits:**
- `crates/bevy_naadf/src/voxel/mod.rs:23-38` — add the `pub mod cell_state { CHILD, UNIFORM_EMPTY, UNIFORM_FULL, SHIFT }` block. Reposition existing `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` to be `#[deprecated]` aliases derived from `cell_state::* << SHIFT` (preserves byte values).
- `crates/bevy_naadf/src/voxel/mod.rs:63-65` — add `pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;` and `pub const CHUNK_VOLUME_VOXELS: usize = CHUNK_DIM_VOXELS.pow(3);` (or `* * *` expansion).
- `crates/bevy_naadf/src/voxel/mod.rs` — add `pub struct CellRaw(pub u32)` newtype with `.state() / .payload() / .new() / .is_child() / .is_empty()` impls.
- `crates/bevy_naadf/src/aadf/cell.rs:28-33` — add `pub const DIRS: [usize; 6] = [DIR_NEG_X, DIR_POS_X, DIR_NEG_Y, DIR_POS_Y, DIR_NEG_Z, DIR_POS_Z];` (3 LOC).
- `crates/bevy_naadf/src/aadf/construct.rs:29` — change `pub const CHUNK_DIM_VOXELS: usize = CELL_DIM * CELL_DIM;` to `pub use crate::voxel::CHUNK_DIM_VOXELS;` (callers don't need to update their `use` lines).
- `crates/bevy_naadf/src/aadf/generator.rs:51` — delete the private redefinition; add `use crate::voxel::CHUNK_DIM_VOXELS;` and cast to `u32` at call sites (`CHUNK_DIM_VOXELS as u32`).

**Rationale:** Lay the foundation for Steps 2 + 3. Adding constants is non-breaking (the deprecated aliases keep existing call sites working until they're migrated in Step 2). `DIRS` is purely additive.

**Post-step state:** `cargo build` green. The deprecation warnings on `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` flag the future Step-2 migration sites (one per use). No call sites have changed yet.

**Verification:** Full gate suite. The deprecation warning count is the implementor's checklist for Step 2.

---

#### Step 2 — Migrate `aadf/cell.rs` to regime-B encode/decode

**Edits:**
- `crates/bevy_naadf/src/aadf/cell.rs:22-25` — replace the `use crate::voxel::{...}` import to drop `CELL_HAS_CHILDREN` and `CELL_UNIFORM_FULL`; add `cell_state` and `CellRaw`.
- `crates/bevy_naadf/src/aadf/cell.rs:118-189` — rewrite `ChunkCell::encode/decode`, `BlockCell::encode/decode` in regime-B form. `VoxelCell` stays as-is (voxel `u16` uses `VOXEL_FULL_FLAG` bit 15, a separate concern). Use `CellRaw::new(state, payload)` for encode and `CellRaw(raw).state()` switch for decode.
- `crates/bevy_naadf/src/aadf/cell.rs:120` — update the encode docblock that cites `bit 31 = has-children, bit 30 = uniform-full` to instead cite the regime-B vocabulary.
- `crates/bevy_naadf/src/voxel/mod.rs` — delete the deprecated `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` aliases once `cell.rs` no longer references them.

**Rationale:** Resolves Finding 2's foundation rot. `aadf/cell.rs` is now the only Rust file that *was* using regime A; after this migration it uses regime B, aligning with all other Rust + WGSL sites. The deprecation aliases get deleted in the same step — they were a build-bridge across steps.

**Post-step state:** `cargo build` green with zero `#[deprecated]` warnings (the aliases are gone). `cargo test --workspace --lib` green — the 7 round-trip tests at `aadf/cell.rs:223-300` exercise every encode/decode path and produce bit-identical `u32`s. Both regimes produce the same bit patterns; the migration is encoder-vocabulary-only.

**Verification:** Full gate suite. `cargo run --bin e2e_render -- --validate-gpu-construction` is the load-bearing gate (it pins the chunks/blocks/voxels byte layout against the GPU reference). Run twice.

---

#### Step 3 — Introduce `VoxelEdit` + `ChunkUniformEdit` types; migrate production set-voxel API + replace inline chunk-pos masks

**Edits:**
- `crates/bevy_naadf/src/world/data.rs` — define `pub struct VoxelEdit { pos: IVec3, ty: VoxelTypeId }` and `pub struct ChunkUniformEdit { pos: UVec3, ty: Option<VoxelTypeId> }` (~25 LOC additions near the top of the file).
- `crates/bevy_naadf/src/world/data.rs:721` — change `set_voxels_batch(edits: &[(IVec3, VoxelTypeId)])` to `set_voxels_batch(edits: &[VoxelEdit])`. The body's `for &(pos, ty) in edits` destructure becomes `for &VoxelEdit { pos, ty } in edits` (1 LOC).
- `crates/bevy_naadf/src/world/data.rs:1099-1101` — change `set_chunks_uniform_batch(chunks: &[([u32; 3], Option<VoxelTypeId>)])` to `set_chunks_uniform_batch(chunks: &[ChunkUniformEdit])`. Body destructure updates similarly. **Note:** the existing API takes `[u32; 3]` for the chunk pos; the new type uses `UVec3` for consistency with `set_voxels_batch`'s `IVec3` vs the brush authors' Bevy idioms — D2's editor side already constructs `UVec3` from world rays.
- `crates/bevy_naadf/src/world/data.rs:330-335, :363-368, :383-388` — replace inline chunk-pos pack/unpack triples with calls to `crate::aadf::edit::unpack_chunk_pos(...)` and `pack_chunk_pos(...)`. **Do not move the `let ptr_unused = …; let _ = ptr_unused;` dead-bind at `:316-317` yet — those lines are in the to-be-moved `set_voxel` body; Step 5 handles them.**
- `crates/bevy_naadf/src/aadf/edit.rs:67-69` — replace inline chunk-pos unpack inside `apply_chunk_edit_cpu` with `unpack_chunk_pos(chunk_pos_packed)` (single 3-line block → single function call).
- `crates/bevy_naadf/src/world/data.rs::tests` — update fixture call sites `wd.set_voxels_batch(&[(IVec3::new(2,3,4), VoxelTypeId(1)), …])` to `wd.set_voxels_batch(&[VoxelEdit { pos: IVec3::new(2,3,4), ty: VoxelTypeId(1) }, …])`. 4-6 tests change.

**D2 coordination:** D2's implementor must update `editor/tools.rs:139,153,160,208,222,282,285,344,422,454,535` (paint/cube/sphere brushes) in the **same delivery** — they construct the tuples and pass them to `set_voxels_batch`. The implementor either lands a paired commit or lands a temporary compat shim `impl From<(IVec3, VoxelTypeId)> for VoxelEdit` that's deleted in a follow-up; the architect leans **paired commit** (cleaner blame).

**Rationale:** Findings 5 + 7 land here. The set-voxel public surface gets named types; the inline chunk-pos masks get the helper. Both are mechanical substitutions with zero behaviour change.

**Post-step state:** `cargo build` green. `cargo test --workspace --lib` green — the 5 `world/data.rs` tests using the tuples have been updated. The 11 D2 brush call sites are updated (D2 implementor's responsibility, paired commit).

**Verification:** Full gate suite. `--oasis-edit-visual` is the most likely regression site (it exercises the brush stroke path that constructs `VoxelEdit`s); run twice.

---

#### Step 4 — Add `EditBatch::iter_voxel_edits` / `iter_block_edits` / `iter_chunks` + `merge_recomputed_aadfs_into_batch` helper

**Edits:**
- `crates/bevy_naadf/src/aadf/edit.rs:189-200` — add the 3 iter methods (~30 LOC) right after the `EditBatch` struct definition.
- `crates/bevy_naadf/src/aadf/edit.rs` — add the `pub(crate) fn merge_recomputed_aadfs_into_batch(...)` helper (~50 LOC) immediately after `recompute_chunk_layer_aadfs` (which it calls internally).

**Rationale:** Pure additions — these methods exist as APIs but are not yet called. Sets up Step 5 to use them when the diagnostic methods move out of `data.rs`. Separating "introduce helper" from "swap call sites" keeps each step's diff small and reviewable.

**Post-step state:** `cargo build` green. New methods compile and are reachable from the test module via `pub` (iters) / `pub(crate)` (merge helper). No production callers yet.

**Verification:** `cargo test --workspace --lib`. No e2e change.

---

#### Step 5 — Extract `set_voxel` + `set_voxels_batch_oracle` to `world/oracle.rs` (Finding 1 + 3)

**Edits:**
- `crates/bevy_naadf/src/world/oracle.rs` — **NEW FILE** (~330 LOC). Migrate the bodies of `set_voxel` (from `data.rs:235-398`) and `set_voxels_batch_oracle` (from `data.rs:1181-1342`) into free functions `pub(crate) fn set_voxel(world: &mut WorldData, pos: IVec3, ty: VoxelTypeId)` and `pub(crate) fn set_voxels_batch_oracle(world: &mut WorldData, edits: &[VoxelEdit])`. Replace internal `self.foo` references with `world.foo`. **Use the Step 4 helpers**: replace the duplicate recompute-post-amble (~50 LOC × 2) with one call to `merge_recomputed_aadfs_into_batch(&mut world.chunks_cpu, [sx, sy, sz], &mut batch)`. Replace `chunks_exact(33)/(65)` loops with `iter_voxel_edits()/iter_block_edits()`. Delete the `let ptr_unused; let _ = ptr_unused;` dead-bind (`data.rs:316-317`).
- `crates/bevy_naadf/src/world/mod.rs:12` — add `pub(crate) mod oracle;` between `pub mod buffer;` and `pub mod data;`. Update the module-level docblock at `:1-10` to mention the diagnostic oracle.
- `crates/bevy_naadf/src/world/data.rs:235-398` — **DELETE** `set_voxel`'s body. Replace the method (if `pub`-surface attestation is desired) with no method at all — callers go through `crate::world::oracle::set_voxel(&mut wd, …)`.
- `crates/bevy_naadf/src/world/data.rs:1163-1342` — **DELETE** `set_voxels_batch_oracle`'s body similarly.
- `crates/bevy_naadf/src/world/data.rs:19-34` — update the `## DIAGNOSTIC-ONLY methods` docblock to point at `crate::world::oracle` instead of describing the now-removed methods.
- `crates/bevy_naadf/src/world/data.rs::tests` — every test calling `wd.set_voxel(...)` or `wd.set_voxels_batch_oracle(...)` switches to the new free-function form. The `set_voxels_batch_oracle_emits_synthetic_aadf_entries` test at `:1551-1571` either stays in `data.rs::tests` with updated call sites, or moves into `world/oracle.rs`'s own `#[cfg(test)] mod tests`. **Architect's preference:** move it into `oracle.rs::tests` (it's the canonical oracle-behaviour test).
- `crates/bevy_naadf/src/bin/e2e_render.rs` `--edit-mode` dispatch (D6's territory, but the import will need updating) — change `wd.set_voxel(...)` to `crate::world::oracle::set_voxel(&mut wd, ...)`. **D6 implementor coordinates** — the implementer for D1 flags this as a 1-line follow-up.

**Rationale:** Finding 1 (the headline IoC violation) + Finding 3 (DRY collapse, via the Step 4 helper) land here. After this step, `WorldData`'s `pub` API is exactly the 5 production methods. The diagnostic surface is `pub(crate)`-reachable only.

**Post-step state:** `cargo build` green. `cargo test --workspace --lib` green. `WorldData`'s public method count drops from 7 to 5. The file `world/data.rs` shrinks from 1731 LOC to ~1080 LOC. The new `world/oracle.rs` is ~330 LOC. Net D1 LOC delta: −1731 + 1080 + 330 = **−321 LOC** at this step alone.

**Verification:** Full gate suite including `cargo run --bin e2e_render -- --edit-mode` (the diagnostic gate that uses `set_voxel`). Run twice per `feedback-multiple-runs-rule-out-false-positives`. Also `--validate-gpu-construction` (byte-equality oracle); also `--runtime-edit-mode` (production path).

---

#### Step 6 — SSoT-6: collapse hash-coefficient table to D1

**Edits:**
- `crates/bevy_naadf/src/aadf/block_hash.rs:395-404` — rename `fn build_polynomial_coefficients()` to `pub fn hash_coefficients()` (promote to `pub`, rename to match D5's existing exported name). Update the in-file caller at `:109`.
- `crates/bevy_naadf/src/render/construction/hashing.rs:30-50` — **DELETE** the duplicate function body. Replace with `pub use crate::aadf::block_hash::hash_coefficients;` — a one-line re-export so the 9 `render/construction/mod.rs` consumers continue to import via `use crate::render::construction::hashing::hash_coefficients;` without source changes.

**Coordination:** D5's architect approves the consumer-side change (the re-export in `hashing.rs`). The re-export is a non-breaking change for D5 (every existing import resolves identically); the deletion of the duplicate function body lets the SSoT-6 audit close cleanly.

**Rationale:** Finding 8 lands. Two Rust copies → one Rust SSoT. D1 owns the SSoT (`aadf/block_hash.rs`); D5 consumes via re-export. The WGSL side (`chunk_calc.wgsl:131-134 hash_coefficients` storage buffer) remains the GPU-side consumer; D5's existing upload path (which already calls `hashing::hash_coefficients()` to seed the GPU buffer) keeps working because of the re-export.

**Post-step state:** `cargo build` green. `cargo test --workspace --lib` green (the D5 tests at `hashing.rs:143,166,189` continue to pass via the re-export). Net delta: ~10 LOC saved by deleting the duplicate.

**Verification:** Full gate suite. `--validate-gpu-construction` is the canonical gate for hash-correctness (byte-equality with the WGSL output).

---

#### Step 7 — Cleanup: dead re-exports, aadf/mod.rs docblock, `build_chunk_edit_window_solid_type`

**Edits:**
- `crates/bevy_naadf/src/aadf/edit.rs:571-574` — **DELETE** the dead `pub use crate::aadf::cell::unpack_voxel as cell_unpack_voxel;` and the `#[allow(unused_imports)] use unpack_voxel as _unpack_voxel;` import-suppressor. Explorer Suspicion 2 confirms these are vestigial test-bridge code with no functional purpose.
- `crates/bevy_naadf/src/aadf/edit.rs:355-360` — `#[allow(dead_code)] pub fn build_chunk_edit_window_solid_type` — move to `#[cfg(test)] mod tests { ... }`'s `fn build_chunk_edit_window_solid_type` so it's actually only compiled in tests. Verified caller analysis: only `aadf/edit.rs::tests` uses it (explorer Suspicion 2 said "Only the doc-comment says 'Test-helper only'" — true; no production callers).
- `crates/bevy_naadf/src/aadf/mod.rs:6-10` — update the module docblock to list all 7 submodules (`block_hash`, `bounds`, `cell`, `construct`, `edit`, `entity`, `generator`). Explorer's Side note 2 caught the docblock listing only 4.
- `crates/bevy_naadf/src/aadf/entity.rs:159-164` — extract the 6-mask + 6-shift constants from inside `from_types` to module-level `const ENTITY_AADF_MASKS: [u32; 6]` and `const ENTITY_AADF_BIT_SHIFTS: [u32; 6]` (~15 LOC of pure refactor; algorithm body uses the named consts instead of `0x3D` etc).

**Rationale:** Cleanup pass. Each edit is local. Combining into one step keeps the migration shape from sprawling into 10+ steps for trivial things.

**Post-step state:** `cargo build` green. `cargo test --workspace --lib` green. ~20 LOC saved (dead re-exports + the `pub` test helper that no longer needs `#[allow(dead_code)]`).

**Verification:** Full gate suite. The entity-side mask extraction is functionally identical; `--vox-e2e` exercises entity rendering.

---

#### Step 8 — Optional: WGSL shader-def upload for SSoT-3 constants

**Edits (D5 ARCHITECT'S DECISION — this step blocks on D5's design):**
- `crates/bevy_naadf/src/render/pipelines.rs:278-279` (D5 territory) — extend the `shader_defs` vec to inject `CELL_DIM` / `CHUNK_DIM_VOXELS` / `CELL_CHILDREN` (and possibly `BLOCK_STATE_*`).
- `crates/bevy_naadf/src/assets/shaders/*.wgsl` (D4/D5 territory) — replace bare `16u` / `4u` / `64u` literals with `#{CHUNK_DIM_VOXELS}u` / `#{CELL_DIM}u` / `#{CELL_CHILDREN}u` references.

**Rationale:** Out of D1's direct scope (the implementor cannot edit D4/D5 files). Listed here for D5's architect to consume. **D1 implementor SKIPS this step entirely** — D5's architect designs and D5's implementor lands it. D1 has done its job by exposing the Rust constants.

**Post-step state:** N/A from D1's perspective.

**Verification:** N/A from D1's perspective.

---

## 4. What stays / what changes / what's removed

### Stays unchanged (intentionally)

- **`aadf/bounds.rs` public API** — the algorithm is correct, the docblocks are accurate, and the explorer's Finding 4 reject keeps it stable. Internal `step_axis(... 0x3D, 0x3E, ...)` mask literals stay inline (the alternative was extracting a shared cross-file mask table, which both finds 4 and `bounds.rs` itself caution against).
- **`aadf/cell.rs` public API surface** — `ChunkCell`/`BlockCell`/`VoxelCell` enums, their `encode`/`decode` methods, `Aadf6`, `pack_voxels`/`unpack_voxel`, `BlockPtr`/`VoxelPtr`. The migration in Step 2 changes only the *implementation* — the public API surface is identical.
- **`aadf/edit.rs::apply_chunk_edit_cpu`, `apply_block_edit_cpu`, `apply_voxel_edit_cpu`** — the W2 GPU oracle trio. **SACRED per user directive ("cpu oracle stays").** Step 4 adds iter methods on the sibling `EditBatch`; the oracles themselves untouched.
- **`aadf/edit.rs::process_edit_batch`, `build_chunk_edit_window_from_world`, `set_voxel_in_window`, `pack_chunk_pos`, `unpack_chunk_pos`, `recompute_chunk_layer_aadfs`** — all kept. The migration consumes them more; it doesn't replace them.
- **`aadf/construct.rs`** — algorithm + public API stay. Only `CHUNK_DIM_VOXELS:29` becomes a re-export from `voxel/mod.rs`.
- **`aadf/generator.rs`** — algorithm + public API stay. Only the private `CHUNK_DIM_VOXELS:51` is deleted (in favour of the SSoT). **The filename `generator.rs` stays** (explorer Side note 5 proposed `world_generator.rs` rename; D1 architect rejects to keep diff small and because the explorer flagged this as low-priority anyway).
- **`aadf/entity.rs::EntityData::from_types`** — algorithm body stays per Finding 4 reject. Step 7 extracts the magic constants to named tables; the kernel is unchanged.
- **`aadf/block_hash.rs::BlockHashingHandler`** + its methods — unchanged. Step 6 only renames `build_polynomial_coefficients` to `hash_coefficients` and promotes to `pub`.
- **`world/data.rs::WorldData::seed_block_hashing, ray_traversal, get_voxel_type, set_voxels_batch, set_chunks_uniform_batch`** — the 5 production methods stay. Method bodies update internally (Step 3's `VoxelEdit` rename; Step 1's `CellRaw` / `cell_state` usage at the `>> 30` sites). Their public signatures change only for the F5 named-type swap.
- **`world/buffer.rs`** (`GrowableBuffer<T>`) — completely untouched. See §6 Side notes.
- **`voxel/mod.rs`** — every existing constant stays. `VoxelType` / `VoxelTypeId` / `MaterialBase` / `MaterialLayer` unchanged. Additions only.
- **`world/mod.rs::WorldPlugin`** — unchanged. The plugin remains a placeholder; resources are inserted by `voxel::grid::setup_test_grid`.

### Changes

- `voxel/mod.rs` — adds `cell_state` submodule, `CHUNK_DIM_VOXELS`, `CHUNK_VOLUME_VOXELS`, `CellRaw` newtype. Deprecates+deletes `CELL_HAS_CHILDREN` / `CELL_UNIFORM_FULL` aliases. (Net ~+20 LOC.)
- `aadf/cell.rs` — `ChunkCell::encode/decode` + `BlockCell::encode/decode` rewritten in regime-B form. `DIRS` array added. (Net ~+15 LOC.)
- `aadf/edit.rs` — `EditBatch::iter_voxel_edits / iter_block_edits / iter_chunks` added. `merge_recomputed_aadfs_into_batch` added. Dead re-exports `cell_unpack_voxel` + `_unpack_voxel` deleted. `build_chunk_edit_window_solid_type` moved behind `#[cfg(test)]`. Inline mask triple at `:67-69` replaced by `unpack_chunk_pos`. (Net ~+40 LOC.)
- `aadf/entity.rs` — `ENTITY_AADF_MASKS` + `ENTITY_AADF_BIT_SHIFTS` extracted to module-level consts. (Net ~+5 LOC.)
- `aadf/construct.rs` — `CHUNK_DIM_VOXELS:29` becomes re-export of `voxel::CHUNK_DIM_VOXELS`. (Net ~−1 LOC.)
- `aadf/generator.rs` — private `CHUNK_DIM_VOXELS:51` deleted, uses `voxel::CHUNK_DIM_VOXELS`. (Net ~−1 LOC.)
- `aadf/block_hash.rs` — `build_polynomial_coefficients` renamed to `pub fn hash_coefficients`. (Net ~0 LOC.)
- `aadf/mod.rs` — module docblock updated to list all 7 submodules. (Net ~+5 LOC.)
- `world/data.rs` — `VoxelEdit` + `ChunkUniformEdit` types added. `set_voxel` + `set_voxels_batch_oracle` extracted to `world/oracle.rs` (large deletion). `set_voxels_batch` + `set_chunks_uniform_batch` signatures update to consume the named types. Inline chunk-pos masks at `:330-388, :1278-1331` replaced by `pack_chunk_pos`/`unpack_chunk_pos`. Internal `>> 30` / `& 0x3FFF_FFFF` patterns optionally migrated to `CellRaw` helpers at hot sites. `let ptr_unused; let _ = …;` dead-bind deleted. (Net ~−650 LOC; the file lands at ~1080 LOC.)
- `world/mod.rs` — adds `pub(crate) mod oracle;` registration; docblock mentions the diagnostic-only seam. (Net ~+10 LOC.)
- `render/construction/hashing.rs` (D5 territory but D1 SSoT consumer) — `hash_coefficients` body replaced by `pub use crate::aadf::block_hash::hash_coefficients;`. (Net ~−10 LOC.) **D5 architect approves; D5 implementor lands.**

### Removed

- `aadf/edit.rs:571-574` — `cell_unpack_voxel` re-export + `_unpack_voxel` import-suppressor. Zero callers verified by Grep.
- `world/data.rs::set_voxel` + `set_voxels_batch_oracle` `pub` methods (bodies move to `world/oracle.rs` as free functions; the methods themselves are gone — callers go through the free functions).
- `voxel::CELL_HAS_CHILDREN` + `voxel::CELL_UNIFORM_FULL` constants (deleted after Step 2 once `aadf/cell.rs` no longer cites them; transiently `#[deprecated]` between Step 1 and Step 2). **`CELL_PAYLOAD_MASK` stays** — it's still needed by `CellRaw::payload()` and a few raw-bit-twiddling sites (and equals `0x3FFF_FFFF` which is fine to keep named).
- `render/construction/hashing.rs::hash_coefficients` function body (replaced by re-export from `aadf::block_hash::hash_coefficients`).
- `aadf/generator.rs:51` private `CHUNK_DIM_VOXELS` constant (replaced by import of `voxel::CHUNK_DIM_VOXELS`).
- `world/data.rs:316-317` dead-bind `let ptr_unused = …; let _ = ptr_unused;` (gone with the `set_voxel` body extraction).

---

## 5. Decisions & rejected alternatives

### Load-bearing decisions

**D1.1 — State-bit regime B is canonical.** Faithful-port rule binds: C# `WorldData.cs::FillChunkData/FillBlockData/SetChunk` and `EditingHandler.cs::processChunks` all use `>> 30` discriminator + `(state << 30) | payload` constructor exclusively. The Rust port's `aadf/cell.rs` regime A is the deviation; migrate it. **WGSL's `BLOCK_STATE_CHILD = 2u` etc. is the source vocabulary** for the Rust constant names (`cell_state::CHILD`). One naming pool, three implementations (Rust source, Rust binary output, WGSL source), one source-of-truth: `voxel::cell_state`.

**D1.2 — D1 owns SSoT-6 (hash coefficients).** AADF data layer is D1's domain; the GPU consumer is a consumer. `aadf::block_hash::hash_coefficients` is the Rust SSoT; D5 re-exports. WGSL `chunk_coefficients` storage buffer is uploaded by D5's existing path; no WGSL change required.

**D1.3 — `pub(crate)` extraction over `#[cfg(feature = "diagnostic")]` gating for Finding 1.** The explorer's Open Question #1 listed two options; `pub(crate)` wins because (a) no build-matrix complication (the e2e gate would otherwise need to add a `--features diagnostic` to every CI run), (b) it preserves source-tree visibility for the diagnostic surface (the file is right there in `world/oracle.rs`, not gated out of the IDE's view), (c) zero behavioural divergence between dev / release / CI builds.

**D1.4 — `WorldData` set-voxel pub surface preservation.** The diagnostic methods become free functions in `world::oracle` rather than disappearing. Free-function on `&mut WorldData` matches the pattern `aadf::edit::process_edit_batch(&mut …)` already establishes. This avoids changing `WorldData`'s impl block (the production methods stay as `&mut self` methods).

**D1.5 — Keep `aadf/generator.rs` filename.** Explorer Side note 5 proposed `world_generator.rs` rename for discoverability. Rejected here because (a) faithful-port rule keeps file naming close to C# (`World/Generator/WorldGeneratorModel.cs` already lacks a `world_` prefix), (b) the orchestration's user directive favors small diffs (the rename costs ~12 import-site updates for ~0 discoverability gain — anyone reading the file's first line of docs sees what it does).

**D1.6 — Keep `world/buffer.rs` location.** Explorer Side note 4 proposed moving `GrowableBuffer<T>` to `render/`. Rejected here because the file is D1's path-list (the orchestrator scoped it that way); relocating crosses D1↔D4 ownership. Side notes flag this for the orchestrator; D1 implementer does NOT move it.

### Rejected alternatives

**RA-1 — `enum Dir6 { NegX, … }` (Finding 10).** Rejected: hot-loop sensitivity per `00-reuse-audit.md §3.5 UA-4`. The implementer cannot verify codegen non-regression without `cargo-asm` and disassembly comparison; the LOC savings are zero. Compromise: add `DIRS: [usize; 6]` array for `for &dir in DIRS.iter() { … }` typed iteration where the call site isn't on the hot path.

**RA-2 — Share `EntityData::from_types`'s AADF kernel with `compute_aadf_layer` (Finding 4).** Rejected: bit-for-bit GPU oracle equality at W4 is too fragile to risk. The C# port keeps the inline form; the faithful-port rule binds. Compromise: extract the 6 magic mask constants + 6 magic bit shifts to a named table (Step 7); the algorithm body stays.

**RA-3 — `#[cfg(feature = "diagnostic")]` gating instead of `pub(crate)`.** Rejected — see D1.3 above. The orchestration sequencing means D1 must not break the e2e gates that exercise the diagnostic path; a Cargo feature would force every CI gate to pass `--features diagnostic` and add a build-matrix dimension.

**RA-4 — Move `WorldData` itself to a sibling crate.** Not on the table. The `WorldData` resource is load-bearing for Bevy's render-world hand-off (`extract_world_changes` in D5's domain), and the cross-crate import dance would be more architecture churn than the orchestration intends.

**RA-5 — Cross-file `ENTITY_AADF_MASKS` table imported by both `entity.rs` and `bounds.rs::step_axis`.** Rejected: `bounds.rs::step_axis` is inside the hot AADF computation loop and accepts the masks positionally — extracting them at the call site (`compute_aadf_layer`) would impose an unnecessary const-load at the call boundary and complicate the perf-sensitive code. Land the entity-side extraction (named consts) but keep `bounds.rs`'s inline literals.

**RA-6 — Promote `aadf::generator::ModelData` to D7 / `crate::voxel`.** Out of scope. `ModelData` is the producer of the world-gen pipeline; its location follows the consumer (`generator.rs`). D1's brief is data structures, not pipeline ownership.

---

## 6. Assumptions made (future readers verify)

**A1 — D5 architect agrees to consume D1's `hash_coefficients` SSoT via re-export.** Step 6's delete-and-reexport on `render/construction/hashing.rs:30-50` is in D5's path list; D1 architect's design depends on D5 not redefining the function. If D5's architect rejects (e.g. argues for moving to a third location), Step 6's plan changes.

**A2 — D5 architect agrees to inject SSoT-3 constants as shader-defs in a separate step.** Step 8 is listed in D1's design but explicitly punted to D5's implementor. D1's refactor lands cleanly without Step 8 — the bare WGSL `16u`/`4u`/`64u` literals are correct by paper-decree-of-`CELL_DIM=4`-forever, so they don't break.

**A3 — D2 implementor migrates the 11 brush call sites in `editor/tools.rs` in the same delivery as D1's Step 3.** Without that, Step 3 doesn't compile (the production `set_voxels_batch` signature changes). Either paired commit or temporary `impl From<(IVec3, VoxelTypeId)> for VoxelEdit` compat shim — architect's preference is paired commit.

**A4 — D6 implementor updates the `--edit-mode` e2e CLI dispatch's `set_voxel` call site in `bin/e2e_render.rs`** when Step 5 lands. The diagnostic free function `crate::world::oracle::set_voxel(&mut wd, pos, ty)` replaces the method call.

**A5 — D5 e2e gate bodies (`render/construction/mod.rs:9207, :9350, :9416`) that call `set_voxel`/`set_voxels_batch` resolve via the new pub-surface shapes.** Step 3's `VoxelEdit` type change ripples into D5; Step 5's `set_voxel` removal ripples into D5. D5's architect must coordinate when designing the construction-module split — those call sites get rewritten as part of D5's refactor anyway (per the brief, mod.rs is the headline restructure).

**A6 — Round-trip tests at `aadf/cell.rs:223-300` provide sufficient correctness coverage for Step 2's encode/decode migration.** They cover all three states across both ChunkCell and BlockCell, plus VoxelCell (which is unchanged). The fingerprint of "encode then decode round-trips to the input" pins regime-B's bit-pattern identity to regime A.

**A7 — `cargo test --workspace --lib` is the canonical correctness gate.** Per `01-context.md §Verification gates`. Implementer adds tests **only** to cover the new functions — `merge_recomputed_aadfs_into_batch`, `EditBatch::iter_*`, `CellRaw::*`. The existing test surface already covers the refactored functions.

**A8 — Behavioural equivalence at `pending_edits.batches` is the load-bearing W2 contract.** Step 5's move of `set_voxel`/`set_voxels_batch_oracle` into free functions writes the same `EditBatch` shape that the W2 GPU dispatch consumes. The `set_voxels_batch_oracle_emits_synthetic_aadf_entries` test at `world/data.rs:1551-1571` (moves to `oracle.rs::tests` in Step 5) pins this.

---

## 7. D4↔D5 / D2 / D6 / D7 coordination notes

**For D2 architect (editor-and-settings-ui):**
- D1 defines `pub struct VoxelEdit { pos: IVec3, ty: VoxelTypeId }` and `pub struct ChunkUniformEdit { pos: UVec3, ty: Option<VoxelTypeId> }` in `crates/bevy_naadf/src/world/data.rs`.
- D2's brushes at `editor/tools.rs:139,153,160,208,222,282,285,344,422,454,535` should be updated to construct `VoxelEdit { pos, ty }` literals; do **not** redefine the struct in D2.
- The 3 brush implementations (paint/cube/sphere) may also benefit from the BrushShape trait the D2 explorer (DUP-2) suggested. D1 has no opinion on that.

**For D4 architect (render-pipeline):**
- D1 promotes `CHUNK_DIM_VOXELS` to a single Rust SSoT in `voxel/mod.rs` (Step 1). D4 + D5 own the WGSL-side SSoT-3 plumbing (shader-defs upload).
- No direct D1↔D4 file overlap. `render/gpu_types.rs`, `render/prepare.rs`, `render/pipelines.rs::NaadfPipelines` stay untouched by D1.

**For D5 architect (gpu-construction):**
- **Step 6 requires D5's cooperation:** delete the duplicate `hash_coefficients` body at `render/construction/hashing.rs:43-50` and replace with `pub use crate::aadf::block_hash::hash_coefficients;`. The 9 import sites in `render/construction/mod.rs` continue to resolve unchanged.
- **D5's mod.rs split will displace** the e2e gate fixtures at `render/construction/mod.rs:9207, :9350, :9416` that consume D1's `set_voxel` (now `crate::world::oracle::set_voxel`) and `set_voxels_batch` (now takes `&[VoxelEdit]`). D5's implementer updates those call sites during its own refactor.
- **The state-bit regime migration (Step 2)** does NOT touch WGSL — the WGSL shaders already use regime B. The Rust migration's output is byte-identical. **No D5 WGSL change required.**

**For D6 architect (e2e-and-playwright):**
- The `--edit-mode` CLI gate dispatch in `bin/e2e_render.rs` needs its `WorldData::set_voxel(...)` call site updated to `crate::world::oracle::set_voxel(&mut wd, ...)` when Step 5 lands.
- The non-deterministic gates (`--oasis-edit-visual`, `--runtime-edit-mode`) are the load-bearing verification surface for D1's set-voxels_batch path; run ≥2× per the project's non-determinism rule.

**For D7 architect (app-and-camera):**
- No direct D1↔D7 overlap.
- The `WorldData` `Default` impl + `Resource` derive stay; D7's `build_app_with_args` continues to register `WorldPlugin`.

---

## 8. Side notes / observations / complaints

1. **`aadf/generator.rs` is misnamed at the docs-discoverability level, but renaming costs more than it's worth in source churn.** D1 architect's call: keep the name. Explorer Side note 5 disagrees; the design rejects the rename and explains why under D1.5.

2. **`world/buffer.rs` (GrowableBuffer) sits awkwardly in D1's path list — 100% of callers live in D4/D5.** Verified via `Grep`: zero D1 (`aadf/` + `world/`) callers of `GrowableBuffer<T>` outside `world/buffer.rs` itself. The file is GPU-buffer machinery whose only logical home is `crate::render::`. **D1 architect's design leaves it in place** (per D1.6 above — relocating crosses ownership). Flag for the orchestrator: if a future tightening pass wants to move `world/buffer.rs` to `render/`, D4 should own that move, not D1.

3. **`world/data.rs::set_voxels_batch` is 360 LOC of tightly-anchored C# line citations** (`EditingHandler.cs:75-180` etc. cited throughout the body). The faithful-port rule binds. Step 3's signature change (`VoxelEdit` substitution) preserves the cited stanza structure; the implementor MUST resist the temptation to "tighten" the inner logic during this pass. **All inner-block refactors of `set_voxels_batch`'s body are explicitly out of scope** for D1 — the body is a 1:1 port of the C# and its line-by-line traceability is load-bearing.

4. **The brief mentioned that "PBR raymarching work lives on a SEPARATE branch and is already ready"** — no PBR concerns are surfaced in D1's domain (D1 has zero PBR-related code; the cell encoding is paper §3.1, not paper §4 / PBR's domain). This design is paper-port-clean.

5. **`aadf/edit.rs::set_voxel_in_window` (line 432) and `build_chunk_edit_window_from_world` (line 373)** are called from BOTH the production `set_voxels_batch` AND the diagnostic-only `set_voxel`/`set_voxels_batch_oracle`. They are NOT pure diagnostic helpers — they stay `pub` in `aadf::edit` after Step 5. Explorer Side note 3 caught this; the architect's design respects it.

6. **The 4×2 production/diagnostic × rehash/no-rehash matrix the explorer described in Side note 9** is preserved in the design:
   - `WorldData::set_voxels_batch` — production runtime, NO whole-world rehash.
   - `WorldData::set_chunks_uniform_batch` — production brush-fast-path, NO whole-world rehash.
   - `world::oracle::set_voxel(&mut wd, ...)` — diagnostic, WITH whole-world rehash.
   - `world::oracle::set_voxels_batch(&mut wd, ...)` — diagnostic, WITH whole-world rehash.
   The split is now structural (production methods on `WorldData`, diagnostic free functions in `oracle::*`), not just doc-comment annotated.

7. **The user's verbatim "cpu oracle stays" directive is preserved structurally, not just verbally.** The `world/oracle.rs` extraction makes the oracle's purpose visible at the file-tree level. The W2 GPU oracle trio (`apply_chunk_edit_cpu`, `apply_block_edit_cpu`, `apply_voxel_edit_cpu`) stays in `aadf/edit.rs` — those are bit-level GPU-shader oracles, distinct from the world-data-API oracle.

8. **Docblock rot to clean up alongside (explorer Side notes 2 + 7):** `aadf/mod.rs:1-10` docs list only 4 of 7 submodules (Step 7 fixes), and `world/data.rs:1-34` / `aadf/edit.rs:1-41` / `aadf/bounds.rs:1-49` each rehash the "DIAGNOSTIC-ONLY vs production runtime" architecture in nearly identical prose. The design does not consolidate these — Step 7 fixes the `aadf/mod.rs` one only. Consolidating the diagnostic-architecture prose to a single canonical statement in `world/mod.rs` is a documentation-only follow-up; out of scope.

9. **The implementor will encounter the explorer's `let ptr_unused = …; let _ = ptr_unused;` dead-bind at `world/data.rs:316-317`** (explorer Side note 6). The Step 5 extraction discards it. Implementor should not re-introduce it — the comment at `:318` ("The pointer we wrote into `blocks_cpu` is `b_cursor + idx * 64`.") is the explanation.

10. **The `aadf::edit::set_voxel_in_window` body has subtle "high/low half-word of u32 pair" logic** (lines 446-457). It is exercised by 4 tests including the user-reported phantom-voxel regression test (`small_edit_one_voxel_into_populated_chunk_emits_exactly_one`, `world/data.rs:1683-1730`). Implementor leaves it alone. The function is `pub` and stays `pub`.

11. **Equal-footing complaint (per user CLAUDE.md):** The brief framed `WorldData`'s set-voxel methods as "an IoC violation" but the actual rot is subtler — the diagnostic methods themselves are honest about what they do (`#[doc(hidden)]` + `DIAGNOSTIC-ONLY` callouts in every docblock); the rot is that they coexist with production methods on the *same impl block*, bloating IDE auto-complete. The fix (`pub(crate) mod oracle`) makes the seam **visible** structurally rather than just annotated. This is the actual win — not a behavioural correctness improvement, but a discovery-surface tightening. The implementer should not be surprised that no tests catch this "fix" — there isn't a test that says "the public API surface is the right size", just the new presence of `world/oracle.rs` as a discoverable file.

12. **The explorer's Open Question #2** about backward-compatible aliases for `CELL_HAS_CHILDREN`/`CELL_UNIFORM_FULL` was answered by D1.1 + Step 2: deprecation aliases ride one step (between Steps 1 and 2), then delete. No long-term compat layer.

13. **The explorer's Open Question #3** about Finding 4 (entity AADF kernel share) was answered by D1.5 + Step 7: REJECT the kernel share; extract only the 6 magic constants to named tables. Bit-for-bit GPU match preserved.

14. **The explorer's Open Question #4** about hash-coefficient SSoT location (D1 vs D5 vs new shared module) was answered by D1.2: D1 owns it; D5 re-exports.

---

## 9. Open conflicts

**None.** The design stays within D1's path list and forbidden moves. Cross-domain ripples (D2's brush call sites, D4/D5's shader-def upload, D5's e2e gate fixtures, D5's hashing re-export, D6's CLI dispatch) are flagged for the relevant architects but do not require forbidden moves from D1.

The `world/buffer.rs` relocation question (§6 Side note 2) is **not** an open conflict — D1 architect's design is to leave it in place. If the orchestrator later wants the move, D4 architect handles it; D1 has not designed the move.
