# 07 — Phase A-2 Implementation Log

## impl findings — Phase A-2 Batch 1 (steps 1–5) (2026-05-14)

Executed steps 1–5 of the `06-design-a2.md` §12 Phase-A-2 implementation
sequence, in order. Every step ends at a compiling state; steps 2, 4, 5 (the ▶
runnable steps) were smoke-run on the windowed GPU app. Toolchain: the pinned
`rust-toolchain.toml` stable channel, `mold` linker from `.cargo/config.toml`,
default-feature build (with `dlss` — the SDK env vars are set on this machine).
Test command is `cargo test --bin bevy-naadf` (binary-only crate, no `lib.rs`).
The NAADF C# + HLSL source at `/mnt/archive4/DEV/NAADF/NAADF/` is readable via
Read/Glob/Grep — `commonTaa.fxh` and `WorldRender.cs` were ported from the
actual files.

---

### Step 1 — GPU types + WGSL sample format

**Files edited:** `src/render/gpu_types.rs`.
**Files created:** `src/assets/shaders/taa_common.wgsl`.

- `gpu_types.rs`: added `GpuTaaParams` (§4.2 — `mat4 + mat4 + ivec3+pad +
  vec3+pad + 4×u32 + 4×u32` = 192 bytes) and `GpuCameraHistorySlot` (§4.3 —
  `mat4 + vec3+pad + vec2+vec2pad` = 96 bytes), both `#[repr(C)]` +
  `bytemuck::Pod`. Added the two compile-time size asserts (`== 192`, `== 96`).
  The module doc was extended to name the new structs + their WGSL counterpart
  location (`taa.wgsl`).
- `taa_common.wgsl` (NEW): a faithful WGSL port of `commonTaa.fxh` —
  `taa_compress_sample` / `taa_decompress_sample` (the 64-bit `vec2<u32>` sample
  format, §3), `taa_hash_from_data` (the `getHashFromData` hash), the
  `taa_neighbor_offsets[9]` constant, and `TAA_SAMPLE_RING_DEPTH = 16u` (the §6
  VRAM lever). Per §3.2's implementer note: WGSL has no implicit float→uint
  truncation, so the exponential colour compression does `u32(...)` explicitly
  and `min(255u, ...)` / `& 0xFFu` per channel; the decompressed `color.a` is
  always `1.0` (the load-bearing 0.25-spp per-sample weight). A `TaaSample`
  struct replaces the HLSL `out`-params of `decompressSample` (WGSL has no
  `out` params).

**Deviation (small, logged) — `taa_compress_sample` takes the distance as a
float, not pre-converted f16 bits.** The HLSL `compressSample(uint distComp,
...)` takes the f16 bits (its caller `renderFirstHit.fx:115` does the
`f32tof16` at the call site). The port's `taa_compress_sample(dist: f32, ...)`
folds the `f32tof16` into the helper — matching `06-design-a2.md` §6.1's WGSL
snippet, which passes `sample_dist` (a float) to the helper. WGSL has no
`f32tof16` scalar builtin; the helper does `pack2x16float(vec2(dist, 0.0)) &
0xFFFFu` to get the single f16's bits. Behaviour-identical, cleaner call site.

**Build + tests:** `cargo build` succeeded (no new warnings — the struct-size
asserts reference the new structs so they don't dead-code-warn). `cargo test
--bin bevy-naadf` → **39 passed** (the size asserts are compile-time).

---

### Step 2 — `CameraHistory` + frame counter + jitter + shared camera helpers

**Files created:** `src/render/taa.rs`.
**Files edited:** `src/render/mod.rs`, `src/render/extract.rs`, `src/main.rs`.

- `taa.rs` (NEW): the `CameraHistory` main-world resource (§2.3 — the four
  parallel 128-deep rings `positions` / `view_proj` / `jitter` + `frame_count`
  + `taa_index` + `current_jitter`); the `CAMERA_HISTORY_DEPTH = 128` /
  `TAA_SAMPLE_RING_DEPTH = 16` consts (the Rust source-of-truth); `taa_index_of()`
  (`128 - (frame_count % 128) - 1` — `WorldRender.cs:88`); `halton_jitter()` (a
  faithful port of `WorldRender.cs`'s `Halton1D`/`Halton2D`/`getJitter` — the
  2-D Halton of `(frame % 32) + 1` in bases `(3,7)`, minus `0.5`);
  `rotation_only_view_proj()` (the shared helper — `clip_from_view *
  Mat4::from_quat(rotation).inverse()`); and the `update_camera_history`
  `Update` system (§9.3).
- `update_camera_history` follows §9.3 / §13.6 **exactly**: it derives
  `taa_index` from the *current* (pre-increment) `frame_count`, writes the rings
  at that slot, **stores `taa_index` on `CameraHistory`**, then increments
  `frame_count`. `taa_index` is never re-derived render-side — the off-by-one
  trap is eliminated by storing it. `current_jitter` is stored the same way
  (computed once, read by `prepare_frame_gpu`).
- `extract.rs`: `extract_camera` refactored to call `rotation_only_view_proj()`
  instead of its inline matrix build (the formula now lives in exactly one
  place — both `extract_camera` and `update_camera_history` call it).
- `main.rs`: `CameraHistory` registered via `init_resource` (it has a `Default`
  impl); `update_camera_history` added to `Update` with
  `.after(camera::sync_position_split)` (it must see this frame's `PositionSplit`).
- `mod.rs`: `pub mod taa;` declared.

**Deviation (small, logged) — `update_camera_history` reads the camera's
`Transform` + `PositionSplit` component, not `GlobalTransform`.** §9.3
recommendation (a) says `update_camera_history` queries `(&Camera,
&GlobalTransform, &PositionSplit)`. But `GlobalTransform` propagation runs in
`PostUpdate`, so in an `Update` system `GlobalTransform` is last-frame's value.
The camera has no parent, so `Transform == GlobalTransform` for it, and
`Transform` *is* the current-frame value in `Update` (this is the same
reasoning `sync_position_split` already relies on — `04-impl.md` step 3). The
system therefore queries `(&Camera, &Transform, &PositionSplit)` and builds the
rotation-only view-proj from `transform.rotation`. `rotation_only_view_proj`
itself stays a pure `(camera, rotation: Quat) -> Mat4` helper — the formula is
in one place; each caller supplies the rotation from the transform source
correct for its schedule (`extract_camera` passes `global_transform.rotation()`,
`update_camera_history` passes `transform.rotation`).

**Deviation (small, logged) — `CameraHistory.positions` is `[PositionSplit; 128]`,
followed §2.3 verbatim.** (Initially considered storing plain `Vec3` world
positions; reverted to match §2.3's explicit `[PositionSplit; …]` typing — the
`PositionSplit` subtraction in `prepare_taa` is the D1 camera-relative-rendering
precision trick, so the int+frac split must be preserved through the ring.)

**Build + smoke-run:** `cargo build` succeeded (one new warning:
`TAA_SAMPLE_RING_DEPTH` unused — consumed in step 5, expected). Smoke-run
(`cargo run`, timeout-capped ~30 s, with a temporary `info!` in
`update_camera_history`, reverted after): clean — window opens on the RTX 5080
/ Vulkan, the grid builds, **`frame_count` is real and monotonic** (1, 2, 3, …),
**`taa_index` follows `128 - (frame_count % 128) - 1` exactly** (frame 1 → 127,
frame 120 → 8, frame 1920 → 0), `jitter` is `(0,0)` (TAA off — `AppArgs.taa`
still `false`). No panics, no errors, clean exit. Phase A still renders (no
render-path change yet).

---

### Step 3 — Extract the camera history + the non-inverted view-proj

**Files edited:** `src/render/extract.rs`, `src/render/mod.rs`.

- `extract.rs`: added `view_proj` to `ExtractedCameraData` — the *non-inverted*
  rotation-only `clip_from_view_rot`, stored directly (before the `.inverse()`)
  so `prepare_taa` has the C# `camMatrix` without a redundant inverse (§9.2).
  Added the `ExtractedCameraHistory` render-world resource (the rings +
  `frame_count` + `taa_index` + `current_jitter` + a `valid` flag) and the
  `extract_camera_history` `ExtractSchedule` system (a cheap fixed-cost copy of
  the 128-element arrays).
- `mod.rs`: `init_resource::<ExtractedCameraHistory>()`; `extract_camera_history`
  added to the `ExtractSchedule` tuple.

No deviations. `ExtractedCameraData` derives `Default, Clone, Copy` — the added
`Mat4` field is fine for all three. `ExtractedCameraHistory` gets a hand-written
`Default` (arrays of 128 don't auto-derive `Default`).

**Build + tests:** `cargo build` succeeded; `cargo test --bin bevy-naadf` →
**39 passed**.

---

### Step 4 — Fix `frame_count` / `rand_counter` / `taa_index` / `taa_jitter` in `prepare_frame_gpu`

**Files edited:** `src/render/prepare.rs`.

This lands the carried `05-review.md` §4 fix.

- `prepare_frame_gpu`: dropped the `Res<Time>` param; added
  `Res<ExtractedCameraHistory>`. `GpuRenderParams.frame_count` /
  `rand_counter` are now `extracted_history.frame_count` (the real monotonic
  counter); `taa_index` is `extracted_history.taa_index` (the *stored* index,
  not re-derived — §9.3); `taa_jitter` is `extracted_history.current_jitter`
  (zero unless `AppArgs.taa`, since `update_camera_history` keeps `current_jitter`
  zero when TAA is off). The bogus `time.elapsed().as_millis()` /
  `elapsed_secs * 1000` lines are gone. `rand_counter == frame_count` is the
  deliberate A-2 simplification (§4.1, §13.3 — the `randValues[32]` table is not
  ported; the load-bearing property is a per-frame-varying RNG salt, which the
  counter is) — documented in a `prepare.rs` comment.
- `FLAG_IS_TAA` is **not** set in Batch 1 — the TAA logic (the first-hit ring
  write + the reproject node) is Batch 2; Batch 1 only lands the plumbing.
  Documented in the comment.

No deviations.

**Build + smoke-run:** `cargo build` succeeded. Smoke-run (`cargo run`,
timeout-capped ~30 s, with a temporary `info!` in `prepare_frame_gpu`, reverted
after): clean — **the render world receives a real monotonic `frame_count`**
(1, 2, 3, …), `rand_counter == frame_count`, `taa_index` follows the formula,
`taa_jitter` is `(0,0)`. No WGSL / pipeline-validation errors, no panics, clean
exit. **Phase A renders identically** (jitter still zero, TAA node not added) —
the only change is the counters are now real.

---

### Step 5 — `TaaGpu` + `prepare_taa` + buffer creation + the blit-source swap

**Files edited:** `src/render/taa.rs`, `src/render/mod.rs`, `src/render/prepare.rs`,
`src/render/pipelines.rs`, `src/render/graph.rs`,
`src/assets/shaders/naadf_first_hit.wgsl`, `src/assets/shaders/naadf_final.wgsl`.

- `taa.rs`: added the `TaaGpu` render-world resource (§9.4 — `taa_samples` the
  16-ring, `taa_sample_accum`, `camera_history`, `taa_params`, `pixel_count`,
  `taa_first_hit_bind_group`) and the `prepare_taa` `PrepareResources` system
  (§9.2). `prepare_taa` creates the screen-space buffers (`taa_samples` =
  `pixel_count * 16` × `vec2<u32>`, `taa_sample_accum` = `pixel_count` ×
  `vec2<u32>` — both `STORAGE | COPY_DST`, zero-cleared on creation), the
  fixed-size `camera_history` (128 × `GpuCameraHistorySlot`) and `taa_params`
  (`GpuTaaParams` uniform); handles viewport-resize (re-creates only the
  screen-space buffers, keeps the fixed-size ones); uploads the
  `GpuCameraHistorySlot[128]` array every frame (deriving
  `cam_pos_from_cur_int = (positions[i] - current_camera).to_world()` via the
  `PositionSplit` subtraction — the D1 trick); uploads `GpuTaaParams` every
  frame (`sample_age` fixed at `TAA_SAMPLE_RING_DEPTH = 16`, clamped to
  `[1, 16]` per §7.1 / §13.4); and builds `taa_first_hit_bind_group` (rebuilt
  only when `taa_samples` is re-created).
- `mod.rs`: `prepare_taa` added to `PrepareResources` (before
  `prepare_frame_gpu` in `PrepareBindGroups`, so `TaaGpu` exists when
  `prepare_frame_gpu` binds `taa_sample_accum`).
- **Blit-source swap:** `FrameGpu.shaded_color` **deleted**; `prepare_frame_gpu`
  now takes `Option<Res<TaaGpu>>` and binds `taa_gpu.taa_sample_accum` at frame
  bind-group slot 3 and blit bind-group slot 1 (where it bound the local
  `shaded_color`). `prepare_frame_gpu` no longer creates / zero-clears
  `shaded_color` (that buffer — now `taa_sample_accum` — is created +
  zero-cleared by `prepare_taa`).
- `pipelines.rs`: the `frame_layout` / `blit_layout` binding comments renamed
  `shaded_color` → `taa_sample_accum` (the layout *shape* is unchanged — same
  `vec2<u32>` type, same r/w access). Added `taa_layout` (`@group(2)`, one
  read-write `taa_samples` storage binding) — see the deviation below.
- `naadf_first_hit.wgsl` + `naadf_final.wgsl`: the `shaded_color` binding
  renamed to `taa_sample_accum` (`@group(1) @binding(3)` and
  `@group(0) @binding(1)` respectively) — pure renames, the element format and
  the write/read sites are unchanged (the Phase-A stand-in was deliberately
  built to the `taaSampleAccum` `vec2<u32>` format). File-header + binding
  comments updated.
- `graph.rs`: doc comments only — renamed `shaded_color` → `taa_sample_accum`
  in the node-system docs (the node systems themselves are unchanged; they bind
  whatever `prepare_frame_gpu` put in the bind groups).

**Deviation (small, logged) — `taa_layout` (`@group(2)`) is created in `pipelines.rs`
in Batch 1 step 5, not Batch 2 step 6.** `06-design-a2.md` §12 step 6 nominally
adds `taa_layout` to `pipelines.rs`. But step 5's `TaaGpu` struct (per §9.4)
has a `taa_first_hit_bind_group: BindGroup` field that step 5 must build — and
building it needs `taa_layout` to exist. The minimal forward-pull is to create
the `taa_layout` *descriptor* in step 5 (one `BindGroupLayoutDescriptor`),
which lets `TaaGpu` be complete and self-consistent as §9.4 specifies. The
load-bearing part of step 6 — *extending `first_hit_pipeline`'s layout to 3
groups* so the first-hit pass actually binds `@group(2)` — is **not** done in
Batch 1; `first_hit_pipeline` still has the 2-group `[world_layout,
frame_layout]` layout, and `taa_first_hit_bind_group` is built but unused until
Batch 2 wires it. A half-populated `TaaGpu` was the messier alternative. Logged
in the `pipelines.rs` `taa_layout` doc comment.

**Build + tests + smoke-run:** `cargo build` succeeded (12 warnings — *down*
from 14; the `shaded_color` removal cleared two — all remaining are the
pre-existing dead-code profile from `04-impl.md`, plus `TaaGpu`'s
not-yet-consumed fields which are held alive by the resource so they don't
warn). `cargo test --bin bevy-naadf` → **39 passed**. Smoke-run (`cargo run`,
timeout-capped ~30 s): **clean** — window opens on the RTX 5080 / Vulkan, the
grid builds, the render graph (first-hit compute → final blit) compiles and
runs with **no WGSL compile errors, no pipeline-validation errors, no
bind-group validation errors, no panics**, exits cleanly (exit code 0).
`prepare_taa` created `TaaGpu`, the first-hit pass writes the real
`taa_sample_accum` buffer (owned by `TaaGpu`), and the final blit reads it.
With the TAA reproject node absent, the pipeline behaves identically to Phase
A's `shaded_color` path — **the app renders exactly as Phase A did, now reading
the real `taa_sample_accum`** — the designed-in drop-in swap, proven before any
TAA logic.

---

## State at end of Batch 1

- **Step 1 — GPU types + sample format:** `gpu_types.rs` has `GpuTaaParams`
  (192 B) + `GpuCameraHistorySlot` (96 B) + size asserts; `taa_common.wgsl`
  ports `commonTaa.fxh` (the 64-bit sample (de)compress, the hash, the 3×3
  neighbour offsets, `TAA_SAMPLE_RING_DEPTH = 16u`).
- **Step 2 — camera history + frame counter + jitter:** `taa.rs` has the
  `CameraHistory` resource, the `CAMERA_HISTORY_DEPTH`/`TAA_SAMPLE_RING_DEPTH`
  consts, `taa_index_of()`, `halton_jitter()`, `rotation_only_view_proj()`, and
  `update_camera_history` (computes `taa_index` once per frame, stores it —
  §9.3 exactly). Wired into `main.rs` (`Update`, after `sync_position_split`);
  `extract_camera` refactored onto the shared helper.
- **Step 3 — extract:** `ExtractedCameraData` gains `view_proj` (the
  non-inverted rotation-only matrix); `ExtractedCameraHistory` +
  `extract_camera_history` mirror the rings into the render world.
- **Step 4 — counter fix landed:** `prepare_frame_gpu` sets a real monotonic
  `frame_count` / `rand_counter` / the stored `taa_index` / the extracted
  `taa_jitter` — the carried `05-review.md` §4 fix, on its own.
- **Step 5 — `TaaGpu` + the blit-source swap:** `prepare_taa` creates +
  (re)sizes the TAA buffers and uploads `camera_history` + `taa_params` each
  frame; `FrameGpu.shaded_color` is deleted and `prepare_frame_gpu` binds the
  real `taa_sample_accum` (owned by `TaaGpu`); the WGSL + layout binding names
  are renamed `shaded_color` → `taa_sample_accum`. `taa_layout` (`@group(2)`)
  exists but `first_hit_pipeline`'s layout is still 2-group.
- **Files:** 2 new (`src/render/taa.rs`, `src/assets/shaders/taa_common.wgsl`),
  9 modified (`src/main.rs`, `src/render/{extract,gpu_types,graph,mod,pipelines,
  prepare}.rs`, `src/assets/shaders/{naadf_first_hit,naadf_final}.wgsl`).
- **Build:** `cargo build` clean (12 dead-code warnings, all pre-existing
  profile or not-yet-consumed Batch-2 surface — no new real issues).
- **Tests:** `cargo test --bin bevy-naadf` → **39 passed, 0 failed** (no
  regressions; step 1's struct-size asserts are compile-time).
- **Smoke-run:** `cargo run` launches on the RTX 5080 / Vulkan, builds the grid,
  the first-hit → final-blit render graph compiles and runs with no WGSL /
  pipeline / bind-group validation errors and no panics, exits clean. The app
  renders identically to Phase A — Batch 1 is pure infrastructure + the
  designed-in drop-in swap.

### Deviations from `06-design-a2.md` (all small + local, logged inline above)

1. **`taa_compress_sample` takes `dist: f32`, folds the `f32tof16` in** (step 1)
   — matches §6.1's WGSL snippet; the HLSL `compressSample` takes pre-converted
   f16 bits. Behaviour-identical, cleaner call site.
2. **`update_camera_history` reads `&Transform` (+ `&PositionSplit`), not
   `&GlobalTransform`** (step 2) — `GlobalTransform` propagation runs in
   `PostUpdate`, so it is stale in an `Update` system; `Transform` is the
   current-frame value and equals `GlobalTransform` for the parent-less camera.
   `rotation_only_view_proj` stays a pure `(camera, rotation: Quat)` helper —
   the formula is still in one place.
3. **`taa_layout` (`@group(2)`) created in `pipelines.rs` in step 5, not step 6**
   — step 5's `TaaGpu` (§9.4) has a `taa_first_hit_bind_group` field that needs
   the layout to exist. The layout *descriptor* is the minimal forward-pull;
   `first_hit_pipeline`'s layout is **not** extended (that stays Batch 2 step 6),
   so the group is built-but-unused in Batch 1.

None of these is a large redesign; each is the kind of "small local deviation,
made and logged" the brief explicitly allows. No blocker required pausing for
orchestrator/user consultation.

### What Batch 2 (steps 6–9) / the reviewer needs to know

1. **Test command is `cargo test --bin bevy-naadf`** (binary-only crate).
2. **`taa_layout` already exists in `NaadfPipelines`** — Batch 2 step 6 only
   needs to *extend `first_hit_pipeline`'s layout* to `[world_layout,
   frame_layout, taa_layout]` and have `naadf_first_hit_node` bind
   `@group(2) = taa_gpu.taa_first_hit_bind_group`. The layout descriptor and the
   `TaaGpu.taa_first_hit_bind_group` field are built and ready.
3. **`TaaGpu` is complete** — `taa_samples` (16-ring), `taa_sample_accum`,
   `camera_history`, `taa_params` are all created, sized, zero-cleared, and (the
   ring/uniform) uploaded each frame by `prepare_taa`. Batch 2 needs only to
   *consume* them: the first-hit `taa_samples` write (step 6), the reproject
   pass reading `taa_samples` + `camera_history` + `taa_params` (steps 7–8).
4. **`sample_age` is fixed at 16** (`TAA_SAMPLE_AGE` const in `taa.rs`, clamped
   to `[1, TAA_SAMPLE_RING_DEPTH]`) — no runtime knob (§13.4).
5. **`taa_reproject_bind_group` is NOT built yet** — per §5.5 it must be built
   in `prepare_frame_gpu` (it mixes `TaaGpu` + `FrameGpu.first_hit_data`).
   Batch 2 step 8 adds it.
6. **`taa_params.view_proj`** is `ExtractedCameraData.view_proj` — the
   *non-inverted* rotation-only `clip_from_view_rot` (the C# `camMatrix`). The
   reproject WGSL must use `M * v` against it (the `05-review.md` perspective-fix
   convention — column-vector, glam-built matrix), exactly as `get_ray_dir` was
   fixed to. `inv_view_proj` is its inverse (the C# `invCamMatrix`).
7. **`camera_history` slot layout:** `view_proj` = the past frame's
   rotation-only view-proj; `cam_pos_from_cur_int` = that frame's camera pos
   *relative to the current camera int position* (recomputed every frame in
   `prepare_taa` via the `PositionSplit` subtraction); `jitter` = that frame's
   Halton jitter. The reproject pass reads all three.
8. **`FLAG_IS_TAA` is wired but never set** — `prepare_frame_gpu` always builds
   `flags = FLAG_CHECK_SUN`. Batch 2 step 6 sets `FLAG_IS_TAA` when `AppArgs.taa`
   is on; step 9 flips `AppArgs.taa`'s default to `true`.
9. **`taa_jitter` is already plumbed** — `update_camera_history` computes
   `halton_jitter(frame_count)` (gated on `AppArgs.taa`), stores it as
   `CameraHistory.current_jitter` *and* in `jitter[taa_index]`;
   `prepare_frame_gpu` reads `current_jitter` into `GpuRenderParams.taa_jitter`.
   The same `frame_count` feeds both, so the first-hit jitter and the
   `camera_history[taa_index].jitter` for un-jittering are guaranteed equal. It
   is zero today only because `AppArgs.taa` is `false`.
10. **`color_compression.wgsl` was NOT created** — per §13.1 the TAA sample's
    exponential colour compression lives in `taa_common.wgsl` (from
    `commonTaa.fxh`); `commonColorCompression.fxh` is a Phase-B file.
11. **The `05-review.md` §4 non-A-2 secondary issues were not touched** —
    `prepare_world_gpu`-every-frame and the zeroed `GpuRenderParams.bbox` fields
    are out of A-2 scope and left alone, as the brief requires.

## Verdict

**Ready for Batch 2.** Steps 1–5 complete, `cargo build` clean, all 39 unit
tests pass, the app smoke-runs clean on the RTX 5080 / Vulkan with no WGSL /
pipeline / bind-group validation errors and no panics. The Batch-1 done-bar is
met: the app renders **identically to Phase A** — Batch 1 is pure
infrastructure (the GPU types, the camera-history ring, the frame counter, the
jitter plumbing, the `TaaGpu` buffers) plus the designed-in `shaded_color` →
`taa_sample_accum` drop-in swap, which is now reading the real
`taa_sample_accum` buffer through the same code path Phase A used for
`shaded_color`. No blockers. No design deviation large enough to need
orchestrator/user consultation — the three deviations logged above are all
small and local, made and logged per the brief's allowance. Scope stopped at
step 5 — no Batch 2 (steps 6–9) work started.

---

## impl findings — Phase A-2 Batch 2 (steps 6–9) (2026-05-14)

Batch 2 (steps 6–9 of `06-design-a2.md` §12) was implemented by a previous
agent and committed as `8abd2ec` ("feat: implement Phase A-2 TAA reproject node
and first-hit ring write"), but that agent was interrupted before writing this
log. This section **reconstructs** what Batch 2 did from the committed code,
records the **step-9 `hud.rs` TAA-timing line** added by this (the `review`)
group — the one missing piece — and logs the build / test / smoke-run results.
The verification of the Batch-2 TAA code against the design + the NAADF HLSL is
in `08-review-a2.md` (the review deliverable); this section is the impl record.

Test command (unchanged): `cargo test --bin bevy-naadf` (binary-only crate).

---

### Steps 6–8 — reconstructed from the committed code (`8abd2ec`)

**Step 6 — first-hit `taa_samples` ring write.** `pipelines.rs`:
`first_hit_pipeline`'s layout extended from `[world_layout, frame_layout]` to
`[world_layout, frame_layout, taa_layout]` (`pipelines.rs:223-227`) — the
`taa_layout` descriptor itself was already created in Batch 1 step 5.
`naadf_first_hit.wgsl`: added the `@group(2) @binding(0) var<storage, read_write>
taa_samples` binding (`:52`), imported `taa_compress_sample` +
`TAA_SAMPLE_RING_DEPTH` from `taa_common.wgsl` (`:32`), and added the
`if ((params.flags & FLAG_IS_TAA) != 0u)` ring-write block (`:170-187`) — a
faithful port of `renderFirstHit.fx:109-117`'s `if (isTAA)` path:
`specular_normals = 0u` hardcoded (plane-0-only, entity-free),
`sample_dist = select(distance_ray, 65520.0, voxel_type_raw == 0u)`,
`taa_compress_sample(...)` into ring slot `params.taa_index %
TAA_SAMPLE_RING_DEPTH`. `graph.rs`: `naadf_first_hit_node` binds
`@group(2) = taa_gpu.taa_first_hit_bind_group` (`graph.rs:203`).

**Step 7 — the TAA reproject WGSL.** `src/assets/shaders/taa.wgsl` (NEW, 423
lines): the port of `albedo/renderTaaSampleReverse.fx`'s `reprojectOldSamples`
→ `reproject_old_samples`. Contains `get_hit_data_from_planes_a2` (the
single-plane reduction of `getHitDataFromPlanes`), `get_screen_pos_projection` /
`get_screen_index_projection` (ports of the `commonRenderPipeline.fxh` helpers,
returning structs in place of HLSL `out` params), the 3×3 neighbourhood
precompute, the reprojection loop, the accumulation into `taa_sample_accum`, and
the `GpuTaaParams` + `GpuCameraHistorySlot` WGSL struct decls. Entity blocks
omitted, the rough-specular branch left as a structural dead-code comment, the
sample ring `% 16` (both sites), the camera-history ring `% 128`. Every matrix
multiply is `M * v` + `w`-divide (the `05-review.md` perspective-fix convention).

**Step 8 — the TAA node + pipeline + graph wiring.** `pipelines.rs`: added
`taa_reproject_layout` (`pipelines.rs:200-212` — `taa_params` uniform +
`camera_history` / `first_hit_data` / `taa_samples` read storage +
`taa_sample_accum` rw storage), `taa_reproject_pipeline` (entry
`reproject_old_samples`, `pipelines.rs:235-242`), `TAA_REPROJECT_SHADER` const.
`extract.rs`: added `ExtractedTaaConfig` (mirrors `AppArgs.taa`) +
`extract_taa_config`. `prepare.rs`: `prepare_frame_gpu` builds
`taa_reproject_bind_group` (mixes `TaaGpu` + `FrameGpu.first_hit_data`,
`prepare.rs:435-445`) and sets `FLAG_IS_TAA` when `extracted_taa.enabled`.
`graph.rs`: `naadf_taa_reproject_node` (`graph.rs:226-284`) + the
`TAA_REPROJECT_SPAN` const (`graph.rs:151`); the node gates its dispatch on
`ExtractedTaaConfig.enabled` and early-returns when TAA is off (leaving
`taa_sample_accum` bit-identical to Phase A). `mod.rs`: registered
`ExtractedTaaConfig`, added `extract_taa_config` to `ExtractSchedule`, inserted
`naadf_taa_reproject_node` into the `Core3d` `.chain()` between first-hit and
final-blit. `main.rs`: `AppArgs.taa` default flipped `false` → `true` (the §9.5
default flip — confirmed: `main.rs:51` reads `taa: true`).

**Verification of steps 6–8 against the design + the NAADF HLSL:** see
`08-review-a2.md` §1–§4 — verdict: the TAA logic is faithfully ported,
0.25-spp-ready, and uses the correct matrix convention.

---

### Step 9 — the missing piece, added by the `review` group

Batch 2's commit `8abd2ec` did **not** add the step-9 `hud.rs` TAA-node timing
line (`06-design-a2.md` §11) — `hud.rs` was left exactly as Phase A had it. This
group added it (the only implementation work in this group's scope; the
`AppArgs.taa` default flip, also part of step 9, was already done in `8abd2ec`).

**File edited:** `src/hud.rs`.

- Imported `TAA_REPROJECT_SPAN` from `render::graph` (alongside the existing
  `FIRST_HIT_SPAN` / `FINAL_BLIT_SPAN`).
- Added the `const TAA_REPROJECT_GPU_PATH = "render/naadf_taa_reproject/elapsed_gpu"`
  + `TAA_REPROJECT_CPU_PATH = "render/naadf_taa_reproject/elapsed_cpu"` path pair,
  matching the Phase-A pattern for the other two render nodes.
- Added the `const`-checked
  `assert!(matches_span(TAA_REPROJECT_GPU_PATH, TAA_REPROJECT_SPAN));` line to the
  compile-time path/span consistency block (`hud.rs:30-33`), so the HUD path
  cannot drift from the `render::graph` span name.
- Added a `write_timing(s, &diagnostics, "taa-reproject", TAA_REPROJECT_GPU_PATH,
  TAA_REPROJECT_CPU_PATH)` call in the `"NAADF passes:"` block, **between**
  `first-hit` and `final-blit` (matching the render order, per §11).
- Updated the surrounding "two render nodes" comment to "three render nodes".

`write_timing` itself is unchanged (it is generic). The optional cosmetic
renderer-mode-string update (§11, "low priority") was **not** done — it is
explicitly optional and out of this group's minimal scope.

No deviations.

---

### Build + tests + smoke-run

- **`cargo build`** — succeeds. 11 warnings, all the pre-existing dead-code
  profile carried from `04-impl.md` / Batch 1 — none new from the `hud.rs` edit.
- **`cargo test --bin bevy-naadf`** — **39 passed, 0 failed.** No regressions
  (the `hud.rs` change is HUD-only; the `gpu_types` struct-size asserts are
  compile-time and still hold).
- **Smoke-run** — exactly **one** timeout-capped (~30 s) `cargo run`, after the
  `hud.rs` edit, on the RTX 5080 / Vulkan. Result: **the app launches, no panic,
  no WGSL compile error, no pipeline/bind-group validation error, clean exit
  (code 0).** The `taa.wgsl` `reproject_old_samples` pipeline compiles (the early
  one-frame "shader could not be loaded" line is the normal async asset-load
  transient — the pipeline resolves to `Creating` then dispatches), and
  `naadf_taa_reproject_node` dispatches every frame with `AppArgs.taa` on (the
  default). The full three-node graph (first-hit → TAA reproject → final blit)
  runs. **One observation from the smoke-run, NOT a launch failure:** the
  leftover TEMP STEP-8 instrumentation committed in `8abd2ec` is still active —
  the run logged `TAA_DEBUG[center]: ...` and `TAA_DEBUG reproject_node:
  DISPATCHING` every frame. That instrumentation was never reverted before the
  Batch-2 commit; it is logged as the **blocking issue** in `08-review-a2.md` §5
  (with the full file:line revert list). It is incomplete-cleanup, not a launch
  or logic failure — the app builds, launches, and exits clean — so it is a
  review finding to be reverted as a separate scoped task, not an impl fix this
  group makes (the brief scopes this group's impl work to the `hud.rs` line only
  and forbids touching the Batch-2 TAA code).

No verification loop was run — the TAA correctness check is the static analysis
in `08-review-a2.md`; the app was run exactly once, only to confirm it still
builds + launches + exits clean after the `hud.rs` edit.

---

## State at end of Phase A-2

- **Batch 1 (steps 1–5)** — GPU types + the 64-bit sample format
  (`taa_common.wgsl`), the `CameraHistory` ring + monotonic frame counter +
  Halton jitter + shared camera helpers (`taa.rs`), the extract plumbing
  (`ExtractedCameraData.view_proj`, `ExtractedCameraHistory`), the carried
  `05-review.md` §4 `frame_count`/`rand_counter` fix, and the `TaaGpu` resource
  + `prepare_taa` + the `shaded_color` → `taa_sample_accum` drop-in swap. Logged
  in full above; committed `91c67e3`.
- **Batch 2 (steps 6–8)** — the first-hit `taa_samples` ring write
  (`naadf_first_hit.wgsl` `@group(2)` + the `FLAG_IS_TAA`-gated write), the TAA
  reproject WGSL (`taa.wgsl` — port of `renderTaaSampleReverse.fx`), the TAA
  node + `taa_reproject_pipeline` + `taa_reproject_layout` + graph wiring, and
  the `AppArgs.taa` default flip to `true`. Committed `8abd2ec`. Verified
  faithful + 0.25-spp-ready + correct matrix convention in `08-review-a2.md`.
- **Step 9** — the `AppArgs.taa` default flip landed in `8abd2ec`; the `hud.rs`
  TAA-node timing line was missing and was added by this group (above).
- **Files:** Phase A-2 total — 3 new (`src/render/taa.rs`,
  `src/assets/shaders/taa.wgsl`, `src/assets/shaders/taa_common.wgsl`),
  ~10 modified (`src/main.rs`, `src/hud.rs`, `src/render/{extract,gpu_types,
  graph,mod,pipelines,prepare,taa}.rs`,
  `src/assets/shaders/{naadf_first_hit,naadf_final}.wgsl`).
- **Build:** `cargo build` clean (11 pre-existing dead-code warnings, none new).
- **Tests:** `cargo test --bin bevy-naadf` → **39 passed, 0 failed.**
- **Smoke-run:** one run — launches on the RTX 5080 / Vulkan, the full
  three-node render graph compiles and runs, no WGSL / pipeline / bind-group
  validation errors, no panics, clean exit.
- **Blocking issue (carried into the review gate):** the leftover TEMP STEP-8
  instrumentation from `8abd2ec` was never reverted — it is still committed and
  active across `taa.wgsl`, `graph.rs`, `mod.rs`, `taa.rs`. It corrupts one pixel
  of `taa_sample_accum`, does a blocking per-frame GPU readback, and spams logs.
  Full file:line evidence + the recommended revert are in `08-review-a2.md` §5.
  **Phase A-2 should not close until this instrumentation is reverted** (a
  mechanical, separately-scoped task — the TAA logic underneath is verified
  correct).

### Verdict

**Phase A-2 implementation is complete; the step-9 `hud.rs` timing line is
landed; build + 39 tests + the single smoke-run are green.** The Batch-2 TAA
logic is verified faithful, 0.25-spp-ready, and correct on the matrix
convention (`08-review-a2.md`). The one blocking issue — the un-reverted TEMP
STEP-8 instrumentation committed in `8abd2ec` — is a separate, mechanical revert
task flagged in `08-review-a2.md` §5; it is not a logic or design defect, and it
was not fixed here because the brief scopes this group's impl work to the
`hud.rs` line and forbids touching the Batch-2 TAA code. Once that revert lands,
Phase A-2 is ready for the user interactive temporal-stability review gate.
