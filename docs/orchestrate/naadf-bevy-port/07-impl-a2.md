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
