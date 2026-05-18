# 03-design — web-vox-color-divergence

2026-05-18

Architect-phase design for the web-only voxel color divergence root-caused
in `02-research.md`. Verbatim user goal restated:

> Async `.vox` loading works on web (geometry + voxel types correct);
> per-voxel materials render as near-black instead of the colorful Oasis
> aesthetic the native build produces from the same fixture. Native renders
> correctly; web does not.

This file is the binding hand-off into the implementer phase. The
implementer reads `## Implementation plan` numbered checklist top-to-bottom
and `## Decisions & rejected alternatives` for the load-bearing context.

Every path below is absolute or worktree-relative to
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming/`.

---

## Decision matrix

The three candidates verbatim from `01-context.md` Decision 3, scored on
seven axes. Where `02-research.md` `## What the architect needs` already
ranked applicability, this matrix extends the analysis with implementation
cost / risk axes; the research's ruling is the starting point, not the
final word.

| axis | C1: Changed<T> re-buildable extract | C2: cache-invalidate at install site | C3: suppress default scene |
|---|---|---|---|
| **correctness for current symptom** | yes — re-fires extract+prepare on `Changed<VoxelTypes>`. | yes — removes `WorldGpu` before install completes. | yes — default-scene install never lands, so build-once gate fires once with the .vox palette. |
| **correctness for hypothetical "live re-import / world reload"** | yes (general). | yes (general). | NO — the build-once gate stays closed after the first install; future reload would re-encounter the same bug. The `extract.rs:60-66` docstring explicitly calls this out as unfixed. |
| **steady-state performance** | one extra is_changed read per frame across two systems. Negligible. | identical to today after install — only fires at install boundaries. Negligible. | identical to today (no new code in the hot path). |
| **worst-case re-fire frequency** | once per `Changed<VoxelTypes>` flip, which today is "once per .vox install" (≤2 per app lifetime: default-scene + .vox). | same — once per install boundary. | n/a (no re-fire mechanism added). |
| **idiom fit (Bevy/render-world conventions)** | strongest — Bevy's `Changed<T>` is the canonical change-trigger; `Extract<Res<T>>::is_changed()` is supported. | weaker — `commands.remove_resource::<WorldGpu>()` is grep-zero across the codebase today; pattern would need to also work from the main world (where install runs), but `WorldGpu` is a render-world resource. **The brief's literal phrasing of C2 conflates worlds** — `commands` in `install_imported_vox` (main-world Update) cannot remove a render-world `WorldGpu`. C2 as written is mechanically wrong; the executable version is "main-world install inserts a marker, render-world system reads the marker, removes WorldGpu" — which collapses back into C1's shape with extra steps. | smallest idiom delta — adds a marker resource + a short-circuit; reuses `Startup` ordering. |
| **residual architectural risk** | moderate — naive "rebuild WorldGpu wholesale" cascades: W5 chain's `gpu_producer_has_run` is sticky, so a fresh WorldGpu would have zero geometry. **Mitigated by the focused-refresh shape**: keep WorldGpu, refresh only `voxel_types` buffer + rebuild `WorldGpu.bind_group` + invalidate `FrameGpu.calc_new_taa_sample_bind_group`. See research's `## What the architect needs` last bullet on palette length asymmetry. | same cascade risk as C1 — moot since C2 reduces to C1. | LEAVES the build-once gap intact for a hypothetical future "live re-import / world reload" feature. The `extract.rs:64-66` docstring already acknowledges this; C3 does not close it. |
| **implementation surface** | 4 source files: `render/extract.rs` (modify `stage_world_gpu_buildonce`), `render/prepare.rs` (modify `prepare_world_gpu` signature + add refresh branch), `render/mod.rs` (no system-graph change — the modified systems stay where they are), `voxel/grid.rs` (debug! demote only). ≈ 80–110 net lines added (refresh branch + the `VoxelTypesRefresh` resource definition). | larger surface — would need a main-world marker + a render-world removal system + ConstructionGpu state reset (gpu_producer_has_run flip-back) so the W5 chain re-runs against the fresh buffers + bind-group cascade. ≈ 150+ net lines. | smaller diff (≈ 40 lines) BUT requires fixing the fetch-failure fallback — currently `web_vox.rs:318-322` comments rely on the default scene already being live "underneath the overlay" when the HTTP fetch fails; suppressing it means the failure handler must install the default scene explicitly via a new Update system (`setup_test_grid` is `Startup` and cannot rerun). Net ≈ 60–80 lines once the fallback hole is covered. |
| **rollback ease** | trivial — single `Changed<>` predicate to revert, then `prepare_world_gpu` reverts to single-branch. Diff is contiguous. | trivial. | trivial — drop the marker check in `setup_test_grid`. |
| **fits forbidden-moves list** | yes — no `#[cfg(target_arch = "wasm32")]` in render path (the refresh trigger is `Changed<VoxelTypes>`, target-agnostic). | yes (same reasoning). | yes for the suppression part; **but the failure-fallback Update system risks adding web-specific scheduling** if not carefully shared with native (forbidden move 1 borderline). |

### Notes on the matrix

- **C2 reduces to C1.** Per the brief, C2 is "schedule `commands.remove_resource::<WorldGpu>()` in `install_imported_vox`". `install_imported_vox` runs in the main-world Update schedule; `WorldGpu` is a render-world resource. Main-world `Commands::remove_resource::<WorldGpu>` is a no-op (the type isn't a main-world resource). Executable C2 requires a main-world → render-world signaling step, at which point it becomes C1 with a custom signaling resource rather than `Changed<VoxelTypes>`. The Bevy-idiomatic signal IS `Changed<T>`. Recommendation collapses C1 + C2 into a single candidate.
- **C3 is leaky on the fetch-failure path.** `web_vox.rs:284-338` `startup_fetch_default_vox` relies on the comment at `:318-322`: *"The default embedded scene installed by `voxel::grid::setup_test_grid` at Startup is already live underneath this overlay"*. C3 invalidates that assumption — the user would see pure sky on fetch failure unless a separate "install default scene" Update system handles the fallback.
- **The 13 → 257 palette-length asymmetry** (research `## What the architect needs` last bullet) is naturally handled by `GrowableBuffer<GpuVoxelType>::upload_all`: it calls `reserve_discard` which grows the buffer if needed (`world/buffer.rs:198-201`). All three candidates use `upload_all`, not `append`; the matrix's "correctness for current symptom" reflects this.

---

## Recommendation

**Adopt Candidate 1 (Changed<T> re-buildable extract path)**, implemented
in the **focused-refresh shape** — NOT the naive "rebuild WorldGpu
wholesale" shape. Specifically:

1. Modify `stage_world_gpu_buildonce` (`crates/bevy_naadf/src/render/extract.rs:191-227`) so its gate-pass behaviour widens to: when `WorldGpu` already exists AND `Extract<Res<VoxelTypes>>::is_changed()` is true, emit a new transient `VoxelTypesRefresh { types: Vec<VoxelType> }` resource carrying the new palette. The pre-existing build-once path (when `WorldGpu` is `None`) is preserved untouched.
2. Modify `prepare_world_gpu` (`crates/bevy_naadf/src/render/prepare.rs:184-604`) to take `Option<ResMut<WorldGpu>>` instead of `Option<Res<WorldGpu>>`, and add a `Option<Res<VoxelTypesRefresh>>` parameter. When `WorldGpu` is `Some` AND `VoxelTypesRefresh` is `Some`, take the **focused-refresh** path: re-pack the new palette to `Vec<GpuVoxelType>`, call `world_gpu.voxel_types.upload_all(...)` (which reallocates if needed via `world/buffer.rs:185-201`), rebuild `WorldGpu.bind_group` using the (possibly new) `voxel_types.buffer()` plus the unchanged `chunks_buffer / blocks.buffer() / voxels.buffer() / world_meta / placeholder_*` handles, then remove `FrameGpu` so `prepare_frame_gpu` re-creates `calc_new_taa_sample_bind_group` (which also binds `voxel_types`, `prepare.rs:921`).
3. Remove `VoxelTypesRefresh` at the end of the refresh branch (single-use staging, mirrors `WorldGpuStaging`'s drop semantic).

### Rationale grounded in matrix + research

- **Closes the underlying architectural gap** — the `extract.rs:60-66`
  docstring's "future feature ever needs a whole-world re-upload" caveat
  is fixed by this design. Any future live-reload / drag-drop-after-startup
  / runtime palette swap (currently grep-zero use cases, but the path is
  now safe) re-uses the same `Changed<VoxelTypes>` plumbing.
- **C3's user-symptom-only scope is insufficient** for the fetch-failure
  case (web_vox.rs:318-322 explicitly depends on the default scene already
  being live). Fixing C3 cleanly also requires Update-time fallback
  installation, which negates its supposed simplicity advantage and
  increases the risk of native/web schedule drift (borderline forbidden
  move 1).
- **Preserves W5-chain geometry buffers.** The research's
  `## Decisions & rejected alternatives` rules out Q3 readback as the
  cause and confirms the W5 chain writes `chunks_buffer / blocks /
  voxels` on the existing WorldGpu post-install. The focused-refresh
  shape KEEPS those buffer allocations; we never call `commands.insert_resource(WorldGpu { ... })` on the refresh
  path, so `prepare_construction`'s side state
  (`gpu_producer_has_run` flag, `model_data_*_buffer` allocations,
  the W5 cursor-seeded buffers) is untouched.
- **Bevy 0.19 `Commands::insert_resource`-over-existing IS confirmed to
  flip `Changed<R>`.** Verified by reading `bevy_ecs-0.19.0-rc.1/src/world/mod.rs:1908-1927`
  → `insert_resource_by_id:2965-2985` → `insert_by_id_with_caller:1126-1159`,
  which goes through `insert_dynamic_bundle` with `InsertMode::Replace`
  and updates change ticks via `world.change_tick()` at line 1135. The
  pattern is sound; no caveat from Bevy that the design needs to compensate
  for.
- **`Extract<Res<T>>::is_changed()` is reliable.** The `Extract<P>`
  SystemParam wraps an inner SystemState that tracks `last_run` against
  the main world's change ticks (`bevy_render-0.19.0-rc.1/src/extract_param.rs:75-118`,
  `bevy_ecs-0.19.0-rc.1/src/system/function_system.rs:441-466`); inner
  `Res<T>::is_changed()` compares correctly against the resource's
  `added/changed_tick`.
- **Re-extracting `WorldGpuStaging` does NOT interact with the W5 chain.**
  The research's `## Decisions & rejected alternatives` "rejected
  Hypothesis 5" + audit row at `construction/mod.rs:1577-1743` /
  `:2009-2016` confirm: the W5 chunk / block / voxel allocation chain
  reads `model_data` (a separate render-world resource, `ModelDataRender`)
  and `world_gpu.chunks_size_in_chunks / chunks_buffer / blocks / voxels`,
  NOT `voxel_types`. Architect-verified at `mod.rs:1275, 1289, 1304, 1583,
  1719-1721, 1763-1769, 1860-1862, 1866-1868, 1904, 2292`. The focused
  refresh never touches the buffers the W5 chain depends on.
- **The matrix's "correctness for live re-import / world reload"
  axis is non-decorative.** The faithful-port rule
  (memory `bevy-naadf-faithful-port-rule.md`) says C# semantics are the
  baseline; C# NAADF does not support world reload, so we don't add the
  feature. **But the docstring at `extract.rs:64-66` already promised
  the build-once path would be re-runnable IF such a feature ever lands**
  — fixing the gap NOW means the promise is real, costs no extra surface
  area beyond what C1 already buys, and the alternative (C3) leaves a
  documented architectural lie in place.
- **`upload_all` is the right call, not `append`.** Per research's last
  bullet — the 13 → 257 palette is a wholesale replacement, not an
  extension. `GrowableBuffer::upload_all` (`world/buffer.rs:198-201`)
  calls `reserve_discard` which grows if needed and writes from offset 0.
  `append` would corrupt the buffer with a stale prefix.

The focused-refresh shape was the architect's adaptation, not in the
verbatim brief — it falls under the "Architect picks" mandate of
Decision 3.

---

## Implementation plan

Numbered checklist the implementer executes mechanically. Each step ends
with the verification action that proves it works. Steps 1–7 are the fix;
steps 8–11 are gate extension + log demote; step 12 is the
demonstrate-gate-extension-fails-pre-fix proof (mandatory per
`01-context.md` `## Verification surface` last paragraph + Decision 4).

### Fix steps

**Step 1 — Add `VoxelTypesRefresh` transient resource.**

File: `crates/bevy_naadf/src/render/extract.rs`. Add a new `#[derive(Resource, Default)]` struct alongside `WorldGpuStaging` (after line 87, before the `WorldDataMeta` definition):

```
/// Transient render-world hand-off carrying a refreshed palette
/// (`web-vox-color-divergence` fix, 2026-05-18). Emitted by
/// `stage_world_gpu_buildonce` when `Changed<VoxelTypes>` fires AFTER
/// `WorldGpu` is built (the async .vox install case + any future
/// runtime palette swap). Consumed and dropped by `prepare_world_gpu`'s
/// focused-refresh branch.
#[derive(Resource, Default)]
pub struct VoxelTypesRefresh {
    pub types: Vec<crate::voxel::VoxelType>,
}
```

Re-export in `crates/bevy_naadf/src/render/mod.rs` alongside `WorldGpuStaging` (line 44).

Verification: `cargo build --workspace` — the resource compiles and is reachable.

**Step 2 — Widen `stage_world_gpu_buildonce` gate to emit `VoxelTypesRefresh` on palette change.**

File: `crates/bevy_naadf/src/render/extract.rs:191-227`. Modify the function:

- Keep the existing build-once gate at `:201-203` exactly as is (the `WorldGpu is_some() OR staging_existing is_some()` short-circuit). When that gate fires, **before returning**, check: if `world_gpu_already_built.is_some()` AND `voxel_types.as_ref().is_some_and(|vt| vt.is_changed())` AND no `VoxelTypesRefresh` already pending, emit `commands.insert_resource(VoxelTypesRefresh { types: voxel_types.as_ref().unwrap().types.clone() })`. The `Extract<Option<Res<VoxelTypes>>>` system param exposes `.is_changed()` on the inner `Res<VoxelTypes>` once dereferenced.
- The "no `VoxelTypesRefresh` already pending" check: take an additional parameter `voxel_types_refresh_existing: Option<Res<VoxelTypesRefresh>>` and skip the insert when it `is_some()`. Mirrors how the existing gate guards against double-insertion of `WorldGpuStaging`.
- Add a one-line `debug!` log when the refresh is emitted, with prefix `[palette-refresh]`, for symmetry with the existing `[palette-install]` / `[palette-upload]` logs (which the implementer demotes in Step 9).

Verification: `cargo build --workspace` + the unit-level reasoning that the build-once path is unchanged when WorldGpu is None.

**Step 3 — Change `prepare_world_gpu` signature: `Res<WorldGpu>` → `Option<ResMut<WorldGpu>>`, add `Option<Res<VoxelTypesRefresh>>`.**

File: `crates/bevy_naadf/src/render/prepare.rs:184-604`. Modify the function signature:

```
pub fn prepare_world_gpu(
    mut commands: Commands,
    staging: Option<Res<WorldGpuStaging>>,
    mut existing: Option<ResMut<WorldGpu>>,             // ← was Option<Res<WorldGpu>>
    voxel_types_refresh: Option<Res<crate::render::extract::VoxelTypesRefresh>>, // ← new
    pipelines: Res<NaadfPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    construction_config: Option<Res<crate::render::construction::ConstructionConfig>>,
) {
```

Verification: `cargo build --workspace` — the system signature compiles.

**Step 4 — Replace the build-once short-circuit with a two-branch dispatcher.**

File: `crates/bevy_naadf/src/render/prepare.rs:201-204`. Replace the early-return:

```
if existing.is_some() {
    return;
}
```

with:

```
if let Some(world_gpu) = existing.as_mut() {
    // WorldGpu exists — check for the focused-refresh path before bailing.
    if let Some(refresh) = voxel_types_refresh.as_deref() {
        // … (Step 5 fills this in)
    }
    return;
}
```

This keeps the structural short-circuit (no fall-through to the build-once
build code) while opening the door for the refresh branch in Step 5.

Verification: `cargo build --workspace`.

**Step 5 — Implement the focused-refresh body.**

File: `crates/bevy_naadf/src/render/prepare.rs`, inside the `if let Some(refresh)` block from Step 4. The body:

1. Re-pack the new palette to `Vec<GpuVoxelType>` mirroring `:380-388`:
   ```
   let voxel_types_data: Vec<GpuVoxelType> = if refresh.types.is_empty() {
       vec![GpuVoxelType { data: [0; 4] }]
   } else {
       refresh.types.iter().map(GpuVoxelType::from_voxel_type).collect()
   };
   ```
2. Emit a `debug!` log with the `[palette-upload]` prefix (mirrors the existing log at `prepare.rs:501-507`) — for the refresh path label it `(refresh)` so it's distinguishable from the build-once upload.
3. Call `world_gpu.voxel_types.upload_all(&voxel_types_data, &render_device, &render_queue);` — this is the same call as the build-once path at `:508` and handles the 13 → 257 reallocation via `GrowableBuffer::reserve_discard` (`world/buffer.rs:185-201`).
4. Rebuild the world bind group using all of WorldGpu's existing buffer handles, plus the (possibly new) `voxel_types.buffer()`. Mirror the `BindGroupEntries::sequential((...))` block at `:573-586`. Assign to `world_gpu.bind_group = new_bind_group;`. Note: `ResMut<WorldGpu>` lets us mutate the field in place; no need to re-insert the resource.
5. Remove `FrameGpu` so `prepare_frame_gpu` re-runs its bind-group creation. `calc_new_taa_sample_bind_group` at `:914-925` binds `world_gpu.voxel_types.buffer()` and must pick up the new handle; `prepare_frame_gpu`'s `if existing.is_none()` path at `:863-925` rebuilds all five bind groups when FrameGpu is absent. Note: this also forces a per-pixel storage buffer rebuild (the `needs_new_storage` short-circuit at `:763-797`). Cost is one-shot per palette refresh (≤2 events per app lifetime), acceptable.
6. Remove `VoxelTypesRefresh` so the refresh branch is single-shot (`commands.remove_resource::<crate::render::extract::VoxelTypesRefresh>();`).

Estimated body size: ~40 lines.

Verification: `cargo build --workspace`.

**Step 6 — Verify the schedule-ordering invariants.**

File: `crates/bevy_naadf/src/render/mod.rs:148-160` (`ExtractSchedule` registration), `:177-187` (`PrepareResources` registration). No change required — the existing ordering is:
- `ExtractSchedule`: `stage_world_gpu_buildonce` runs before `prepare_world_gpu` (different schedules); `VoxelTypesRefresh` is inserted via Commands which flushes before PrepareResources.
- `PrepareResources`: `prepare_world_gpu` precedes nothing that would race the refresh.
- `prepare_frame_gpu` runs in `PrepareBindGroups` (after `PrepareResources`, `:188-190`), so the `commands.remove_resource::<FrameGpu>()` from Step 5 lands before `prepare_frame_gpu` runs the next time.

Verification: read the existing schedule registration; document the ordering as a `// SAFETY:` style comment near the new refresh-branch code.

**Step 7 — Confirm the W5 chain is unaffected.**

File: `crates/bevy_naadf/src/render/construction/mod.rs:1493-1497`. `prepare_construction` takes `Option<ResMut<crate::render::prepare::WorldGpu>>` — same `ResMut` mutability we're using in `prepare_world_gpu`. Bevy's scheduler treats these as conflicting access and serializes them within the same `PrepareResources` system set, so there is no concurrent-modification hazard. No code change required; this is a verification step.

`prepare_construction` reads `world_gpu.chunks_buffer / blocks / voxels / chunks_size_in_chunks` (verified at `:1275, :1289, :1304, :1583-1585, :1719-1721, :1763-1765, :1860-1862, :1904, :2292`); it does NOT read `world_gpu.voxel_types`. The construction bind groups (`construction_world`, `construction_bounds_world`, …) bind `chunks_buffer / blocks / voxels` but not `voxel_types`. The focused refresh's bind-group rebuild only updates `WorldGpu.bind_group` (the @group(0) world bind group); the construction bind groups stored in `ConstructionBindGroups` are untouched. The W5 chain's `gpu_producer_has_run` flag and the W5 GPU buffers (`hash_map`, `segment_voxel_buffer_w5`, `model_data_*_buffer`) are on `ConstructionGpu`, NOT `WorldGpu` — unaffected.

Verification: read the construction module; confirm the listed bind-group bindings via grep. No source change.

### Gate extension steps (Decision 4 — must land in this orchestration)

**Step 8 — Add `region_channel_max` helper to `Framebuffer`.**

File: `crates/bevy_naadf/src/e2e/framebuffer.rs:237-258` — alongside `region_mean`. Add:

```
/// Maximum-of-channel-means over `rect`. Returns the largest of
/// (mean_R, mean_G, mean_B), each in `0.0..=255.0`. Useful for
/// "the frame has at least one colored channel above floor X" gates
/// where Rec.709 luminance is too lossy (a green-only frame has lum=180
/// but R+B near zero; a colorless dark-blue-gray frame has lum=10).
///
/// Added by `web-vox-color-divergence` (2026-05-18) Decision 4 to catch
/// the near-black-but-structurally-correct regression class the
/// luminance-only gate at `vox_e2e.rs:402-433` and the SSIM-only gate
/// at `vox_web_parity.rs:117-190` are blind to.
pub fn region_channel_max(&self, rect: Rect) -> f32 {
    let m = self.region_mean(rect);
    m[0].max(m[1]).max(m[2])
}
```

Verification: `cargo test --workspace --lib` — no test exercises this directly yet, but the build must succeed.

**Step 9 — Promote `assert_vox_geometry_visible` from luminance-only to per-channel.**

File: `crates/bevy_naadf/src/e2e/vox_e2e.rs:402-433`. Add a new constant alongside `SKY_LUMINANCE_CEILING` at `:126`:

```
/// Per-channel mean-max floor for the central screen region. Calibrated
/// against the synthesised emissive fixture (which renders fully-saturated
/// colors) and the near-black regression class (channel max ≈ 8/255). A
/// non-skybox capture with any meaningful color must exceed this floor;
/// the near-black regression sits well below it.
///
/// **Rationale for 30.0:** the native reference capture
/// `target/e2e-screenshots/vox_web_parity_loaded.png` (sandy beige + green
/// + dark roof tiles, per `01-context.md` "Visual evidence") has measured
/// R/G/B channel means well above 60 in the central region. 30.0 is
/// half the calibrated reference's lowest channel, leaving 2× headroom
/// against natural framebuffer noise. The pre-fix near-black render
/// reports channel max ≈ 8 (from `02-research.md` web log readout's
/// implication that absorption * Vec3::ZERO produces near-zero output),
/// which is comfortably below the floor.
const VOX_GEOMETRY_CHANNEL_MAX_FLOOR: f32 = 30.0;
```

Modify `assert_vox_geometry_visible` body — after the existing luminance check (lines 416-431), add:

```
let channel_max = fb.region_channel_max(region);
println!(
    "e2e_render --vox-e2e: vox_geometry channel max (max of mean_R / G / B) = {:.1} \
     (threshold > {:.0} — non-skybox + meaningful color)",
    channel_max, VOX_GEOMETRY_CHANNEL_MAX_FLOOR,
);
if channel_max <= VOX_GEOMETRY_CHANNEL_MAX_FLOOR {
    return Err(format!(
        "vox-e2e gate FAIL — central screen region channel-max {:.1} is at or \
         below the per-channel floor ({:.0}). The render produced structurally \
         present geometry but colorless / near-black voxels. Likely cause: a \
         palette-upload regression on the build-once GPU resource path \
         (web-vox-color-divergence class — see \
         docs/orchestrate/web-vox-color-divergence/). Mean rgba = {mean:?}; \
         region pixel rect = ({}, {}, {}, {}).",
        channel_max, VOX_GEOMETRY_CHANNEL_MAX_FLOOR,
        region.x0, region.y0, region.x1, region.y1,
    ));
}
Ok(())
```

(The existing `Ok(())` at end is replaced by the conditional above.)

Verification: `timeout 120s cargo run --bin e2e_render -- --vox-e2e` must PASS post-fix (color floor cleared) and the demonstrate-fails proof in Step 12 must show this gate FAILing on the pre-fix state.

**Step 10 — Add per-channel assertion to `vox_web_parity` loaded capture.**

File: `crates/bevy_naadf/src/e2e/vox_web_parity.rs`. The current `run_vox_web_parity_compare` (`:199-339`) reads the loaded PNG into `loaded_fb` at `:296`. Insert the per-channel assertion BEFORE the SSIM compare at `:307` (so both run and the most diagnostic error fires first):

```
// web-vox-color-divergence Decision 4 (2026-05-18): the SSIM-only compare
// at `:307` is color-blind by construction — a structurally-correct but
// all-near-black render still scores SSIM ≈ 0 vs the skybox baseline
// (different structure regardless of color). Add a per-channel spread
// assertion on the loaded frame itself.
let central = Rect::from_fractional(&loaded_fb, 0.30, 0.30, 0.70, 0.70);
let loaded_channel_max = loaded_fb.region_channel_max(central);
println!(
    "e2e_render --vox-web-parity: loaded frame central rect channel max = {:.1} \
     (threshold > {:.0} — meaningful per-voxel color)",
    loaded_channel_max, VOX_WEB_PARITY_CHANNEL_MAX_FLOOR,
);
if loaded_channel_max <= VOX_WEB_PARITY_CHANNEL_MAX_FLOOR {
    eprintln!(
        "e2e_render --vox-web-parity: FAIL — loaded frame channel max {:.1} <= floor {:.0}. \
         The .vox install path rendered structurally correct geometry but colorless / \
         near-black voxels (web-vox-color-divergence class).",
        loaded_channel_max, VOX_WEB_PARITY_CHANNEL_MAX_FLOOR,
    );
    return 1;
}
```

Plus the threshold constant near `VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX` at `:117`:

```
/// Per-channel mean-max floor on the central rect of the
/// `vox_web_parity_loaded.png` capture. See `vox_e2e.rs::VOX_GEOMETRY_CHANNEL_MAX_FLOOR`
/// for the rationale; same calibration applies. 30.0 leaves 2× headroom
/// above natural noise and well below the colorful Oasis reference's
/// measured ~60+ R/G/B means.
pub const VOX_WEB_PARITY_CHANNEL_MAX_FLOOR: f32 = 30.0;
```

Import `Rect` if not already in scope at the top of the file.

Verification: `timeout 120s cargo run --bin e2e_render -- --vox-web-parity` must PASS post-fix; Step 12 demonstrates the failure pre-fix.

**Step 11 — Demote diagnostic logs from `info!` to `debug!`.**

Per Decision 2 + forbidden move 11, the four `info!` blocks added by `02-research.md` Stage A become `debug!`:

- `crates/bevy_naadf/src/render/prepare.rs:501-506` (`[palette-upload]`). Change `info!(` → `debug!(`.
- `crates/bevy_naadf/src/voxel/grid.rs:220-226` (`[palette-install] install_empty_world`). Change `info!(` → `debug!(`.
- `crates/bevy_naadf/src/voxel/grid.rs:346-352` (`[palette-install] install_default_embedded_in_fixed_world`). Change `info!(` → `debug!(`.
- `crates/bevy_naadf/src/voxel/grid.rs:637-643` (`[palette-install] install_imported_vox`). Change `info!(` → `debug!(`.

The new `[palette-upload] (refresh)` log added by Step 5 uses `debug!` from the start.

**The new `[palette-refresh]` log added in Step 2** uses `debug!` from the start.

**Playwright forwarder verdict: KEEP.** The handlers at `e2e/tests/vox-loading.spec.ts:155-171` and `:206-220` forward `[palette-upload]` / `[palette-install]` console messages from the wasm bridge to Node-side `console.log`. Pairing the kept forwarder with the `debug!`-demoted Rust side means a future regression dispatch can run `RUST_LOG=bevy_naadf=debug just test-wasm 2>&1 | tee` and immediately see the same `[palette-*]` trace the research phase used, without re-instrumenting. Decision: leave forwarder in place; no change at `vox-loading.spec.ts`.

Verification: `cargo test --workspace --lib` — no tests assert on these log lines; `just test-wasm` + the captured Playwright output should be quieter by default but reproducible on demand.

### Pre-merge "demonstrate gate fails on pre-fix" step

**Step 12 — Demonstrate the extended gates FAIL on the pre-fix state.**

This is the critical pre-merge step from `01-context.md` `## Verification surface` last paragraph + Decision 4. Without it, the gate extensions could be no-ops that pass either way.

Procedure (record both runs in `04-impl.md`):

1. With all changes from Steps 1–11 committed, **stash only the fix part of the diff** keeping the gate extensions live. Concretely: `git stash push -p` and select hunks corresponding to Steps 1–7 (the source/extract/prepare changes), leaving Steps 8–11 (gate extensions + log demote) in the working tree. Alternatively, comment out the new refresh branch in `prepare.rs` so the build-once gate behaves as pre-fix.
2. Run `timeout 120s cargo run --bin e2e_render -- --vox-web-parity` — expect FAIL on the channel-max assertion (it tests `vox_web_parity_loaded.png` which on the pre-fix native code is colorful enough to pass… HOLD: native gate ALREADY passes today because native install is synchronous). 
   - **Important caveat:** the channel-max assertion on the native `--vox-web-parity-loaded` capture CANNOT exhibit the pre-fix failure because native doesn't hit the build-once bug. The native gate's role is to lock in a healthy floor; demonstrating gate-fails-on-pre-fix requires the **web** test path.
3. Run `timeout 300s just test-wasm` to exercise the Playwright `vox-loading.spec.ts` test. The current SSIM-only failure (loaded ≈ skybox per `02-research.md` web log) is the symptom; the test already fails today. The implementer should ALSO add a per-channel canvas-mean assertion to the Playwright spec to mirror the native gate (out-of-scope inline; defer to a follow-up TODO in `04-impl.md` if not added in the same dispatch).
4. Un-stash / un-comment the fix. Run `timeout 120s cargo run --bin e2e_render -- --vox-web-parity` again — expect PASS (channel-max ≥ 30 satisfied on the loaded native capture; SSIM dissimilarity ≥ 0.85 satisfied).
5. Run `timeout 300s just test-wasm` again — expect PASS (the wasm-side regression is fixed).
6. Record both runs in `04-impl.md`: pre-fix gate output + post-fix gate output, side by side.

The pre-fix demonstration on the **native gate** (Step 12.2) is necessarily weaker than the wasm one because the bug is web-only. The architect's verdict: this is acceptable — the wasm-side test_wasm IS the load-bearing demonstration; the native gate extension serves as a fence against any future regression that might cross-pollinate (e.g. someone refactoring `install_imported_vox` and breaking native too).

---

## Verification plan

Per memory `feedback-e2e-gates-must-fail-fast.md`: all `cargo run` commands wrapped in `timeout`.

The sequence the implementer runs after Steps 1–11 land, in order:

1. `cargo build --workspace`
   - **Expected:** green. Type-checks the new `VoxelTypesRefresh` resource + modified system signatures + Framebuffer helper.
   - **On failure:** the implementer fixes types / imports / function signatures before proceeding.

2. `cargo build --target wasm32-unknown-unknown --bin bevy-naadf --no-default-features --features webgpu`
   - **Expected:** green. The target-conditional code in the render path is unchanged (forbidden move 1 honoured); wasm build remains as-is.
   - **On failure:** inspect for accidentally pulled-in `std::sync` / threading APIs in the refresh branch — none should be needed.

3. `cargo test --workspace --lib`
   - **Expected:** all 184 tests green (the unit suite at the last orchestration checkpoint). No new unit tests are required — the refresh path is integration-tested by the e2e gates.
   - **On failure:** investigate; no pre-existing failures should be tolerated (project memory: "ALWAYS investigate test failures — no such thing as pre-existing failures").

4. `timeout 120s cargo run --bin e2e_render -- --vox-web-parity`
   - **Expected:** PASS on both the existing SSIM check AND the new per-channel assertion (Step 10). Output shows `loaded frame central rect channel max = <60ish> (threshold > 30 …)`.
   - **On failure on the per-channel assertion:** the native reference capture is darker than the architect's 30.0 calibration assumed — re-baseline by reading actual `region_channel_max` from `target/e2e-screenshots/vox_web_parity_loaded.png` and adjusting `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR` down to (measured − 10) so 2× headroom is preserved. Document the actual measurement.
   - **On failure on SSIM:** unrelated regression; investigate.

5. `timeout 120s cargo run --bin e2e_render -- --vox-e2e`
   - **Expected:** PASS on the existing luminance check AND the new per-channel max (Step 9). Output shows `channel max (max of mean_R/G/B) = <high>`.
   - **On failure on per-channel:** the synthesised emissive fixture's central rect is darker than expected — measure + adjust `VOX_GEOMETRY_CHANNEL_MAX_FLOOR` analogously.

6. `timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual`
   - **Expected:** PASS. This gate exercises the edit-mode brush which mutates `ResMut<WorldData>` (`editor/mod.rs:140`). The fix's `Changed<VoxelTypes>` predicate does NOT fire on `WorldData` mutations, so the brush-driven flow is unaffected.
   - **On failure:** check that the refresh branch isn't somehow being triggered by brush strokes; trace `[palette-refresh]` log (RUST_LOG=bevy_naadf=debug to expose it).

7. `timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle`
   - **Expected:** PASS. The CPU oracle phase routes through `install_vox_sized_to_model` which inserts VoxelTypes at Startup — pre-`WorldGpu` build-once frame. The build-once path fires once with the correct palette; the refresh path stays dormant.
   - **On failure:** suspect a subtle ordering issue in the modified extract gate; verify `VoxelTypesRefresh` is not emitted on the Startup pass.

8. `timeout 300s just test-wasm`
   - **Expected:** PASS, all `e2e/tests/vox-loading.spec.ts` cases including the SSIM-dissimilarity case (loaded canvas must look colorful, not near-black). The `[palette-refresh]` console message (via the Playwright forwarder kept in Step 11) confirms the refresh path fires on the web async timeline.
   - **On failure on SSIM:** the fix didn't land — the loaded canvas still resembles the skybox baseline. Re-trace the wasm console output for the `[palette-refresh]` line; if absent, the `Changed<VoxelTypes>` predicate isn't firing (verify `commands.insert_resource(VoxelTypes { ... })` over an existing resource does flip `Changed` in Bevy 0.19 — the source-read in `## Decisions & rejected alternatives` confirms it should).

9. **Demonstrate gate fails on pre-fix (Step 12 from Implementation plan).** Stash the fix; re-run the wasm test; confirm FAIL. Un-stash; re-run; confirm PASS. Record both transcripts in `04-impl.md`.

---

## Risk register

1. **Risk:** `Commands::insert_resource`-over-existing does not trip `Changed<R>` for the render-world's `Extract<Res<T>>::is_changed()` query in Bevy 0.19.
   - **Mitigation:** verified via source read in `## Decisions & rejected alternatives`. The verification chain: `bevy_ecs-0.19.0-rc.1/src/world/mod.rs:1908` → `insert_resource_with_caller` → `insert_resource_by_id:2965-2985` → `insert_by_id_with_caller:1126-1159` → `insert_dynamic_bundle` with `InsertMode::Replace` and `world.change_tick()` at line 1135. Replace mode updates change ticks.
   - **Contingency if wrong:** swap to an explicit cache-invalidate at the install site — add a new main-world marker resource `PaletteChanged` inserted next to the `VoxelTypes` insert at `grid.rs:645`, drained by `stage_world_gpu_buildonce` via the existing `Extract<Option<Res<PaletteChanged>>>` pattern. Cost: ~20 extra lines, same semantics.

2. **Risk:** `world_gpu.voxel_types.upload_all(...)` reallocation invalidates an interleaved consumer that the architect missed.
   - **Mitigation:** the consumer audit in Step 7 enumerates every bind group that references `world_gpu.voxel_types.buffer()`: only two — `WorldGpu.bind_group` (rebuilt in Step 5) and `FrameGpu.calc_new_taa_sample_bind_group` (rebuilt via the FrameGpu removal in Step 5). The construction-side bind groups bind `chunks_buffer/blocks/voxels` but NOT `voxel_types` (verified via `grep "world_gpu\." crates/bevy_naadf/src/render/construction/mod.rs`).
   - **Contingency if a third consumer surfaces:** add its bind group to the rebuild list. The pattern is contiguous.

3. **Risk:** the Q3 cross-frame readback state machine (`construction/mod.rs:1028-1325`) gets confused by `WorldGpu`'s bind_group field swapping mid-flight.
   - **Mitigation:** Q3 readback reads from GPU buffers via wgpu `map_async` against the buffer handles `populate_cpu_mirror_from_gpu_producer` cached when it started. It does NOT re-read `world_gpu.bind_group`. The buffer handles for chunks/blocks/voxels are unchanged (only `voxel_types` reallocates, which Q3 doesn't read).
   - **Contingency:** none needed — the readback path is structurally independent.

4. **Risk:** `GrowableBuffer<GpuVoxelType>::upload_all` semantics for 13 → 257 length expansion.
   - **Mitigation:** verified via `crates/bevy_naadf/src/world/buffer.rs:185-201` — `reserve_discard(new_len)` grows + discards old contents; `write(0, data, queue)` then writes from offset 0. The full 257-entry palette is written.
   - **Contingency:** if `reserve_discard` has a bug at the 257-entry capacity boundary, fall back to `commands.insert_resource(WorldGpu { ..fresh.. })` — heavier cascade but unambiguous. Pre-condition: cover the ConstructionGpu state reset (set `gpu_producer_has_run = false`, clear `cpu_mirror_populated`) so the W5 chain re-runs. NOT recommended unless `reserve_discard` is provably wrong.

5. **Risk:** `commands.remove_resource::<FrameGpu>()` in Step 5 forces a per-pixel storage buffer rebuild (`first_hit_data` + `first_hit_absorption` + `final_color` plus TaaGpu's `taa_sample_accum`). The first frame after rebuild may show transient noise from zero-initialized accumulators.
   - **Mitigation:** acceptable — the rebuild happens once per palette refresh (≤2 events per app lifetime: default-scene + .vox install). TAA convergence reseeds within a few frames. The `--vox-web-parity-loaded` gate's `PARITY_WARMUP_FRAMES` already absorbs this.
   - **Contingency:** if the visual flicker is objectionable, target the rebuild more narrowly — only rebuild `calc_new_taa_sample_bind_group` and `bind_group` (the two FrameGpu bind groups referencing voxel_types), via a new `RebuildOnPaletteChange` flag on FrameGpu and a focused per-bind-group rebuild in `prepare_frame_gpu`. ~30 extra lines. Defer to a follow-up if observed.

6. **Risk:** the editor's `ResMut<WorldData>` brush at `editor/mod.rs:140` somehow flips `Changed<VoxelTypes>` indirectly, causing refresh per brush stroke.
   - **Mitigation:** `WorldData` and `VoxelTypes` are SEPARATE resource types (`world/data.rs`). `ResMut<WorldData>` only flips `Changed<WorldData>`; nothing in the editor path inserts or modifies `VoxelTypes`. Verified via `grep -rn "ResMut<VoxelTypes>" crates/bevy_naadf/src/` — zero matches.
   - **Contingency:** none needed.

7. **Risk:** the `VoxelTypesRefresh.types.clone()` palette copy in `stage_world_gpu_buildonce` is large (257 × 32 bytes ≈ 8 KiB). Allocation pressure in `ExtractSchedule`.
   - **Mitigation:** 8 KiB is negligible; the existing `WorldGpuStaging` clone is ~48 MiB. The clone fires once per `Changed<VoxelTypes>` event.
   - **Contingency:** none.

8. **Risk:** `--vox-gpu-construction` gate (mentioned in `01-context.md` "Verification surface" but not in the user's verification plan as a step) regresses because its bind-group setup interacts with the focused refresh.
   - **Mitigation:** the `--vox-gpu-construction` gate routes through the Startup install path (`GridPreset::Vox { path }` with `vox_gpu_oracle_cpu_phase=false`), which inserts VoxelTypes at Startup. The build-once path fires on frame 1 with the correct palette; `Changed<VoxelTypes>` doesn't re-fire because no subsequent insert happens. The refresh path stays dormant.
   - **Contingency:** if regression observed, capture the `[palette-refresh]` debug log and confirm absence.

9. **Risk:** the Playwright forwarder at `vox-loading.spec.ts:155-171` and `:206-220`, kept per Step 11's decision, fails silently when the wasm `debug!` is not enabled (the default `RUST_LOG` for the test harness).
   - **Mitigation:** the forwarder only forwards messages whose text matches `[palette-*]`. If the wasm side is at INFO level, no `[palette-*]` lines emit and the forwarder is a no-op — does not break the test. The forwarder is INSURANCE for future regression diagnosis (`RUST_LOG=bevy_naadf=debug just test-wasm`), not an active check.
   - **Contingency:** none.

10. **Risk:** the `Changed<VoxelTypes>` predicate fires the first time `install_default_embedded_in_fixed_world` inserts `VoxelTypes` at Startup. On that frame, the refresh path's gate (`WorldGpu.is_some()`) is false, so the build-once path takes over. The implementation must ensure refresh is NOT emitted in that case.
    - **Mitigation:** Step 2's gate explicitly tests `world_gpu_already_built.is_some() AND voxel_types.is_changed()`. The first-frame default insert satisfies the second but not the first; no refresh emitted.
    - **Contingency:** none — gate logic is explicit.

11. **Risk:** the per-channel floor 30.0 in Step 9/10 is too low and false-passes a subtle regression where most channels are dim but one channel barely clears 30.
    - **Mitigation:** the failure mode for this bug is ALL channels near zero (Vec3::ZERO multiply). A single-channel-only failure would require a different bug (e.g. a swizzle regression), which is orthogonal.
    - **Contingency:** future per-axis assertions could be added; out of scope here.

---

## Decisions & rejected alternatives

Stable named decisions the implementer reads BEFORE applying any source edit. Each decision names what was chosen, what was rejected, why, and the fact-that-would-flip-the-call.

- **Decision D-CHOOSE-CANDIDATE — Picked Candidate 1 (Changed<T> re-buildable extract) in focused-refresh shape.**
  - Rejected Candidate 2 (cache-invalidate at install site) because the brief's literal form is mechanically wrong: `install_imported_vox` runs in the main world and cannot `commands.remove_resource::<WorldGpu>()` for a render-world resource. The executable version of C2 collapses into C1 with a custom signaling resource instead of `Changed<T>`; the Bevy-idiomatic signal IS `Changed<T>`, so the collapsed C2 is strictly inferior.
  - Rejected Candidate 3 (suppress default scene during pending .vox) because (a) it leaves the build-once gap explicitly acknowledged at `extract.rs:64-66` UNFIXED for hypothetical future live-reload, and (b) the fetch-failure fallback at `web_vox.rs:318-322` relies on the default scene already being live — C3 invalidates that and forces a separate Update-time fallback system. C3's net diff is similar to C1's once the fallback hole is patched, with worse architectural payoff.
  - **Flip condition:** if Bevy 0.19's `insert_resource`-over-existing turns out NOT to flip `Changed<R>` for the next `Extract<Res<T>>::is_changed()` query — the source read disproves this, but a runtime probe is the final word. If proven wrong: shift to "main-world marker resource" variant (Risk 1 contingency).

- **Decision D-FOCUSED-REFRESH — Picked focused refresh (preserve geometry buffers, swap palette only) over full WorldGpu rebuild.**
  - Rejected full WorldGpu recreation because the W5 GPU producer chain's `ConstructionGpu.gpu_producer_has_run` flag is sticky; a fresh WorldGpu would have empty chunks/blocks/voxels buffers and the producer would not re-fire, rendering sky-only. Resetting `gpu_producer_has_run = false` is possible but requires cross-module coupling that the focused refresh avoids.
  - **Flip condition:** if a future feature requires resizing `chunks_size_in_chunks` mid-app, focused refresh is insufficient and full rebuild + `gpu_producer_has_run` reset is needed. Not the current case.

- **Decision D-FRAMEGPU-INVALIDATE — Remove FrameGpu wholesale, accept one-shot pixel-buffer rebuild, vs. narrowly rebuild only the calc_new_taa_sample_bind_group.**
  - Picked wholesale removal because it's a 1-line `commands.remove_resource::<FrameGpu>()` versus introducing a new dirty-flag mechanism on FrameGpu. The cost (TAA accumulator reseed for a few frames) fires at most twice per app lifetime.
  - **Flip condition:** if Playwright SSIM compare becomes flaky due to TAA-reseed transients, narrow the rebuild (Risk 5 contingency). Cost: ~30 extra lines.

- **Decision D-PALETTE-FLOOR-30 — Picked channel-max floor of 30.0 (on 0..255 scale).**
  - Rejected 20.0 (`01-context.md` Decision 4's tentative suggestion) for 2× headroom over noise; rejected 60.0+ for less margin against natural framebuffer variance. 30.0 sits at ~half the calibrated reference's lowest channel.
  - **Flip condition:** if the native `vox_web_parity_loaded.png` reference's measured channel max is below 60 (i.e. the test camera angle produces a dimmer view than estimated), re-baseline to (measured − 10) per Verification plan step 4.

- **Decision D-KEEP-PLAYWRIGHT-FORWARDER — Kept the Playwright `[palette-*]` console forwarder.**
  - Rejected removal because the forwarder is dormant at default `RUST_LOG` and only active when the user opts into `RUST_LOG=bevy_naadf=debug just test-wasm 2>&1 | tee`. Future regression diagnosis directly reuses the research-phase tooling without re-instrumenting.
  - **Flip condition:** if the forwarder's startup overhead is measured to add >100ms to Playwright init, remove it. Not currently a concern.

- **Decision D-NATIVE-PRE-FIX-DEMO-WEAK — Acknowledged that the "demonstrate gate fails on pre-fix" step has weaker coverage on the native `--vox-e2e` / `--vox-web-parity` gates than on the wasm test.**
  - Rejected adding a synthetic web-simulation gate to the native binary (would re-architect the harness for negligible benefit). The wasm-side test_wasm IS the load-bearing demonstration; the native gate extensions serve as a fence against future cross-target regression.
  - **Flip condition:** if a similar build-once-with-async-install pattern shows up in a hypothetical "native drag-drop after Startup" path, retrofit a delayed-install native gate. Out of scope here.

- **Decision D-NO-CHANGED-WORLDDATA — Do NOT trigger refresh on `Changed<WorldData>`.**
  - The editor brush at `editor/mod.rs:140` writes via `ResMut<WorldData>` every frame the user paints; triggering refresh on that would explode bind-group rebuilds. The W2 delta chain (`extract_world_changes` at `construction/mod.rs:846`) already handles WorldData mutations via `pending_edits.batches`. The W5 GPU producer chain handles `ModelData` mutations via its own sticky `gpu_producer_has_run` flag.
  - **Flip condition:** if a feature later requires re-sizing the world (changing `chunks_size_in_chunks`), `WorldData` changes will need to invalidate WorldGpu fully. New scope.

- **Decision D-LOGS-DEBUG-NOT-TRACE — Demoted instrumentation to `debug!`, NOT `trace!`.**
  - `01-context.md` forbidden move 11 leaves the choice between `debug!` and `trace!` to the architect. `debug!` keeps the messages reachable via standard `RUST_LOG=bevy_naadf=debug`; `trace!` would require `RUST_LOG=bevy_naadf=trace` which is louder and less standard for diagnostics. `debug!` is the canonical "off by default, on for diagnosis" level.

---

## Assumptions made

Explicit list of design-time assumptions and risk if wrong. Surfaced for the synthesis pause.

- **Assumption:** Bevy 0.19's `Commands::insert_resource` over an existing resource flips `Changed<R>` for the next `Extract<Res<R>>::is_changed()` query. Verified by source read in Decisions section; risk-if-wrong is mitigated by the explicit-marker fallback in Risk 1.

- **Assumption:** `Extract<Option<Res<T>>>` exposes `.is_changed()` on the inner `Res<T>`. Verified via `bevy_render-0.19.0-rc.1/src/extract_param.rs:50-118` (Extract is a thin SystemState wrapper) + `bevy_ecs-0.19.0-rc.1/src/system/function_system.rs:441-466` (SystemState tracks last_run and propagates change_tick to inner params). Risk-if-wrong: vanishingly small — the API is documented and stable.

- **Assumption:** `GrowableBuffer<GpuVoxelType>::upload_all` correctly reallocates and discards old contents for a 13 → 257 length transition. Verified at `crates/bevy_naadf/src/world/buffer.rs:185-201`. Risk-if-wrong: per Risk 4 contingency.

- **Assumption:** The construction-side bind groups (`construction_world`, `construction_bounds_world`, etc.) do NOT bind `world_gpu.voxel_types`. Verified by grep at `crates/bevy_naadf/src/render/construction/mod.rs` of `world_gpu\.` references — every match is `chunks_buffer / blocks / voxels / chunks_size_in_chunks`, none is `voxel_types`. Risk-if-wrong: per Risk 2 contingency.

- **Assumption:** `commands.remove_resource::<FrameGpu>()` from within `prepare_world_gpu` (PrepareResources) lands BEFORE `prepare_frame_gpu` (PrepareBindGroups) runs the next time, so FrameGpu is reliably re-created with the new voxel_types buffer. Bevy's command buffer flushes at system set boundaries; PrepareResources → PrepareBindGroups is a hard boundary. Verified by reading `crates/bevy_naadf/src/render/mod.rs:177-190`. Risk-if-wrong: vanishingly small.

- **Assumption:** The `voxel_types` palette length on the loaded-phase native reference capture `vox_web_parity_loaded.png` produces a central-rect channel-max above 60 (justifying the 30.0 floor with 2× headroom). Not measured directly; inferred from the prose description "sandy beige building walls, green palm trees, dark stone-tile roofs, wooden doors". Risk-if-wrong: gate threshold needs re-baselining (Verification plan step 4 contingency).

- **Assumption:** No `Changed<VoxelTypes>` consumer exists today (the audit confirms zero queries crate-wide). Adding the new extract-side `is_changed()` query does not race with any other consumer. Verified by `grep -rn "Changed<VoxelTypes>\|Changed<WorldData>" crates/bevy_naadf/src/` → zero matches.

- **Assumption:** The wasm Playwright `vox-loading.spec.ts` test, after the fix, reliably exercises the refresh path (the `[palette-refresh]` debug log fires once during the test run). Inference from `02-research.md`'s confirmed timeline: the test reproduces the async timing window that triggers `Changed<VoxelTypes>` after WorldGpu is built. Risk-if-wrong: the wasm test passes the SSIM compare but the refresh log doesn't fire — would indicate the `Changed<>` predicate didn't fire as expected; fall back to Risk 1's marker-resource variant.

- **Assumption:** The W5 GPU producer chain's `gpu_producer_has_run` flag stays true after the focused refresh (no need to re-fire the producer). Justified because the producer wrote chunks/blocks/voxels into WorldGpu's buffers BEFORE the refresh, and those buffers are preserved. Risk-if-wrong: would manifest as missing geometry post-refresh — but the user-observed bug confirms geometry was correct pre-fix on web (hovering reveals correct types), so the W5 chain demonstrably already populated the buffers; the refresh doesn't touch them.

- **Assumption:** `Rect::from_fractional` and `Framebuffer::region_mean` produce stable, repeatable measurements across runs at the e2e-fixed camera pose. Verified by the existing gate at `vox_e2e.rs:402-433` which has historically been non-flappy. Risk-if-wrong: framebuffer noise could push the channel-max below 30 sporadically; mitigation is to re-baseline downward or raise headroom.
