# 01b — wgpu/Vulkan storage-texture barrier hazard: prior-art research

**Date:** 2026-05-15
**Author:** delegated wgpu-barrier-research sub-agent (read-only on code).
**Scope:** prior art and technical-options inventory for B-7 (the
`naadf_gpu_producer_node` → renderer storage-vs-sampled visibility hazard).
**Pinned versions:** `bevy = 0.19.0-rc.1`, `wgpu = 29.0.3` (per
`Cargo.lock`).

This document is research, not design. A downstream architect agent turns
the ranked option list in §4 into a concrete chosen-design with code.

## wgpu-barrier-research findings (2026-05-15)

## 1. Problem statement (refined)

### 1.1 The texture, the two views, the two pass families

The chunks 3D texture is a single `wgpu::Texture` created in
`prepare_world_gpu` at
`crates/bevy_naadf/src/render/prepare.rs:223-238` with:

```
TextureFormat::Rg32Uint
TextureDimension::D3
size = (size_in_chunks.x, .y, .z)
usage = TEXTURE_BINDING | COPY_DST | STORAGE_BINDING
```

The same function then creates a single default `TextureView` and stashes it
on `WorldGpu` as `chunks_view`
(`crates/bevy_naadf/src/render/prepare.rs:280`).

**The view is bound under two different binding types in two different
bind-group layouts:**

- **Renderer side** — `NaadfPipelines::world_layout` slot 0 is
  `texture_3d(TextureSampleType::Uint)`
  (`crates/bevy_naadf/src/render/pipelines.rs:317`), consumed in WGSL as
  `@group(0) @binding(0) var chunks: texture_3d<u32>;`
  (`crates/bevy_naadf/src/assets/shaders/world_data.wgsl:54`) by every
  render-side `shoot_ray` consumer (`naadf_first_hit_node`,
  `naadf_global_illum_node`, `naadf_spatial_resampling_node`, etc.).
- **Construction side** — `construction_world_layout` slot 0 is
  `texture_storage_3d(TextureFormat::Rg32Uint, StorageTextureAccess::ReadWrite)`
  (`crates/bevy_naadf/src/render/construction/chunk_calc.rs:69`), consumed
  in WGSL as a `texture_storage_3d<rg32uint, read_write>` and written by
  `textureStore` (see `chunk_calc.wgsl:414`, `world_change.wgsl`,
  `entity_update.wgsl`). The construction-side bind group is built in
  `render::construction::mod.rs:1444-1463`, and at line 1444 a *fresh*
  `chunks.create_view(&TextureViewDescriptor::default())` is taken
  expressly to "let the construction writes land" — the dual-view
  workaround from Phase-C followup #1.

### 1.2 The producer node sits in the same `RenderContext`

`naadf_gpu_producer_node` is a system added to the `Core3d` schedule
(`crates/bevy_naadf/src/render/mod.rs:279`) at the head of the chained
NAADF render-graph systems. It receives a `RenderContext` as a system
parameter and records its three dispatches via
`render_context.command_encoder()`
(`crates/bevy_naadf/src/render/construction/mod.rs:1874`). In Bevy 0.19,
`RenderContext::command_encoder()` returns the single shared
`CommandEncoder` that *every* render-graph system in the same render-frame
records into until the encoder is flushed
([Bevy PR 7248 — Support recording multiple CommandBuffers in
RenderContext][bevy-pr-7248]).

Each producer dispatch helper
(`chunk_calc::dispatch_calc_block_from_raw_data_world_sized`,
`dispatch_compute_voxel_bounds`, `dispatch_compute_block_bounds`) begins
its **own compute pass** via `encoder.begin_compute_pass(...)`
(`crates/bevy_naadf/src/render/construction/chunk_calc.rs:200, 224, 244`).
`naadf_first_hit_node` also begins its own compute pass
(`crates/bevy_naadf/src/render/graph.rs:98`). So between the producer's
last `textureStore(chunks, ...)` and the renderer's first
`textureLoad(chunks, ...)` there are **four compute-pass boundaries**
inside a single shared `CommandEncoder` — and additionally a few storage
buffer touches (the W3 bounds-init seed) on a *separate* encoder/submit
(`crates/bevy_naadf/src/render/construction/mod.rs:1200-1211`), gated to
not race the producer.

### 1.3 The symptom and the gap-doc hypothesis

With `gpu_producer_skip_upload = false` (current default in
`prepare.rs:203`): CPU upload writes the chunks texture with the
construct-CPU oracle output; producer dispatches still fire; renderer
reads the CPU upload; framebuffer is correct (emissive 247, solid 242,
sky 145.9 at `e2e_render` baseline).

With `gpu_producer_skip_upload = true` (the flag flipped in
`prepare.rs:203`, the goal of Phase-C followup #1): chunks texture is
zero-initialised via `queue.write_texture` of zero data; producer
dispatches fire (logs "GPU producer chain DISPATCHED"); renderer reads
this texture; **framebuffer collapses** (emissive 10.7, solid 7.0,
geometry vanishes) — i.e. the renderer sees the zeroed texture, not the
producer's writes.

`12-alignment-gap.md` §4 B-7 hypothesises:

> Likely needs an explicit pipeline barrier between regime-1 GPU
> dispatch and the first render frame OR a different bind-group aliasing
> strategy.

`prepare.rs:189-202` records the same suspicion verbatim:

> the pure-GPU producer path […] does NOT propagate the storage-texture
> writes through to the renderer's reads — likely a wgpu/Vulkan
> storage→sampled barrier hazard with the chunks-texture being aliased
> to both `texture_storage_3d<rg32uint, read_write>` (construction bind
> group) and `texture_3d<u32>` (renderer bind group).

### 1.4 Assessment of the hypothesis

After reading the wgpu / WebGPU sync-model literature (§2 below), the
"missing pipeline barrier" framing is **unlikely to be the root cause as
stated**, because:

1. wgpu 29's tracker correctly de-duplicates `TextureView` objects to
   their parent `Arc<Texture>` via `view.parent` and `view.selector`
   (`wgpu_core::track::texture::TextureViewBindGroupState::add_single`
   does `self.merge_single(&view.parent, Some(view.selector.clone()),
   *usage)`) — two views of the same texture are NOT seen as separate
   resources. Aliasing is therefore tracked.
2. wgpu auto-inserts the storage→sampled image-layout transition at
   compute-pass boundaries (WebGPU [sync-scope][webgpu-sync-scope] model;
   wgpu inherits this — see [gpuweb/gpuweb#59][gpuweb-59], [gfx-rs/wgpu
   #724 / PR #1157][wgpu-pr-1157]). Crossing into a new compute pass
   gives wgpu the opportunity to flush.
3. The producer node and every renderer node share the same
   `RenderContext::command_encoder()` (§1.2), so the transition fits
   inside one `CommandEncoder.finish()` — not cross-submit.

**Re-stated hypothesis (more likely):** the issue is not "wgpu fails to
insert a barrier" but **"the producer's outputs are not what the renderer
needs to see when CPU upload is skipped."** Concretely:

- The producer's `calc_block_from_raw_data` writes every chunk's `.x`
  channel — `textureStore(chunks, vec3<i32>(chunk_pos), vec4<u32>(state,
  0u, 0u, 0u))` at `chunk_calc.wgsl:414`. The dispatch shape is the
  world's chunk extent (`dispatch_calc_block_from_raw_data_world_sized`
  at `chunk_calc.rs:194-211`), so every chunk position is covered.
- The producer's `compute_voxel_bounds` / `compute_block_bounds` write
  the 5-bit AADFs into the `.x` bits — they are also dispatched at
  upper-bound block / chunk counts (`mod.rs:1871-1872`).
- But the producer dispatches against a `segment_voxel_buffer` built
  from `ExtractedWorld::dense_voxel_types` (`mod.rs:935-960`) — this is
  the CPU-side dense volume mirror. **It is non-empty only when the test
  scene's `setup_test_grid` system has authored
  `WorldData::dense_voxel_types`.** The producer node *itself* short-circuits
  when `extracted_world.dense_voxel_types.is_empty()`
  (`mod.rs:1841-1846`).
- **And here is the deviation residual:** `validate_gpu_construction`
  populates `dense_voxel_types` on a 1×1×1 fixture and byte-equals 388
  bytes against the CPU oracle (gate PASS). The e2e harness ALSO
  populates `dense_voxel_types` via `setup_test_grid` (so the producer
  dispatches every run, log fires). But the e2e gate verifies the
  GPU/CPU outputs match on the fixture, NOT that the runtime producer's
  GPU output equals the CPU `construct()` output the renderer would
  read when CPU upload is enabled.

**Two specific suspects to verify (not in scope of this research, but the
candidate fixes §4 should aim to discriminate):**

- **Suspect A:** producer writes are landing but to a value that
  visually-collapses the framebuffer — e.g. the dispatch shape doesn't
  cover all chunks on the 4×2×4 test world (the cubic-vs-world-sized
  fix from Phase-C followup #1 already addressed one case of this — see
  `chunk_calc.rs:185-211` and `16-impl-c-followups.md` T1 decision
  "Dispatch shape (cubic vs world-sized)"). If the producer's `.x`
  encoding doesn't match what the renderer's `chunks_decode_state`
  expects, all chunks read as `BLOCK_STATE_UNIFORM_EMPTY` and geometry
  vanishes.
- **Suspect B:** the barrier IS being inserted, but the renderer's read
  goes through a stale TextureView object whose Vulkan
  `VkImageViewCreateInfo.usage` was implicitly stamped at first-bind
  time to `SAMPLED` (Vulkan 1.2+ `VK_KHR_maintenance2` allows narrowing
  view usage). Mention only — wgpu doesn't expose this, and ash inputs
  for a default view leave usage as the texture's full usage mask, so
  this is unlikely.

The gap-doc's "missing pipeline barrier" framing is the right *symptom*
to fix (the visible signal is "writes don't propagate") but the root
cause has not been forensically established. **Option (a) and (b) below
should be tried first** because they're cheap to verify, **and (e)
adds a diagnostic step that discriminates Suspect A from Suspect B
without committing to a fix.**

## 2. wgpu barrier model — the relevant facts

`Cargo.lock` pins:

- `wgpu = 29.0.3`
- `wgpu-core = 29.0.3`
- `wgpu-hal = 29.0.3`

### 2.1 Sync scope, usage scope, pass boundaries

WebGPU defines a [sync scope / usage scope][webgpu-spec-sync-scopes]:
within one "usage scope" (a render pass or a single compute dispatch),
the set of resource usages must be a "compatible usage list" — overlapping
writes / reads must not conflict.

For storage resources, **WebGPU explicitly says UAVs are NOT synchronised
between consecutive dispatches in the same compute pass** ([gpuweb/gpuweb
issue #59 — "Proposal: allow storage to be unsynchronized within a
compute pass"][gpuweb-59], adopted into the spec):

> the user is expected to place memory barriers if they want to serialize
> the side effects.

But **at the boundary of a compute pass**, the API has explicit knowledge
of "this resource was written, that resource will be read" and inserts the
barrier automatically. See gpuweb #59:

> an implementation can figure out the possible hazards and insert
> appropriate barriers automatically.

So opening a new `ComputePass` ends the producer's usage scope, the
tracker observes the storage-write, and the next compute pass that binds
the resource as sampled triggers wgpu to emit a storage→sampled image
layout transition + a memory barrier on the Vulkan backend.

### 2.2 wgpu's automatic synchronisation surface

`wgpu::CommandEncoder` exposes one explicit API:

```rust
pub fn transition_resources<'a>(
    &mut self,
    buffer_transitions: impl Iterator<Item = BufferTransition<&'a Buffer>>,
    texture_transitions: impl Iterator<Item = TextureTransition<&'a Texture>>,
)
```

documented as ([wgpu 29 CommandEncoder][wgpu-cmdenc-docs]):

> Transition resources to an underlying hal resource state. This is an
> advanced, native-only API (no-op on web) that has two main use cases…
> wgpu inserts automatic barrier command buffers between user submissions
> when resource states need adjustment.

`transition_resources` is **only** for batching cross-submit barriers
into one place; it does NOT bypass the implicit sync scope. There is no
"emit a UAV barrier here" knob.

### 2.3 Tracker behaviour with aliased views

`wgpu-core 29` ([source][wgpu-track-texture]) tracks textures by the
parent `Arc<Texture>` and a `TextureSelector` (mip + array layer range).
`TextureViewBindGroupState::add_single` extracts the parent texture from
the view and inserts into the unified state under `view.parent`:

```text
unsafe { self.merge_single(&view.parent, Some(view.selector.clone()), *usage)? }
```

Two `TextureView`s of the same parent collapse to the same tracker entry
(by index). The tracker does see `STORAGE_READ_WRITE` and `RESOURCE`
(sampled) as conflicting usages **inside one usage scope** and as
requiring a transition **across usage scopes**.

### 2.4 `STORAGE_READ_WRITE` is an exclusive bit

`STORAGE_READ_WRITE` is *exclusive within a usage scope* — see the wgpu
forum response on ["share a texture across compute and render pipeline
in wgpu"][rust-forum-share-tex]:

> Storage read/write is an exclusive usage that cannot coexist with
> other usages in the same scope.

Practical consequence: the port already does the right thing by
**binding via two different bind groups for two different passes**. The
[forum response][rust-forum-share-tex] also confirms the
recommended pattern is exactly the port's pattern (the dual-view fix
landed at `mod.rs:1434-1446`):

> separate bind groups allow wgpu to insert the "memory barrier" between
> them that tells the hardware that writes from the compute pass will be
> read by the render pass.

That sentence is the key external evidence that the port's bind-group
shape is the recommended one — so option (b) "use a single bind group
for both usages" is NOT viable; it's actually the *opposite* of best
practice.

### 2.5 Image-layout consistency (the historical bug, already fixed)

[wgpu#724 — "Image layout consistency between STORAGE and
SAMPLED"][wgpu-724] is the historical Vulkan-validation bug:

> When a texture is used as both SAMPLED and STORAGE_READ in a render
> pass (inside a sync scope), the only layout compatible with both is
> General.

Fixed in [PR #1157][wgpu-pr-1157] (merged 2021-01-19). Since
~wgpu-0.7, a texture created with `STORAGE_BINDING` is auto-promoted to
Vulkan `VK_IMAGE_LAYOUT_GENERAL` even when bound as `SAMPLED` only.
This means the renderer's read view of our `Rg32Uint` chunks texture
is already in `GENERAL` layout (the texture has `STORAGE_BINDING` in
its usage mask). Therefore Vulkan layout transitions are NOT happening
between the construction and render binds; only the memory barrier
remains.

### 2.6 No explicit barrier API for the in-frame case

There is no `wgpu::CommandEncoder::pipeline_barrier(...)` or similar.
The model is implicit: open a new compute pass, change the binding
type → wgpu's tracker emits a Vulkan barrier as the compute pass
encoder is recorded.

## 3. Bevy 0.19 render-graph implications

### 3.1 Render-graph systems share one `CommandEncoder`

The port adds NAADF nodes to the `Core3d` schedule via `add_systems` —
they are render-graph SYSTEMS, not `Node` types (see
`crates/bevy_naadf/src/render/mod.rs:272-300`). Bevy injects
`RenderContext` as a system parameter; from
[bevy 0.19 RenderContext docs][bevy-rendercontext-docs]:

> Returns the current command encoder, creating one if it does not
> already exist.

`add_command_buffer` can interrupt the encoder and append a finished
`CommandBuffer`, after which a fresh encoder starts. The port does NOT
call `add_command_buffer` from the producer node, so the producer's
three compute passes and the renderer's compute passes (atmosphere,
first-hit, …) **all live in one encoder, one submit**. wgpu's
intra-encoder tracking has full visibility.

### 3.2 Phase-C followup #1 already chose this pattern

`16-impl-c-followups.md` T1 documents the decision:

> Moving the dispatch into a render-graph node uses the same
> `CommandEncoder` as the renderer's reads, letting wgpu/Vulkan's
> intra-encoder barrier insertion serialise the storage→sampled
> transition.

That decision is *correct* against the wgpu sync model (see §2 above).
The hazard the comment describes ("the storage-buffer writes propagate;
the storage-texture writes don't") only makes sense if either:

- (i) wgpu's texture tracker has a known mis-tracking bug that the
  storage-buffer tracker does not have — but the §2.3 source-walk
  shows the texture tracker resolves to the parent, same as the buffer
  tracker; or
- (ii) the producer's outputs are not in fact correct (Suspect A in §1.4).

### 3.3 Known Bevy issues touching this surface

- [bevy #16003][bevy-issue-16003] — "Label the command encoder of the
  RenderContext". Cosmetic; confirms single-encoder semantics.
- [bevy #5042][bevy-issue-5042] — "Parallelize the core pipeline passes
  with wgpu's RenderBundles". Discussion confirms the render-graph is
  currently single-threaded with one encoder.
- [bevy #5062][bevy-issue-5062] — "Render Graph as Systems". The
  refactor that produced the system-based pattern the port uses; same
  encoder semantics.
- No open Bevy issue I could find specifically claims "writes from a
  custom compute-system render node are invisible to a downstream
  render node within the same Core3d frame". This is consistent with
  the wgpu sync model: it should just work.

The `bevy_solari` crate's clustered-forward node ordering provides one
concrete prior art — the `solari_lighting_node` writes to a 3D world
probe texture and the downstream lighting pass reads it; same physical
texture, two different views. This works without explicit barriers
([bevyengine/bevy bevy_solari source][bevy-solari-source] — the node
graph just chains them and trusts wgpu's tracker). The pattern is
identical to ours.

## 4. Candidate fixes — ranked

Ranked best-first. Effort estimates are small (≤2h) / medium (≤1d) /
large (≥1d).

### (e) Diagnostic: dump the chunks texture mid-frame  **[recommended FIRST]**

**Mechanism.** Add a `--validate-gpu-producer-runtime` mode to
`e2e_render` that:
1. boots with `gpu_producer_skip_upload = true`,
2. lets one frame run end-to-end (producer + bounds_init + renderer),
3. reads back the `world_gpu.chunks` texture (via `COPY_SRC` →
   `copy_texture_to_buffer` → `map_async`),
4. byte-compares against the CPU `construct()` output the renderer
   *would* have seen with CPU upload enabled.

If the readback matches the CPU oracle → wgpu IS propagating the writes
and the geometry collapse is downstream (Suspect A — likely the
producer's outputs are *valid but different* from the CPU oracle in
some subtle field the renderer depends on; e.g. block-pointer
numbering, AADF bit-positions, voxel-buffer indexing — see W1
Assumption #7 in `12-alignment-gap.md`). Fix moves into "make the
producer outputs renderer-compatible."

If the readback is zeros → wgpu is NOT propagating the writes (the
gap-doc's hypothesis is correct). Fix moves into options (a) / (c) /
(d).

**Effort.** Small (~2h, mirrors the existing `--validate-gpu-construction`
plumbing in `e2e_render`).

**Risk.** Zero — read-only.

**Verifiability.** Direct: the readback is a 388-byte (1×1×1) or larger
buffer; bit-compare is trivial. Then visual on the e2e screenshot.

**Reference implementation.** The existing
`--validate-gpu-construction` mode (`crates/bevy_naadf/src/render/
construction/mod.rs:2300-2580`) already does GPU→CPU readback +
byte-compare; just lift the pattern onto the production `WorldGpu`
buffers and run on frame 2.

### (a) Explicit `transition_resources` after the producer node  **[likely a no-op, but cheap to disprove]**

**Mechanism.** At the end of `naadf_gpu_producer_node`, call
`render_context.command_encoder().transition_resources(...)` with a
`TextureTransition` from `STORAGE_READ_WRITE` → `RESOURCE` (sampled) on
`world_gpu.chunks`. This forces wgpu to emit the barrier at a known
point.

**Effort.** Small (~1h).

**Risk.** Low. `transition_resources` is documented as the explicit
batching API; calling it should be idempotent if wgpu's auto-insertion
already did the job. Cannot break the CPU-upload fallback path because
the producer node early-exits when `gpu_construction_enabled = false`.

**Verifiability.** With `gpu_producer_skip_upload = true`, run e2e and
check the framebuffer luminance gates. If geometry returns →
barrier-omission was real. If not → §1.4 Suspect A.

**Reference implementation.** [wgpu 29 transition_resources
docs][wgpu-cmdenc-docs]. Caveat: no Bevy plugin in the wild uses this
API for this purpose (the auto-tracker is usually sufficient); we'd be
exploring slightly new territory. The wgpu wiki and forum (§2.4 quote)
recommend "separate bind groups" — which the port already has — as the
correct mechanism.

### (b) Single bind group with dual access  **[NOT RECOMMENDED — anti-pattern]**

**Mechanism.** Replace the two bind groups with one binding that
satisfies both usages. In WGSL this would mean either:
(b.1) declare the renderer's `chunks` as
`texture_storage_3d<rg32uint, read>` (read-only storage) and use
`textureLoad` instead of `textureSample` — but the renderer already
uses `textureLoad` exclusively (it's a 3D unsigned-integer texture, no
sampler). The bind-group-layout would become
`StorageTextureAccess::ReadOnly` for the renderer side.
(b.2) Keep the renderer's sampled bind, and additionally bind the
chunks as a *second* slot with storage access in the construction
pipeline only.

**Effort.** Medium (~1d — touches every consumer WGSL file +
`world_data.wgsl`'s `@group(0) @binding(0)`).

**Risk.** **High and discouraged.** §2.4's forum response is explicit:
"separate bind groups allow wgpu to insert the 'memory barrier'." A
single bind group is the WRONG direction. `STORAGE_READ_WRITE` is
exclusive in a usage scope (§2.4) — you cannot bind a texture as
storage AND as sampled in the same compute pass. b.1 (read-only
storage on the renderer side) is technically valid but:
- requires `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` for
  `Rg32Uint` read-only storage on some backends;
- forfeits the renderer-side sampled-binding's hardware
  cache/path which is generally faster than storage on read;
- still requires the cross-pass barrier (storage-write → storage-read
  is also a usage transition).

It also breaks the architectural symmetry the port has spent four
workstreams building.

**Verifiability.** Same as (a).

**Reference implementation.** None — the port's two-bind-group pattern
matches every prior-art voxel/compute pipeline I could find
(bevy_solari, bevy-pbr clustered forward). C# NAADF uses ONE
`Texture3D<uint>` resource shared by D3D11 SRV (sampled) + UAV (read-
write), but D3D11 implicit-barrier semantics make that orthogonal
to wgpu (§5 below).

### (c) Render-graph node ordering: explicit edges, not implicit chain  **[unlikely to help on its own]**

**Mechanism.** The port currently relies on Bevy's `.chain()` to give
the producer-before-first-hit ordering
(`crates/bevy_naadf/src/render/mod.rs:271-300`). `.chain()` enforces
*system* ordering inside the schedule, which transitively gives
encoder-record ordering, which IS what wgpu's tracker needs. So
ordering is already correct. Switching to manual
`add_render_graph_edges` (the Bevy 0.18-style render-graph) would
NOT improve sync — it would just change the API surface; the
tracker still operates on the recorded encoder commands.

**Effort.** Medium (~1d — port from system-based to Node-based render
graph; touching ~15 nodes).

**Risk.** Medium — a structural refactor for likely no benefit on the
sync axis. Would also re-open D3 ("Solari strip-vs-dormant") era
choices for no good reason.

**Verifiability.** Same as (a).

**Reference implementation.** [Bevy custom-post-processing
example][bevy-custom-pp-example] shows the node-based pattern.

### (d) Submission-fence approach: producer in its own submit + wait  **[overkill, may regress fps]**

**Mechanism.** Move the producer dispatches into `prepare_construction`
via `render_device.create_command_encoder()` + `render_queue.submit()`
+ `device.poll(Maintain::Wait)`. The poll ensures the GPU has finished
the producer's writes before the renderer's encoder is recorded.

**Effort.** Medium (~half a day).

**Risk.** High — `Maintain::Wait` introduces a CPU-GPU pipeline stall
every startup (the producer is one-shot, so it's a one-time cost), but
losing the same-encoder optimisation means the producer's writes
become a cross-submit transition (wgpu still inserts the barrier
automatically, just at submission boundary instead of compute-pass
boundary). The same-encoder approach was the *result* of Phase-C
followup #1's investigation — moving back to separate-submit was
explicitly tried and found insufficient (see
`16-impl-c-followups.md` T1 decisions section: "the chunk_calc chain's
`texture_storage_3d` writes to chunks did NOT propagate via
separate-encoder submit to the renderer's `texture_3d` reads"). So
(d) is RE-TRYING WHAT ALREADY FAILED unless paired with a
`Maintain::Wait` synchronisation — which is the new ingredient.

**Verifiability.** Same as (a).

**Reference implementation.** `gpu_readback.rs` Bevy example
([gpu_readback.rs][bevy-gpu-readback]) uses `poll(Maintain::Wait)` for
GPU→CPU sync; same primitive.

### Summary ranking

| # | Option | Effort | Risk | Verdict |
|---|---|---|---|---|
| (e) | Read-back diagnostic | small | none | **try first — discriminates root cause** |
| (a) | Explicit `transition_resources` after producer | small | low | **try second; cheap to add** |
| (b) | Single bind group | medium | high | DO NOT — anti-pattern per §2.4 |
| (c) | Manual render-graph edges | medium | medium | unlikely to help; do not start here |
| (d) | Separate submit + `Maintain::Wait` | medium | medium | re-tries failed pattern; only if (e)+(a) fail |

## 5. What C# / MonoGame / DX11 does

`NAADF/World/Data/WorldData.cs:200-207`:

```cs
chunkProcessor.Parameters["blocks"].SetValue(dataBlockGpu.GetBuffer());
chunkProcessor.Parameters["voxels"].SetValue(dataVoxelGpu.GetBuffer());

chunkProcessor.Techniques[0].Passes["ComputeVoxelBounds"].ApplyCompute();
App.graphicsDevice.DispatchCompute((int)(voxelCount / 64), 1, 1);

chunkProcessor.Techniques[0].Passes["ComputeBlockBounds"].ApplyCompute();
App.graphicsDevice.DispatchCompute((int)(blockCount / 64), 1, 1);

blockHashingHandler.SyncGpuToCpu();          // ← CPU readback BLOCKS until GPU done
initialVoxelCompressionFactor = blockHashingHandler.GetCompressionFactor();
sw.Stop();
Console.WriteLine("Construction time: " + sw.Elapsed.TotalMilliseconds + " milliseconds");
```

**Two crucial observations:**

1. **C# does NOT emit explicit barriers between the producer dispatches
   and the renderer's reads.** DX11's deferred-context model auto-
   inserts UAV barriers between consecutive Dispatch calls that touch
   the same UAV resource (DX11 implicit hazard tracking). MonoGame's
   `DispatchCompute` is a thin wrapper over D3D11
   `ID3D11DeviceContext::Dispatch`, which uses DX11's implicit
   hazard tracker. So C# gets the equivalent of wgpu's auto-
   barrier-at-pass-boundary, for free.
2. **C# completes all construction synchronously, before rendering
   starts.** The flow is:
   - `Construction time: <ms>` is printed BEFORE `RenderInternal` runs
     for frame 0.
   - `blockHashingHandler.SyncGpuToCpu()` performs a blocking GPU→CPU
     readback — D3D11's `Map` with `D3D11_MAP_READ` stalls until
     prior dispatches finish.
   - Only after this returns does `isLoaded = true`, and only after
     that does the render loop pick up `data` and start dispatching
     `firstHitEffect` etc.

`WorldRenderBase.cs:RenderInternal` (lines 173-470 covered in §1.7 of
the port's `12-alignment-gap.md`) **never re-dispatches the
construction passes** — they run once at world load and the output
remains static (W2 edits and W3 background bounds are separately
dispatched as part of frame-time updates, but those are different
passes that take the already-built chunks texture as input).

**Therefore C# gets two free benefits the port does not:**

- (a) DX11 implicit UAV barriers replace the explicit "open a new
  compute pass" mechanism the port needs.
- (b) DX11 + the blocking readback creates a hard CPU-GPU sync fence
  before the renderer ever runs, eliminating any race between the
  producer's writes and the renderer's reads.

For the faithful-port rule: **(b) is the canonical reference design**.
The port should architecturally mirror it (produce-then-render with a
synchronisation point), not "barrier-inside-the-render-frame", which is
a port artefact of pushing the construction dispatch into the per-frame
render graph. **Phase-C followup #1's choice to put the producer in the
render graph was the deviation from C#**; the gap-doc honest residual
records this.

The C# `chunkCalc.fx` shader (`NAADF/Content/shaders/world/data/chunkCalc.fx`)
is the source the port WGSL `chunk_calc.wgsl` is transliterated from —
the algorithm matches verbatim; the difference is *only* the
dispatch-time orchestration (one-shot pre-render in C# vs. per-frame
render-graph node in port).

## 6. Recommended next step

**The architect agent should dispatch option (e) FIRST — the read-back
diagnostic — to determine whether wgpu is failing to propagate writes
(Suspect B) or the producer's outputs are subtly wrong vs the CPU
oracle (Suspect A). Only one of those is "a wgpu barrier hazard"; the
other is a Phase-C consumer-symmetry bug masquerading as one.**

Specifically:

1. Land a `--validate-gpu-producer-runtime` mode in `e2e_render` that
   reads back `world_gpu.chunks` after frame 0 (with
   `gpu_producer_skip_upload = true`) and byte-compares to
   `WorldData::construct()`'s CPU oracle output. **Estimated effort: 2h.**
   This is read-only diagnostic plumbing; it CANNOT regress.
2. If the readback matches the CPU oracle: the wgpu barrier is fine.
   The bug is elsewhere — likely in the unit-pointer-numbering
   semantics that diverge between CPU `HashMap` iteration order and
   GPU open-addressing-by-hash (W1 Assumption #7 in `12-alignment-gap.md`).
   The renderer's `chunks_decode_state` + `blocks[...]` /
   `voxels[...]` lookups depend on consistent pointer numbering
   between producer and consumer. Phase-D then becomes "make the GPU
   producer's outputs renderer-compatible against any
   pointer-numbering assignment" — not a barrier fix at all.
3. If the readback returns zeros (or any value clearly distinct from
   the CPU oracle): the wgpu barrier hypothesis is confirmed. Land
   option (a) — call `transition_resources` at the end of
   `naadf_gpu_producer_node` — and re-measure.
4. If (a) does not fix it, the C# pattern (option that mirrors §5:
   one-shot startup dispatch + `Maintain::Wait` fence + CPU upload
   becomes pure fallback) is the architecturally-faithful next step.
   This is option (d) reformulated as "run the producer ONCE at
   `Startup` (mirroring `WorldData.cs` flow), fence, then never run
   again." It would preserve the CPU-upload path as the E4 fallback
   per `01-context.md` §2e.

**Constraints honoured:**

- All four options preserve `cpu_fallback = true` and
  `prepare_world_gpu`'s CPU-upload code path — the renderer reads from
  `WorldGpu` regardless of which producer filled it. The faithful-port
  rule is satisfied: the C# `set_voxel`+`UpdateWorld` editing path
  (already ported in W2) keeps working unchanged.
- No proposed fix modifies the C# reference.
- Versions cited from `Cargo.lock`: `wgpu = 29.0.3`, `bevy = 0.19.0-rc.1`.

---

[bevy-pr-7248]: https://github.com/bevyengine/bevy/pull/7248
[webgpu-sync-scope]: https://www.w3.org/TR/webgpu/#programming-model-synchronization
[webgpu-spec-sync-scopes]: https://www.w3.org/TR/webgpu/#programming-model-synchronization
[gpuweb-59]: https://github.com/gpuweb/gpuweb/issues/59
[wgpu-pr-1157]: https://github.com/gfx-rs/wgpu/pull/1157
[wgpu-cmdenc-docs]: https://docs.rs/wgpu/29.0.3/wgpu/struct.CommandEncoder.html
[wgpu-track-texture]: https://github.com/gfx-rs/wgpu/blob/v29/wgpu-core/src/track/texture.rs
[rust-forum-share-tex]: https://users.rust-lang.org/t/how-to-share-a-texture-across-compute-and-render-pipeline-in-wgpu/125393
[wgpu-724]: https://github.com/gfx-rs/wgpu/issues/724
[bevy-rendercontext-docs]: https://docs.rs/bevy_render/0.19.0-rc.1/bevy_render/renderer/struct.RenderContext.html
[bevy-issue-16003]: https://github.com/bevyengine/bevy/issues/16003
[bevy-issue-5042]: https://github.com/bevyengine/bevy/issues/5042
[bevy-issue-5062]: https://github.com/bevyengine/bevy/issues/5062
[bevy-solari-source]: https://github.com/bevyengine/bevy/tree/main/crates/bevy_solari
[bevy-custom-pp-example]: https://bevy.org/examples/shaders/custom-post-processing/
[bevy-gpu-readback]: https://docs.rs/bevy/latest/src/gpu_readback/gpu_readback.rs.html
