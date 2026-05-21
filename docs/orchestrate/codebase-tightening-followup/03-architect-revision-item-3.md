# Item 3 architect revision — D4 revised §3.4

**Author:** delegate-architect (item-3 revision sub-agent), dispatched
under the `codebase-tightening-followup` orchestration.
**Date:** 2026-05-21.
**Target file (downstream):** `docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md` §3.4.
**Scope:** §3.4 only — the `ShaderType` cutover spec. No other sections,
no other items, no source edits.

Verified at `/mnt/archive4/DEV/bevy-naadf/` HEAD `2bb03d1` ("D4 final
cleanup — prepare.rs split…"); every file:line citation re-Read or
re-Grepped before landing.

---

## Original §3.4 context

The original §3.4 (`docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md:353-464`) specified
a single mechanical recipe for cutting 7 uniform structs from
`#[repr(C)] #[derive(Pod, Zeroable)]` over to `#[derive(ShaderType)]` —
"drop every `_padN` field" plus "the compile-time `assert!(std::mem::size_of::<…>() == …)`
guard at `gpu_types.rs:845` drops because `encase` enforces the layout
at serialisation time" (`03-architecture.md:404-411`). The exemplar
shown was `GpuRenderParams` (`gpu_types.rs:60-117`), where every `_padN`
sits at a std140-natural alignment break — drop-pads-and-let-encase-re-pad
produces a byte-equivalent buffer.

**What was wrong:** the recipe is correct for 5-of-6 D4-owned uniforms
but breaks on `GpuGiParams`. Per the bailing implementor's hand-walk
(`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1011-1075`)
and the fresh-eyes investigation
(`docs/orchestrate/codebase-tightening-followup/02-investigation-item-3-gpugiparams.md`):

- `GpuGiParams._pad5/6/7` (`gpu_types.rs:511-516`) sit at offsets 292..304.
  They are NOT a std140-natural break — `sun_shadow_taps: u32` (offset
  288) followed by `max_ray_steps_secondary: u32` (currently 304) is
  two scalars of align-4 with no natural break between them. They are
  hand-inserted to force `max_ray_steps_secondary` onto a fresh 16-byte
  row, per the `21-design-quality-panel.md` §4.2 row-boundary contract.
  encase would place `max_ray_steps_secondary` at offset **292** if the
  trio is dropped — **12-byte layout divergence**.
- `GpuGiParams._pad8/9/10` (`gpu_types.rs:541-545`) sit at offsets
  324..336. They are hand-inserted trailing pad to keep the struct
  16-byte-aligned at 336. encase rounds the struct size up to its
  member-alignment max (16) automatically, so without the trailing trio
  the struct ends at offset 312 (last byte of `spatial_iter_count`) and
  rounds up to **320**, not 324 (the impl log's stated value is off by
  4) and not 336.
- Either drift alone breaks byte-equivalence; both must be resolved
  jointly.
- The corresponding WGSL fields `gi_params.wgsl:129-131` (`pad_b/c/d`)
  and `:145-147` (`pad_e/f/g`) mirror the Rust pads. Any Rust-side
  deletion must be matched by WGSL-side deletion or the GPU read
  addresses fall out of step.

**What the investigator surfaced:**

1. **Zero existing `ShaderType` consumer.** `grep -rn ShaderType
   /mnt/archive4/DEV/bevy-naadf/crates/` returns one hit — a docblock at
   `crates/bevy_naadf/src/render/pipelines.rs:369` explaining why the
   codebase uses `bytemuck::bytes_of` + `std::mem::size_of` directly.
   The `encase` crate (`Cargo.lock` v0.12.0) and `bevy_encase_derive`
   are transitive deps; the source uses neither. **Item 3 is
   precedent-setting.** The implementor cannot crib from a sibling.
2. **`pad_b..pad_g` have zero WGSL readers.** Confirmed by `grep -rn
   "pad_b\|pad_c\|pad_d\|pad_e\|pad_f\|pad_g"` across the WGSL tree —
   only declarations in `gi_params.wgsl` itself, no other site reads
   the fields by name. They are write-only-from-Rust lanes; safe to
   delete from WGSL without breaking call sites.
3. **The "drop the offset asserts" stance is wrong-in-spirit.** The
   runtime test docblock at `gpu_types.rs:944-951` explicitly says the
   asserts exist as a defense against future-tooling regressions —
   `#[cfg(feature = ...)]` wrapping, refactors that strip `const _ =
   assert!(...)`, editor auto-fixes. "encase enforces it" is not a
   sufficient reason to drop a free, localised, compile-time error
   message that catches drift.
4. **The cleanup win is the OTHER 5 structs.** `GpuGiParams` carries
   6 of the ~70 `_padN` fields across the 6-struct cutover surface.
   Item 3's value is "let one helper + one encoding regime cover all 6
   structs" — not "delete 6 more pads from the GI struct."

---

## Revised §3.4 spec (the deliverable)

The implementor lifts this section verbatim to replace
`docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md`
§3.4 in full.

> ### 3.4 `ShaderType` cutover — concrete shape (2 exemplars: clean case + named-pad case)
>
> The cutover converts 6 D4-owned uniform structs from
> `#[repr(C)] #[derive(Pod, Zeroable)]` to `#[derive(ShaderType)]`. The
> 7th candidate (`GpuConstructionParams`) is D5-coordinated — D4
> architect spec is 6 structs; D5 handles its own (re-flagged in
> the migration steps).
>
> **Item 3 is the codebase's first `ShaderType` consumer.** Verified by
> `grep -rn ShaderType crates/bevy_naadf/` returning one hit at
> `crates/bevy_naadf/src/render/pipelines.rs:369` (a docblock, no use).
> The implementor cannot follow-the-leader; both exemplars below are
> load-bearing.
>
> The 6 structs split into two encoding-shape buckets that share the
> same `#[derive(ShaderType)]` cutover but differ on which fields stay:
>
> | bucket | structs | rule |
> |---|---|---|
> | **Clean — all pads std140-natural** | `GpuRenderParams`, `GpuCamera`, `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams` | drop every `_padN` field; encase re-pads at natural alignment breaks |
> | **Named-pad — non-natural row-boundary pads** | `GpuGiParams` | **keep `_pad5/6/7/8/9/10` as named `u32` fields** under `#[derive(ShaderType)]`; encase serialises six explicit `u32` lanes at the same offsets |
>
> ---
>
> #### 3.4.1 Exemplar A — clean case (`GpuRenderParams`)
>
> **Before** (`crates/bevy_naadf/src/render/gpu_types.rs:59-117` — 58 LOC):
>
> ```rust
> #[repr(C)]
> #[derive(Clone, Copy, Debug, Pod, Zeroable)]
> pub struct GpuRenderParams {
>     pub screen_width: u32,
>     pub screen_height: u32,
>     pub frame_count: u32,
>     pub rand_counter: u32,
>     pub taa_index: u32,
>     pub flags: u32,
>     pub max_ray_steps_primary: u32,
>     pub _pad0b: u32,
>     pub sky_sun_dir: Vec3,
>     pub _pad1: u32,
>     pub sun_color: Vec3,
>     pub _pad2: u32,
>     pub taa_jitter: Vec2,
>     pub _pad3: Vec2,
>     pub bounding_box_min: Vec3,
>     pub _pad4: u32,
>     pub bounding_box_max: Vec3,
>     pub _pad5: u32,
> }
> ```
>
> **After** (~24 LOC):
>
> ```rust
> use bevy::render::render_resource::ShaderType;
>
> #[derive(Clone, Copy, Debug, Default, ShaderType)]
> pub struct GpuRenderParams {
>     pub screen_width: u32,
>     pub screen_height: u32,
>     pub frame_count: u32,
>     pub rand_counter: u32,
>     pub taa_index: u32,
>     pub flags: u32,
>     pub max_ray_steps_primary: u32,
>     pub sky_sun_dir: Vec3,
>     pub sun_color: Vec3,
>     pub taa_jitter: Vec2,
>     pub bounding_box_min: Vec3,
>     pub bounding_box_max: Vec3,
> }
> ```
>
> Every dropped `_padN` (8 of them, including the `Vec2`-shaped `_pad3`)
> sits at a std140-natural alignment break — `vec3`-trail-of-row, or
> `Vec2`-pre-`vec3`. encase inserts the same padding at serialisation.
> Resulting buffer: byte-identical 112-byte uniform.
>
> ---
>
> #### 3.4.2 Exemplar B — named-pad case (`GpuGiParams`)
>
> `GpuGiParams` carries six pads that are NOT std140-natural row breaks:
>
> - `_pad5/6/7` (`gpu_types.rs:512-516`) — three `u32`s at offsets
>   292..304 that force `max_ray_steps_secondary` onto a fresh 16-byte
>   row (offset 304). Without them encase places `max_ray_steps_secondary`
>   at offset 292 (12-byte divergence).
> - `_pad8/9/10` (`gpu_types.rs:541-545`) — three `u32`s at offsets
>   324..336 that pad the struct out to two extra 16-byte rows after
>   `spatial_iter_count`. Without them encase rounds the struct to size
>   320 (last byte of `spatial_iter_count` at 311, rounded up to 320 per
>   `align(16)`). The current struct is 336 bytes; encase would produce
>   320.
>
> Both pads exist for legibility / row-boundary documentation, per the
> `21-design-quality-panel.md` §4.2 + `20-impl-phase-d-shadow-A.md`
> row-boundary contract.
>
> **The recipe for `GpuGiParams`: keep all 6 pads as named `u32` fields
> under `#[derive(ShaderType)]`.** `ShaderType` accepts arbitrary scalar
> fields; six `u32`s named `_pad5..pad10` serialise as six `u32` lanes
> at the same offsets the current `bytemuck::bytes_of` produces. Buffer
> stays 336 bytes, byte-identical.
>
> **Before** (`gpu_types.rs:412-546`):
>
> ```rust
> #[repr(C)]
> #[derive(Clone, Copy, Debug, Pod, Zeroable)]
> pub struct GpuGiParams {
>     pub inv_view_proj: Mat4,
>     pub view_proj: Mat4,
>     pub cam_pos_int: IVec3,
>     pub _pad0: u32,
>     pub cam_pos_frac: Vec3,
>     pub _pad1: u32,
>     pub sky_sun_dir: Vec3,
>     pub _pad2: u32,
>     pub sun_color: Vec3,
>     pub _pad3: u32,
>     pub screen_width: u32,
>     // ... 19 more u32/f32 scalars ...
>     pub flags: u32,
>     pub _pad4: u32,
>     pub taa_jitter: Vec2,
>     pub sun_shadow_taps: u32,
>     pub _pad5: u32,
>     pub _pad6: u32,
>     pub _pad7: u32,
>     pub max_ray_steps_secondary: u32,
>     pub max_ray_steps_sun: u32,
>     pub max_ray_steps_sun_secondary: u32,
>     pub max_ray_steps_visibility: u32,
>     pub spatial_iter_count: u32,
>     pub _pad8: u32,
>     pub _pad9: u32,
>     pub _pad10: u32,
> }
> ```
>
> **After** (under `#[derive(ShaderType)]` — drop the std140-natural
> `_pad0..pad4` but KEEP `_pad5..pad10`):
>
> ```rust
> #[derive(Clone, Copy, Debug, Default, ShaderType)]
> pub struct GpuGiParams {
>     pub inv_view_proj: Mat4,
>     pub view_proj: Mat4,
>     pub cam_pos_int: IVec3,
>     pub cam_pos_frac: Vec3,
>     pub sky_sun_dir: Vec3,
>     pub sun_color: Vec3,
>     pub screen_width: u32,
>     // ... 19 more u32/f32 scalars (unchanged) ...
>     pub flags: u32,
>     // encase inserts 4 B pad here to bring `taa_jitter` to offset 280
>     // (Vec2 align 8 — natural break, same role _pad4 played).
>     pub taa_jitter: Vec2,
>     pub sun_shadow_taps: u32,
>     /// Row-boundary pad — NOT std140-natural. Forces
>     /// `max_ray_steps_secondary` onto offset 304 (fresh 16-byte row)
>     /// per `21-design-quality-panel.md` §4.2 + `20-impl-phase-d-shadow-A.md`.
>     /// Kept as a named field under `ShaderType` so the offset contract
>     /// is enforced in-code rather than via WGSL+Rust co-edit.
>     pub _pad5: u32,
>     pub _pad6: u32,
>     pub _pad7: u32,
>     pub max_ray_steps_secondary: u32,
>     pub max_ray_steps_sun: u32,
>     pub max_ray_steps_sun_secondary: u32,
>     pub max_ray_steps_visibility: u32,
>     pub spatial_iter_count: u32,
>     /// Trailing pad — keeps the struct 16-byte-row-aligned at 336.
>     /// encase would round to 320 without this trio; the WGSL counterpart
>     /// `gi_params.wgsl:145-147` mirrors these as `pad_e/f/g`.
>     pub _pad8: u32,
>     pub _pad9: u32,
>     pub _pad10: u32,
> }
> ```
>
> Net: `_pad0/1/2/3/4` drop (encase re-pads at natural breaks);
> `_pad5/6/7/8/9/10` stay (named non-natural row-boundary pads).
> **WGSL `gi_params.wgsl` is untouched.** The Rust struct goes from
> 12 pad fields to 6; the WGSL struct's `pad_b..pad_g` stay where they
> are at `gi_params.wgsl:129-131,145-147`. Resulting buffer: byte-
> identical 336-byte uniform.
>
> ---
>
> #### 3.4.3 Per-struct cutover table
>
> | struct | `gpu_types.rs` lines | encoding-shape | pads dropped | pads kept | post-cutover bytes |
> |---|---|---|---|---|---|
> | `GpuRenderParams` | `59-117` | clean | `_pad0b,_pad1,_pad2,_pad3,_pad4,_pad5` (8 fields incl `Vec2`) | none | 112 |
> | `GpuCamera` | `35-49` | clean | `_pad0,_pad1` | none | 96 |
> | `GpuWorldMeta` | `155-172` | clean | `_pad0,_pad1,_pad2` | none | 48 |
> | `GpuTaaParams` | (verify lines) | clean | (verify; per existing assert `:851` total 192) | none | 192 |
> | `GpuAtmosphereParams` | (verify lines) | clean | (verify; per existing assert `:854` total 128) | none | 128 |
> | `GpuGiParams` | `412-546` | named-pad | `_pad0,_pad1,_pad2,_pad3,_pad4` | `_pad5,_pad6,_pad7,_pad8,_pad9,_pad10` | 336 |
>
> The 7th candidate, `GpuConstructionParams`, stays in this PR scope as
> `Pod` — D5's impl owns the write site (`prepare_construction` runs in
> D5 territory). D5 architect coordinates the flip in a separate
> dispatch.
>
> ---
>
> #### 3.4.4 Disposition of the 24 compile-time layout guards (`gpu_types.rs:844-902`)
>
> **Keep all 24 guards in place; do not delete any.**
>
> The original §3.4 said the guards "drop because encase enforces the
> layout at serialisation time" (`03-architecture.md:404-411`). That is
> wrong in spirit. The runtime test docblock at
> `gpu_types.rs:944-951` explicitly notes the asserts exist as defense
> against future-tooling regressions:
>
> > a refactor that adds `#[cfg(feature = ...)]` around a guard, future
> > tooling that strips `const _ = assert!(…)`, an editor auto-fix that
> > "fixes" the casts
>
> encase guarantees layout correctness *if it runs*; the compile-time
> `offset_of!` guards catch the case where it doesn't (a field gets
> renamed without `ShaderType` reflecting the rename, a future macro
> hygiene change reorders fields, a `#[cfg]` strips the derive).
>
> The cost is zero — the guards produce no runtime work, no binary size,
> and a localised compile error if the layout drifts. Drop only with an
> explicit design reason; "encase enforces it" is not sufficient.
>
> Per the named-pad recipe (3.4.2), the values asserted by the existing
> 24 guards remain correct post-cutover:
>
> - `:844` `size_of::<GpuCamera>() == 96` — holds (clean case, encase
>   produces 96).
> - `:845` `size_of::<GpuRenderParams>() == 112` — holds (clean case).
> - `:846` `size_of::<GpuWorldMeta>() == 48` — holds.
> - `:851` `size_of::<GpuTaaParams>() == 192` — holds.
> - `:854` `size_of::<GpuAtmosphereParams>() == 128` — holds.
> - `:857` `size_of::<GpuGiParams>() == 336` — **holds** (named-pad case
>   preserves the 6 pads, struct stays 336).
> - `:860`/`:861` `offset_of!(GpuGiParams, taa_jitter) == 280` — holds
>   (encase inserts 4 B at the natural `Vec2` align-8 break post-`flags`).
> - `:868`/`:869` `offset_of!(GpuGiParams, sun_shadow_taps) == 288` —
>   holds.
> - `:875`/`:877` `offset_of!(GpuGiParams, max_ray_steps_secondary) ==
>   304` — **holds** because `_pad5/6/7` are kept as named fields.
> - `:879`/`:881` `offset_of!(GpuGiParams, spatial_iter_count) == 320` —
>   holds.
> - `:886-897` `GpuConstructionParams` guards — `GpuConstructionParams`
>   stays `Pod` in D4 scope; guards stay verbatim.
> - `:900-902` entity GPU struct guards — `GpuEntityChunkInstance`,
>   `GpuEntityInstanceHistory`, `GpuChunkUpdate` stay `Pod` (D5
>   territory); guards stay verbatim.
>
> **None of the 24 asserted values change post-cutover.** The implementor
> lifts the file as-is for the guard block.
>
> The runtime mirror tests at `gpu_types.rs:911-1018` likewise stay
> verbatim (they cover D5-owned `GpuConstructionParams`,
> `GpuHashValueSlot`, `GpuBoundQueueInfo` — none of which the D4 cutover
> touches).
>
> ---
>
> #### 3.4.5 naga_oil-import-name policy
>
> **The pad-keep policy (3.4.2) sidesteps naga_oil concerns entirely.**
> No WGSL edits in this cutover. `gi_params.wgsl` stays at 156 LOC with
> `pad_a/b/c/d/e/f/g` intact (mirrors Rust `_pad4/5/6/7/8/9/10`).
>
> The `gi_params.wgsl:13-15,47` constraint
> ("naga-oil composable-module structs cannot carry the `_pad0`-style /
> `data1`-style identifiers ... naga writeback rejects trailing-digit
> identifiers and bare `_padN`") applies to **WGSL identifiers only**,
> not Rust field names. The Rust `_pad5..pad10` fields serialise as 6
> anonymous `u32` lanes in the encase output — encase does not propagate
> Rust field names into the WGSL identifier space. The
> `pipelines.rs:369` docblock confirms: bind-group size is determined by
> `min_binding_size`, which `<T as ShaderType>::min_size().get()` returns
> as a byte count, independent of names.
>
> No naga_oil verification step is required. The 5 known importers
> (`naadf_global_illum.wgsl`, `naadf_sample_refine.wgsl`,
> `naadf_spatial_resampling.wgsl`, `naadf_denoise_split.wgsl`,
> `ray_queue_calc.wgsl`) consume `gi_params.wgsl` unchanged.
>
> ---
>
> #### 3.4.6 Upload-site `write_uniform` helper (unchanged from original §3.4)
>
> One helper at `prepare/mod.rs` absorbs the encase write boilerplate;
> 6 upload sites swap to call it. See original §3.4 paragraph for
> `write_uniform<T: ShaderType + WriteInto>` signature and the affected
> upload-site table — recipe unchanged from the original §3.4.
>
> One amendment: the bind-group `min_binding_size` query at
> `pipelines.rs:372-379` (currently
> `NonZeroU64::new(std::mem::size_of::<GpuFoo>() as u64).unwrap()`)
> swaps to
> `<GpuFoo as ShaderType>::min_size()` for the 6 converted structs.
> `GpuConstructionParams`'s entry stays on `size_of`. The
> `pipelines.rs:369` docblock updates to reflect the new bucket split
> (6 are `ShaderType`, 1 stays `Pod`).
>
> ---
>
> #### 3.4.7 Implementor lift instructions (atomic, in order)
>
> 1. Add `use bevy::render::render_resource::ShaderType;` (and
>    `WriteInto` where needed) to `gpu_types.rs` + `prepare/mod.rs`.
> 2. For each of the 5 **clean** structs (`GpuRenderParams`,
>    `GpuCamera`, `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams`):
>    swap `#[repr(C)] #[derive(Pod, Zeroable)]` →
>    `#[derive(ShaderType)]` (+ keep `Clone, Copy, Debug, Default`);
>    delete every `_padN` field; field bodies + docs otherwise unchanged.
> 3. For `GpuGiParams`: swap the derive line per (2); delete
>    `_pad0/_pad1/_pad2/_pad3/_pad4` only; **keep
>    `_pad5/_pad6/_pad7/_pad8/_pad9/_pad10`** with their existing
>    docstrings (the row-boundary documentation is load-bearing for
>    future readers).
> 4. `gpu_types.rs:844-902` — **do not touch.** All 24 guards stay
>    verbatim with their existing asserted values; they continue to hold
>    post-cutover.
> 5. `gpu_types.rs:911-1018` — **do not touch.** Runtime mirror tests
>    cover D5-owned structs only.
> 6. `gi_params.wgsl` — **do not touch.** No WGSL edits in this PR.
> 7. Add `write_uniform<T: ShaderType + WriteInto>(…)` to
>    `prepare/mod.rs` per original §3.4 paragraph.
> 8. Swap 6 upload sites to `write_uniform(...)` per original §3.4
>    table. The D5-territory `GpuConstructionParams` upload site
>    (`prepare_construction`) stays on `bytemuck::bytes_of`.
> 9. `pipelines.rs:372-379` — swap `size_of::<GpuFoo>()` →
>    `<GpuFoo as ShaderType>::min_size().get()` for the 5 affected
>    bind-layout entries (camera, render_params, world_meta, taa_params,
>    atmosphere_params). `GpuGiParams`'s bind site (if separate;
>    architect verifies at edit time) flips the same way.
> 10. Update the docblock at `pipelines.rs:369` to reflect the cutover:
>     "5 D4-owned uniforms now consume `ShaderType::min_size()`;
>     `GpuGiParams` consumes `ShaderType::min_size()` too. The remaining
>     `bytemuck::bytes_of` users are `GpuConstructionParams` (D5) and the
>     packed-array structs (`GpuVoxelType`, `GpuCameraHistorySlot`,
>     `GpuSampleValid`, `GpuBucketInfo`)."
>
> Verification per original §3.4 Step 2 verification line — full lib
> tests + full e2e gate ladder; non-deterministic gates (`oasis-edit-visual`,
> `vox-gpu-oracle`) ≥3 runs per
> `feedback-multiple-runs-rule-out-false-positives` memory.

---

## Decisions & rejected alternatives

### Decision 1 — pad-keep policy: ADOPT investigator's recommendation

**Chosen:** keep `_pad5/6/7/8/9/10` on `GpuGiParams` as named `u32`
fields under `#[derive(ShaderType)]`.

**Alternative considered:** coordinated WGSL+Rust deletion — drop both
the Rust pads and the WGSL `pad_b..pad_g` fields in one atomic edit,
shrinking the buffer 336 → 320.

**Why pad-keep wins:**

1. **Blast radius.** Pad-keep touches one file (`gpu_types.rs`) and
   leaves the WGSL alone. Deletion touches two SSoT seams synchronously
   — every Rust pad has a WGSL twin at `gi_params.wgsl:129-131,145-147`.
   Two-file synchronous edits are precisely the failure mode the bail
   discipline protects against (`feedback-e2e-gates-must-fail-fast`,
   `feedback-primitives-then-analytical-invariants`).
2. **Two-SSoT-edit cost is real.** The 24 compile-time guards at
   `:844-902` and the runtime tests at `:911-1018` are the project's
   layout oracle. Deletion forces the implementor to adapt 3 guard
   values (`:857`, `:875`, `:879`) AND maintain the implicit invariant
   that the Rust-deleted-pad and WGSL-deleted-pad land in the same PR.
   Pad-keep is a one-attribute swap per struct (`Pod, Zeroable` →
   `ShaderType`) and a one-token field-delete (for the 5 natural pads).
3. **naga_oil concern.** The investigator showed the concern is
   probably benign for deletion (no new identifiers introduced — pure
   delete), but the architect should not push the implementor into a
   workspace-build sanity check they'd otherwise sidestep. The
   `gi_params.wgsl:78-84` `rand_counter_b` workaround docblock proves
   the naga_oil-identifier rule has real teeth historically; adding
   even an inert verification step costs implementor budget.
4. **The named-pad approach is well-precedented in WGSL/Rust uniform
   mirroring.** `GpuWorldMeta._pad0/1/2` at `gpu_types.rs:161-171` are
   already named std140-pad fields; `GpuConstructionParams._pad`
   pattern at `gpu_types.rs:887-897` declares row-boundary pads
   explicitly under `Pod`. Pad-keep continues the idiom under a
   different derive; deletion breaks the idiom.
5. **The cleanup win is the OTHER 5 structs.** Per the investigator's
   framing observation: `GpuGiParams` contributes 6 of the ~70 `_padN`
   fields D4 surface eliminates. The win from Item 3 is "uniform
   encoding across 6 structs," not "delete 6 more pads from the GI
   struct." Pad-keep preserves the win.

**Flip condition:** if a future quality-panel refactor needs to add or
remove `max_ray_steps_*` knobs and the row-boundary calculus changes,
the architect at that time can collapse the named pads into encase's
natural break. The current named-pad form documents the row contract
in-code, which is a feature.

### Decision 2 — coordinated WGSL+Rust deletion plan: N/A (rejected by decision 1)

No coordinated deletion fires because Decision 1 keeps the pads. This
section is preserved as a no-op in the §3.4 spec only to make the
"why not delete" trace legible to the next reader.

If a future architect decides to delete (against the recommendation),
the plan would need:

- WGSL `gi_params.wgsl:129-131` (`pad_b/c/d`) + `:145-147`
  (`pad_e/f/g`) deletion (zero readers confirmed by grep).
- Rust `gpu_types.rs:512-516` + `:541-545` deletion.
- Guard updates at `:857` (336→320), `:875` (304→292), `:879`
  (320→308).
- naga_oil verification (workspace build + `--validate-gpu-construction`
  pass) confirming the 5 known importers still compose.

This is *not* in scope for the current cutover.

### Decision 3 — cutover scope: ADOPT full 6 structs

**Chosen:** all 6 D4-owned uniforms cut over in one PR
(`GpuRenderParams`, `GpuCamera`, `GpuWorldMeta`, `GpuTaaParams`,
`GpuAtmosphereParams`, `GpuGiParams`).

**Alternative considered:** 5 structs (exclude `GpuGiParams`, leave it
`Pod` permanently or until a future PR).

**Why full 6 wins:**

1. **The "two encoding regimes" smell the impl log called out
   (`04-refactoring.md:1063-1069`) only fires if `GpuGiParams` is
   excluded.** With pad-keep enabling its inclusion, all 6 structs
   speak the same encoding contract: `ShaderType` derive, `write_uniform`
   helper, `min_size()` for bind layout.
2. **The `write_uniform` helper would have to branch per struct type
   in the partial-cutover path** (`04-refactoring.md:1069`). Full
   cutover keeps the helper monomorphic over `T: ShaderType + WriteInto`.
3. **The hazard-elimination story stays whole.** The original §3.4
   claim "the `taa_jitter` placement hazard becomes impossible by
   construction" applies to the `flags`-then-`taa_jitter` row (which
   `_pad4`'s natural break covers under encase). Partial cutover leaves
   `GpuGiParams` exposed to that hazard while every other struct gets
   the eliminator — anti-symmetry.
4. **No additional risk introduced by including `GpuGiParams` via
   pad-keep.** All asserted offsets stay; bind size stays; WGSL stays.
   The cutover is mechanically identical to the 5 clean structs plus
   a 6-field retain.

**Flip condition:** if a `cargo test` post-cutover surprises the
implementor with `GpuGiParams`-specific layout drift (e.g. encase
chooses a different `Vec3` align rule than the std140 walk
predicts), the implementor can stage-2-fallback to 5-of-6 and flag
`GpuGiParams` for follow-up. The architect's expectation is this won't
fire — encase v0.12 is std140-conservative per the investigator's
walk and the 5 clean cases mirror the same vec3-trail-of-row pattern.

### Decision 4 — 24 compile-time guards disposition: ADOPT keep-all

**Chosen:** keep all 24 `const _: () = assert!(offset_of!(...))` and
`assert!(size_of!(...))` guards at `gpu_types.rs:844-902` verbatim.

**Alternative considered:** drop the guards per original §3.4's "encase
enforces the layout at serialisation time" rationale
(`03-architecture.md:404-411`).

**Why keep-all wins:**

1. **The guards exist explicitly as future-tooling-regression defense,
   not as encase replacement.** The runtime test docblock at
   `gpu_types.rs:944-951` documents this verbatim: a `#[cfg(feature)]`
   strip, a future macro hygiene change, an editor auto-fix can all
   silently disable the encase derive without disabling the
   `offset_of!` asserts. The asserts are an independent layer of
   defense.
2. **Cost is zero.** Compile-time asserts produce no runtime work, no
   binary size, no test overhead. They fire as a localised compile
   error with a stable line number if layout drifts. There is no
   maintenance cost a delete would relieve.
3. **The asserted values do not change under the named-pad recipe.**
   `size_of::<GpuGiParams>() == 336` holds (6 pads kept). All `offset_of!`
   values hold (`taa_jitter == 280`, `sun_shadow_taps == 288`,
   `max_ray_steps_secondary == 304`, `spatial_iter_count == 320`). The
   guard block needs zero edits.
4. **encase is not the layout authority — the WGSL declaration is.**
   The guards pin the Rust struct's offsets against the WGSL counterpart
   `gi_params.wgsl:50-148`. If encase's derive ever diverges from the
   WGSL std140 contract (e.g. a future encase version changes a
   `Vec3` packing rule), the guards catch it before the GPU bind reads
   garbage. encase enforcing layout *at serialisation time* is a
   different property from guards enforcing layout *at compile time of
   the Rust struct*.

**Flip condition:** if a future major encase release explicitly takes
ownership of layout invariants and the guards become genuinely
redundant, the architect at that time can revisit. The current state
of the world (precedent-setting first consumer, the encase issue
tracker noting vec3-packing edge cases referenced in the investigator's
walk note at `02-investigation-item-3-gpugiparams.md:131-135`) makes
that revisit premature.

---

## Assumptions made

1. **encase v0.12.0 (`Cargo.lock:2535`) places `Vec3` as size=12,
   align=16 for purposes of the next-field offset** — i.e. a `Vec3`
   followed by a `u32` field puts the `u32` at `vec3_offset + 16`, not
   `+ 12`. This is the conservative std140 rule the investigator's walk
   (`02-investigation-item-3-gpugiparams.md:99-135`) hand-derived and
   the existing `_pad0..pad3` u32 fields mirror. The implementor must
   verify this via the post-cutover `cargo build` against the existing
   guards; if it diverges, Decision 3's flip condition fires.

2. **The 5 clean structs (`GpuRenderParams`, `GpuCamera`,
   `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams`) all reduce to
   byte-identical buffers post-pad-drop.** The investigator hand-walked
   `GpuRenderParams` (the exemplar) and the audit
   (`00-reuse-audit.md:154-163`) cited the same conclusion for the
   other 4 via the impl log
   (`04-refactoring.md:1022-1028`). The architect did not
   independently re-walk `GpuTaaParams` and `GpuAtmosphereParams` —
   relying on the existing compile-time size guards at `:851` and
   `:854` to enforce this at edit time.

3. **`pad_b..pad_g` have zero WGSL readers other than their own
   declarations.** Verified by `grep -rn "pad_b\|pad_c\|pad_d\|pad_e\|pad_f\|pad_g"
   /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/assets/shaders/`
   — only 7 hits in `gi_params.wgsl` (declarations + the docblock at
   `:126`). One hit in `atmosphere.wgsl:56` is a different identifier
   (`pad_cam`). This makes Decision 2's deletion-path verification
   step trivial; it also makes Decision 1's pad-keep safe (no consumer
   would break if the architect ever did flip to deletion later).

4. **Bevy 0.19 makes `ShaderType` available via
   `bevy::render::render_resource::ShaderType`.** Per the original §3.4
   `use bevy::render::render_resource::ShaderType;` import line; not
   independently verified by the architect via `cargo doc`. If the
   import path differs (e.g. needs `bevy::render::extract_resource`),
   the implementor adjusts at edit time — the rule is mechanical.

5. **`write_uniform<T: ShaderType + WriteInto>` from original §3.4 §3.4.6
   compiles as-written.** The architect did not verify the `WriteInto`
   bound is the correct trait alias against Bevy 0.19's encase
   re-export. If the trait-bound name differs, the implementor adjusts.

6. **The implementor will not absorb the WGSL→Rust pad-name semantic
   delta into the named-pad case.** I.e. the implementor keeps the
   existing `_pad5/6/7/8/9/10` Rust names rather than renaming them to
   match the WGSL `pad_b/c/d/e/f/g`. Encase ignores field names for
   wire-layout purposes (`pipelines.rs:369` docblock confirms the
   bind-size query is name-independent), so the names can stay either
   way; the architect picks "leave the names alone" for minimum
   blast-radius. Implementor should not gold-plate a rename.

---

## Side notes / observations / complaints

- **The orchestration's framing is partially miscalibrated.** Item 3
  is described as "ShaderType cutover for `GpuGiParams`" — but the
  blocking decision is **whether to keep the named pads**, not whether
  to do the cutover. The recommended path lands the cutover with all 6
  pads kept; net pad-count delta on `GpuGiParams` is zero. The
  cleanup win lives in the OTHER 5 structs (drop ~8+2+3+(taa)+(atm)
  pads) and in the unification of encoding across 6 structs. If the
  orchestration's goal is "reduce `_padN` count," Item 3 contributes
  ~9% of that goal regardless of path choice. If the goal is "uniform
  encoding across the D4 uniform surface," Item 3's pad-keep path
  achieves it fully. The investigator surfaced this as well
  (`02-investigation-item-3-gpugiparams.md:567-577`). **Recommended
  reframing:** Item 3's value is "single encoding regime, 6 structs,
  one helper" — not "delete 6 more pads."

- **The original §3.4 has a subtle structural problem the
  pad-keep path papers over rather than fixes.** The §3.4 recipe
  conflates two distinct concerns: (a) "use `ShaderType` to eliminate
  the `vec3`-then-scalar trap" (real win — encase enforces alignment
  at edit time), and (b) "delete all `_padN` fields" (a derivation, not
  a goal). The named-pad approach explicitly separates these:
  ShaderType serves goal (a) for every struct; pad-deletion serves
  goal (b) only where the pad is std140-natural. The architect should
  consider making this split structural in `gpu_types.rs` going
  forward — explicit `// row-boundary pad (non-natural)` comments on
  `GpuGiParams._pad5..pad10` would distinguish them from future
  natural pads.

- **The 24 compile-time guards are the most under-appreciated piece
  of layout-correctness infrastructure in the codebase.** They survived
  the `taa_jitter`-offset-280 trap (`gpu_types.rs:838-867` comments
  document the trap hitting 3× before the explicit pin). The
  original §3.4's "drop them, encase enforces it" stance treats them as
  belt-and-suspenders redundancy with encase. They are not — they're
  upstream of encase, validating the Rust source layout before encase
  even runs. Dropping them would shift the defense-in-depth posture
  from compile-time to runtime-only (the runtime mirror tests at
  `:911-1018`). The named-pad path preserves both layers intact.

- **Item 3 IS the codebase's first `ShaderType` consumer.**
  `grep -rn ShaderType /mnt/archive4/DEV/bevy-naadf/crates/` returns
  one hit — the docblock at `pipelines.rs:369`. This means the cutover
  is precedent-setting: every downstream design (`write_uniform`
  helper shape, `min_size()` vs `size_of`, derive-bound ordering,
  named-pad-vs-deleted-pad idiom) becomes the template the next
  `ShaderType` user copies. The named-pad approach as the precedent is
  *better* than a deleted-pad precedent — it shows the next reader
  both halves of the recipe (clean case + row-boundary case) rather
  than over-specifying that all pads must disappear. The two-exemplar
  §3.4 makes this explicit; future ShaderType users see both shapes.

- **The implementor's bail was correct discipline, not a failure
  mode.** Per `02-investigation-item-3-gpugiparams.md:236-258`, the
  bail correctly identified that picking between (a)/(b)/(c) is an
  architect-level decision. The framing "bailed (per safety rule)"
  was the right move — re-dispatching without architect revision
  would have produced a third bail. The orchestration's hard-gate
  pause is the system working as designed.

- **No source code modified by this architect revision.** Per the
  brief's read-only constraint on source. This file is the only
  deliverable; the implementor's downstream PR lifts §3.4 verbatim
  from `## Revised §3.4 spec (the deliverable)` above.

- **One pre-existing inconsistency observed but not in scope.** The
  `GpuConstructionParams` runtime mirror test at
  `gpu_types.rs:953-1004` asserts internal field offsets (`hash_map_size`
  at 36, `segment_size_in_chunks` at 40, etc.) that the compile-time
  guards at `:886-897` don't cover. The runtime test is the more
  comprehensive of the two; the compile-time guards are intentionally
  the row-boundary subset. This is a pre-existing design choice
  (compile-time guards = `vec3` row-boundary safety; runtime mirror =
  full per-field coverage), not a bug. Flagged here because the same
  asymmetry should apply to any new `GpuGiParams` tests the implementor
  adds — pin row boundaries at compile time, mirror the full layout at
  runtime only if a regression risk justifies the test code.
