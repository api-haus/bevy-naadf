# Diagnostics-package research + design

This document scopes the data-collection apparatus for the wasm32/WebGPU
chunk-AADF non-determinism investigation. It is a research+design pass — no
code changes, no builds run. The next dispatch implements what is specified
here.

## Source versions (pinned)

| Component | Version | Source |
|-----------|---------|--------|
| wgpu / wgpu-core / wgpu-hal / wgpu-types | `29.0.3` | `Cargo.lock` lines `wgpu = "29.0.3"` (4 packages) |
| bevy / bevy_render | `0.19.0-rc.1` | `Cargo.lock` |
| WebGPU spec revision examined | Editor's Draft (gpuweb.github.io) + W3C TR snapshot, 2026-05 | https://gpuweb.github.io/gpuweb/ , https://www.w3.org/TR/webgpu/ |
| WGSL spec revision examined | Editor's Draft (gpuweb.github.io), 2026-05 | https://gpuweb.github.io/gpuweb/wgsl/ |
| Bevy `RenderDevice` source | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.0-rc.1/src/renderer/render_device.rs` |
| Bevy `RenderAdapter` / `RenderAdapterInfo` / `RenderInstance` / `RenderQueue` | `…/bevy_render-0.19.0-rc.1/src/renderer/mod.rs:125-139` |
| wgpu `Adapter` / `Device` accessors | `…/wgpu-29.0.3/src/api/adapter.rs`, `…/wgpu-29.0.3/src/api/device.rs` |
| `Limits` / `Features` / `AdapterInfo` / `DownlevelCapabilities` types | `…/wgpu-types-29.0.3/src/{limits,features,adapter}.rs` |
| Existing probe persistence | `e2e/tests/vox-horizon-parity.spec.ts:215-308` (the `[aadf-probe]` console-filter → `target/e2e-screenshots/vox_horizon_{native,web}.aadf-probe.log` pair) |
| Existing Q4 binding-size logger | `crates/bevy_naadf/src/render/prepare.rs:535-571` |
| Existing diagnostics file | `crates/bevy_naadf/src/diagnostics.rs` (Press-`P` runtime dump; `Update` schedule, main-world resources only — does NOT touch `RenderApp`) |

The diagnostic surface enumerated in §A is the **wgpu 29 / WebGPU spec
2026-05** surface; later wgpu major versions add/rename fields and several
features have moved into / out of `Features::all_webgpu_mask()` across the
29 → 30 → 31 series. Pinning matters because the impl agent will read live
values from the running crate, not from docs.rs.

---

## A — wgpu diagnostic surface map

### A.1 Adapter-level read-only surface (`wgpu::Adapter`)

Reached via `bevy::render::renderer::RenderAdapter` (a `Resource` in the
`RenderApp`). `RenderAdapter` is a tuple-newtype around
`Arc<WgpuWrapper<wgpu::Adapter>>` — no methods of its own, deref to the
inner `Adapter`.

| Source | Method/field | Type | Available in | Diagnostic value |
|---|---|---|---|---|
| `Adapter` | `get_info()` | `AdapterInfo` | both | name, vendor PCI id, device PCI id, device_pci_bus_id, driver, driver_info, backend, subgroup_min_size, subgroup_max_size, transient_saves_memory, device_type (`Other`/`IntegratedGpu`/`DiscreteGpu`/`VirtualGpu`/`Cpu`) — see §A.5 |
| `Adapter` | `features()` | `Features` | both | bitmask of every feature the adapter exposes — see §A.4 |
| `Adapter` | `limits()` | `Limits` | both | best limits the adapter could grant — see §A.2 |
| `Adapter` | `get_downlevel_capabilities()` | `DownlevelCapabilities` | both | `{flags: DownlevelFlags, limits: DownlevelLimits, shader_model: ShaderModel}` — see §A.3 |
| `Adapter` | `get_texture_format_features(fmt)` | `TextureFormatFeatures` | both | per-format usages + flags. We only care about a few formats — see §A.6 |
| `Adapter` | `is_surface_supported(&surface)` | `bool` | both | irrelevant for compute-only diagnostics |
| `Adapter` | `get_presentation_timestamp()` | `PresentationTimestamp` | both | nanosecond-resolution timestamp at the time of call; pair with `Instant::now()` to translate timestamp-query results to wall clock |
| `Adapter` | `cooperative_matrix_properties()` | `Vec<CooperativeMatrixProperties>` | native only (gated on `EXPERIMENTAL_COOPERATIVE_MATRIX`) | irrelevant to the bug |
| `Adapter` | `as_hal::<A>()` | `Option<impl Deref<Target = A::Adapter>>` | **native only** (`#[cfg(wgpu_core)]`) | escape hatch to the raw Vulkan/Metal/DX12 adapter; not callable on `wasm32` because there is no `wgpu_core` underneath |

### A.2 `wgpu_types::Limits` — every field (29.0.3)

`Limits` is a `#[repr(C)]` struct of `u32` and `u64` fields, all `pub`.
Source: `wgpu-types-29.0.3/src/limits.rs:123-300`. Every field listed,
no abbreviation:

| Field | Type | Spec default | Notes |
|---|---|---|---|
| `max_texture_dimension_1d` | u32 | 8192 | |
| `max_texture_dimension_2d` | u32 | 8192 | |
| `max_texture_dimension_3d` | u32 | 2048 | |
| `max_texture_array_layers` | u32 | 256 | |
| `max_bind_groups` | u32 | 4 | |
| `max_bindings_per_bind_group` | u32 | 1000 | |
| `max_dynamic_uniform_buffers_per_pipeline_layout` | u32 | 8 | |
| `max_dynamic_storage_buffers_per_pipeline_layout` | u32 | 4 | |
| `max_sampled_textures_per_shader_stage` | u32 | 16 | |
| `max_samplers_per_shader_stage` | u32 | 16 | |
| `max_storage_buffers_per_shader_stage` | u32 | 8 | NAADF compute pipelines bind ≥6 storage buffers per stage; relevant. |
| `max_storage_textures_per_shader_stage` | u32 | 4 | |
| `max_uniform_buffers_per_shader_stage` | u32 | 12 | |
| `max_binding_array_elements_per_shader_stage` | u32 | 0 / 500000 | |
| `max_binding_array_acceleration_structure_elements_per_shader_stage` | u32 | 0 | |
| `max_binding_array_sampler_elements_per_shader_stage` | u32 | 0 / 1000 | |
| `max_uniform_buffer_binding_size` | u64 | 64 KiB | |
| `max_storage_buffer_binding_size` | u64 | 128 MiB | **Already logged by Q4 instr.** Dawn reports 2 GiB − 4 here; the Q4 hypothesis is REFUTED. |
| `max_vertex_buffers` | u32 | 8 | |
| `max_buffer_size` | u64 | 256 MiB | NAADF voxels buffer is 1024 MiB; this is the underlying allocator cap. |
| `max_vertex_attributes` | u32 | 16 | |
| `max_vertex_buffer_array_stride` | u32 | 2048 | |
| `max_inter_stage_shader_variables` | u32 | 16 | |
| `min_uniform_buffer_offset_alignment` | u32 | 256 | |
| `min_storage_buffer_offset_alignment` | u32 | 256 | |
| `max_color_attachments` | u32 | — | |
| `max_color_attachment_bytes_per_sample` | u32 | 32 | |
| `max_compute_workgroup_storage_size` | u32 | 16384 | per-workgroup shared memory; bounds_calc uses `workgroup<atomic<u32>>` — relevant. |
| `max_compute_invocations_per_workgroup` | u32 | 256 | NAADF bound shaders use `@workgroup_size(64)` — well under. |
| `max_compute_workgroup_size_x` | u32 | 256 | |
| `max_compute_workgroup_size_y` | u32 | 256 | |
| `max_compute_workgroup_size_z` | u32 | 64 | |
| `max_compute_workgroups_per_dimension` | u32 | 65535 | **Load-bearing for this bug.** `compute_voxel_bounds` dispatch is 134M groups; if the device caps at 65 535 (spec default) the dispatch is silently clipped. Spec default is the WebGPU minimum guarantee, not the actual Dawn value — must read at runtime. |
| `max_immediate_size` | u32 | 0 | feature-gated |
| `max_non_sampler_bindings` | u32 | 1_000_000 | DX12-only |
| `max_task_mesh_workgroup_total_count` | u32 | 0 | mesh-shader gated |
| `max_task_mesh_workgroups_per_dimension` | u32 | 0 | mesh-shader gated |
| `max_task_invocations_per_workgroup` | u32 | 0 | mesh-shader gated |
| `max_task_invocations_per_dimension` | u32 | 0 | mesh-shader gated |
| `max_mesh_invocations_per_workgroup` | u32 | 0 | mesh-shader gated |
| `max_mesh_invocations_per_dimension` | u32 | 0 | mesh-shader gated |
| `max_task_payload_size` | u32 | 0 | mesh-shader gated |
| `max_mesh_output_vertices` | u32 | 0 | mesh-shader gated |
| `max_mesh_output_primitives` | u32 | 0 | mesh-shader gated |
| `max_mesh_output_layers` | u32 | 0 | mesh-shader gated |
| `max_mesh_multiview_view_count` | u32 | 0 | mesh-shader gated |
| `max_blas_primitive_count` | u32 | 0 | ray-query gated |
| `max_blas_geometry_count` | u32 | 0 | ray-query gated |
| `max_tlas_instance_count` | u32 | 0 | ray-query gated |
| `max_acceleration_structures_per_shader_stage` | u32 | 0 | ray-query gated |
| `max_multiview_view_count` | u32 | 0 | multiview gated |

**Highlighted as plausibly-load-bearing for the symptom** (the next dispatch
will weight these in its diagnosis):

- `max_compute_workgroups_per_dimension` — dispatch-size cap. Symptom is
  consistent with the **134M-workgroup `compute_voxel_bounds`** dispatch
  being silently clipped on web. The handoff says split-by-8 and split-by-128
  variants were tried and SSIM noise-moved (0.789→0.811, 0.793) — but
  noise-moved is not equal to "exhaustively refuted". If Dawn caps at, say,
  16 777 215 (2²⁴ − 1) the cap is hit on web and not on native and the
  symptom could still be partially explained.
- `max_storage_buffer_binding_size` — already logged, already refuted, keep
  logging for regression detection.
- `max_buffer_size` — orthogonal allocator cap; not yet logged.
- `max_compute_workgroup_storage_size` — bounds_calc declares
  `var<workgroup> any_bounds_increase: atomic<u32>` (4 B) — well under.
- `max_storage_buffers_per_shader_stage` — `naadf_bounds_compute_node` binds
  groups 0+1+2 with 8+ storage buffers; if Dawn's per-stage cap is lower
  than the layout demands, binding silently no-ops on the wasm side.

### A.3 `DownlevelCapabilities`

| Field | Type | Notes |
|---|---|---|
| `flags` | `DownlevelFlags` (u32 bitflags) | see below |
| `limits` | `DownlevelLimits` | currently empty struct in 29.0.3 |
| `shader_model` | `ShaderModel` | enum `Sm2`/`Sm4`/`Sm5` |

`DownlevelFlags` (every flag — `wgpu-types-29.0.3/src/limits.rs:840-968`):

`COMPUTE_SHADERS`, `FRAGMENT_WRITABLE_STORAGE`, `INDIRECT_EXECUTION`,
`BASE_VERTEX`, `READ_ONLY_DEPTH_STENCIL`,
`NON_POWER_OF_TWO_MIPMAPPED_TEXTURES`, `CUBE_ARRAY_TEXTURES`,
`COMPARISON_SAMPLERS`, `INDEPENDENT_BLEND`, `VERTEX_STORAGE`,
`ANISOTROPIC_FILTERING`, `FRAGMENT_STORAGE`, `MULTISAMPLED_SHADING`,
`DEPTH_TEXTURE_AND_BUFFER_COPIES`, `WEBGPU_TEXTURE_FORMAT_SUPPORT`,
`BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED`, `UNRESTRICTED_INDEX_BUFFER`,
`FULL_DRAW_INDEX_UINT32`, `DEPTH_BIAS_CLAMP`, `VIEW_FORMATS`,
`UNRESTRICTED_EXTERNAL_TEXTURE_COPIES`, `SURFACE_VIEW_FORMATS`,
`NONBLOCKING_QUERY_RESOLVE`, `SHADER_F16_IN_F32`.

`is_webgpu_compliant()` returns `self.flags.contains(DownlevelFlags::compliant()) && self.limits == DownlevelLimits::default() && self.shader_model >= Sm5`.
Log this boolean — if Dawn reports the adapter as non-compliant we should
know it.

`INDIRECT_EXECUTION` is the critical one for this bug: the handoff cites
"storage→indirect barrier on `bound_dispatch` buffer" as a working
hypothesis. If `INDIRECT_EXECUTION` is `true` on both targets the bug is
not gross-feature-absence; the divergence is in *how* indirect is
implemented underneath.

### A.4 `Features` — every flag exposed in wgpu 29

`Features` is split into `FeaturesWGPU` (native-extension features) and
`FeaturesWebGPU` (the upstream WebGPU spec set). Both are bitflags.
Source: `wgpu-types-29.0.3/src/features.rs:611-1788`. Names below are the
SCREAMING_SNAKE_CASE Rust constants; the kebab-case spec name follows in
parens when different.

#### `FeaturesWebGPU` (web+native)

`DEPTH_CLIP_CONTROL` (`depth-clip-control`),
`DEPTH32FLOAT_STENCIL8` (`depth32float-stencil8`),
`TEXTURE_COMPRESSION_BC` (`texture-compression-bc`),
`TEXTURE_COMPRESSION_BC_SLICED_3D` (`texture-compression-bc-sliced-3d`),
`TEXTURE_COMPRESSION_ETC2` (`texture-compression-etc2`),
`TEXTURE_COMPRESSION_ASTC` (`texture-compression-astc`),
`TEXTURE_COMPRESSION_ASTC_SLICED_3D` (`texture-compression-astc-sliced-3d`),
`TIMESTAMP_QUERY` (`timestamp-query`),
`INDIRECT_FIRST_INSTANCE` (`indirect-first-instance`),
`SHADER_F16` (`shader-f16`),
`RG11B10UFLOAT_RENDERABLE` (`rg11b10ufloat-renderable`),
`BGRA8UNORM_STORAGE` (`bgra8unorm-storage`),
`FLOAT32_FILTERABLE` (`float32-filterable`),
`FLOAT32_BLENDABLE` (`float32-blendable`),
`DUAL_SOURCE_BLENDING` (`dual-source-blending`),
`CLIP_DISTANCES` (`clip-distances`),
`IMMEDIATES` (`immediates`),
`PRIMITIVE_INDEX` (`primitive-index`).

**`TIMESTAMP_QUERY` is the load-bearing one for §C runtime instrumentation.**
If Dawn supports it on the wasm target (it is in the WebGPU spec but Chrome
gates it behind `--enable-unsafe-webgpu`/dev flags — which the project's
`web-static` recipe already sets, `justfile:131-132`), we can write GPU
timestamps inside the bounds_calc pass and measure actual on-device timing
of each round.

#### `FeaturesWGPU` (mostly native-only)

`SHADER_FLOAT32_ATOMIC`, `TEXTURE_FORMAT_16BIT_NORM`,
`TEXTURE_COMPRESSION_ASTC_HDR`, `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`,
`PIPELINE_STATISTICS_QUERY`, `TIMESTAMP_QUERY_INSIDE_ENCODERS`,
`TIMESTAMP_QUERY_INSIDE_PASSES`, `MAPPABLE_PRIMARY_BUFFERS`,
`TEXTURE_BINDING_ARRAY`, `BUFFER_BINDING_ARRAY`,
`STORAGE_RESOURCE_BINDING_ARRAY`,
`SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING`,
`STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING`,
`PARTIALLY_BOUND_BINDING_ARRAY`, `MULTI_DRAW_INDIRECT_COUNT`,
`ADDRESS_MODE_CLAMP_TO_ZERO`, `ADDRESS_MODE_CLAMP_TO_BORDER`,
`POLYGON_MODE_LINE`, `POLYGON_MODE_POINT`, `CONSERVATIVE_RASTERIZATION`,
`VERTEX_WRITABLE_STORAGE`, `CLEAR_TEXTURE`, `MULTIVIEW`,
`VERTEX_ATTRIBUTE_64BIT`, `TEXTURE_ATOMIC`, `TEXTURE_FORMAT_NV12`,
`TEXTURE_FORMAT_P010`, `EXTERNAL_TEXTURE`, `EXPERIMENTAL_RAY_QUERY`,
`SHADER_F64`, `SHADER_I16`, `SHADER_EARLY_DEPTH_TEST`, `SHADER_INT64`,
`SUBGROUP` (`subgroups`), `SUBGROUP_VERTEX`, `SUBGROUP_BARRIER`,
`PIPELINE_CACHE`, `SHADER_INT64_ATOMIC_MIN_MAX`,
`SHADER_INT64_ATOMIC_ALL_OPS`, `VULKAN_GOOGLE_DISPLAY_TIMING`,
`VULKAN_EXTERNAL_MEMORY_WIN32`, `TEXTURE_INT64_ATOMIC`,
`UNIFORM_BUFFER_BINDING_ARRAYS`, `EXPERIMENTAL_MESH_SHADER`,
`EXPERIMENTAL_RAY_HIT_VERTEX_RETURN`, `EXPERIMENTAL_MESH_SHADER_MULTIVIEW`,
`EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS`, `PASSTHROUGH_SHADERS`,
`SHADER_BARYCENTRICS`, `SELECTIVE_MULTIVIEW`,
`EXPERIMENTAL_MESH_SHADER_POINTS`, `MULTISAMPLE_ARRAY`,
`EXPERIMENTAL_COOPERATIVE_MATRIX`, `SHADER_PER_VERTEX`,
`SHADER_DRAW_INDEX`, `ACCELERATION_STRUCTURE_BINDING_ARRAY`,
`MEMORY_DECORATION_COHERENT`, `MEMORY_DECORATION_VOLATILE`.

**Highlighted as plausibly-load-bearing:**

- `SUBGROUP` / `SUBGROUP_BARRIER` — wgsl shaders do not currently use
  subgroup ops (confirmed by `grep -E "subgroup|wave" *.wgsl` returning
  nothing in `bounds_calc.wgsl` / `chunk_calc.wgsl` / `world_change.wgsl`),
  so divergence here is "unlikely-relevant". Still log so future shader
  changes that *do* use subgroups don't surprise us.
- `MAPPABLE_PRIMARY_BUFFERS` — native-only by spec; if `map_async()` on the
  web reads back a buffer that was just written by a compute pass, the
  semantics are different from native. Relevant to
  `populate_cpu_mirror_from_gpu_producer`.
- `TIMESTAMP_QUERY`, `TIMESTAMP_QUERY_INSIDE_ENCODERS`,
  `TIMESTAMP_QUERY_INSIDE_PASSES` — see §A.4 above and §C runtime
  instrumentation.
- `MEMORY_DECORATION_COHERENT` / `MEMORY_DECORATION_VOLATILE` — these are
  the WGSL escape hatches for "tell the compiler this storage variable
  needs coherent / volatile semantics across invocations". Native-only;
  Dawn does not currently expose them. If present on native and absent on
  web, wgpu may emit different SPIR-V/HLSL for the same WGSL on the two
  targets — directly relevant to a cross-pass-visibility symptom.
- `SHADER_FLOAT32_ATOMIC` — bounds_calc uses `atomic<u32>` only; not
  relevant.

Action: log `features.iter_names()` on both targets as a sorted list of
strings, plus the raw `[u64; 2]` bit pattern.

### A.5 `AdapterInfo` field-by-field

Source: `wgpu-types-29.0.3/src/adapter.rs:111-180`.

| Field | Type | Notes |
|---|---|---|
| `name` | String | e.g. `"NVIDIA GeForce RTX 4090"` on native; `"WebGPU"` or a sanitised string on Dawn |
| `vendor` | u32 | PCI vendor ID (native) / Dawn-synthesised |
| `device` | u32 | PCI device ID (native) / Dawn-synthesised |
| `device_pci_bus_id` | String | Vulkan only, otherwise empty |
| `driver` | String | driver name |
| `driver_info` | String | driver version + build info |
| `backend` | `Backend` enum (`Noop`/`Vulkan`/`Metal`/`Dx12`/`Gl`/`BrowserWebGpu`) | **Critical:** native = `Vulkan` here, web = `BrowserWebGpu` |
| `subgroup_min_size` | u32 | hardware subgroup minimum (NVIDIA=32, AMD GCN/Vega=64, AMD RDNA+=32, Intel=8/16) |
| `subgroup_max_size` | u32 | hardware subgroup maximum |
| `transient_saves_memory` | bool | hint for `TextureUsages::TRANSIENT` |
| `device_type` | `DeviceType` enum | `Other`/`IntegratedGpu`/`DiscreteGpu`/`VirtualGpu`/`Cpu` |

The user's machine is a CachyOS Linux box. Native = wgpu's Vulkan backend.
Web = Dawn's Vulkan backend (Chrome on Linux uses Vulkan under Dawn, not
ANGLE/OpenGL — confirmable via `chrome://gpu`). So the symptom is
**Vulkan-direct vs Vulkan-via-Dawn**, not e.g. Vulkan-vs-Metal. The
diagnostic snapshot will confirm this directly via `backend` +
`driver_info`.

### A.6 Other adapter-level surface (not via `Adapter` methods)

| Source | Method | Notes |
|---|---|---|
| `Device::poll(PollType::Wait)` | sync the device; on wasm `Poll` no-ops because the browser owns the event loop. Reading the result is useless on wasm, but a `Wait` on native gives us a fence. |
| `Device::on_uncaptured_error(callback)` | optional global error hook; we don't need this for the diagnostic |
| `Device::set_device_lost_callback` | wasm-relevant: lets us know if Dawn loses the device mid-run |
| `Queue::get_timestamp_period()` | nanoseconds per timestamp tick; needed to interpret `timestamp-query` results |
| `Queue::on_submitted_work_done()` | promise/future that settles when all prior submits complete; useful as a sync point for §C runtime instrumentation |
| `Queue::submit([])` | empty submit, returns a `SubmissionIndex`; can be used as a fence |

### A.7 Bevy-side escape hatches

| Bevy resource | Underlying wgpu type | Notes |
|---|---|---|
| `RenderAdapter` | `Arc<WgpuWrapper<wgpu::Adapter>>` | `Resource` in `RenderApp`; `RenderAdapter.0.deref()` → `&Adapter` |
| `RenderAdapterInfo` | `WgpuWrapper<AdapterInfo>` | `Resource` in `RenderApp`; cached `get_info()` from initialization |
| `RenderDevice` | wraps `wgpu::Device` | `RenderDevice::wgpu_device() -> &wgpu::Device` is the escape hatch (`render_device.rs:259`). Also `.features()` / `.limits()` are thin wrappers. |
| `RenderQueue` | `Arc<WgpuWrapper<Queue>>` | newtype, deref to `&Queue` |
| `RenderInstance` | `Arc<WgpuWrapper<Instance>>` | newtype, deref to `&Instance` |

Bevy does NOT expose adapter-level `features()` / `limits()` /
`downlevel_capabilities()` / `get_texture_format_features()` directly — you
have to reach through `RenderAdapter`. The existing Q4 logger uses
`RenderDevice::limits()` which is the **device-side** limits (post-creation
clamp), not the adapter's full capability. **Always log both:** adapter
gives us "what the GPU could do", device gives us "what wgpu actually
asked for and got". Divergence between them is a configuration smell.

---

## B — Chrome/Dawn/WebGPU vs Vulkan/WebGPU semantic divergence catalog

For each item: best-available cite, native-vs-Dawn delta, relevance flag.
The "relevance" judgement weights only how plausible the divergence is as
a *partial* cause for "cross-pass atomic visibility wobble + non-determinism
+ specifically wasm32". Many spec issues are real but irrelevant; the
flags isolate what to instrument first.

### B.1 Queue submission ordering and visibility — **likely-relevant**

- **Spec text (WebGPU §3.2.2 "Promise Ordering"):** ordering guarantee is
  *only* across `q.onSubmittedWorkDone()` and `b.mapAsync()` settles.
  "applications must not rely on any other promise settlement ordering."
- **Spec text (WebGPU §3.4.4 "Synchronization and Usage Scopes"):** "in a
  compute pass, each dispatch command is one usage scope. state-setting
  compute pass commands… do not contribute their bound resources directly
  to a usage scope."
- **Spec text (WebGPU §19 "Queues", §16 "Compute Passes"):** the spec does
  NOT define explicit memory-ordering guarantees between compute passes
  within one command encoder, between command encoders within one
  `queue.submit()`, or between two `queue.submit()` calls. The
  fence-semantics are implementation-defined as long as the validation
  passes.
- **wgpu Vulkan backend:** inserts a Vulkan
  `vkCmdPipelineBarrier(SHADER_WRITE→SHADER_READ + STORAGE_BIT)` between
  successive compute passes that touch the same storage buffer, *within
  one encoder*. Across `submit` boundaries, Vulkan's queue-family submit
  semantics inject an implicit memory dependency on the global queue
  timeline.
- **Dawn:** also tracks storage-buffer last-writer per pass and inserts a
  barrier — but the barrier scope and the moment of insertion differ
  subtly. Dawn's tracker is per-encoder; across `commandEncoder.finish() +
  device.queue.submit([cb1, cb2])` Dawn relies on the underlying Vulkan
  driver's queue-timeline semantics to provide ordering. There is no
  explicit Dawn-level barrier between cb1 and cb2.
- **Cite:** WebGPU spec §16 (Compute Passes) + §19 (Queues); WGSL §14.5
  Memory Model (high-level wording: "without explicit synchronization, no
  ordering guarantees exist between invocations"). The single most
  load-bearing observation is the spec's silence — it does not pin
  cross-pass / cross-submit visibility, so Dawn and wgpu-native can
  legitimately differ as long as both validate.
- **Relevance:** **likely-relevant.** The handoff's "per-round
  encoder+submit on wasm32" experiment was neutral on SSIM — which means
  the cross-submit-fence hypothesis is at best partial. Worth pinning the
  exact insertion behavior via runtime instrumentation (§C.B "per-pass
  buffer-state hash") rather than reasoning from spec text.

### B.2 Storage-buffer atomic visibility across compute passes — **likely-relevant**

- **WGSL §14.5 (Memory Model):** atomic ops on storage buffers have
  **device scope** by default. The spec borrows the C11 memory-model
  formalism. Atomic ops within one invocation are sequentially consistent;
  across invocations, only explicit barriers / atomic acquire-release
  pairs synchronize.
- **WGSL §17.11.1 (`storageBarrier`):** synchronises storage buffer +
  storage texture memory operations *within a workgroup*. "Invocations
  must collectively execute storageBarrier in uniform control flow."
  Workgroup-scope barrier — does NOT synchronise across workgroups, across
  dispatches, or across passes.
- **Spec gap:** WGSL does not provide a cross-pass / cross-dispatch
  `deviceBarrier()`. Cross-pass visibility relies on the WebGPU host-side
  semantics (each dispatch is its own usage scope; pass-end implicitly
  flushes for the next pass on the queue timeline).
- **In practice (wgpu-Vulkan):** Vulkan compute-to-compute barrier between
  passes is `VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT + ACCESS_SHADER_WRITE → ACCESS_SHADER_READ`,
  which IS what's needed for atomic write→atomic read visibility on
  storage buffers.
- **In practice (Dawn):** same barrier shape on the Vulkan side
  underneath. BUT Dawn's barrier insertion is gated on the
  *PassResourceUsageTracker* knowing the buffer was atomically written.
  WGSL `atomic<u32>` is lowered through Tint → SPIR-V/HLSL/MSL with
  acquire-release semantics. If the lowering loses the `Coherent` /
  `MakeAvailable` decoration on the storage-buffer access (which on Dawn
  is the default WebGPU semantic), the write may be visible on
  one backend and not the other.
- **Cite:** WGSL §14.5.3 "Scoped Operations" (device vs workgroup vs
  invocation scope); WGSL §17.11.1 (storageBarrier is workgroup-scope).
- **Relevance:** **likely-relevant.** The handoff's symptom — wildly
  varying `bound_queue_info[].size` atomic reads on the web — fits "atomic
  store from pass N is non-deterministically visible to atomic load in
  pass N+1." The diagnostic should:
  1. Confirm both targets emit the same WGSL atomic decorations.
  2. Run a tight cross-pass atomic visibility microbenchmark (§C.B).

### B.3 Indirect-dispatch buffer barriers — **possibly-relevant**

- **WebGPU spec (§16 "Compute Passes" + §10 "Buffers"):** the indirect
  dispatch buffer must have `BufferUsages::INDIRECT`; the spec does NOT
  call out a specific barrier between a compute write to the indirect
  buffer and a `dispatchWorkgroupsIndirect` consuming it. The barrier is
  implicit in the usage-scope tracking.
- **wgpu-Vulkan:** inserts a
  `STORAGE→INDIRECT_COMMAND_READ` barrier across the encoder when it sees
  a buffer used as STORAGE in pass A and as INDIRECT in pass B.
- **Dawn:** also tracks `Indirect` usage, but the barrier mapping is in
  `dawn/src/dawn/native/CommandBufferStateTracker.cpp` and Dawn
  historically has had bugs in this area for cross-pass storage→indirect.
  See e.g. crbug.com/dawn/1338 (storage→indirect barrier on Vulkan)
  category of issues — closed but the class is real.
- **Project context:** the handoff already mitigates this by direct-
  dispatching on wasm32 (`bounds_calc.rs:413-466`), so this divergence is
  *currently bypassed* on the wasm path. Still worth instrumenting
  because if the wasm direct-dispatch is reverted (e.g. for perf) the
  divergence comes back.
- **Cite:** WebGPU §16, §10.2 (BufferUsages); Dawn source path
  `dawn/src/dawn/native/CommandBufferStateTracker.{h,cpp}`.
- **Relevance:** **possibly-relevant.** Currently bypassed on wasm. Keep
  on the watchlist; surface the `bound_dispatch_indirect` buffer's last
  write epoch in §C.B's per-pass state hash so any future re-introduction
  of indirect-dispatch is auditable.

### B.4 Validation behaviour and silent no-op — **possibly-relevant**

- **Dawn validation:** strict by default; mismatched bind groups, missing
  COPY_DST flags, exceeding `max_compute_workgroups_per_dimension` etc.
  raise a `GPUValidationError`. The browser surfaces these as
  `console.error("WebGPU validation failure: …")` and the offending
  command becomes a no-op for the rest of the encoder.
- **wgpu-native validation:** runs the same `wgpu-core` validator (the
  one wgpu-web ALSO runs, before Dawn's), then Vulkan validation layers
  if `wgpu::Backends::VULKAN` is loaded with `--features=vulkan-portability`
  / `WGPU_VALIDATION=1`. By default, neither validation layer is on in a
  release build — the dispatch reaches the driver verbatim.
- **The "silent drop" hypothesis from the handoff:** REFUTED for the
  total-workgroup-count case (134M→splits did not change the symptom).
  But "silently drop a dispatch when a bind-group binding goes
  out-of-range" is still in play if some other limit is exceeded on web
  but not native.
- **Cite:** WebGPU §22 (Error Handling, popErrorScope / uncapturedError);
  Dawn `dawn/src/dawn/native/CommandRecordingContext.cpp`.
- **Relevance:** **possibly-relevant.** Capture all `console.error` lines
  containing "WebGPU" / "GPUValidationError" in the existing Playwright
  collector (the spec already does this — see
  `vox-horizon-parity.spec.ts:215-237` for the ConsoleCollector pattern;
  extend it to surface validation errors as test annotations).

### B.5 `max_compute_workgroups_per_dimension` and other dispatch limits — **possibly-relevant**

- **Spec default:** 65 535 per dimension (`Limits::defaults()`).
- **WebGPU implementations are required to expose ≥ 65 535**; the actual
  number is hardware-dependent.
- **Dawn-on-Vulkan (Linux, modern NVIDIA/AMD):** typically reports
  `2_147_483_647` (Vulkan's
  `maxComputeWorkGroupCount` max).
- **wgpu-Vulkan (native):** same — propagated from the same Vulkan
  property.
- **Project context:** `compute_voxel_bounds` was previously dispatching
  134_217_728 workgroups (chunk_count × something) which violates the
  spec default 65 535 but may pass on a hardware that reports the
  higher number. The handoff refuted this as the cause (split-by-8 and
  split-by-128 produced SSIM noise, not a step change). However the
  diagnostic should pin the actual reported number on the user's web
  target.
- **Cite:** WebGPU §3.6 "Limits", wgpu-types `Limits::defaults()`.
- **Relevance:** **possibly-relevant.** Already on the radar; the
  diagnostic surface adds it to the formal snapshot, not the ad-hoc Q4
  logger.

### B.6 `map_async` and readback semantics — **possibly-relevant**

- **WebGPU spec (§22 "Buffer Mapping"):** `mapAsync()` "guarantees that
  all submitted work whose execution time-ordering precedes this call has
  completed". The browser implementation polls until the GPU work
  finishes, then maps the buffer.
- **wgpu-native:** same guarantee — but mediated through `Device::poll`
  which the application must call. On wasm32 the browser drives the
  event loop, so `poll(PollType::Poll)` is a no-op cooperative yield
  (`render_device.rs` wraps `Device::poll`).
- **Practical divergence:** on wasm32, if the application reads back a
  storage buffer that was modified by a compute pass earlier in the same
  frame, the `map_async` callback fires *after* the compute pass's GPU
  work completes — but the value seen reflects the queue-timeline state
  at submit time, not the host-timeline state. In a multi-encoder /
  multi-submit flow (which the wasm-specific bounds_calc path explicitly
  uses!), the readback may straddle multiple submits.
- **Project context:** `populate_cpu_mirror_from_gpu_producer`
  (`construction/mod.rs:1042-1465`) does cross-frame readback. The probe1
  / probe2 pipeline (`mod.rs:1465-3565`) reads back chunks +
  bound_queue_info + bound_refined_info on web and observes the wildly
  varying atomic values per the handoff.
- **Cite:** WebGPU §22 ("GPUBuffer.mapAsync()" algorithm).
- **Relevance:** **possibly-relevant.** The probe readback is *itself*
  observing the bug. The diagnostic should not assume the probe is the
  ground truth — it could be the readback that is racy, not the source
  data. §C.B's "per-pass buffer-state hash" reads the buffer **inside**
  a compute shader (no readback round-trip) and dumps the hash to a
  diagnostic slot; that bypasses any `mapAsync`-related divergence.

### B.7 Subgroup / wave-intrinsic divergence — **unlikely-relevant**

- **WGSL `subgroup_*` builtins:** part of the proposed WebGPU subgroups
  feature; not yet stabilised (wgpu's `SUBGROUP` feature is "native-only
  for now" per the wgpu-types comment).
- **Project context:** `grep -E "subgroup|wave|derivative" *.wgsl` over
  bounds_calc, chunk_calc, world_change — 0 hits. The shaders do not
  use subgroup intrinsics.
- **Relevance:** **unlikely-relevant** for the *current* shader code.
  Still capture the feature flag so a future shader change that opts in
  doesn't silently regress the cross-target behaviour.

### B.8 NaN / infinity / FP ordering in compute shaders — **possibly-relevant**

- **WGSL §14.6 "Floating Point Evaluation":** WGSL does NOT require
  IEEE-754 strict mode; "an implementation may evaluate floating-point
  expressions using a precision or a representation that is greater than
  that of the source type" (FMA fusion, contraction, reordering).
- **wgpu-Vulkan:** Vulkan compute shaders default to SPIR-V's relaxed FP
  mode (allows `mad` fusion + reordering). Vulkan SPV_KHR_float_controls
  can pin this but wgpu does not by default.
- **Dawn-on-Vulkan / Dawn-on-Metal:** Tint emits SPIR-V/MSL with the same
  relaxed mode. Tint's reordering can DIFFER from naga's because they
  are separate translators with separate constant-folding / `select(x,
  y, c)` lowering.
- **Project context:** `chunk_calc.wgsl` and `bounds_calc.wgsl` are
  primarily integer / bit manipulation; the only float arithmetic is
  the per-chunk AADF skip-distance computation. Per the handoff, the
  probe shows web sees skip distances 0–1 (varying) while native shows
  3–4. If the skip-distance computation involves
  `(some_int / chunk_size) * scale - bias` in floats, a fused-multiply-add
  could land the bit pattern in a different equivalence class.
- **Cite:** WGSL §14.6; Tint vs naga divergence is folklore but real
  (Tint defaults to disabling FMA contraction on Metal but enables it on
  SPIR-V/HLSL; naga's default differs).
- **Relevance:** **possibly-relevant** *if* the chunk-AADF skip-distance
  shader path does any float math. The diagnostic should grep the
  shaders for `f32` / `vec3<f32>` operations on the skip-distance path
  and, if any are found, propose a fixed-FP-mode override (or move the
  computation to integer arithmetic). Until that grep is done in the
  *next* dispatch, treat this as possibly-relevant rather than
  unlikely.

### B.9 Texture format / storage texture access modes — **unlikely-relevant**

- The bug is in compute-pass storage *buffer* state, not storage
  textures. The shaders do not declare any `texture_storage_*` bindings
  in the bounds_calc / chunk_calc / world_change chain (confirmed by
  the grep in §A.4).
- **Relevance:** **unlikely-relevant.** Log `Adapter::get_texture_format_features`
  for the surface-format only, not for the entire format set.

### B.10 Device limits vs adapter limits — **possibly-relevant**

- **Spec text (WebGPU §3.6):** "Once a device is requested, you may only
  use resources up to the limits requested *even if* the adapter supports
  better limits." (mirrored in `wgpu_types::Limits` doc).
- **wgpu's default behaviour:** if Bevy doesn't explicitly pass
  `required_limits = adapter.limits()`, the device is created with
  `Limits::defaults()` — which is the **conservative WebGPU minimum**.
  That means even though Dawn's adapter reports 2 GiB
  `max_storage_buffer_binding_size`, the device may have been created
  with only 128 MiB unless Bevy asked for more.
- **Bevy 0.19's behaviour:** Bevy's `WgpuSettings` defaults to
  `Limits::default()` (the minimum) on wasm32 and `adapter.limits()` on
  native. The Q4 logger reads device limits → reports 2 GiB → confirms
  Bevy 0.19 IS lifting the limits on wasm. But there are ~50 limits in
  the struct; only `max_storage_buffer_binding_size` has been spot-
  checked. Several limits (e.g. `max_compute_workgroups_per_dimension`,
  `max_buffer_size`) may still be at the WebGPU minimum even if the
  adapter could grant more.
- **Cite:** WebGPU §3.6, `wgpu_types::Limits` doc, Bevy 0.19's
  `RenderPlugin` configuration.
- **Relevance:** **possibly-relevant.** The diagnostic must log both
  `adapter.limits()` and `device.limits()` side-by-side and flag any
  field where `device < adapter`. That divergence is the place where
  Bevy's `WgpuSettings` may need an explicit raise on wasm32.

---

## C — Diagnostic-package design

### C.1 Overall shape

Two output channels:

- **Static device snapshot.** One JSON per target, written once at
  startup, containing every read-only field from §A.
- **Runtime instrumentation hooks.** New shader counters + readback
  channels for the bounds_calc / chunk_calc / world_change pipelines, so
  the next round of diagnosis works on observed dynamic state.

The static snapshot is the floor; the dynamic instrumentation is what
will actually pin the bug.

### C.2 Static device snapshot

#### Module location

Extend `crates/bevy_naadf/src/diagnostics.rs` (the existing Press-`P`
module). Add a new sub-module `device_snapshot` that owns:

- The `DeviceSnapshot` struct.
- The render-app system that builds it.
- The main-world relay that writes it to disk on native, and emits it as
  a tagged `info!` line on wasm32.
- A `DeviceSnapshotPlugin` that wires everything in.

Why extend `diagnostics.rs` rather than open a new module: the user's
brief says "match existing diagnostic style." The Press-`P` dump already
owns the `diagnostics` namespace; adding a startup snapshot beside the
key-press dump is the natural shape and avoids fragmenting the
diagnostics layer.

#### Bevy schedule

- **Build the snapshot** in the **`RenderApp`'s `Render` schedule, in
  `RenderSet::PrepareAssets`** (or the first set where `RenderAdapter`,
  `RenderAdapterInfo`, `RenderDevice`, and `RenderQueue` are all
  populated — typically `RenderSet::Prepare` works in Bevy 0.19, but the
  impl agent should grep for an existing `Res<RenderAdapter>` consumer
  and copy its scheduling).
- Use a once-only gate: `static SNAPSHOT_LOGGED:
  std::sync::atomic::AtomicBool = AtomicBool::new(false);` set via
  `swap(true, Relaxed)`.
- **Relay to the main world** via Bevy's `MainWorld` resource extraction
  pattern: write the snapshot to a `Resource` in the render world,
  extract it back to the main world in `ExtractSchedule`, then have a
  main-world `Update` system that consumes it once and emits the
  serialised output via `info!(target: "device-snapshot", "{}", json)`.
- Reason for the main-world relay: on native we want to write to a
  file on disk, which is best done in the main world where `std::fs` is
  available without wgpu-side complications. On wasm32 `info!` goes
  through Bevy's `LogPlugin` → `console.log` → Playwright capture,
  which is the existing pattern.

#### Output struct shape

```rust
// crates/bevy_naadf/src/diagnostics.rs (additions)

use serde::Serialize;

#[derive(Serialize, Debug, Clone)]
pub struct DeviceSnapshot {
    /// Schema version. Bump on breaking changes; the comparison recipe
    /// hard-asserts the two snapshots have matching `schema_version`.
    pub schema_version: u32,
    /// `"native"` or `"web"` — set by the build, not read from the
    /// adapter. Lets the comparison recipe know which side is which
    /// without parsing the rest.
    pub target: &'static str,
    /// ISO-8601 timestamp at the moment the snapshot was taken.
    pub captured_at: String,
    pub adapter_info: SnapshotAdapterInfo,
    /// `Adapter::features()` — sorted list of kebab-case names.
    pub adapter_features: Vec<String>,
    /// `Adapter::features()` — raw `[u64; 2]` bit pattern for byte-exact
    /// diff.
    pub adapter_features_bits: [u64; 2],
    /// `Adapter::limits()` — every field flattened.
    pub adapter_limits: SnapshotLimits,
    /// `Adapter::get_downlevel_capabilities()`.
    pub downlevel: SnapshotDownlevel,
    /// `Device::features()` / `Device::limits()` — what wgpu actually
    /// asked for.
    pub device_features: Vec<String>,
    pub device_features_bits: [u64; 2],
    pub device_limits: SnapshotLimits,
    /// Fields where `device_limits[k] < adapter_limits[k]`. Empty means
    /// Bevy lifted limits to the adapter ceiling for every field.
    pub limit_deltas: Vec<LimitDelta>,
    /// `Queue::get_timestamp_period()` — for interpreting any
    /// timestamp-query result.
    pub queue_timestamp_period_ns: f32,
    /// True if `Adapter::get_downlevel_capabilities().is_webgpu_compliant()`.
    pub downlevel_is_webgpu_compliant: bool,
    /// Build-time facts that won't be available from the runtime
    /// adapter but matter for reproducibility.
    pub build: SnapshotBuild,
}

#[derive(Serialize, Debug, Clone)]
pub struct SnapshotAdapterInfo {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub device_pci_bus_id: String,
    pub driver: String,
    pub driver_info: String,
    pub backend: String, // "vulkan" | "metal" | "dx12" | "gl" | "webgpu" | "noop"
    pub device_type: String, // "Other" | "IntegratedGpu" | "DiscreteGpu" | "VirtualGpu" | "Cpu"
    pub subgroup_min_size: u32,
    pub subgroup_max_size: u32,
    pub transient_saves_memory: bool,
}

/// Mirror every field from `wgpu_types::Limits` 29.0.3 §A.2.
/// Generated by hand because `wgpu_types::Limits` does NOT have
/// `Serialize` on by default in Bevy's feature flags.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub max_texture_dimension_1d: u32,
    pub max_texture_dimension_2d: u32,
    pub max_texture_dimension_3d: u32,
    pub max_texture_array_layers: u32,
    pub max_bind_groups: u32,
    pub max_bindings_per_bind_group: u32,
    pub max_dynamic_uniform_buffers_per_pipeline_layout: u32,
    pub max_dynamic_storage_buffers_per_pipeline_layout: u32,
    pub max_sampled_textures_per_shader_stage: u32,
    pub max_samplers_per_shader_stage: u32,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub max_uniform_buffers_per_shader_stage: u32,
    pub max_binding_array_elements_per_shader_stage: u32,
    pub max_binding_array_acceleration_structure_elements_per_shader_stage: u32,
    pub max_binding_array_sampler_elements_per_shader_stage: u32,
    pub max_uniform_buffer_binding_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub max_vertex_buffers: u32,
    pub max_buffer_size: u64,
    pub max_vertex_attributes: u32,
    pub max_vertex_buffer_array_stride: u32,
    pub max_inter_stage_shader_variables: u32,
    pub min_uniform_buffer_offset_alignment: u32,
    pub min_storage_buffer_offset_alignment: u32,
    pub max_color_attachments: u32,
    pub max_color_attachment_bytes_per_sample: u32,
    pub max_compute_workgroup_storage_size: u32,
    pub max_compute_invocations_per_workgroup: u32,
    pub max_compute_workgroup_size_x: u32,
    pub max_compute_workgroup_size_y: u32,
    pub max_compute_workgroup_size_z: u32,
    pub max_compute_workgroups_per_dimension: u32,
    pub max_immediate_size: u32,
    pub max_non_sampler_bindings: u32,
    // Mesh-shader / ray-query / multiview fields included even when 0
    // so the diff catches a future enable-flag flip:
    pub max_task_mesh_workgroup_total_count: u32,
    pub max_task_mesh_workgroups_per_dimension: u32,
    pub max_task_invocations_per_workgroup: u32,
    pub max_task_invocations_per_dimension: u32,
    pub max_mesh_invocations_per_workgroup: u32,
    pub max_mesh_invocations_per_dimension: u32,
    pub max_task_payload_size: u32,
    pub max_mesh_output_vertices: u32,
    pub max_mesh_output_primitives: u32,
    pub max_mesh_output_layers: u32,
    pub max_mesh_multiview_view_count: u32,
    pub max_blas_primitive_count: u32,
    pub max_blas_geometry_count: u32,
    pub max_tlas_instance_count: u32,
    pub max_acceleration_structures_per_shader_stage: u32,
    pub max_multiview_view_count: u32,
}

#[derive(Serialize, Debug, Clone)]
pub struct SnapshotDownlevel {
    /// kebab-case sorted list, like `["compute-shaders","indirect-execution",…]`.
    pub flags: Vec<String>,
    pub flags_bits: u32,
    pub shader_model: String, // "Sm2" | "Sm4" | "Sm5"
}

#[derive(Serialize, Debug, Clone)]
pub struct LimitDelta {
    pub field: String,
    pub adapter_value: u64,
    pub device_value: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct SnapshotBuild {
    pub wgpu_version: &'static str,   // "29.0.3"
    pub bevy_version: &'static str,   // "0.19.0-rc.1"
    pub git_sha: &'static str,        // from env! at build time, fallback "unknown"
    pub profile: &'static str,        // "debug" | "release"
    pub target_arch: &'static str,    // env!("CARGO_CFG_TARGET_ARCH")
    pub target_os: &'static str,
}
```

**Sample JSON output (truncated):**

```json
{
  "schema_version": 1,
  "target": "web",
  "captured_at": "2026-05-19T14:32:01Z",
  "adapter_info": {
    "name": "Mesa Intel(R) HD Graphics 530 (SKL GT2)",
    "vendor": 32902, "device": 6418, "device_pci_bus_id": "",
    "driver": "Dawn-Tint", "driver_info": "Vulkan 1.3.275",
    "backend": "webgpu", "device_type": "IntegratedGpu",
    "subgroup_min_size": 8, "subgroup_max_size": 32,
    "transient_saves_memory": false
  },
  "adapter_features": ["depth-clip-control","timestamp-query","shader-f16",…],
  "adapter_features_bits": [0, 0],
  "adapter_limits": {
    "max_storage_buffer_binding_size": 2147483644,
    "max_compute_workgroups_per_dimension": 65535,
    "max_compute_workgroup_size_x": 256,
    …
  },
  "downlevel": {
    "flags": ["compute-shaders","indirect-execution",…],
    "flags_bits": 16777215,
    "shader_model": "Sm5"
  },
  "device_features": ["timestamp-query",…],
  "device_features_bits": [0, 0],
  "device_limits": { … },
  "limit_deltas": [
    { "field": "max_compute_workgroups_per_dimension", "adapter_value": 65535, "device_value": 65535 }
  ],
  "queue_timestamp_period_ns": 41.6666,
  "downlevel_is_webgpu_compliant": true,
  "build": {
    "wgpu_version": "29.0.3", "bevy_version": "0.19.0-rc.1",
    "git_sha": "c594de3", "profile": "release",
    "target_arch": "wasm32", "target_os": "unknown"
  }
}
```

#### Wasm output route

Match the existing `[aadf-probe]` pipeline (proven, in use, persists to
disk via the Playwright harness).

1. **Wasm side:** emit one `info!(target: "device-snapshot", "{}",
   serde_json::to_string(&snapshot).unwrap())` line during the one-shot
   render-app system. Prefix the actual JSON line with a sentinel
   `[device-snapshot]` so the Playwright filter is regex-cheap.
2. **Playwright side:** extend
   `e2e/tests/vox-horizon-parity.spec.ts:215-237` ConsoleCollector
   filter — add `text.includes("[device-snapshot]")` to the `if (text.includes("Q4 instrumentation") || …)` chain. Push to a separate
   `wgpuSnapshotLines: string[]` array.
3. **Playwright side:** at the end of the test, write the captured line(s)
   to `target/diagnostics/device-snapshot-web.json` (after stripping the
   `[device-snapshot] ` prefix). Use `fs.mkdir(targetDir, {recursive:
   true})` to create the directory.
4. **Native side:** the snapshot system writes directly to
   `target/diagnostics/device-snapshot-native.json` via `std::fs::write`
   in the main-world consumer (not the render-app system; render-world
   shouldn't do file I/O).

**Output paths:**

- Native: `target/diagnostics/device-snapshot-native.json`
- Web:    `target/diagnostics/device-snapshot-web.json`

Both relative to the workspace root (`{{naadf_dir}}` in the existing
justfile). Symmetric paths so the comparison recipe can `diff` them.

### C.3 Runtime instrumentation hooks (dynamic surface)

The static snapshot alone won't pin the bug — it tells us what the GPU
*could* do, not what it actually did during the failing run. Three
proposed dynamic hooks, in priority order:

#### C.3.1 `bound_refined_info[]` extension — atomic counter dump per round

`bound_refined_info` already has 16 u32 slots (`construction/mod.rs:3480`
sizes it to `16u64 * 4` bytes; the shader writes [0..7] —
`bounds_calc.wgsl:293-313` + `:420`). Slots [8..15] are unused. Repurpose
them as atomic counters:

| Slot | Field | Written by | Read by |
|---|---|---|---|
| 8 | `regime2_round_index` | `prepare_group_bounds` (one-thread scan; non-atomic incr) | probe2 readback |
| 9 | `atomic_size_load_min` | `compute_group_bounds` (atomicMin via atomicCompareExchange loop) | probe2 readback |
| 10 | `atomic_size_load_max` | `compute_group_bounds` (atomicMax) | probe2 readback |
| 11 | `atomic_size_load_sum_lo` | atomicAdd | probe2 readback |
| 12 | `atomic_size_load_sum_hi` | overflow detect | probe2 readback |
| 13 | `enqueued_size_after_increment_min` | atomicMin on `atomicAdd` result | probe2 readback |
| 14 | `enqueued_size_after_increment_max` | atomicMax on `atomicAdd` result | probe2 readback |
| 15 | reserved | | |

The diagnostic question this answers: **is `atomicLoad(&bound_queue_info[qi].size)` returning the same value across rounds on web vs native?**
If `atomic_size_load_min == atomic_size_load_max == seed_value` on
native and the same on web, the bug is downstream of the atomic load.
If the load is jittery on web (min ≠ max for the same logical round)
and stable on native, that's the smoking gun for cross-pass atomic
visibility (B.2).

Slot 9-14 are device-scope atomics on a fresh storage buffer; this is
a microbenchmark of the exact mechanism the bug is suspected to involve.

#### C.3.2 Per-pass buffer-state hash — checksum the chunks buffer inside the shader

Add a new compute pipeline `hash_chunks` that takes the chunks storage
buffer + a single-element `array<atomic<u32>>` hash sink and runs
one workgroup of 64 threads that XORs every word in the chunks buffer
into the sink via `atomicXor`. Dispatch it AT THE END of each regime-2
round, before the next round's prepare pass. Read back the sink in the
probe2 readback.

The hash is content-addressable: if the chunks buffer state is
deterministic across runs (it should be — the same .vox input drives
the same construction), the hash should be the same number every run
of the same test. Variability of the hash = variability of the chunks
state = chunks_calc is non-deterministic and the bug is upstream of
bounds_calc.

This bypasses any `mapAsync`-related divergence (§B.6) because the
hash is computed entirely on-GPU; the readback only fetches a single
u32.

#### C.3.3 Timestamp queries on the bounds_calc pass

If `device.features().contains(Features::TIMESTAMP_QUERY)` is true on
both native and web (testable from §C.2's snapshot), add timestamp
writes to the bounds_calc compute pass. The wgpu API:

- Create a `wgpu::QuerySet { ty: QueryType::Timestamp, count: 2 * n_rounds }`.
- In each round's `begin_compute_pass`, set `timestamp_writes:
  Some(ComputePassTimestampWrites { query_set, beginning_of_pass_write_index: Some(2*i), end_of_pass_write_index: Some(2*i + 1) })`.
- After all rounds, `encoder.resolve_query_set` into a buffer + read
  back.

Multiply each pair's delta by
`snapshot.queue_timestamp_period_ns` and log per-round wall time. The
diagnostic question this answers: **are some rounds taking
unexpectedly long on web (Dawn watchdog warning territory), or
unexpectedly short (a dispatch was silently dropped)?**

The handoff REFUTED the watchdog hypothesis with a split-by-N
experiment, but a watchdog that affects only some rounds within a
larger dispatch is still consistent with the symptom. Direct
measurement settles it.

If `TIMESTAMP_QUERY_INSIDE_PASSES` is *also* present (likely true on
native, may be false on Dawn pending the spec), additionally write a
timestamp BETWEEN the prepare pass and the compute pass within each
round. That measures the prepare→compute gap, which is the cross-pass
visibility window the bug is suspected to live in.

### C.4 Comparison recipe

Add to `justfile`:

```just
# Diff the device snapshots from native and web. Fails if either is
# missing; prints a structured diff if they differ. Run AFTER:
#   1. `cargo run --bin e2e_render -- --device-snapshot-native`
#      writes target/diagnostics/device-snapshot-native.json
#   2. `cd e2e && npx playwright test device-snapshot.spec.ts --headed`
#      writes target/diagnostics/device-snapshot-web.json
diag-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    native="target/diagnostics/device-snapshot-native.json"
    web="target/diagnostics/device-snapshot-web.json"
    if [[ ! -f "$native" ]]; then echo "missing: $native — run \`just diag-native\` first" >&2; exit 2; fi
    if [[ ! -f "$web" ]]; then echo "missing: $web — run \`just diag-web\` first" >&2; exit 2; fi
    # jq-based field-by-field diff with a stable key order. The diff
    # excludes `captured_at` (timestamps differ trivially) and
    # `build.git_sha` if equal on both sides. Lists set-differences for
    # `adapter_features`, `device_features`, `downlevel.flags`.
    cargo run --quiet -p bevy_naadf --bin diag_compare -- "$native" "$web"

# Take a fresh native device snapshot.
diag-native:
    cargo run --release --bin e2e_render -- --device-snapshot-native

# Take a fresh web device snapshot (requires `just web-build-release`).
diag-web: web-build-release
    cd e2e && npx playwright test device-snapshot.spec.ts --headed

# Convenience: run both then diff.
diag: diag-native diag-web diag-compare
```

The `diag_compare` binary is a tiny new `bin/diag_compare.rs` that
deserialises both JSON files into the `DeviceSnapshot` struct (or a
`serde_json::Value` if version-skew tolerance is wanted), walks them
field-by-field, and prints:

- Fields present on one side only.
- Fields with different values (with both values shown).
- A "load-bearing" highlight on a curated allowlist of fields the
  handoff has already implicated (e.g. `max_compute_workgroups_per_dimension`,
  `max_storage_buffer_binding_size`, all `device_features` containing
  `atomic` or `memory-decoration`, etc.).

Output is plain text to stdout; exit 0 if equivalent (per the
project-defined equivalence), exit 1 if diverged. The "expected
divergences" allowlist starts with `target`, `captured_at`,
`build.target_arch`, `build.target_os`, `adapter_info.backend`,
`adapter_info.driver*`, `adapter_info.name` — these MUST differ
between native and web.

### C.5 Wiring summary

New files / sections:

- `crates/bevy_naadf/src/diagnostics.rs` — extended with `device_snapshot`
  sub-module + `DeviceSnapshotPlugin`.
- `crates/bevy_naadf/src/bin/diag_compare.rs` — new tiny CLI binary.
- `crates/bevy_naadf/src/bin/e2e_render.rs` — add a `--device-snapshot-native`
  flag that boots the app once, waits for the snapshot to fire, exits
  with the JSON written.
- `e2e/tests/device-snapshot.spec.ts` — new minimal Playwright spec
  (boots the canvas, waits for `[device-snapshot]` console line, writes
  it to disk, exits).
- `justfile` — `diag-compare`, `diag-native`, `diag-web`, `diag` recipes.
- `e2e/tests/vox-horizon-parity.spec.ts` — extend the existing
  ConsoleCollector filter to also capture `[device-snapshot]` lines,
  so the existing parity gate ALSO writes the web snapshot as a side
  effect (means the snapshot is taken every time the parity test runs,
  without a separate Playwright invocation).

Dynamic hook implementations:

- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — extend
  `bound_refined_info` slot usage to [8..14] per §C.3.1.
- `crates/bevy_naadf/src/render/construction/mod.rs:3480` — bump
  `refined_size` if 16 slots needs to grow (it doesn't — 16 is
  already the buffer size, slots 8-14 are within it).
- `crates/bevy_naadf/src/render/construction/mod.rs:3546-3570` —
  extend the probe2 readback decode to print the new slots.
- `crates/bevy_naadf/src/assets/shaders/` — new `hash_chunks.wgsl` or
  inlined into bounds_calc.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — wire
  the hash dispatch + timestamp queries if features support.

The impl agent will deliver these as a coherent change in the next
dispatch. This document is the design only.

---

## Decisions & rejected alternatives

- **Decision: Static JSON snapshot + dynamic shader-side counters.**
  *Rejected alternative:* JSON-only static snapshot. *Why this won:*
  the user's brief explicitly says "static device-snapshot is necessary
  but not sufficient — design the dynamic surface too." A static
  snapshot alone tells us what Dawn *could* do; only dynamic
  instrumentation tells us what it actually did during a failing run.

- **Decision: JSON output format (serde_json).** *Rejected alternative:*
  TOML. *Why this won:* the existing `[aadf-probe]` pipeline pipes
  through `console.log` and is split on newlines. JSON is a single line
  per snapshot — fits the pipeline trivially. TOML's multi-line shape
  would need extra Playwright-side stitching. The diff recipe also
  benefits from `serde_json::Value`'s structured walk.

- **Decision: Sentinel-prefix output line `[device-snapshot]
  {json...}`.** *Rejected alternative:* dedicated `tracing` target +
  separate `console.log` channel. *Why this won:* matches the existing
  `[aadf-probe]` convention exactly (`construction/mod.rs:1473`,
  `bounds_calc.rs:477`). Zero new infrastructure on the WASM tracing
  bridge side.

- **Decision: Place the static-snapshot system in the `RenderApp`'s
  `Render` schedule + relay to main world via Extract.** *Rejected
  alternative:* run it directly on the main-world `Startup` schedule
  and call `app.get_sub_app(RenderApp).world().resource::<RenderAdapter>()`.
  *Why this won:* (a) Bevy 0.19's main world cannot safely borrow
  render-app resources from a main-world system; the supported access
  pattern IS via the render-app's own systems. (b) The
  `validate_gpu_construction*` modes in `e2e_render.rs` (line 81-95)
  already do "build report on render-app side, copy to main world",
  this is the same pattern.

- **Decision: Persist on disk via `target/diagnostics/device-snapshot-{target}.json`.**
  *Rejected alternative:* `target/e2e-screenshots/device-snapshot-{target}.json`
  (the existing probe directory). *Why this won:* `target/e2e-screenshots/`
  is already a flat dump-ground for PNGs + probe logs. Diagnostic
  outputs are conceptually different (read by `diag_compare`, not by
  the SSIM gate). A dedicated directory keeps the artefacts
  distinguishable and lets the impl agent add a `.gitignore` for
  `target/diagnostics/` without affecting `target/e2e-screenshots/`.

- **Decision: Match the existing two-target relay pattern (native →
  std::fs::write, web → console.log → Playwright → fs.writeFile).**
  *Rejected alternative:* WASM-side `fetch()` POST to a local
  endpoint. *Why this won:* the existing probe pipeline ALREADY
  successfully ferries data from WASM to a host-side file via the
  Playwright harness. Adding a separate fetch-to-local-server path
  doubles the moving parts for no net gain. `web-static`'s miniserve
  is read-only by design.

- **Decision: Reuse the unused `bound_refined_info[8..14]` slots.**
  *Rejected alternative:* allocate a new dedicated diagnostics storage
  buffer + bind-group entry. *Why this won:* (a) `bound_refined_info`
  is already wired up, readback-mapped, and decoded in the existing
  probe2 path. Reuse is one shader edit + one decode print, vs
  ~hundreds of lines of bind-group plumbing for a new buffer. (b)
  The unused slots are there. The 16-slot buffer was sized
  defensively; using more of it is free. The impl agent should add a
  comment block at the top of `bound_refined_info`'s WGSL declaration
  enumerating the slot map so future maintainers don't trample it.

- **Decision: Hash chunks via in-shader atomicXor.** *Rejected
  alternative:* CRC32 / xxHash via lookup tables in the shader.
  *Why this won:* atomicXor of every word is order-independent (XOR
  is commutative + associative) so workgroup-of-64 parallelism is
  trivial. Lookup-table CRC requires shared workgroup memory + a
  reduction tree. A non-cryptographic XOR hash is plenty for change
  detection.

- **Decision: Timestamp queries only IF the feature is present on
  BOTH targets.** *Rejected alternative:* shim with a CPU-side
  `Instant::now()` on the WASM target. *Why this won:* a host-side
  timer measures the round-trip of issuing the dispatch, not the
  on-device execution. The whole point of the timestamp query is to
  isolate GPU-side wall-clock time. If the feature is absent on web,
  log "timestamps_unsupported" and move on — better than a confounded
  number.

- **Decision: Plain-text stdout output from `diag_compare`.**
  *Rejected alternative:* JSON output. *Why this won:* the user reads
  the diff. JSON is for machines; this is for humans triaging the
  bug. A future iteration can add `--json` if needed.

---

## Assumptions made

These need to be verified by the impl agent before coding starts.
Most are mechanical (file paths, schedule names) and will be caught
by the build. The non-mechanical ones are flagged.

1. **Bevy 0.19's `RenderApp` exposes `RenderAdapter`, `RenderAdapterInfo`,
   `RenderDevice`, `RenderQueue` as fetchable resources** — verified via
   `bevy_render-0.19.0-rc.1/src/renderer/mod.rs:125-139`. Mechanical.

2. **`Adapter::get_downlevel_capabilities()` is available on wgpu 29 wasm32**
   — confirmed by `wgpu-29.0.3/src/api/adapter.rs:174-176` which has no
   `#[cfg(not(target_arch = "wasm32"))]` gate. Mechanical.

3. **`Queue::get_timestamp_period()` returns a sensible `f32` on wasm32
   even when `TIMESTAMP_QUERY` is absent** — best-guess; needs verification.
   If it returns NaN or panics, gate the field with the feature
   detection (i.e. only call it if `device_features` contains
   `timestamp-query`).

4. **Bevy 0.19's `RenderPlugin` does not re-export the adapter into the
   main world** — needs verification. Bevy's main-world↔render-world
   sync historically relies on `ExtractSchedule`. The impl agent should
   grep for `MainWorld` consumers of `RenderAdapterInfo` to confirm the
   relay pattern.

5. **`tracing` / `bevy::log::info!` on wasm32 routes through `console.log`
   without truncation for ≥10 KiB lines** — verified empirically: the
   existing `[aadf-probe2]` lines in `construction/mod.rs:3554-3568` are
   already long multi-field logs, and Playwright captures them whole.
   A typical `DeviceSnapshot` JSON is ~3–5 KiB; safe.

6. **`std::fs::write` is callable from a main-world Bevy system on native**
   — yes, no caveats. `wasm32` has no `std::fs::write`; the wasm path
   uses `info!` only.

7. **Playwright `page.on("console")` callback fires for `info!`-level lines
   reliably across Chrome stable + headed** — confirmed by the existing
   `vox-horizon-parity.spec.ts:216-237` which already relies on this
   for the Q4 + W5 + aadf-probe lines.

8. **`target/diagnostics/` is creatable by `playwright`-driven `fs.mkdir`
   recursively** — yes, standard Node `fs` API.

9. **The unused `bound_refined_info[8..15]` slots are not aliased by any
   other writer in `bounds_calc.wgsl` / `chunk_calc.wgsl` /
   `world_change.wgsl`** — needs verification. Greped only `bounds_calc.wgsl`;
   the other shaders may also bind `bound_refined_info`. If they do, the
   impl agent must pick fresh slots or grow the buffer (which is cheap;
   16 → 32 u32 is a 64-byte buffer).

10. **The `hash_chunks` compute pipeline can share the chunks buffer's
    existing bind-group layout** — needs verification. If the bind-group
    layout for the chunks buffer is per-pipeline, a new pipeline needs
    its own. The cheap alternative: emit the hash dispatch as part of
    the existing `prepare_group_bounds` pipeline (extend the WGSL with
    a `@workgroup_size(64)` branch keyed off a uniform).

11. **`max_compute_workgroups_per_dimension` reported by Dawn on the
    user's machine is >= what NAADF's `compute_voxel_bounds` actually
    dispatches** — the handoff says 134M groups were tried, split-by-N
    was tried, neither resolved the bug. This is *plausibly* refuted
    but **needs to be in the snapshot anyway** so we can stop arguing
    about it.

12. **The user's Chrome stable build on CachyOS uses Dawn-on-Vulkan,
    not Dawn-on-OpenGL or SwiftShader** — needs verification.
    `adapter_info.driver` + `adapter_info.driver_info` in the snapshot
    will pin this in plain text. The `justfile`'s `web-static` recipe
    passes `--enable-unsafe-webgpu --enable-webgpu-developer-features`,
    which on Linux defaults to Dawn-on-Vulkan if a Vulkan loader is
    present. Confirmable post-hoc from the first snapshot capture.

---

## Open questions for orchestrator / user

These were close calls; the design above takes a default but the
orchestrator may want a different choice.

1. **Path of the diagnostic output: `target/diagnostics/` (new dir) vs
   `target/e2e-screenshots/` (existing dump-ground for probes + PNGs).**
   Default chosen above: new dir, for hygiene. If the user prefers
   everything land in `target/e2e-screenshots/` to match the existing
   probe convention, that's a one-string change in the snapshot system
   + the Playwright spec.

2. **Schedule entry of the static snapshot: on a dedicated `--device-snapshot-native`
   e2e_render mode (extra CLI flag, dedicated single-purpose boot)
   vs piggybacking on `--vox-horizon-native` (the snapshot fires
   every time the parity gate runs, so we always have a fresh one
   when the gate fails).** Default chosen above: BOTH — a dedicated
   mode for explicit invocation + a piggyback on the existing gate
   for zero-friction freshness.

3. **Whether to ship a `--required-limits = adapter.limits()` override
   in Bevy's `WgpuSettings` on wasm32 right now**, while we're here.
   If §A.10 turns out true (Bevy is creating the device with
   conservative WebGPU minimums instead of the adapter's actual
   ceiling), some limits other than `max_storage_buffer_binding_size`
   may be silently clamped on wasm and not on native. The default
   above does NOT ship this change — the diagnostic snapshot's
   `limit_deltas` field surfaces the divergence, and the *next*
   dispatch decides whether to raise.

4. **Whether to add the dynamic hooks (`bound_refined_info[8..14]`,
   chunks-hash, timestamps) in the SAME dispatch as the static
   snapshot, or in a follow-up.** Default chosen above: design all
   three now, let the impl agent decide phasing based on its budget.
   The static snapshot alone is a 2–3 file change; the dynamic hooks
   add another 3–4 (shader edit + readback decode + maybe new
   pipeline). If the impl agent is rate-limited, prioritise the
   static snapshot — the bug isn't going anywhere.

5. **Whether the snapshot should also dump a per-format
   `Adapter::get_texture_format_features()` for a curated short list
   (the surface format + `r32uint` + `rgba8unorm` etc.).** Default
   chosen above: log only the surface format. Storage-texture features
   are unlikely-relevant per §B.9; expanding to every texture format
   bloats the JSON without diagnostic gain. If the impl agent finds a
   storage-texture binding in any of the bounds_calc / chunk_calc /
   world_change shaders, revisit.
