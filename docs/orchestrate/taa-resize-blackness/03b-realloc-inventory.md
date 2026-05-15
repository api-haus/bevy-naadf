# 03b — Inventory: what survives a pixel_count change today

This inventory walks every GPU resource (and CPU-side ring) managed by the
three prepare systems + `update_camera_history`. For each, it records whether
the resource is recreated on resize, whether it is zero-cleared after
recreation, and whether anything **survives** a `pixel_count` change.

Source files audited:
- `crates/bevy_naadf/src/render/taa.rs:286-464` — `prepare_taa`
- `crates/bevy_naadf/src/render/gi.rs:224-408` — `prepare_gi`
- `crates/bevy_naadf/src/render/prepare.rs:443-931` — `prepare_frame_gpu`
- `crates/bevy_naadf/src/render/taa.rs:188-224` — `update_camera_history`
  (main-world frame system; owns `CameraHistory` resource)

---

## `prepare_taa` (taa.rs:286-464)

The match at lines 322-371 has three arms: same pixel_count (clone everything),
mismatched pixel_count with existing (resize), `None` (first build).

| Resource | Recreated on resize? | Zero-cleared on (re)create? | Survives resize? | Notes |
|---|---|---|---|---|
| `TaaGpu.taa_samples` (Buffer) | yes (`create_screen_buffers`, line 335) | yes (line 383) | no | `pixel_count * ring_depth × vec2<u32>`. Sized by viewport. |
| `TaaGpu.taa_sample_accum` (Buffer) | yes (line 335) | yes (line 384) | no | `pixel_count × vec2<u32>`. Sized by viewport. |
| `TaaGpu.taa_dist_min_max` (Buffer) | yes (line 335) | yes (line 385) | no | `pixel_count × vec2<u32>`. Sized by viewport. |
| `TaaGpu.camera_history` (Buffer) | **NO** — cloned from old (line 340) | n/a (handle clone) | **YES** | Fixed-size `128 × GpuCameraHistorySlot`. Buffer handle survives. The **CONTENTS** are rewritten every frame from `extracted_history` (lines 396-419), but the ring's source — main-world `CameraHistory.{positions, view_proj, view_proj_inv, jitter}` — is monotonically advancing and was populated at the OLD projection. |
| `TaaGpu.taa_params` (Buffer) | **NO** — cloned from old (line 341) | n/a (handle clone) | YES (rewritten/frame) | `GpuTaaParams` uniform. Contents fully overwritten every frame at line 442. Survival is cosmetic. |
| `TaaGpu.pixel_count` (u32 field) | yes (set to new value, line 461) | n/a | no | scalar |
| `TaaGpu.taa_first_hit_bind_group` (BindGroup) | yes — rebuilt because `needs_new_storage == true` (line 446-453) | n/a | no | rebuilt on every recreate |

**Main-world `CameraHistory` resource** (taa.rs:64-94, owned by main world,
extracted to render world by `extract_camera_history`):

| Field | Recreated/reset on resize? | Survives resize? | Notes |
|---|---|---|---|
| `CameraHistory.positions[128]` | NO | YES | 128-entry ring of `PositionSplit`. Indexed by `taa_index_of(frame_count)`. Entries written at the OLD projection / OLD aspect ratio persist for up to 128 frames. |
| `CameraHistory.view_proj[128]` | NO | YES | Rotation-only view-proj matrices. Stale across resize. |
| `CameraHistory.view_proj_inv[128]` | NO | YES | inverse of view_proj. Stale across resize. |
| `CameraHistory.jitter[128]` | NO | YES | Halton jitter values. Aspect-independent → less stale, but still old-frame state. |
| `CameraHistory.frame_count` | NO (monotonic counter) | YES | This is INTENTIONALLY monotonic — not reset on resize. |
| `CameraHistory.taa_index` | derived from frame_count | n/a | recomputed each frame |
| `CameraHistory.current_jitter` | n/a (recomputed each frame) | n/a | overwritten each frame |

---

## `prepare_gi` (gi.rs:224-408)

The match at lines 248-266 has only two arms: same pixel_count (clone all), or
anything else (call `create_gi_buffers` wholesale — first build OR resize go
through the same path).

| Resource | Recreated on resize? | Zero-cleared on (re)create? | Survives resize? | Notes |
|---|---|---|---|---|
| `GiGpu.ray_queue` (Buffer) | yes (`create_gi_buffers`) | yes (line 529) | no | `(pixel_count + 1) × u32`. Pixel-sized. |
| `GiGpu.ray_queue_indirect` (Buffer) | yes (recreated by `create_gi_buffers`) | yes (line 530) + reseeded `[0,1,1,0,0]` (lines 547-551) | no | 5×u32 — fixed-size but currently recreated on every resize via `create_gi_buffers` wholesale path. |
| `GiGpu.valid_samples` (Buffer) | yes | yes (line 531) | no | `pixel_count * 2 × GpuSampleValid`. |
| `GiGpu.invalid_samples` (Buffer) | yes | yes (line 532) | no | `pixel_count * 8 × vec4<u32>`. |
| `GiGpu.valid_samples_refined` (Buffer) | yes | yes (line 533) | no | `bucket_count * 32 × vec4<u32>`. |
| `GiGpu.valid_samples_compressed` (Buffer) | yes | yes (line 534) | no | `bucket_count * 8 × vec4<u32>`. |
| `GiGpu.bucket_info` (Buffer) | yes | yes (line 535) | no | `bucket_count × vec2<u32>`. |
| `GiGpu.sample_counts` (Buffer) | yes | yes (line 536) | no | **`SAMPLE_COUNTS_LEN = 128 + 3` × vec2<u32>** — FIXED-SIZE but currently recreated + zero-cleared on resize. The 128-frame GI accumulation ring. Drained on every resize → `refineBuckets` `< 12` gate stays closed for ~128 frames → no indirect bounce → black shadows. |
| `GiGpu.valid_dispatch` (Buffer) | yes | yes (line 537) + reseed `[1,1,1,0,0]` | no | 5×u32 — fixed-size; reseeded on each create. |
| `GiGpu.invalid_dispatch` (Buffer) | yes | yes (line 538) + reseed `[1,1,1,0,0]` | no | 5×u32 — fixed-size; reseeded on each create. |
| `GiGpu.denoise_preprocessed` (Buffer) | yes | yes (line 539) | no | `pixel_count × vec4<u32>`. |
| `GiGpu.denoise_preprocessed_horizontal` (Buffer) | yes | yes (line 540) | no | `pixel_count × vec4<u32>`. |
| `GiGpu.gi_params` (Buffer) | yes (recreated via `create_gi_buffers`) | written-every-frame at line 388 | n/a | Uniform — overwritten every frame; current contents are first-build-zero (no clear needed). |
| `GiGpu.pixel_count` / `bucket_count` / `bucket_size` | yes (set on Resource insert) | n/a | no | scalars |

**Note on the stale comment at gi.rs:243-247**: the existing comment already
documents the intent that `sample_counts` MUST NOT be re-zeroed on a resize it
survives — but the code path doesn't honor that: the `_` arm at line 265
calls `create_gi_buffers` which DOES allocate a fresh `sample_counts` and
zero-clear it. So today: `sample_counts` IS recreated + zero-cleared on
resize. (Consistent with the user's directive — but the comment is
misleading.)

---

## `prepare_frame_gpu` (prepare.rs:443-931)

The match at lines 600-632 has two arms: same pixel_count (clone everything),
or any mismatch (recreate the three storage buffers).

| Resource | Recreated on resize? | Zero-cleared on (re)create? | Survives resize? | Notes |
|---|---|---|---|---|
| `FrameGpu.camera` (Buffer) | **NO** — cloned (line 636) | n/a (handle clone) | YES (rewritten/frame) | `GpuCamera` uniform. Overwritten line 653. |
| `FrameGpu.render_params` (Buffer) | **NO** — cloned (line 636) | n/a (handle clone) | YES (rewritten/frame) | `GpuRenderParams` uniform. Overwritten line 654. |
| `FrameGpu.first_hit_data` (Buffer) | yes (line 610) | yes (line 664) | no | `pixel_count × vec4<u32>`. |
| `FrameGpu.first_hit_absorption` (Buffer) | yes (line 618) | yes (line 665) | no | `pixel_count × vec2<u32>`. |
| `FrameGpu.final_color` (Buffer) | yes (line 624) | yes (line 666) | no | `pixel_count × vec2<u32>`. |
| `FrameGpu.pixel_count` (u32) | yes | n/a | no | scalar |
| `FrameGpu.bind_group` etc. (BindGroup × 5) | yes (when `needs_new_storage`) | n/a | no | rebuilt on every recreate |
| `GiBindGroups.*` (BindGroup × 6 + pixel_count) | yes (when `needs_new_storage` or stale) | n/a | no | rebuilt on every recreate. |

---

## `update_camera_history` (taa.rs:188-224, main-world Update)

This is the writer that populates the main-world `CameraHistory` ring (see
table above). It runs every frame in `Update` and writes ONE slot of the
128-entry ring at `taa_index_of(frame_count)`. It does NOT observe resize;
the entire ring entries from before the resize remain valid-looking but at
the OLD projection matrices.

The bug-relevant fact: 128 frames is the time it takes for the writer's
descending `taa_index` cursor to overwrite every old-projection slot.

---

## Other prepare systems checked, no resize concerns

- `prepare_world_gpu` — build-once; doesn't depend on viewport.
- `prepare_atmosphere` — built once at startup with a fixed atmosphere LUT;
  resize-agnostic.

---

## Summary — currently preserved across a real resize

These resources do NOT get reallocated or zero-cleared on a pixel_count
change today:

1. `TaaGpu.camera_history` (GPU buffer) — handle cloned. Contents are
   re-uploaded every frame BUT from the main-world ring which itself carries
   old-projection entries.
2. `TaaGpu.taa_params` (GPU buffer) — handle cloned; contents rewritten every
   frame (cosmetic survival, but still a clone-not-recreate).
3. `FrameGpu.camera` (GPU buffer) — handle cloned; contents rewritten every
   frame (cosmetic).
4. `FrameGpu.render_params` (GPU buffer) — handle cloned; contents rewritten
   every frame (cosmetic).
5. **Main-world `CameraHistory.{positions, view_proj, view_proj_inv,
   jitter}[128]`** — the 128-deep CPU ring. Old-projection entries persist for
   up to 128 frames. THIS is the most consequential survivor — it is what
   feeds the `TaaGpu.camera_history` buffer rewrite every frame.

---

## Under the "reallocate all, preserve nothing" directive

These MUST be force-recreated or zero-cleared on resize:

### Already reallocated + zero-cleared today (no change needed):
- All `TaaGpu` screen-space buffers (`taa_samples`, `taa_sample_accum`,
  `taa_dist_min_max`).
- All `GiGpu` buffers including `sample_counts` and the indirect-dispatch
  ones — `create_gi_buffers` already does the wholesale recreate.
- All `FrameGpu` screen-space buffers (`first_hit_data`,
  `first_hit_absorption`, `final_color`).
- All bind groups (rebuilt under `needs_new_storage`).

### Need to be CHANGED to force-recreate (currently survive):
1. **`TaaGpu.camera_history` (GPU buffer)** — must be recreated AND zero-cleared
   on resize, not cloned. (`taa.rs:340`)
2. **`TaaGpu.taa_params` (GPU buffer)** — must be recreated on resize. The
   contents are overwritten every frame, but the directive says "preserve
   nothing." Recreate the buffer handle. (`taa.rs:341`)
3. **`FrameGpu.camera` (GPU buffer)** — must be recreated on resize.
   (`prepare.rs:636`)
4. **`FrameGpu.render_params` (GPU buffer)** — must be recreated on resize.
   (`prepare.rs:636`)
5. **Main-world `CameraHistory` ring** — its CPU arrays must be reset to
   defaults on resize. Without this, `prepare_taa` continues to upload
   old-projection matrices into the freshly-zeroed GPU `camera_history`
   buffer on the next frame, defeating the GPU-side reset.

The C# reference confirms (5) is the **faithful** behavior:
`WorldRenderBase.cs:150-154` recreates the camera-matrix CPU arrays as fresh
zero-initialised C# arrays on every `CreateScreenTextures()` call. The Bevy
port currently diverges — its `CameraHistory` survives resize. The
"reallocate all, preserve nothing" directive RESTORES the C# behavior here.

### Implementation strategy

The trigger for "this is a resize" lives in `prepare_taa` and `prepare_gi`'s
match arms — `Some(existing) if existing.pixel_count != pixel_count`. The
trigger for `prepare_frame_gpu` is the same comparison at line 602.

The main-world `CameraHistory` reset cannot be done inside `prepare_*` (those
are render-world systems; `CameraHistory` is a main-world resource). The
clean place is `update_camera_history` itself — but it has no signal from the
render world that a resize happened.

**Chosen approach (avoid introducing a cross-world signal):** zero-reset the
main-world `CameraHistory` arrays from a NEW main-world system that runs
when the primary `Window`'s physical resolution has changed since last frame.
This catches the same event the render-side resize uses (the resize
propagates through `Camera::physical_viewport_size()` after `Window`
resolution changes), and is bit-exact with the C# event ordering
(`WindowSizeChanged` → `ScreenUpdate` → CPU array reset).

Alternative considered: zero-reset on the render world based on
`extracted_camera.viewport_size` change. Rejected because the CPU
`CameraHistory` is owned by the main world; modifying it from the render
world requires a cross-world resource or running the system in `ExtractSchedule`
which is conceptually inverted.

### Buffers NOT touched (and why)

- All bind groups: already rebuilt when storage changes — no action needed.
- `gi_params` uniform: already inside `create_gi_buffers`'s wholesale recreate
  path — already reallocated.
- The two uniform handles `FrameGpu.camera` / `FrameGpu.render_params` are
  arguably cosmetic since they're rewritten every frame. The directive says
  "preserve NOTHING" — recreate them too.

### What about WorldGpu / AtmosphereGpu?
- `WorldGpu` is dimension-agnostic (voxel data) — out of scope, doesn't
  depend on `pixel_count`.
- `AtmosphereGpu` is a fixed-size LUT — out of scope, no `pixel_count`
  dependency.
