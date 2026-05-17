# Context bundle — `web-chunks-storage-buffer`

> Required reading for the `delegate-consolidated` agent before any design or
> implementation work. This file is self-contained: every fact the agent
> needs is inlined here or at a named file:line on disk.

## Goal (verbatim user words)

> "is it possible to make this work on webGPU on the web without sacrificing
> something?" → user picked Option A: "lets do option A" — replace the
> `chunks` 3D `Rg32Uint` storage texture (`texture_storage_3d<rg32uint,
> read_write>` in 4 construction WGSL shaders + `texture_3d<u32>` in 2 render
> WGSL shaders) with a flat WebGPU-spec-compliant **storage buffer**
> (`array<vec2<u32>>` indexed by `flatten_index(chunk_pos, sx, sx*sy)`).

The WebGPU spec only permits `StorageTextureAccess::ReadWrite` on the
`r32{uint,sint,float}` allow-list. `Rg32Uint` `read_write` works on native
wgpu (Vulkan/Metal/DX12) via `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`
but is rejected by Chrome's Dawn WebGPU implementation as of Chrome 148. The
e2e smoke test (`just test-wasm-full`) currently fails with a downstream
`DeviceLost: Destroyed` after these three bind-group layouts fail validation:

- `naadf_construction_world_bind_group_layout` (`render/construction/chunk_calc.rs:60-88`)
- `naadf_construction_bounds_world_bind_group_layout` (`render/construction/bounds_calc.rs:70-89`)
- `naadf_entity_world_bind_group_layout` (`render/construction/entity_update.rs:78-94`)

Headed-chrome console dump captured the validation error verbatim:

```
Caught rendering error: Texture format TextureFormat::RG32Uint does not
support storage texture access StorageTextureAccess::ReadWrite.
 - While validating entries[0]
 - While validating [BindGroupLayoutDescriptor "naadf_construction_world_bind_group_layout"]
 - While calling [Device].CreateBindGroupLayout(…)
```

## User decisions from the Step 4 Q&A

| Question | Decision | Why it's load-bearing |
|---|---|---|
| Execution mode | **Consolidated** | One uninterrupted trace; design space already tightly constrained by the audit. |
| Fixture scope | **All 5 sites lockstep** (production `prepare.rs` + 4 test fixtures in `bounds_calc/tests.rs`, `construction/world_change.rs`, `construction/mod.rs` ×2) | The 3 construction layouts alias the same chunks resource; partial migration leaves wgpu unable to bind both descriptor shapes on the same allocation. `cargo test` must stay green at the end of the dispatch. |
| Verification gates (all 4 required) | `cargo test --workspace --lib`, `just web-build`, `just test-wasm-full`, `cargo run --bin e2e_render -- <mode>` (multiple modes — see "Verification" below) | The WebGPU gate (`just test-wasm-full`) is the load-bearing proof; the others guard against native regressions. |
| Stride source | **Read `world_meta.size_in_chunks` inline at each call site** — `flatten_index(chunk_pos, world_meta.size_in_chunks.x, world_meta.size_in_chunks.x * world_meta.size_in_chunks.y)`. Construction shaders read `params.size_in_chunks` analogously. | No new uniform fields. Matches existing precedent at `chunk_calc.wgsl:347` (`params.segment_size_in_chunks`). |

## Audit summary (full table in `00-reuse-audit.md`)

The audit found **nothing greenfield is required**. Every primitive the
migration needs already exists in the codebase:

1. **`flatten_index(pos, stride_y, stride_z)` helper** at
   `crates/bevy_naadf/src/assets/shaders/common.wgsl:32-34` — exact x-fastest
   formula `pos.z * stride_z + pos.y * stride_y + pos.x`. Already imported
   into `ray_tracing.wgsl:35`. Call as
   `flatten_index(chunk_pos, sx, sx * sy)` — note `stride_y = sx` and
   `stride_z = sx * sy`.
2. **`array<vec2<u32>>` storage-buffer precedents** —
   `pipelines.rs:350-353` (frame-data: `taa_sample_accum`,
   `first_hit_absorption`, `final_color`), `world_change.wgsl:131`
   (`changed_chunks_dynamic`), `entity_update.wgsl:81`
   (`chunk_updates_dynamic`). Identical element type to the new chunks
   binding.
3. **Fixed-size buffer creation precedent** — `prepare.rs:404-441`
   (`world_meta` uniform + `placeholder_entity_*` storage buffers,
   `STORAGE | COPY_DST`, `mapped_at_creation: false`, then
   `queue.write_buffer` + `.as_entire_buffer_binding()`).
4. **`GrowableBuffer<T>` is NOT the right tool** — chunks is fixed-size at
   world build; bare `device.create_buffer` + `queue.write_buffer` is the
   right shape.
5. **CPU side already speaks `[u32; 2]` per chunk** — `aadf/edit.rs:21-59`
   already treats the GPU readback as a flat `[[u32; 2]]` slice keyed by
   linear chunk index. The migration *simplifies* the readback path
   (removes `bytes_per_row` row padding).
6. **W4's design-doc trace** (`docs/orchestrate/naadf-bevy-port/15-design-c.md`
   §1.7, §6 assumption #6 + `23-design-bevy-018-downgrade.md` §5) already
   documented that `Rg32Uint` `STORAGE_READ_WRITE` is **NOT** in the wgpu
   guaranteed feature set — only available via
   `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`. **The buffer
   migration eliminates the dependency on that extension entirely.** W4's
   `.x`/`.y` field-selector discipline carries forward byte-for-byte under
   `array<vec2<u32>>`.

## Required reading (in order, before any edits)

1. **`docs/orchestrate/web-chunks-storage-buffer/00-reuse-audit.md` —
   READ IN FULL.** Especially the `## Quoted precedents` section: it
   contains the exact WGSL/Rust snippets to mirror for the new bindings,
   the `flatten_index` signature, the precedent buffer-creation calls, and
   all 5 fixture sites.
2. **`crates/bevy_naadf/src/render/prepare.rs:165-478`** — the production
   `prepare_world_gpu` system. The single seam that owns the chunks
   resource. Lines `251-307` build the texture; lines `404-441` build the
   `world_meta` + placeholder buffers (the precedent to mirror). Line
   `448-461` builds the world bind group; the `chunks_view` reference at
   `:452` becomes `chunks_buffer.as_entire_buffer_binding()`. Field
   declarations: `WorldGpu` struct around `:55-90` (drop
   `chunks: Texture`, `chunks_view: TextureView`; add `chunks_buffer: Buffer`).
3. **`crates/bevy_naadf/src/render/pipelines.rs:312-331`** — the
   `world_layout` slot-0 declaration. Flip `texture_3d(TextureSampleType::Uint)`
   to `storage_buffer_read_only_sized(false, None)`.
4. **`crates/bevy_naadf/src/render/construction/chunk_calc.rs:60-88`,
   `bounds_calc.rs:70-89`, `entity_update.rs:78-94`** — the three
   construction layout descriptors. Flip binding 0 from
   `texture_storage_3d(TextureFormat::Rg32Uint, StorageTextureAccess::ReadWrite)`
   to `storage_buffer_sized(false, None)` in each.
5. **WGSL shaders (6 files):**
   - `crates/bevy_naadf/src/assets/shaders/world_data.wgsl:43-54` — flip
     `var chunks: texture_3d<u32>;` to
     `var<storage, read> chunks: array<vec2<u32>>;`. Consumed by every
     render-side reader.
   - `crates/bevy_naadf/src/assets/shaders/ray_tracing.wgsl:283-295` — one
     read site: `textureLoad(chunks, vec3<i32>(chunk_pos), 0)` becomes
     `chunks[flatten_index(chunk_pos, world_meta.size_in_chunks.x, world_meta.size_in_chunks.x * world_meta.size_in_chunks.y)]`,
     returning `vec2<u32>` directly (no longer `vec4<u32>`); `.x`/`.y` field
     selectors still apply.
   - `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:97` — flip
     `texture_storage_3d<rg32uint, read_write>` to
     `var<storage, read_write> chunks: array<vec2<u32>>;`. Update the one
     write at `:414`:
     `textureStore(chunks, vec3<i32>(chunk_pos), vec4<u32>(state, 0u, 0u, 0u))`
     becomes
     `chunks[flatten_index(vec3<u32>(chunk_pos), params.size_in_chunks.x, params.size_in_chunks.x * params.size_in_chunks.y)] = vec2<u32>(state, 0u);`.
     **NB**: the original write zeroed `.y`, but W4's discipline is
     `.y`-preserving. Verify against `15-design-c.md` §1.7: this write
     fires at chunk-build time when there are no entities yet, so `.y = 0`
     is correct. Keep the W4 comment chain intact.
   - `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:98, 210, 357,
     394` — one binding flip + read at `:210, :357` + write at `:394`. Same
     pattern; uses `params.size_in_chunks` for stride.
   - `crates/bevy_naadf/src/assets/shaders/entity_update.wgsl:76, 107, 108`
     — one binding flip + read at `:107` + write at `:108`. The write is
     `.y`-only via a read-modify-write that **preserves `.x`** — this is
     load-bearing per W4.
   - `crates/bevy_naadf/src/assets/shaders/world_change.wgsl:110, 317, 376,
     443, 445` — one binding flip + 2 reads + 2 writes. Writes preserve
     either `.x` or `.y` per W4 (see line comments around `:316-376`).
6. **Test fixtures (4 sites, all flip lockstep):**
   - `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:480-516`
     — W3 fixture.
   - `crates/bevy_naadf/src/render/construction/world_change.rs:662-698` —
     W2 fixture (creates the test-side chunks texture for `apply_changes`).
   - `crates/bevy_naadf/src/render/construction/mod.rs:2473-2515` — W1
     validation fixture.
   - `crates/bevy_naadf/src/render/construction/mod.rs:3819-3866` — second
     validation fixture (around the read-back assertions).
   - `crates/bevy_naadf/src/render/construction/mod.rs:4453-…` — fifth
     site; verify the surrounding context (it may be an `Rg32Uint` view
     creation rather than a fresh allocation — handle accordingly).
7. **Readback path** —
   `crates/bevy_naadf/src/render/construction/mod.rs:3599-3666` (`read_chunks_texture_to_cpu`-style fn) +
   `crates/bevy_naadf/src/render/construction/world_change.rs:584-672` (W2
   read-back). With buffer source, the row-padded `bytes_per_row.next_multiple_of(256)`
   walk collapses to a flat `bytemuck::cast_slice::<u8, [u32; 2]>`
   memcpy; the readback staging buffer stays buffer-shaped. Simplification,
   not invention.
8. **CPU consumer** — `crates/bevy_naadf/src/aadf/edit.rs:21-59, 582` —
   already speaks `[[u32; 2]]` per chunk; verify the migration changes
   nothing on this side (it shouldn't — the layout was already a flat
   `[[u32; 2]]`).
9. **W4 design-doc trace** —
   `docs/orchestrate/naadf-bevy-port/15-design-c.md` §1.7, §6 assumption #6
   (chunks widening intent + the explicit caveat about
   `STORAGE_READ_WRITE` not being in the WebGPU spec) +
   `docs/orchestrate/naadf-bevy-port/23-design-bevy-018-downgrade.md` §5
   (the format-features audit naming this exact gap). Honour W4's `.x`/`.y`
   field-selector discipline. The migration is a **representation change,
   not a semantic change**.

## Forbidden moves

1. **Never run `cargo run --bin bevy-naadf` as a verification step** (project
   CLAUDE.md, binding rule). It boots a windowed app for 30s and proves
   nothing. The verification surface is `cargo build`, `cargo test
   --workspace --lib`, and `cargo run --bin e2e_render -- <mode>` (the e2e
   gates). The user does the live visual check on the binary themselves.
2. **Don't invent a new linear-index helper.** `flatten_index` already
   exists in `common.wgsl:32` and is the audit's named reuse target. Don't
   add `chunks_index(coord)` or `linear_chunk_idx(coord)` on top of it.
3. **Don't gate the migration on a wgpu feature flag.** The whole point of
   the buffer migration is to eliminate the dependency on
   `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`. The new buffer
   binding is bare WebGPU-spec-compliant.
4. **Don't break W4's `.x`/`.y` field discipline.** Every existing
   read/write site has explicit `.x` or `.y` field selectors. The buffer
   migration preserves them byte-for-byte. Don't conflate `.x` and `.y`,
   don't drop the entity-preserve writes (`chunk_calc.wgsl:414`,
   `world_change.wgsl:376`, `entity_update.wgsl:108`, `bounds_calc.wgsl:394`).
5. **Don't switch to `GrowableBuffer<[u32; 2]>` for chunks.** The audit's
   borderline-call analysis explicitly rejected it: chunks is fixed-size at
   world build; `GrowableBuffer`'s growth/headroom semantics are dead code
   for this use case. Use plain `Buffer` + `queue.write_buffer`.
6. **Don't make `WorldGpu` carry both the texture and the buffer "for
   compatibility".** Hard cut: drop the `chunks` and `chunks_view` fields,
   replace with `chunks_buffer: Buffer`. Every consumer references one
   resource.
7. **No partial migration.** All 5 fixture sites + 3 construction layouts +
   1 renderer layout + 6 WGSL shaders flip together. `cargo test` must be
   green at the end of the dispatch (user decision).
8. **Don't pre-empt visual verification.** If a runtime behaviour needs to
   be proven and isn't covered by an existing e2e gate, the correct move
   is to add a new gate to `e2e_render`, not boot the binary. (Project
   CLAUDE.md `## Verification discipline` is binding.)

## Verification gates (consolidated agent must run these before completing)

Per the Step 4 Q&A, all four are required:

1. **`cargo test --workspace --lib`** — must pass. Confirms construction
   logic (W1/W2/W3/W4 dispatches, AADF math, batch encoder) behaves
   identically with the buffer representation. ~184 tests.
2. **`just web-build`** — must compile cleanly. Confirms the wasm32 target
   builds with the new bindings.
3. **`just test-wasm-full`** — must pass. The load-bearing WebGPU gate:
   `e2e/tests/wasm-smoke.spec.ts` boots the wasm app under headless Chrome
   and asserts no `bevy.error`-typed console output (the patched
   `ConsoleCollector` catches Bevy's `%cERROR%`-styled `console.log`
   entries from `tracing-wasm`). **A clean pass here is the proof the
   migration achieved its goal.**
4. **`cargo run --bin e2e_render -- <mode>`** — at minimum the following
   modes to cover the chunks-touching dispatch paths:
   - `cargo run --bin e2e_render` (the default `baseline` mode)
   - `cargo run --bin e2e_render -- --validate-gpu-construction` (W1
     CPU/GPU oracle byte-equality; exercises every construction shader)
   - `cargo run --bin e2e_render -- --edit-mode` (W2 path)
   - `cargo run --bin e2e_render -- --entities` (W4 entity-update path)
   - `cargo run --bin e2e_render -- --oasis-edit-visual` (the canonical
     framebuffer-diff gate)
   - `cargo run --bin e2e_render -- --runtime-edit-mode` (runtime edit path)

If a gate fails, **investigate the root cause** (project rule: there is no
such thing as a pre-existing failure). Fix and re-run. The implementation
log records every gate's pass/fail status.

## Deliverable shape (in `02-design-impl.md`)

The consolidated agent flushes each stage to `02-design-impl.md` as it
goes:

```
# 02 — Design + self-review + implementation log

## Design
<the architecture: bind-group layout deltas, WGSL binding declarations, the
flatten_index call-site shape, the resource creation diff, the readback
simplification, the WorldGpu field rename>

## Decisions & rejected alternatives
<numbered decisions, each with the rejected alternatives and one-line why-not>

## Assumptions made
<numbered assumptions about codebase behaviour that the consolidated agent
took on faith; e.g. "assumed bevy_render's `as_entire_buffer_binding()` is
8-byte-element-stride safe">

## Independent review (<ISO date>)
<the agent's self-review of the design against the success criteria;
explicitly flags anything high-risk it considers worth escalating to a
fresh-eyes delegate-reviewer dispatch>

## Implementation log (<ISO date>)
<file-by-file change summary, each gate's pass/fail status, any deviations
from the design and why>
```

## Success criteria (used by the agent's self-review)

1. All 6 WGSL shaders compile against the new bind-group layouts; no
   `texture_storage_3d<rg32uint, read_write>` or `texture_3d<u32>`
   references remain for `chunks`.
2. `WorldGpu.chunks` and `WorldGpu.chunks_view` are gone; `WorldGpu.chunks_buffer:
   Buffer` is the single source of truth. The 3 construction bind-group
   builders and the `world_layout` bind group all bind the same buffer
   resource.
3. All 5 fixture sites use the buffer representation. `cargo test
   --workspace --lib` is green.
4. `just web-build` compiles cleanly.
5. `just test-wasm-full` passes — no `DeviceLost`, no Bevy `%cERROR%`-marker
   entries from the renderer.
6. The named `cargo run --bin e2e_render -- <mode>` gates all exit `Ok`.
7. W4's `.x`/`.y` field discipline is preserved byte-for-byte at every
   read/write site (the `.y`-preserve writes in `world_change`,
   `entity_update`, `bounds_calc`, `chunk_calc` still preserve the right
   channel).
8. The dispatch produces no warnings about unused imports
   (`texture_storage_3d`, `StorageTextureAccess`, `TextureFormat::Rg32Uint`
   should be entirely removed from the construction Rust sources, not just
   the call sites).
