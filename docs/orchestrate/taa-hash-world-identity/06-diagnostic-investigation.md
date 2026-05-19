# 06 — Diagnostic investigation: TAA hash fixes didn't move the artefact, why?

Read-only diagnostic dispatch, fresh eyes on the streaming-world origin-shift
blink. The orchestration's prior bias — "the TAA hash is missing a
world-identity term" — was followed for two iterations (`05-impl-taa-hash-
world-identity.md`); both passed every deterministic gate and neither moved
the user-visible artefact. This investigation drops that bias and re-reads
the live code.

The core conclusion: **the prior diagnosis identified a real-but-non-dominant
symptom and missed a much bigger structural bug one layer deeper**. The TAA
reproject pipeline (and the ReSTIR-GI reproject pipeline that hangs off the
same camera-history ring) treats the camera position as an absolute reference
frame, but in the streaming preset that position is **window-local** — and the
window-local frame **shifts by 256 voxels per axis whenever the residency
origin moves**. The renderer never reconciles past-frame window-local
positions into the current window-local frame, so on every origin shift the
entire 32-frame camera-history ring becomes coordinate-frame-incoherent at a
stroke. Both pre-fix and post-fix the reject test does its job correctly given
the data it receives — but the data it receives is wrong.


## Pipeline map

Every per-pixel / per-screen-bucket / per-sample buffer that accumulates
across frames in this renderer and could plausibly carry the artefact's
source. R/W file:line refs are inside the worktree.

1. **`taa_samples`** (`pixel_count * ring_depth × vec2<u32>`, ring_depth=32
   default). Slot-major ring. Writer:
   `crates/bevy_naadf/src/assets/shaders/taa.wgsl:525-528`
   (`calc_new_taa_sample`). Reader:
   `crates/bevy_naadf/src/assets/shaders/taa.wgsl:374-377`
   (`reproject_old_samples` history-reject loop). Stores the per-pixel
   compressed sample including the 16-bit `hash` field — the field this
   orchestration was trying to fix. **THE intended TAA history-reject buffer.**

2. **`taa_sample_accum`** (`pixel_count × vec2<u32>`). Per-pixel
   `(weight, R, G, B)` running sum across the reproject pass + the
   calc-new pass. Overwriter:
   `crates/bevy_naadf/src/assets/shaders/taa.wgsl:447-450`
   (`reproject_old_samples` writes `(color_sum.a, color_sum.rgb)` from the
   accepted-history loop). Folder:
   `crates/bevy_naadf/src/assets/shaders/taa.wgsl:533-542`
   (`calc_new_taa_sample` adds the current frame's `light` and
   `sample_weight + 1`). Reader (downstream):
   `crates/bevy_naadf/src/assets/shaders/naadf_final.wgsl:54-58` (the final
   blit divides RGB by `max(1, weight)` for screen output) AND
   `crates/bevy_naadf/src/assets/shaders/spatial_resampling.wgsl:652-664`
   (the denoise path reads `cur_taa_color` for the bilateral pre-pass).
   **Composite secondary buffer; its contents are determined entirely by the
   hash-reject behaviour of `reproject_old_samples`.**

3. **`taa_dist_min_max`** (`pixel_count × vec2<u32>`). Per-pixel
   `(distMin | distMax<<16, valid_normals_spec)`. Writer:
   `crates/bevy_naadf/src/assets/shaders/taa.wgsl:337-338` (the reproject
   pass's 3×3 neighbourhood precompute). Readers:
   `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:444-465` (the
   ReSTIR-GI reprojection-validity test) — `valid_dist_cur` ∈
   `[distMin*1022/1024, distMax*1026/1024]`. **Per-frame, not history-bearing
   directly** — but the GI sample reproject reads it as a current-frame
   distance gate.

4. **`valid_samples`** (`pixel_count * 2 × SampleValid`, wrapping ring).
   ReSTIR lit-sample ring. Writer:
   `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:518-525`. Reader:
   `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:341-468`
   (`reproject_sample` → bucketing). Each stored sample carries the WRITING
   frame's `pixel_pos_old` and `taa_index_old` — the reproject pass uses
   `camera_history[frame_index_old].cam_pos_from_cur_int` (binding 10) to
   reconstruct the surface-virtual-pos and project it into the current
   screen. **History-bearing across the configured GI accumulation window
   (~64 past frames).**

5. **`invalid_samples`** (`pixel_count * 8 × vec4<u32>`, wrapping ring).
   ReSTIR unlit-sample ring. Same lifecycle as `valid_samples` — written by
   `naadf_global_illum.wgsl:530-533`, read by
   `sample_refine.wgsl::count_invalid_data`. Same reprojection mechanism.

6. **`sample_counts`** (`128 + 3 × SampleCountSlot{atomic<u32>, atomic<u32>}`).
   The 128-frame lit/unlit-count accumulation ring. Writers (atomic add):
   `crates/bevy_naadf/src/assets/shaders/naadf_global_illum.wgsl:483-489`.
   Reader: `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:264-275`
   (`compute_valid_history` walks the ring back from `accum_index`).
   **History-bearing across up to `sample_max_accum` frames; not pixel-bound,
   not coordinate-bound** — counts only.

7. **`bucket_info`** (`bucket_count × BucketInfoSlot{atomic<u32>, u32}`).
   Reset per frame at
   `crates/bevy_naadf/src/assets/shaders/sample_refine.wgsl:201-204`. Not
   history-bearing.

8. **`valid_samples_refined`** / **`valid_samples_compressed`**. Per-frame
   working buffers — populated freshly each frame by `count_valid_data_and_refine`
   / `refine_buckets`. Not history-bearing.

9. **`denoise_preprocessed`** / **`denoise_preprocessed_horizontal`**. The
   separable bilateral denoiser's per-frame scratch — written by
   `spatial_resampling.wgsl::calc_spatial_resampling` (denoise path), read by
   `denoise_split.wgsl::calc_denoise_*`. Per-frame, not history-bearing
   directly. **But** the colour `denoise_preprocessed` stores includes
   `cur_taa_color` (read from `taa_sample_accum`, line 652) and `taa_weight`
   (read from `taa_sample_accum`, line 88 of `denoise_split.wgsl`), so the
   denoiser's bilateral weights are determined by `taa_sample_accum`'s
   accumulated weight. **The denoiser is a propagator of the TAA-accum
   state, not a separate history.**

10. **`camera_history`** (`128 × GpuCameraHistorySlot`). Per-frame `view_proj`,
    `view_proj_inv`, `cam_pos_from_cur_int`, `jitter`. Writer (every frame):
    `crates/bevy_naadf/src/render/taa.rs:404-419` (`prepare_taa` rebuilds the
    ring relative to the CURRENT frame's `current_pos`). Readers:
    `taa.wgsl::reproject_old_samples` AND `sample_refine.wgsl::reproject_sample`.
    **THE coordinate-frame-bearing buffer.** This is the buffer where the
    origin-shift bug surfaces.

11. **`atmosphere_comp`** (octahedral sky precompute). 1/4 updated per frame
    at `naadf_atmosphere.wgsl`. Per-frame quarter-stride, not pixel-bound,
    not coordinate-bound to camera position (uses ray direction only).
    Irrelevant.

12. **`first_hit_absorption`** / **`first_hit_data`** / **`final_color`**
    (Phase-B G-buffer + working colour). All written fresh per frame by
    `naadf_first_hit.wgsl::calc_first_hit`. Not history-bearing.

**Verdict on the map**: only #1 (`taa_samples`), #2 (`taa_sample_accum`, a
derived secondary), #4–6 (the ReSTIR rings + sample-count ring), and #10
(`camera_history`) cross frame boundaries. Of those, #1 + #4 + #5 all
**reproject** past samples into the current frame via #10's
`cam_pos_from_cur_int`. **The reproject geometry depends on
`cam_pos_from_cur_int` being correct.** It isn't, under the streaming
preset's origin shifts — see `## Most likely root cause`.


## Hypothesis verdicts

### H1 — The hash-reject path isn't load-bearing where we think
**Ruled out as the cause of the artefact, but with one structural caveat.**

The 9-iteration loop at `taa.wgsl:269-327` precomputes `valid_hash_center`
(line 322-323) + 8 neighbour hashes in `valid_hashes_comp[]` (line 325). The
reproject loop at `taa.wgsl:419-428` IS the only history-blend path: if a
past sample fails the dist test (`taa.wgsl:387-391`) or the screen-pos test
(`taa.wgsl:400-408`) or the hash test (`taa.wgsl:419-428`), it is `continue`d
and never contributes to `color_sum`. There is NO default-blend fallback.
After the loop, `color_sum.a` is the count of accepted history samples, and
`taa_sample_accum[pixel_index]` is OVERWRITTEN with that count and the colour
sum (`taa.wgsl:447-450`) — so an all-rejected pixel reads back as
`(weight=0, R=0, G=0, B=0)`. The next pass `calc_new_taa_sample` then folds
in this frame's `light` with `sample_weight + 1 = 1`
(`taa.wgsl:533-542`). The final blit divides RGB by `max(1, weight)` =
`max(1, 1) = 1` (`naadf_final.wgsl:58`), so the screen shows just the
current frame's raw GI result with no temporal averaging.

This is **exactly the visual signature the user describes**: a noise burst on
the shift frame that "decays over a handful of frames" as new samples
accumulate (`weight = 1 → 2 → 3 → …`). The hash-reject path IS load-bearing.
The question is "is the hash test the only mechanism that could reject post-
shift samples?" — and the answer is NO (see H3/most-likely below): the
**dist** test and the **screen-position** test ALSO reject past samples whose
`cam_pos_from_cur_int` was computed in a stale coordinate frame, because the
reprojected `screen_pos_new` lands on the wrong pixel and/or
`dist_cur = distance(old_virtual_pos, vec3<f32>(0,0,0))` lands outside the
`dist_min_max ± 0.2%` envelope.

**Structural caveat**: the 8-neighbour hash fallback at `taa.wgsl:421-424`
became broken after the data_id_lo13 extension. The neighbour-hash
precompute at `taa.wgsl:281-282` calls `get_hit_data_from_planes(cur_first_hit,
…, ray_dir)` using the **centre pixel's `ray_dir`** (line 245), not the
neighbour pixel's own ray. Pre-fix the hash didn't depend on the
reconstructed `pos`, so this asymmetric use of `ray_dir` was benign; post-
fix the hash IS derived from `pos`, so the neighbour entries in
`valid_hashes_comp[]` are now derived from "the centre's ray extended to
the neighbour's hit plane" — a world voxel that does NOT correspond to the
neighbour's actual hit voxel. This makes the 8-neighbour fallback
near-useless after the fix (collision rate ~1/8192 instead of representing
the actual neighbour cells). However, this is a *secondary regression* of
the fix, not the artefact's root cause — pre-fix the artefact already
existed without the neighbour-hash being meaningful (the constant-hash
regime made the whole reject a no-op for all-diffuse pixels). Worth a
follow-up fix; not the load-bearing thing.

### H2 — The artifact isn't TAA reprojection at all
**Ruled out as the sole cause, but TAA is not the only affected system.**

There is no SVGF / a-trous / variance-buffer denoiser in this renderer
(`naadf_first_hit.wgsl:1-30` header comment notes `02-research.md` divergence
#11 — NAADF ships only the sparse separable bilateral denoise in
`denoise_split.wgsl`, no variance/moments history). The denoise pass IS
single-frame; it reads `denoise_preprocessed` (written this frame by
`spatial_resampling.wgsl`) and the `taa_weight` field of `taa_sample_accum`
(line 88 of `denoise_split.wgsl`). So the denoiser is not a separate
history-accumulating system — its inputs come from the per-frame spatial
pass + the per-frame TAA accum buffer.

There is no probe / DDGI / irradiance-volume history. There is no per-pixel
moments buffer. The atmosphere precompute is per-frame quarter-stride but
keyed on ray direction (not camera position), so origin shifts do not
affect it.

**Other history-accumulating systems**: the 128-frame ReSTIR-GI
`sample_counts` ring + the wrapping `valid_samples` / `invalid_samples`
rings (pipeline-map items 4-6). These ALSO go through frame-to-frame
reprojection via `camera_history[frame_index_old].cam_pos_from_cur_int`
(`sample_refine.wgsl:367-368`). They suffer from the SAME origin-shift
coordinate-frame bug as TAA — the reprojected `surface_pos_virtual` is off
by `(origin_shift_segments) * SEGMENT_VOXELS = N × 256` voxels, the
`taa_dist_min_max` test at `sample_refine.wgsl:461-465` rejects samples
whose `dist_cur` falls outside `dist_min_max * (1022..1026)/1024` (a 0.2%
tolerance) — and a 256-voxel mis-projection well exceeds 0.2% of any
reasonable hit distance. **So origin shifts also wipe the ReSTIR-GI
reservoir contributions for several frames** until the camera-history ring
fills with new (post-shift) entries.

So the artefact is **both** TAA AND ReSTIR-GI suffering the same root
cause. They surface together because they share `camera_history` (item 10
in the map).

### H3 — The hash IS varying, but encode/decode is wrong
**Ruled out — the encode/decode IS symmetric.**

Encode site: `taa_compress_sample` packs the 16-bit hash into
`sample_comp.x` at `taa_common.wgsl:131` —
`sample_comp.x = dist_comp | ((hash & 0xFFFFu) << 16u)`. The `& 0xFFFFu`
masks the hash to 16 bits before shifting; `dist_comp` occupies bits 0-15.

Decode site: `taa_decompress_sample` reads `s.hash = sample_comp.x >> 16u`
at `taa_common.wgsl:156`. WGSL `u32 >> 16u` produces a 16-bit value in bits
0-15 of the u32 (the high bits are zero).

Compare site: `taa.wgsl:419` does `s.hash != valid_hash_center`, and
`valid_hash_center = taa_hash_from_data(...) & 0xFFFFu` at `taa.wgsl:321`.
Both sides are 16-bit, both written and read by the same `& 0xFFFFu`
convention. The 8-neighbour fallback at `taa.wgsl:423` also masks with
`& 0xFFFFu`. **No sign-extension, no precision step, no width mismatch.**

The `pcg_hash` function used by iteration-2's helper (at
`ray_tracing_common.wgsl:19-23`) is straight u32 arithmetic — no
implicit conversions, deterministic across frames given the same `u32`
input. The bit-cast `u32(<i32>)` in
`taa.wgsl:217 — pcg_hash(u32(voxel_pos.x))` etc is well-defined per WGSL
spec (two's-complement preserving), and the `floor(first_hit_pos +
vec3<f32>(cam_pos_int))` → `vec3<i32>(…)` chain is the same code at both
read site (taa.wgsl:317) and write site (taa.wgsl:517), going through
the same `taa_data_id_lo13` helper. Same inputs → same output, no skew.

### H4 — `params.cam_pos_int` vs `cnts_params.cam_pos_int` frame-skew
**Ruled out — they are the same buffer.**

Both `params` (at `taa.wgsl:149`) and `cnts_params` (at `taa.wgsl:163`)
bind the SAME `taa_gpu.taa_params` GPU buffer — verified at
`crates/bevy_naadf/src/render/prepare.rs:910-922` (`taa_reproject_bind_group`,
entry 0 = `taa_gpu.taa_params`) AND
`crates/bevy_naadf/src/render/prepare.rs:928-939`
(`calc_new_taa_sample_bind_group`, entry 0 = `taa_gpu.taa_params`).

The uploader is `prepare_taa` (`render/taa.rs:421-442`) — it writes
`GpuTaaParams` (including `cam_pos_int: current_pos.pos_int`) once per
frame BEFORE any compute dispatch this frame. Both passes within a single
frame read from the same memory; no GPU-side mutation between them. **The
two `cam_pos_int` reads are bit-identical within any one frame.**


## Most likely root cause

**The streaming preset's origin shift invalidates the entire 32-frame TAA
camera-history ring AND the 64-frame ReSTIR-GI sample ring at the same
instant, because the renderer never reconciles past-frame positions stored
in the OLD-window-local coordinate frame into the NEW-window-local frame.**

The mechanism, traced to specific files / lines:

1. **`Camera.Transform.translation` is window-local, not world-absolute.**
   `crates/bevy_naadf/src/streaming/camera.rs:177-181`
   (`track_and_pin_camera`) re-pins `Transform.translation` to
   `abs_pos.window_local(origin)` = `abs_pos.to_world() - origin * 256`
   each tick. On an origin-shift frame, the Transform JUMPS by
   `-(new_origin - old_origin) * 256` voxels (per axis).

2. **`PositionSplit` is derived from `Transform.translation`.**
   `crates/bevy_naadf/src/camera/position_split.rs:117-119`
   (`sync_position_split`) writes
   `PositionSplit::from_world(transform.translation)`. So `PositionSplit`
   IS window-local. On origin shift it also jumps by
   `-(new_origin - old_origin) * 256`.

3. **`CameraHistory.positions[i]` stores the WINDOW-LOCAL
   `PositionSplit` of frame i.** `crates/bevy_naadf/src/render/taa.rs:214-215`
   (`update_camera_history`) writes
   `history.positions[slot] = *position_split` at slot `taa_index`. There
   is no system that re-expresses old entries in a new window-local frame
   when origin shifts.

4. **`prepare_taa` uploads `cam_pos_from_cur_int =
   (positions[i] - current_pos).to_world()`.**
   `crates/bevy_naadf/src/render/taa.rs:395-413` does this every frame
   relative to the CURRENT `current_pos`. After an origin shift at frame K:
   * `positions[K]` is in NEW-window-local (just written this frame).
   * `positions[K-1], positions[K-2], …` are in OLD-window-local (written
     before the shift).
   * `current_pos` is in NEW-window-local.
   * `(positions[K-1] - current_pos).to_world()` evaluates to
     `(old_window_local_pos[K-1]) - (new_window_local_pos[K])` =
     `(abs[K-1] + old_origin*256) - (abs[K] + new_origin*256)` =
     `(abs[K-1] - abs[K]) + (old_origin - new_origin) * 256`. **Off by
     `(old_origin - new_origin) * 256` voxels per axis.**

5. **TAA reproject pass uses this wrong `cam_pos_from_cur_int`.**
   `taa.wgsl:364`:
   `let reproject_pos = cur_pos_virtual - slot.cam_pos_from_cur_int;`. If
   `cam_pos_from_cur_int` is off by 256 voxels, `reproject_pos` is off by
   256 voxels — the projection at `taa.wgsl:365-367`
   (`get_screen_index_projection(…, slot.view_proj, …)`) lands on the wrong
   pixel (or off-screen, which `if (!proj.valid) continue;` rejects at line
   368-370).

6. **Even when the wrong pixel passes the screen-pos test**, the dist test
   at `taa.wgsl:387-391` likely rejects: `dist_cur` is computed from the
   `old_virtual_pos` reconstructed via the wrong `cam_pos_from_cur_int`,
   and `dist_min_max` is the current pixel's hit-distance envelope (0.2%
   tolerance) — a 256-voxel offset in world position generally exceeds 0.2%
   of any plausible hit distance.

7. **`s.hash != valid_hash_center` also fires** (the post-fix hash IS
   position-dependent, and the position differs between old-window-local
   and new-window-local frames for the same world voxel) — but this is the
   THIRD reject mechanism stacked on top, NOT the load-bearing one. The
   prior-iteration hash fixes had no visible effect specifically because
   the dist+screen rejects had already fired before the hash test ran.

8. **The same `cam_pos_from_cur_int` is consumed by ReSTIR-GI reprojection**
   at `sample_refine.wgsl:367-368`:
   ```wgsl
   var cam_pos_old_frac = cam_pos_frac + camera_history[frame_index_old].cam_pos_from_cur_int;
   let cam_pos_old_int = cam_pos_int + vec3<i32>(floor(cam_pos_old_frac));
   ```
   The reconstructed `cam_pos_old_int` is wrong by the origin shift; the
   subsequent `get_hit_data_from_planes(first_hit_packed, cam_pos_old_int, …)`
   produces a `surface_pos_virtual` that, when projected, lands at the
   wrong screen bucket and fails the `taa_dist_min_max` dist test at
   `sample_refine.wgsl:461-465`. So the GI **bounce reservoir is also
   wiped for several frames after each origin shift** — the "shadowed
   regions briefly fill with noisy splotches" matches this: shadowed
   regions get their light from indirect bounce (the ReSTIR reservoir),
   and when that reservoir empties they drop to direct-sun-only +
   denoise-noise until new samples accumulate.

The two-iteration hash refinement effort (`05-impl-taa-hash-world-identity.md`)
addressed the third reject mechanism (the hash test) but the dist and
screen-pos tests were already firing first — so neither pass moved the
visual. **The fix was on the wrong reject layer.**


### Secondary-cause ranking (in case the structural fix doesn't fully clear the artefact)

A. **Structural cause** (above) — origin-shift coordinate-frame mismatch in
   `camera_history.positions[]` / `cam_pos_from_cur_int`. Affects TAA +
   ReSTIR-GI simultaneously. **Highest confidence**, evidence is line-grounded
   and the streaming code's own commentary at
   `crates/bevy_naadf/src/streaming/camera.rs:7-23` explicitly acknowledges
   the window-local/world-absolute split exists.

B. **Hash-fix introduced its own regression** — the 8-neighbour fallback at
   `taa.wgsl:421-424` is now near-useless because the neighbour precompute
   at line 281-283 uses the centre's `ray_dir` to reconstruct the
   neighbour's pos, and `data_id_lo13` depends on `pos`. Pre-fix the hash
   didn't depend on pos so this asymmetry was benign. Worth fixing
   regardless of (A), but is a SECOND-ORDER effect: even with the neighbour
   fallback fixed, (A) still wipes the whole history on origin shift.

C. **`taa_data_id_lo13` uses window-local-frame coordinates** — at both
   `taa.wgsl:317` (read) and `taa.wgsl:517` (write), the input is
   `first_hit_result.pos + vec3<f32>(cam_pos_int)`, which reconstructs the
   WINDOW-LOCAL hit position, not the world-absolute one. Same physical
   world voxel gets DIFFERENT `data_id_lo13` IDs before and after an
   origin shift (because the window-local coordinate of that voxel
   changed). So even if (A) were fixed in `cam_pos_from_cur_int`, the
   hash itself would still mismatch across origin shifts. This is a
   third-order effect — independently fixable by adding the absolute-
   world offset (i.e. add `residency.origin * SEGMENT_VOXELS` before
   hashing), but the comment in `taa_common.wgsl:54-57` claims the
   derivation is "world-anchored" — which is **wrong**; it is in fact
   window-local-anchored.


## Recommended next action

**STOP trying to fix the hash.** The hash is a third-line reject behind
two other rejects (screen-pos at `taa.wgsl:400-408` and dist at
`taa.wgsl:387-391`) that are already firing on every origin-shifted sample.
The hash improvements are real but they cannot move the artefact because the
earlier rejects already eliminated the samples the hash would have tested.

**Instrument-first** — before any more shader edits, capture analytical
evidence that the origin-shift is the cause:

1. Read-only diagnostic: at the next origin shift, log the difference
   between `camera_history.positions[taa_index]` (current frame, NEW
   window-local) and `camera_history.positions[taa_index_prev]` (the
   frame just before the shift, OLD window-local). If the diff in
   `.pos_int` is approximately `(new_origin - old_origin) * 256`, the
   structural cause is confirmed. This is a one-line add in
   `crates/bevy_naadf/src/streaming/residency.rs` (around the existing
   `streaming-world residency shift:` `info!` at line 676-689) or a new
   `info!` at `crates/bevy_naadf/src/render/taa.rs:223` gated on
   `extracted_history.frame_count >= 1 && residency.origin_changed_this_frame`.

2. **Build a new e2e gate** that DOES exercise the origin-shift path with
   per-pixel before/after capture. The existing `streaming-window` gate
   (`crates/bevy_naadf/src/e2e/streaming_window.rs:770-855`) checks that
   an origin shift HAPPENS but does not measure the post-shift TAA
   transient. A new gate `--gate streaming-taa-shift-noise` would: drive
   the camera until origin shifts, capture frame N (the shift frame) +
   frames N+1, N+2, N+3, compute the per-pixel variance in shadowed-band
   pixels (luminance < 0.1), assert variance(N) > 3× variance(N+5). This
   would: (a) PASS pre-fix (the artefact exists), (b) FAIL after the
   structural fix lands, (c) become the regression-detection gate.

3. **The structural fix itself**: on every origin shift, re-express every
   entry in `CameraHistory.positions[]` from old-window-local to
   new-window-local by adding `(old_origin - new_origin) * SEGMENT_VOXELS`
   as a `PositionSplit` to each. The right loci:
   * `crates/bevy_naadf/src/render/taa.rs` (the `CameraHistory` resource)
     gains a `rebase_for_origin_shift(delta_segments: IVec3)` method that
     adds `delta_segments * SEGMENT_VOXELS` (as an `IVec3` in `.pos_int`)
     to every entry of `positions[..]`.
   * It is called from a system that runs **after**
     `residency_driver` (`streaming/residency.rs:508`) detects an origin
     shift and **before** `update_camera_history` writes this frame's
     slot. The system reads the origin delta from `Residency` (a new
     `origin_change_this_frame: Option<IVec3>` field, populated by
     `residency_driver`'s `set_origin` call site at line 561-565).
   * Symmetrically, the ReSTIR-GI's `valid_samples` ring carries
     `pixel_pos_old` (line 519, lit-sample compress at
     `naadf_global_illum.wgsl:148-149`) but NOT a window-local-absolute
     reference, so those samples are also implicitly tied to the
     window-local frame at write time. They are only USED via the
     `camera_history` reproject — so if `camera_history` is rebased
     correctly, the GI samples will reproject correctly too. **One fix,
     two systems healed.**

4. **Only after (3) lands**, re-evaluate whether the data_id_lo13 hash
   needs to be rolled back, kept as-is, or made world-absolute. With the
   structural fix in place, the hash's value goes from "providing
   negative value (over-rejection on origin shift)" to "providing the
   intended positive value (catching same-pixel world-data swaps from
   voxel edits)". Probably keep iteration-2's pcg_hash, but document
   that it derives a window-local-anchored ID (which is still distinct
   per voxel within a single window-local frame, just not world-absolute).
   For full robustness against the **`oasis-edit-visual`** case, the hash
   should be made world-absolute by adding `residency.origin *
   SEGMENT_VOXELS` (or zero in the non-streaming preset). That is a
   small follow-up; the structural fix is the critical one.

**Do NOT** add a third packing iteration to the hash. The hash is not
where the artefact lives.


## Out-of-scope observations

1. **The 8-neighbour hash fallback is broken post-fix.** The neighbour
   pre-compute at `taa.wgsl:281-283` uses the CENTRE pixel's `ray_dir`
   (line 245) to reconstruct neighbour positions. Pre-fix the hash didn't
   depend on `pos`, so this was benign. Post-fix the hash IS derived from
   `pos`, so neighbour hashes don't correspond to actual neighbour world
   voxels — the fallback becomes effectively random (~1/8192 hit rate
   instead of representing neighbour cells). Independent of the structural
   bug; should be fixed by recomputing each neighbour's own `ray_dir` via
   `get_ray_dir(params.inv_view_proj, cur_pixel_pos, …)` inside the loop,
   or by deciding the fallback isn't worth fixing and removing it.

2. **`taa_common.wgsl:54-57` claims `data_id_lo13` is "world-anchored"** —
   the comment says "13-bit world-anchored voxel-cell discriminator". This
   is misleading: the helper at `taa.wgsl:215-225` adds `cam_pos_int`
   (which is `params.cam_pos_int` = window-local), so the discriminator
   is actually WINDOW-LOCAL-anchored. Comment is wrong. Same factual
   issue in `taa.wgsl:185-192`'s file-internal helper doc.

3. **`prepare_taa`'s `PositionSplit` subtraction at `taa.rs:405`** is
   advertised as "the D1 trick — keeps it precise for large worlds". But
   under the streaming preset, the int+frac split is in window-local
   coords, not world-absolute. The precision argument still holds for the
   subtraction itself (operating on window-local int+frac is just as
   precise as on world-absolute int+frac); but the result is
   coordinate-frame-incoherent across origin shifts. Worth a comment
   amendment in the file's docs.

4. **The TAA reproject pass's `cam_pos_int` lookup at `taa.wgsl:235`**
   `let cam_pos_int = params.cam_pos_int.xyz` — same window-local value.
   If the structural fix later opts to do reprojection in world-absolute
   coords (rather than rebasing history on shift), all consumer sites
   would need to compose `cam_pos_int + residency_origin * SEGMENT_VOXELS`.
   The rebase-on-shift approach is far less invasive — it modifies one
   buffer at one moment, vs. amending every shader reference.

5. **The existing `streaming-window` e2e gate at
   `crates/bevy_naadf/src/e2e/streaming_window.rs`** measures luminance
   variance and origin-shift-X count but does NOT analytically measure
   the post-shift TAA transient described in the brief. Per the user's
   memory `feedback-primitives-then-analytical-invariants.md`, this gap
   is exactly why two hash iterations passed all gates and didn't move
   the artefact — the gates lack the analytical surface the artefact
   lives in. A new gate is in scope (recommended action #2).

6. **`spatial_resampling.wgsl:652-664`** reads `taa_sample_accum` to compute
   `cur_taa_color` for the denoise pre-pass. After an origin-shift frame
   where TAA accum is wiped (weight=0 after the reproject pass), the
   denoise path falls into the `accum <= 1.0` branch and `cur_taa_color =
   color` (the freshly-sampled colour). The bilateral weighting then
   loses its TAA-stabilised reference colour and the denoise pass
   produces a noisier result. This is a SECOND artefact compounding the
   structural one — but it goes away once `taa_sample_accum` recovers
   (1-2 frames after the shift), consistent with the observed "decay over
   a few frames" pattern.
