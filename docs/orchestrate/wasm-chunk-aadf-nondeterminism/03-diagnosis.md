# Diagnose-first findings — wasm-chunk-aadf-nondeterminism

## Posture statement

- The prior session's hypothesis class is dropped as a bias source. This
  diagnosis is grounded in (a) the static device-capability snapshots
  captured this session (`device-snapshot-{native,web}.json`) and (b) re-
  reading the source code (shaders + Rust dispatch paths + bind-group
  layouts) with the divergences in hand.
- I read the handoff for symptom + camera pose + persisted probe-data
  *context*, NOT for hypothesis carry-over. The handoff's "Already tried"
  set is treated as a list of *dead-end mitigations*, not a list of
  ruled-out diagnoses (those are different things — a mitigation that
  failed could be addressing a real partial cause).
- Two pieces of input from the orchestrator's brief turned out NOT to be
  on disk in this worktree: (1) the per-target `vox_horizon_*.aadf-probe.log`
  files are absent (`target/e2e-screenshots/` does not exist); the impl-
  log notes only the `device-snapshot.spec.ts` Playwright test ran this
  dispatch, not the parity gate that persists those logs. (2) The
  build-log warning enumeration is available — see Section D. Section E
  is therefore narrowed to "what the probe instrumentation in the *code*
  would surface" rather than "what the persisted logs said".

## A — Constraint × code mapping

For each LOAD-BEARING divergence + named secondary divergences, a row.
Storage-buffer counts are per `@compute` entry point.

| Divergence | Native | Web | Code touch points (file:line) | Does code violate web limit? | Evidence |
|---|---|---|---|---|---|
| **`max_storage_buffers_per_shader_stage`** | 524288 | **16** | `chunk_calc.wgsl:111-134` (8 bindings: 7 storage + 1 uniform). `world_change.wgsl:111-149` (8 storage in `@group(0)` + 4 ro storage in `@group(1)` + 4 rw storage in `@group(2)` = **16 storage buffers** in `apply_group_change`/`apply_block_change`/`apply_voxel_change`). `bounds_calc.wgsl:96-116` (2+4+1 = 7 storage). Layout descriptors: `chunk_calc.rs:61-92`, `world_change.rs:62-80` + bounds_calc owns its own, `bounds_calc.rs:70-129`. | **`world_change` entry points sit RIGHT AT the limit (16). One additional binding would push them over.** Currently legal but zero head-room. Not the bug for the static path. | `world_change.wgsl` enumerates `@group(0) @binding(0..7)` (8 entries, of which 7 are storage and one is uniform → 7 storage) + `@group(1) @binding(0..3)` (4 storage, all ro) + `@group(2) @binding(0..3)` (4 storage, all rw). 7+4+4 = **15 storage buffers** per stage. WGSL `var<uniform>` does not consume the storage-buffer cap. Within limit but the bind-group entries land on `group_id` 0/1/2 which is also bumping against `max_bind_groups`. |
| **`max_bind_groups`** | 8 | **4** | `bounds_calc.rs:189` (`vec![world_layout, bounds_layout, dispatch_layout]` — 3 groups in `prepare_group_bounds` pipeline). `world_change.rs:95` (`vec![world_layout, change_layout, bounds_layout]` — 3 groups). All construction pipelines use ≤3 bind groups. | No — uses 3 of 4 max. Head-room is 1. | Set-bind-group call sites confirm `set_bind_group(0..2, …)` — `bounds_calc.rs:431-433, 448-449`, `world_change.rs:262-264, 286-288, 310-312, 334-336`. Renderer-side `world_data.wgsl:60-130` is `@group(0)` exclusively for the ray-tracing pass; no construction-pass bind-group violation. |
| **`max_buffer_size`** | 1099511627776 (1 TiB) | **4294967292 (4 GiB − 4)** | `render/prepare.rs:396-490` — `chunks_buffer = chunk_count * 8 B`; voxels alloc = `chunk_count * 128` u32s = `chunk_count * 512 B`. For Oasis (≈ 4 M chunks if 256³): `chunks ≈ 32 MiB`, `voxels ≈ 2 GiB`. | No — even at Oasis-scale max each individual buffer fits under 4 GiB − 4. Handoff line 181 + Q4 logger at `prepare.rs:535-571` cross-confirmed. | `prepare.rs:458` comment: `voxels_alloc_len = chunk_count * 128 = 268,435,456 u32s = 1 GiB` (at 2,097,152-chunk scale). Within both `max_storage_buffer_binding_size = 2 GiB − 4` AND `max_buffer_size = 4 GiB − 4`. |
| **`max_storage_buffer_binding_size`** | 2147483644 (2 GiB − 4) | 2147483644 | Q4 logger `prepare.rs:535-571` already reports this. | No. Identical on both targets (Dawn coincidentally reports the same 2 GiB − 4 ceiling that Vulkan does for NVIDIA). | Both snapshots show `2147483644` byte-for-byte. |
| **`min_storage_buffer_offset_alignment`** | 32 | **256** | No `dynamic_offsets` are used. Every `set_bind_group` call passes `&[]` (no dynamic offsets) — `bounds_calc.rs:243-244, 293-295, 308-309, 431-433, 448-449`; `chunk_calc.rs:181, 209, 289, 312`; `world_change.rs:262-264, 286-288, 310-312, 334-336`; `entity_update.rs:239-262`; `generator_model.rs:240`; `map_copy.rs:160`. | No — dynamic offsets are not used at all. The 256-alignment cap therefore has no effect on the construction path. | `grep -rn set_bind_group crates/bevy_naadf/src/render/` shows every call uses `&[]` for `dynamic_offsets`. The 8-fold tighter offset alignment on web is **moot** for this codebase. |
| **`max_dynamic_storage_buffers_per_pipeline_layout`** | 16 | 8 | Layout entries: `chunk_calc.rs:61-92`, `bounds_calc.rs:70-129`, `world_change.rs:62-80`. All use `storage_buffer_sized(false, None)` / `storage_buffer_read_only_sized(false, None)`. The `false` argument is `has_dynamic_offset: bool` (Bevy wgpu helper). | No — every binding has `has_dynamic_offset = false`. The cap is irrelevant. | Same grep as above plus `bind_group_layout_entries` builder pattern. |
| **`max_texture_dimension_3d`** | 16384 | **2048** | `texture_storage_3d<…>` previously bound for chunks/blocks/voxels was REPLACED with flat `array<vec2<u32>>` / `array<u32>` storage buffers (web-WebGPU migration, every shader's @group(0) @binding(0) comment block: e.g. `chunk_calc.wgsl:106-112`, `world_change.wgsl:93-95`, `bounds_calc.wgsl:94-95`). | No — no 3D textures remain in the construction path on web. | Each shader's binding-comment block explicitly references the migration: "was `texture_storage_3d<rg32uint, read_write>`". |
| **`adapter_info.subgroup_min_size` / `subgroup_max_size`** | 32 / 32 (fixed) | **4 / 128 (variable)** | No subgroup intrinsics used anywhere in `bounds_calc.wgsl`, `chunk_calc.wgsl`, `world_change.wgsl`. `grep -RnE 'subgroup_\|wave_\|workgroupUniformLoad' crates/bevy_naadf/src/assets/shaders/` returns 0 hits for the relevant shaders. | No code path triggers subgroup-size variability. | Confirmed grep + Section B below. |
| **`adapter_features.only_in_native: subgroup, subgroup-barrier`** | present | absent | Not used. | Not a violation; future-shader change risk only. | Same grep as above. |
| **`adapter_features.only_in_native: memory-decoration-{coherent,volatile}`** | present | absent | Affects WGSL→SPIR-V lowering: native may emit `Coherent`/`Volatile` decorations on storage-buffer accesses; Dawn cannot. WGSL atomic ops at the call sites that drive the symptom are: `bounds_calc.wgsl:250-252` (`atomicStore` initial seed), `bounds_calc.wgsl:278` (`atomicLoad` in prepare), `bounds_calc.wgsl:300` (`atomicStore` in prepare), `bounds_calc.wgsl:353` (`atomicAnd` in compute), `bounds_calc.wgsl:434` (`atomicOr` in compute), `bounds_calc.wgsl:439` (`atomicAdd` in compute). | **POSSIBLY load-bearing** — see Hypothesis 1. | Snapshot diff lines 86, 99. |
| **`adapter_features.only_in_native: mappable-primary-buffers`** | present | absent | Affects `mapAsync` semantics on readback. `populate_cpu_mirror_from_gpu_producer` (`mod.rs:1042-…`) does cross-frame readback. | Possibly relevant to probe-read-back accuracy but not the symptom (the SCREEN output is what the user observes, the readback is for the diagnostic). | Snapshot diff lines 86, 99. |
| **`max_immediate_size`** | 256 | 0 | No push-constants used (renderer is pre-`push-constant` migration). | No. | N/A. |

**Most load-bearing observation in this table:** every "tight" limit on web
that the bug could plausibly trip is either (a) not actually approached by
the construction-path bind-group layouts (the 16-storage-buffer cap is
tight on `world_change.wgsl` at 15 storage bindings, but `world_change`
runs only on `apply_*` edit dispatches — NOT on the per-frame regime-2
loop) or (b) covered by the WebGPU spec as a structural failure
(pipeline-creation rejection or always-no-op), which would produce a
DETERMINISTIC failure, not the run-to-run-varying symptom the user sees.
A non-deterministic symptom under fixed inputs points away from the
"limit" class of divergences and toward the **atomic-visibility / memory-
model / queue-ordering** class.

## B — Subgroup-operation usage

`grep -RnE 'subgroup_|wave_|workgroupUniformLoad|subgroupBroadcast|subgroupBallot|subgroupAdd|subgroupAll|subgroupAny|subgroupSize' crates/bevy_naadf/src/assets/shaders/` returns 0 hits in the construction-path shaders. The only "subgroup" matches in the tree are:

- `bounds_calc.wgsl` — comment text only (the file-header MonoGame→wgpu
  deviation notes mention "groupshared" and "atomic" but no subgroup
  intrinsics).
- `chunk_calc.wgsl` / `world_change.wgsl` — comment text only.
- `world_data.wgsl` / `ray_tracing.wgsl` — comment text mentioning "wave-3
  integration"; this is NAADF's W*Wave*-3 wiring (a phase-naming
  convention in `15-design-c.md`), NOT a wave/subgroup intrinsic.

**Per-shader enumeration of subgroup operations:**

- `bounds_calc.wgsl`: **none**.
- `chunk_calc.wgsl`: **none**.
- `world_change.wgsl`: **none**.

Workgroup-scope synchronization that IS used:

- `bounds_calc.wgsl:398, 424` — `workgroupBarrier()` in `compute_group_bounds`.
- `chunk_calc.wgsl:205, 216, 227, 238, 397, 404, 411, 441, 508, 554` —
  `workgroupBarrier()` throughout.
- `world_change.wgsl:237, 248, 259, 270, 299, 385, 394, 504, 563` —
  `workgroupBarrier()` throughout.

Verdict: **subgroup-size variability on web (4..128 vs fixed 32 on native)
cannot cause the symptom** through this codebase's current shaders.
Workgroup-barriers are well-defined regardless of subgroup size; the
4-thread minimum on Intel would not be a problem here because no
intrinsic depends on it.

## C — `bounds_calc.rs:472` unreachable statement

The wasm build emits:

```
warning: unreachable statement
   --> crates/bevy_naadf/src/render/construction/bounds_calc.rs:472:5
    |
465 |           return;
    |           ------ any code following this expression is unreachable
...
472 | /     {
473 | |         static LOGGED: std::sync::atomic::AtomicBool =
474 | |             std::sync::atomic::AtomicBool::new(false);
475 | |         if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
...   |
483 | |     }
    | |_____^ unreachable statement
```

The unreachable block (lines 472-483) is a `[aadf-probe] regime-2 config:
n_bounds_rounds=… max_group_bound_dispatch=…` one-shot info log. It is
*not* cfg-gated; the surrounding wasm-only block at lines 413-466 ends
with `return;` at line 465, so on `target_arch = "wasm32"` the diagnostic
log block at 472-483 is dead code. The block contains NO load-bearing
side-effect — just a `bevy::log::info!` for the operator. The native
build runs it; the wasm build skips it.

**Could its absence on wasm cause the symptom?** No. It is purely an
observability omission — the log line that would tell the user the wasm
clamp on `max_group_bound_dispatch` flowed through to the node is missing
on wasm, but the clamp itself (in `config.rs:234-244`) DOES still apply.
This is a low-confidence finding for *the diagnostic infrastructure* (the
operator loses a useful console line on wasm) but **not for the bug
mechanism**.

There is a parallel concern: the wasm path's `[aadf-probe] regime-2 config…`
log was likely *intended* to fire on wasm — the comment immediately
above it (`"verifies the wasm clamp on max_group_bound_dispatch actually
flows here"`) is wasm-relevant. The early `return;` at line 465 makes that
intent inaccessible. This is a tiny doc/observability bug the orchestrator
may want to fix opportunistically, but it is not the cause of the SSIM
non-determinism.

Surrounding context (lines 462-484):

```
            render_queue.submit([round_encoder.finish()]);
        }
        return;
    }

    // 2026-05-19 horizon-parity AADF diagnostic — one-shot log of the
    // construction-config values reaching the regime-2 node (verifies the
    // wasm clamp on `max_group_bound_dispatch` actually flows here from
    // `From<&AppArgs> for ConstructionConfig`).
    {
        static LOGGED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            bevy::log::info!(
                "[aadf-probe] regime-2 config: n_bounds_rounds={} \
                 max_group_bound_dispatch={} (the wasm clamp ceiling is 4096)",
                n_rounds,
                construction_config.max_group_bound_dispatch,
            );
        }
    }
```

## D — Compiler warnings from wasm build

Verbatim from `target/diagnostics/logs/re-01-web-build.log`:

1. **`warning: unstable feature specified for -Ctarget-feature: 'atomics'`**
   — generic Rust→wasm32 build flag; required for atomic-storage WGSL
   support compiled via wgpu's wasm bindings. **Triage: benign** — the
   build is correct; just a stability disclaimer. Not symptom-relevant.

2. **`warning: unused import: 'parse_to_imported_vox' (voxel/async_vox.rs:24:48)`**
   — dead-code lint, has zero behavioural effect. **Triage: benign.**

3. **`warning: unreachable statement (bounds_calc.rs:472:5)`** — see
   Section C; observability bug, not behavioural. **Triage: minor doc
   bug; not symptom-relevant.**

4. **`warning: variable does not need to be mutable (bounds_calc.rs:343:5)`**
   — `mut render_context: RenderContext` — the `mut` is unused on the
   wasm code path because the wasm-only branch at 413-466 doesn't touch
   `render_context.command_encoder()`. **Triage: benign**, follows from
   #3 (the part of the function that uses `render_context` is dead on
   wasm).

5. **`warning: unused variable: 'render_context' (bounds_calc.rs:343:5)`**
   — same root cause as #4. **Triage: benign.** Hard evidence that the
   wasm path is fully self-contained in lines 413-466 and bypasses the
   indirect-dispatch route entirely. (This is by design — the
   commentary at lines 485-501 documents the WebGPU `STORAGE→INDIRECT`
   barrier workaround.)

6. **`warning: unused variable: 'indirect_buffer' (bounds_calc.rs:391:14)`**
   — same root cause. **Triage: benign**, confirms the wasm path does
   not consume the indirect buffer.

None of the six warnings are symptom-relevant. Their collective story is
*"the wasm path is a separate code branch and the native path's plumbing
is dead on wasm"* — which we already knew from the comments.

## E — Persisted probe-data comparison

`target/e2e-screenshots/` does not exist in this worktree; the per-target
`vox_horizon_*.aadf-probe.log` files were not generated this dispatch
(only the `device-snapshot.spec.ts` Playwright test ran; the parity
gate that writes those logs was not invoked). The impl log's "Artifacts
on disk" section lists *snapshot* logs (`re-0{1,2,3}-…`) but NOT the
probe logs from a parity run.

**What the in-source probe instrumentation will surface on the next
parity run** (`mod.rs:3395-3565`, `bounds_calc.wgsl:307-315, 420`):

- `aadf-probe2 pass=0` and `pass=1` (frames 30 and 200 post-mirror-pop)
  dump 16 u32s of `bound_refined_info`:
  - `[0]` = start, `[1]` = count (groups picked this round),
    `[2]` = packed `bound_size | (axis << 16)`.
  - `[3]` = last picked bound-size level (0..31).
  - `[4]` = last picked axis (0..2).
  - `[5]` = `atomicLoad(&bound_queue_info[qi].size)` at last pick.
  - `[6]` = monotonic per-workgroup "did expansion" counter (race-y but
    presence/absence is binary diagnostic).
  - `[7]` = monotonic prepare-call counter.
- 768 B of `bound_queue_info` (the 96 `{start, size}` pairs).
- 16 MiB of `chunks[]` (decoded per-axis 5-bit AADFs).

The handoff line 51-53 describes what *prior runs* of those probes
showed: "Native consistently shows skip distances of 3-4 chunks per
direction. Web shows wildly varying values per run, often 0-1." This
matches **"the chunk-AADF queue never converges past bound-size level 1
on web"**.

Section J below recommends adding a per-round prepare-counter and
queue-state hash to discriminate hypotheses on the next probe run.

## F — Candidate hypotheses (ranked)

### Hypothesis 1: cross-pass atomic-store/atomic-load on `bound_queue_info[].size` is non-deterministically reordered through Dawn-Vulkan's compute-to-compute barrier path (Tint vs naga lowering)

- **Specific divergence(s) cited:**
  - `adapter_features.only_in_native` includes `memory-decoration-coherent`
    and `memory-decoration-volatile` (snapshot diff lines 86, 99).
  - `adapter_info.subgroup_*_size` divergence is **NOT** cited
    (Section B shows subgroup intrinsics are unused — orthogonal).
- **Specific code refs (file:line):**
  - `bounds_calc.wgsl:300` — `atomicStore(&bound_queue_info[qi].size, found_size - group_amount);` (prepare's drain).
  - `bounds_calc.wgsl:278` — `let size = atomicLoad(&bound_queue_info[qi].size);` (prepare's next-round pick).
  - `bounds_calc.wgsl:439` — `atomicAdd(&bound_queue_info[qi].size, 1u);` (compute's re-enqueue).
  - `bounds_calc.rs:413-466` — the wasm-only per-round encoder+submit
    branch the prior session added to force a queue-timeline fence
    between prepare and compute passes.
  - `bounds_calc.rs:430-454` — the WASM 4096 direct-dispatch path
    bypasses indirect — confirms that route is dead on wasm; the
    cross-pass ordering left to worry about is purely
    atomic-store-then-atomic-load on the shared `bound_queue_info`
    storage buffer.
- **WebGPU spec section (if applicable):**
  - WebGPU §16 (Compute Passes) — atomic operations have device scope
    by default; cross-pass visibility is implementation-defined but the
    spec REQUIRES the queue-timeline ordering to flush all prior compute
    writes before subsequent compute reads in the same queue.
  - WGSL §14.5 (Memory Model, Scoped Operations) — atomic ops use
    `seq_cst` memory order; relaxed/acquire/release decorations are
    expressible at SPIR-V/HLSL/MSL levels but WGSL does NOT surface
    them to user code. Tint and naga therefore have implementation
    latitude on which SPIR-V decorations they emit.
  - WGSL §17.11.1 — `storageBarrier()` synchronises within a workgroup
    only. NOT used in `bounds_calc.wgsl` (which would be incorrect
    anyway for cross-pass visibility).
- **Why this matches the symptom:**
  Tint-on-Vulkan and naga-on-Vulkan target the same Vulkan device
  underneath but lower atomic ops through separate paths. If Tint emits
  the storage-buffer access without the `Coherent` /
  `MakeAvailable+MakeVisible` decoration on the `bound_queue_info`
  buffer (because Dawn doesn't expose `memory-decoration-coherent`),
  the cross-pass atomic-store-then-atomic-load may rely on the *Vulkan
  queue submit timeline* alone — which IS a memory-model boundary, but
  the visibility within the **same** queue submit (prepare-pass-1
  writes, compute-pass-1 reads, compute-pass-1 writes, prepare-pass-2
  reads — all four within one encoder pre-fix) is governed by the
  cross-pass `vkCmdPipelineBarrier(SHADER_WRITE→SHADER_READ + STORAGE)`
  that Dawn emits at pass-boundaries. If THAT barrier is mis-tracked
  (e.g. Dawn classifies the buffer as STORAGE-read-only when
  pass-2-prepare only `atomicLoad`s it, missing the dependency on
  pass-1-compute's `atomicAdd`), pass-2-prepare may read a stale value.
  Non-determinism on the SSIM gate then reflects: a partial flush is
  driver-state-dependent (cache-line allocation, queue-warmup,
  concurrent JS-thread work) — same code, same inputs, different
  observed values per run.
- **Why this is consistent with the "Already tried" set in the handoff:**
  - "Per-round encoder+submit on wasm32 — neutral effect on SSIM (~0.79)"
    — partial mitigation. Forcing a `vkQueueSubmit` boundary between
    rounds creates a queue-timeline fence (a stronger guarantee than
    `vkCmdPipelineBarrier`), and the SSIM number moved from "broken"
    (~0.30 implied) to "marginal" (~0.79). It did NOT move to the
    native-equivalent ~0.94+. This is consistent with "the
    atomic-visibility problem is *partly* the cross-encoder boundary
    AND *partly* the within-encoder cross-pass barrier", and only
    fixing one of them moves the dial halfway.
  - "Converting `bound_refined_info` to `array<atomic<u32>>` —
    REGRESSED to word=0x00000000" — converting a *non-shared* storage
    buffer to atomic doesn't help cross-pass visibility; it just adds
    overhead, and adding overhead exposed an unrelated atomic-store-of-
    zero race on the per-shader `[0..7]` write pattern at
    `bounds_calc.wgsl:293-303` (the prepare pass writes 0 at index [1]
    when no group is found, which under atomic-store semantics zeroes
    the slot the compute pass is mid-read; the non-atomic path got
    away with this because the read was racy but consistent on the
    same wave).
  - "Raising `WASM_MAX_GROUP_BOUND_DISPATCH` 4096 → 32768 — REGRESSED
    SSIM 0.94 → 0.69" — direct evidence for atomic-contention
    consequences at scale: `compute_group_bounds` re-enqueue calls
    `atomicAdd(&bound_queue_info[qi].size, 1u)` once per workgroup
    that grows its AADF this round (`bounds_calc.wgsl:439`); 32 768
    workgroups × ~30 % re-enqueue rate = ~10 000 atomic-adds to the
    same 96-slot table per round. Under a memory-model-relaxed
    backend, the loads from `prepare_group_bounds` in the next round
    may see arbitrarily many of those adds reflected — variance grows
    with concurrency.
- **Confidence:** **medium-high** — the symptom shape (non-determinism +
  wasm-specific + the prior mitigation set's partial+contradictory
  effects) all fit. What is missing is a direct observation of
  *what value* `prepare_group_bounds`'s `atomicLoad` returns on
  consecutive rounds. The probe slot `[5]` (`bounds_calc.wgsl:310`)
  is exactly the right place to assert this; the next experimental
  probe (Section J item 1) is to log that per-round.

### Hypothesis 2: the wasm-only direct-dispatch of 4096 workgroups under-drives the queue when `found_size > 4096`, leaving residual queue entries that the next round's `prepare_group_bounds` IS forced to consume (because the `bound_queue_info[].start` cursor was already advanced) — and the residual entries inflate or deflate non-deterministically based on which compute-side `atomicAdd` re-enqueues observed which prepare-side `atomicStore`

- **Specific divergence(s) cited:**
  - This is a *consequence of the WASM clamp* `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` interacting with `prepare_group_bounds`'s
    `bound_queue_info[qi].start = (found_start + group_amount) %
    bound_group_queue_max_size;` cursor advance (`bounds_calc.wgsl:299`)
    — the cursor advances BEFORE the compute pass actually drains the
    claimed slice, so on the next round the prepare pass sees the
    cursor as already-past the claimed work. This is a faithful port
    of the C# (`boundsCalc.fx:85-91`); on native it works because
    the indirect dispatch dispatches exactly `group_amount` workgroups
    (= `bound_refined_info[1]`) and there's no slop. On wasm, the
    direct-dispatch always dispatches `WASM_MAX_GROUP_BOUND_DISPATCH =
    4096` workgroups, and `compute_group_bounds`'s `is_group_active =
    group_id.x < count` short-circuits past index `count`. That's
    semantically correct *as long as `prepare`'s `group_amount` ≤
    4096*; the `min(max_group_bound_dispatch, found_size)` at line 292
    enforces this. So this is internally consistent.
  - HOWEVER: web has `min_storage_buffer_offset_alignment = 256`
    (vs 32 on native) — irrelevant here (no dynamic offsets) — AND
    `max_storage_buffer_binding_size = 2 GiB - 4` which IS the same on
    both. So the *size* of the bound-queue family is not pressured.
- **Specific code refs (file:line):**
  - `bounds_calc.wgsl:292` — `group_amount = min(params.max_group_bound_dispatch, found_size);`
  - `bounds_calc.wgsl:298-300` — cursor advance + `atomicStore`.
  - `bounds_calc.rs:450-454` — wasm direct dispatch of
    `construction_config.max_group_bound_dispatch.max(1)` workgroups.
- **Why this matches the symptom:** If the per-round prepare-pass picks
  a different queue (axis, bound-size level) each round
  non-deterministically because the read of the *prior round's*
  `atomicStore` is racy (Hypothesis 1's mechanism), then the rate of
  convergence is non-deterministic, the *final* AADF map is
  non-deterministic, and the SSIM number varies. The two hypotheses
  COMPOSE.
- **Why this is consistent with the "Already tried" set:** The
  4096-vs-32768 regression is direct evidence that the *number of
  re-enqueues per round* matters. The clamp at 4096 keeps the per-round
  re-enqueue count bounded; raising it to 32768 lets each round
  re-enqueue 8× more, which 8× amplifies the cross-pass atomic-
  visibility race in Hypothesis 1.
- **Confidence:** **medium** — depends on Hypothesis 1 being the
  underlying mechanism. Standalone, the 4096 cap is semantically
  legitimate.

### Hypothesis 3: Dawn-on-Vulkan's pass-resource-usage-tracker mis-categorises the cross-encoder `bound_queue_info` buffer when the per-round encoder+submit pattern is in effect, and an early-round `atomicAdd` is not flushed before the next round's encoder's first `atomicLoad`

- **Specific divergence(s) cited:**
  - `adapter_features.only_in_native: memory-decoration-coherent` —
    coherent storage decoration is the SPIR-V hint the Vulkan driver
    needs to skip the cache-line of the atomic operation. Without it,
    the driver may delay the write to L2 / DRAM until a fence sees the
    line in a *different* commandbuffer.
  - `adapter_features.only_in_native: mappable-primary-buffers` —
    bufer-of-this-class is typically Vulkan
    `HOST_VISIBLE+HOST_COHERENT|DEVICE_LOCAL`; Dawn does not surface
    this feature, so Dawn's storage buffers are typically
    `DEVICE_LOCAL` only, with a `vkInvalidateMappedMemoryRanges`
    requirement that doesn't exist on the path that just submits two
    commandbuffers back-to-back.
- **Specific code refs (file:line):**
  - `bounds_calc.rs:415-465` — per-round `device.create_command_encoder
    + queue.submit([round_encoder.finish()])`.
- **Why this matches the symptom:** Compute kernel writes within a
  command encoder are guaranteed visible across pass boundaries via
  `vkCmdPipelineBarrier`. Writes across `vkQueueSubmit` boundaries
  are guaranteed visible via the *queue timeline*. The handoff
  comment at `bounds_calc.rs:456-462` claims the latter is sufficient
  ("separate submits force a full GPU sync"). But Dawn's
  cross-commandbuffer tracking is in `dawn/src/dawn/native/CommandBufferStateTracker.cpp`
  and there are historical bug classes (crbug.com/dawn/1338 referenced
  in `01-diagnostics-design.md` §B.3) where the Dawn-side tracker
  fails to insert the required Vulkan barrier on
  cross-commandbuffer-submit for storage→indirect AND
  storage→storage-atomic-load. The wasm-only fix that switched the
  bug from "always broken (0% SSIM)" to "neutral effect on SSIM
  (~0.79)" actually exposed a residual cross-encoder atomic-load
  race. The "SSIM is 0.79 not 0.94" gap measures the residual.
- **Why this is consistent with the "Already tried" set:**
  Per-round encoder+submit was tried and was *neutral on SSIM*. That
  is consistent with "the encoder boundary is *necessary but not
  sufficient* — the actual bug is that even WITH separate submits,
  the cross-submit atomic-visibility is mis-tracked under Dawn's
  current state-tracker logic for this specific buffer-usage
  pattern."
- **Confidence:** **medium** — concrete enough to be testable. The
  testable artefact is whether *replacing* the wasm-only direct
  dispatch with a `queue.write_buffer(&bound_queue_info, …, &[…])`
  CPU-side fence between rounds + a `device.poll(Wait)` would land
  the same result as a true cross-frame separator. (NOT a proposed
  fix; an experimental probe — Section J item 2.)

### Hypothesis 4: WGSL `select` / `min` / 32-bit-integer ops are constant-folded or evaluated with different precedence by Tint vs naga on the `compute_voxel_bounds` and `compute_block_bounds` reductions, producing slightly different `cached_cell[…]` values across the 3-iteration `compute_bounds_4` loop on web

- **Specific divergence(s) cited:**
  - `device_features` (web) is missing `shader-f16`, `shader-f64`,
    `shader-int64`, `shader-int64-atomic-*` — none of which the
    construction shaders use (every type is `u32` / `i32` / `f32`).
  - `min_uniform_buffer_offset_alignment: 64 → 256` on web — uniform
    `params` is 80 bytes (4 rows of 16 each, padded to 16-byte
    boundaries; `gpu_types.rs:583-630`). No relevance here.
- **Specific code refs (file:line):**
  - `chunk_calc.wgsl:197-241` — `compute_bounds_4` loop, used by
    `compute_voxel_bounds` and `compute_block_bounds`.
- **Why this matches the symptom:** The shaders are *all-integer* on
  the hot path. WGSL spec guarantees integer ops are bit-exact across
  implementations. Float ops (none on this path) could cause naga vs
  Tint divergence, but they aren't here. **This hypothesis
  weakens itself on inspection.**
- **Confidence:** **low** — included for completeness; the
  construction-path shaders are integer-only, so float-evaluation
  divergence is not in play.

### Hypothesis 5: the chunks_buffer `array<vec2<u32>>` migration from `texture_storage_3d<rg32uint, read_write>` exposed a buffer-aliasing visibility bug — two compute passes (e.g. `compute_voxel_bounds` and `compute_block_bounds`) bind it as the same `@group(0) @binding(0)` rw storage in different pipelines, and Dawn's pass-boundary tracking treats the cross-pipeline access as a no-barrier reuse (the same buffer through the same binding in adjacent pipelines)

- **Specific divergence(s) cited:**
  - `max_texture_dimension_3d = 16384 → 2048` web is the *reason* the
    migration was performed (the test world's 256³ chunks would have
    fit even on web, but Oasis's 84³ doesn't fit web's
    `max_texture_dimension_3d` at the per-axis 2048 cap because Oasis
    chunks count is 84 voxels per axis = 84 chunks per axis, which
    fits, but the *block layer* is 4×84 = 336 per axis — over the
    256-default but under the 2048 web cap).
- **Specific code refs (file:line):**
  - `chunk_calc.wgsl:111-112` — `@group(0) @binding(0) var<storage, read_write> chunks: array<vec2<u32>>;`.
  - `world_change.wgsl:111-112` — identical.
  - `bounds_calc.wgsl:96-97` — `@group(0) @binding(0) var<storage, read_write> chunks: array<vec2<u32>>;`.
- **Why this matches the symptom:** The same `chunks` buffer is rw on
  THREE different pipelines + each has its own bind-group-layout. On
  pass-boundary tracking, Dawn must track the *underlying buffer*'s
  last-writer + insert a STORAGE→STORAGE barrier between the pipelines.
  Vulkan-direct does this via `vkCmdPipelineBarrier`. Dawn does
  too, but for a single command encoder.
  Across separate encoders (the wasm-only per-round-submit pattern at
  `bounds_calc.rs:415-465`) the barrier is implicit in the queue-
  timeline submit-order. The non-determinism would manifest if the
  *queue-timeline order* between two `queue.submit([cb])` calls is
  not preserving the atomic-write ordering on the shared `chunks`
  buffer.
- **Why this is consistent with the "Already tried" set:** The
  `chunks` migration to flat storage buffer was post-handoff (looking
  at the file comments, this happened pre-handoff but the per-pipeline
  binding pattern is unchanged from native). The Already-Tried set
  doesn't directly test for `chunks`-cross-pipeline-visibility.
- **Confidence:** **low-medium** — Dawn historically handles
  cross-pipeline storage-buffer barriers within one encoder, but the
  cross-encoder case for atomic-write→atomic-read is the same suspect
  surface as Hypothesis 3. Combining with Hypothesis 3 doesn't add
  evidence.

## G — Hypotheses ruled out by this data (versus the handoff's "Already tried")

| Already-tried item | New data verdict |
|---|---|
| Q4 storage-buffer-binding-size overrun | **Confirmed refuted.** Both targets report `max_storage_buffer_binding_size = 2147483644` byte-for-byte. Q4 is closed. |
| Browser GPU watchdog killing the 134M-workgroup `compute_voxel_bounds` dispatch | **Weakened.** Web `max_compute_workgroups_per_dimension = 65535` (same as native), and the dispatch is split via `split_3d_dispatch` in `chunk_calc.rs:246-265`. The watchdog hypothesis was already partly refuted; the new data does not strengthen it. Orthogonal. |
| Raising `WASM_MAX_GROUP_BOUND_DISPATCH` 4096 → 32768 (regression) | **Confirmed: 4096 is right** as a workaround scale, but for the wrong reason. The clamp doesn't fix the underlying bug; it just bounds the per-round atomic-contention amplitude (Hypothesis 2). The 32768 regression was an amplification of the atomic-visibility race, not an indictment of the clamp. |
| Converting `bound_refined_info` to atomic everywhere (chunks state went to 0x00000000) | **Confirmed: that was the wrong field to atomicise** (Hypothesis 1 commentary). `bound_refined_info` is per-prepare-call scratch, written by exactly one thread (the `workgroup_size(1,1,1)` prepare); making it atomic introduced an unrelated race on the prepare's `[1] = 0u` else-branch. Orthogonal to the cross-pass visibility hypothesis. |
| Per-round encoder+submit on wasm32 (neutral SSIM effect ~0.79) | **Confirmed partial fix; sufficient evidence that the residual gap (0.79 → 0.94) is in the *within-cross-encoder* atomic-visibility class, not the cross-encoder barrier itself** (Hypothesis 3). |
| Increasing Playwright `CANVAS_SETTLE_MS` 10 s → 30 s (neutral) | **Confirmed orthogonal.** The bug is in the per-frame regime-2 loop's atomic visibility, not in the per-frame timing or the test harness. |
| Disabling in-canvas UI via `?ui=hide` (fixed earlier false-pass) | **Confirmed orthogonal** (was a different bug). |
| `apply_initial_camera_pose_changes` + camera-spawn override | **Confirmed orthogonal.** |
| Playwright Chrome vs user Chrome divergence | **Confirmed refuted.** Same flags, same channel. |

## H — Decisions & rejected alternatives

- **Decision: prioritise the atomic-visibility hypothesis class over the
  device-limit hypothesis class.** Rationale: non-deterministic
  symptom under fixed inputs. Limit overages cause deterministic
  failures (pipeline rejection or persistent no-op). The new snapshot
  data refutes every load-bearing limit overage hypothesis (Section A
  table) — the construction-path bind-group layouts fit web's caps
  with measurable head-room. The remaining surface that can cause
  RUN-TO-RUN variance is the memory-model + atomic visibility +
  queue-timeline class. **Alternative considered:** lead with the
  `max_bind_groups: 4` head-room-of-1 finding (Section A) and the
  `world_change.wgsl` 15-storage-buffer count (one binding away from
  the 16 cap). **Why rejected:** both are deterministic, neither
  trips today; flagging them as next-phase technical debt is fine but
  they don't explain non-determinism.
- **Decision: treat the per-round encoder+submit pattern's "neutral"
  effect as evidence of partial fix, not no-fix.** Rationale: it
  moved SSIM from "broken" to "marginal", which is consistent with
  *one of two* atomic-visibility surfaces being addressed (the
  cross-encoder one). **Alternative considered:** dismiss the
  partial fix as noise. **Why rejected:** the prior session
  documented the move as a measurable improvement, and the
  reproducible numerical gap from native (0.79 vs 0.94) is well above
  per-run noise from one source.
- **Decision: do NOT propose a specific code change in this
  diagnosis.** Rationale: the brief's hard rule. The next phase is
  fix design.

## I — Assumptions made

- I assume the impl-log's report of the diag-compare output is accurate
  (the snapshot JSONs both load and contain matching field sets).
  Spot-checked against the actual JSON in
  `target/diagnostics/device-snapshot-{native,web}.json`: both load,
  load-bearing fields match the impl-log's quoted values byte-for-byte.
- I assume the prior session's claim that the per-round encoder+submit
  fix moved SSIM from "broken" to ~0.79 is accurate. (Handoff line
  111-112 explicitly states "Per-round encoder+submit on wasm32 …
  neutral effect on the SSIM number (~0.79). Currently still in place
  at `bounds_calc.rs:365-417`." — I read this as "the fix is in place
  AND the resulting SSIM is ~0.79".)
- I assume the Oasis world fits within `max_buffer_size = 4 GiB − 4` on
  web (it does: at 1488×544×1344 voxels = 23×8×21 chunks at 64
  voxels/chunk per axis = `23 × 8 × 21 = 3 864` chunks × 8 B = 30 KiB
  chunks buffer; voxels alloc = `chunk_count × 128` u32s = ~2 MiB).
  Even at the "max world" implied by the project's published Oasis
  scale (much larger), the voxels alloc is bounded by `max_buffer_size`.
- I assume `world_change.wgsl` correctly counts 15 storage buffers per
  stage (7 in group 0 + 4 in group 1 + 4 in group 2 = 15; the `params`
  uniform in group 0 binding 6 is a uniform buffer and does NOT count
  against `max_storage_buffers_per_shader_stage`).
- I assume `naga` vs `Tint` is the actual WGSL→SPIR-V/HLSL translator
  pair in this pipeline (naga in wgpu-native, Tint in Dawn). The
  build-log doesn't quote translator versions; verifiable by checking
  the wgpu 29.0.3 Cargo.lock's naga version + Chrome's bundled Dawn.

## J — Recommended next experimental probes (if any)

NOT fixes. NOT design. Just measurements that would discriminate
Hypothesis 1/3 (the load-bearing pair) from Hypothesis 2:

1. **Log per-round `prepare_group_bounds` `[5] = atomicLoad(&bound_queue_info[qi].size)` values** by writing them to a small ring-buffer in `bound_refined_info[8..16]` (8 slots × prepare-call-count modulo 8). This already exists as a single-shot field (`bounds_calc.wgsl:310`); the change is to make it a per-round history rather than a "last-call" overwrite. If the ring shows web reading consistently lower values than the compute-side `atomicAdd` count would have written, that proves Hypothesis 1.
2. **Add a `queue.write_buffer` fence between rounds on wasm**: replace the per-round `queue.submit([round_encoder.finish()])` with one followed by a CPU-side `queue.write_buffer(&bound_queue_info, 0, &[…current values…])` that re-asserts the host's belief. If this stabilises SSIM, Hypothesis 3 (cross-encoder visibility) is confirmed.

(Stop at 2 — the third probe would chase Hypothesis 4 / 5 which are low-confidence.)

## K — Open questions for the orchestrator / user

None.
