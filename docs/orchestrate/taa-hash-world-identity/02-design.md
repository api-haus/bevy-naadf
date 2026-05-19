# 02 — Phase K design: structural fix for streaming-world TAA shift artefact

Architect: distributed dispatch (Phase K).
Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`.
Branch: `feat/streaming-world` @ `8853f60` + uncommitted instrumentation log
at `crates/bevy_naadf/src/render/taa.rs:253`.

## Goal restated

The streaming preset's user-visible blink artefact (noisy splotches in
shadowed regions for ~3 frames after every origin shift) is NOT a TAA hash
collision. The diagnostic at `06-diagnostic-investigation.md` proved that
`CameraHistory.positions[]` stores window-local `PositionSplit` values and
jumps by `±256` voxels per axis on every residency origin shift, breaking
`cam_pos_from_cur_int` deltas across the entire 32-frame TAA ring AND the
64-frame ReSTIR-GI sample ring at the same instant. The screen-pos reject
(`taa.wgsl:400-408`) and the 0.2% dist reject (`taa.wgsl:387-391`) then fire
on every post-shift history sample; the hash reject (`taa.wgsl:419-428`) is
a third-line reject that the previous two iterations had already attacked
and could never have moved the artefact.

This design lands the **structural fix** (rebase
`CameraHistory.positions[]` on origin shift) plus two **secondary fixes**
flagged by the diagnostic (8-neighbour hash fallback at `taa.wgsl:281-283`
broken post-iteration-2 because the hash is now position-dependent;
`taa_data_id_lo13` mislabelled "world-anchored" but actually
window-local-anchored) plus a new **`--gate streaming-taa-shift-noise`
analytical e2e gate** that captures the shift transient (shadowed-band
per-pixel variance) and MUST FAIL pre-fix / PASS post-fix.

## Design overview

Five coordinated changes, all small and localised:

```
                main-world Update
                ─────────────────
PreUpdate                  Update
─────────                  ──────
residency_driver           track_and_pin_camera (writes Transform)
  │                          │
  ├──► sets new origin       └─► sync_position_split (writes PositionSplit)
  │                                  │
  │                                  └─► update_camera_history
  │                                        │
  │                                        ├─ (NEW) detect origin-shift via
  │                                        │   Res<Residency>::origin() delta
  │                                        │
  │                                        ├─ (NEW) IF shifted: rebase all
  │                                        │   `positions[..]` by adding
  │                                        │   delta_seg * SEGMENT_VOXELS
  │                                        │   to .pos_int (frac untouched)
  │                                        │
  │                                        └─ writes positions[slot] = current
  └──► extracted via Res<Residency>::origin()
       in render world (already plumbed as
       `StreamingExtractRender.window_origin`)

                render-world ExtractSchedule + PrepareResources
                ───────────────────────────────────────────────
extract_camera_history (unchanged — already mirrors positions[..])
  │
  └─► prepare_taa
        ├─ uploads camera_history slots (cam_pos_from_cur_int now coherent
        │  across the just-shifted frame because positions[..] are all
        │  in the NEW window-local frame post-rebase)
        └─ (NEW) writes `residency_origin_voxels = origin * SEGMENT_VOXELS`
           into GpuTaaParams trailing-padding slot (re-purposes existing
           _pad2/_pad3/_pad4 — no struct widening, no bind-group changes)

                taa.wgsl
                ────────
taa_data_id_lo13 (existing helper)
  ├─ (CHANGED) composes residency_origin_voxels into the world-absolute
  │  reference so the same world voxel hashes the same across origin shifts
  │
reproject_old_samples 3×3 precompute (lines 269-327)
  └─ (CHANGED) each neighbour now gets its own ray_dir via
     get_ray_dir(params.inv_view_proj, cur_pixel_pos, (0,0)) inside the loop
     — the centre ray_dir at line 245 is no longer (mis-)reused for
     neighbour pos reconstruction (it's still used for `pos_virtual` at
     line 346, which IS centre-only)
```

The new e2e gate `streaming-taa-shift-noise` walks the camera through a
streaming-procedural world, captures the frame at the origin-shift event
plus the next 4 frames plus a baseline frame post-recovery, computes
per-pixel temporal variance in shadowed-band pixels (luminance < 0.1) over
the transient window vs the baseline, and asserts `var_transient <
threshold × var_baseline`. Pre-fix the artefact lights up that ratio well
above any plausible threshold; post-fix the ratio settles near 1.

---

## Design — structural rebase

### Where + when

The rebase logic lives **inside** `update_camera_history`
(`crates/bevy_naadf/src/render/taa.rs:188-299`), NOT as a new system. The
diagnostic's "Recommended next action" item 3 suggested a separate system
running between `residency_driver` and `update_camera_history`, but
folding it into `update_camera_history` is strictly simpler and avoids
the schedule-ordering question entirely:

- `residency_driver` runs in `PreUpdate` (`streaming/mod.rs:268-274`) and
  sets the new origin via `WindowedSlotMap::set_origin(new_origin, …)`
  (`streaming/residency.rs:585-592`).
- `track_and_pin_camera` runs in `Update` (`streaming/mod.rs:284-297`) and
  re-pins `Transform.translation` to the new window-local frame.
- `sync_position_split` runs in `Update` after `track_and_pin_camera`
  (`streaming/camera.rs:114-119` → `lib.rs:850-857`).
- `update_camera_history` runs in `Update` `.after(sync_position_split)`
  (`lib.rs:898-901`). It already takes `Option<Res<Residency>>` as a
  parameter (`render/taa.rs:192`) so the resource is already in the
  signature.

By the time `update_camera_history` runs, the just-rebound `position_split`
is *already* in the NEW window-local frame (because `track_and_pin_camera`
re-pinned Transform using the NEW origin). So **the slot the system is
about to write this frame is already correct**. The rebase must apply ONLY
to the entries from PREVIOUS frames — they were written in the OLD
window-local frame.

### Detection mechanism

A new `Local<LastOriginSeen>` on `update_camera_history` (wrapping
`Option<IVec3>`) records the residency origin observed last frame. At entry
time:

```rust
let current_origin = residency.as_deref().map(|r| r.origin()).unwrap_or(IVec3::ZERO);
let last_origin = last_origin_seen.0.unwrap_or(current_origin);
let delta_segments = current_origin - last_origin;
last_origin_seen.0 = Some(current_origin);
```

If `delta_segments != IVec3::ZERO`, an origin shift occurred this frame.
Bevy's `Local<T>` parameter is per-system state, so it survives across
ticks without polluting any cross-world resource.

This intentionally does NOT consume `admissions_this_frame.is_empty()` or
similar derived signals — `Residency::origin()` is the authoritative
source; reading it directly is the lowest-risk diff against the residency
module.

### The rebase call

```rust
if delta_segments != IVec3::ZERO {
    history.rebase_for_origin_shift(delta_segments);
    // (optional: keep the existing instrumentation `info!` line; see
    // §"Removing or keeping the instrumentation log" below.)
}
```

`CameraHistory::rebase_for_origin_shift(delta_segments: IVec3)` is a new
method on the resource (`render/taa.rs:65-94`):

```rust
impl CameraHistory {
    /// Re-express every entry of `positions[..]` from the OLD window-local
    /// frame into the NEW window-local frame after a residency origin
    /// shift. Adds `-delta_segments * SEGMENT_VOXELS` to each entry's
    /// `pos_int`, leaving `pos_frac` untouched.
    ///
    /// Sign rationale: when `residency.origin` advances by `delta_seg`
    /// segments, every absolute-world point gets a `-(delta_seg *
    /// SEGMENT_VOXELS)` voxel adjustment in window-local coords (the
    /// origin moved +N, so a fixed world point's window-local x dropped
    /// by N). Re-expressing OLD window-local entries into the NEW window-
    /// local frame applies that same `-(delta_seg * SEGMENT_VOXELS)`
    /// adjustment. The instrumentation evidence at Phase J confirms this
    /// sign: each shift produced `delta_voxels ≈ −256` for `delta_seg = +1`.
    pub fn rebase_for_origin_shift(&mut self, delta_segments: IVec3) {
        let voxel_delta = -delta_segments * crate::streaming::residency::SEGMENT_VOXELS;
        for ps in self.positions.iter_mut() {
            ps.pos_int += voxel_delta;
            // ps.pos_frac intentionally untouched — voxel_delta is an
            // integer multiple of SEGMENT_VOXELS = 256, so the frac field
            // is already correct (the int part of the shift takes the
            // whole delta).
        }
    }
}
```

### Precision invariant

`PositionSplit` is the int+frac decomposition defined at
`crates/bevy_naadf/src/camera/position_split.rs:21-28`. `SEGMENT_VOXELS =
256` (`streaming/residency.rs:46`) is the unit of the rebase. Adding an
integer multiple of 256 to `.pos_int`:
- leaves `.pos_frac` unchanged (which is the precision-critical
  invariant — frac is normalised to `[0,1)³` per
  `position_split.rs:48-55`);
- requires no `normalise()` call (the resulting `(.pos_int, .pos_frac)`
  is already canonical).

The Rust integer addition is exact (no f32 path, no precision loss).
**This is why a vanilla `IVec3` addition on `.pos_int` is the right
operation** — using `+` on the full `PositionSplit` would route through
`Add for PositionSplit` at `position_split.rs:65-77`, which would
normalise and re-derive the int+frac split: also correct in this case
because the delta is integer, but the cleaner contract is "shifted by an
integer multiple → only `.pos_int` changes".

### WHICH entries get rebased

All `CAMERA_HISTORY_DEPTH = 128` slots in `positions[..]`
(`render/taa.rs:71`). Reasoning:
- The diagnostic shows the artefact spans the full 32-frame TAA ring
  (because every entry's `cam_pos_from_cur_int` is wrong by the shift
  delta) plus the 64-frame ReSTIR-GI ring (same buffer).
- Entries that were never written this run are `PositionSplit::default()`
  = `(IVec3::ZERO, Vec3::ZERO)` (`render/taa.rs:96-108`). Adding the
  delta to them is harmless: they are still being TAA-rejected by the
  `sample_age` walk bound (`taa.wgsl:349`), the dist test
  (`taa.wgsl:387-391` against the zero-distance `cam_pos_from_cur_int`),
  and the screen-pos test (`taa.wgsl:400-408` against an off-screen
  reprojected pos). Rebasing them produces a different-but-still-rejected
  position; no behaviour change.
- The slot **about to be overwritten this frame** (`positions[slot]` at
  `taa.rs:290`) is also rebased — but then immediately overwritten with
  `*position_split` (already in the NEW window-local frame). Order-of-
  operations: rebase ALL 128 slots, then overwrite slot `taa_index`. No
  special-casing required.

### File-by-file refs

- `crates/bevy_naadf/src/render/taa.rs:65-94` — `CameraHistory` resource:
  add `rebase_for_origin_shift` method as an `impl CameraHistory` block.
- `crates/bevy_naadf/src/render/taa.rs:188-299` — `update_camera_history`
  function: add `Local<LastOriginSeen>` parameter, compute
  `delta_segments`, call the rebase BEFORE the existing instrumentation
  block at lines 215-287 and BEFORE the writes at lines 290-298.

Rejected alternative: separate `Last`-stage system (running after
`residency_driver` but before `extract_camera_history`). Rejected because
(a) it adds a second system that needs schedule ordering vs the
instrumentation system AND `update_camera_history`, increasing surface for
ordering bugs; (b) `update_camera_history` already has the
`Option<Res<Residency>>` parameter so the data plumbing is already in
place; (c) the `Local<LastOriginSeen>` carries the per-system state
cleanly with no new resources.

---

## Design — cross-world wiring

**The structural rebase does NOT need cross-world wiring.** The rebase
happens entirely in the main-world `update_camera_history`. The render
world consumes the rebased values through the existing
`ExtractedCameraHistory.positions[..]` → `prepare_taa.cam_pos_from_cur_int`
path, which is byte-identical to the current pipeline.

The brief asked the architect to commit to one of three options for
cross-world wiring; the answer is **option zero**: no cross-world wiring
is needed because the rebase lands in main-world and the render-world
already mirrors the rebased state via `extract_camera_history`
(`render/extract.rs:370-385`).

### Rejected alternatives

1. **New field `Residency.origin_change_this_frame: Option<IVec3>` +
   `ExtractResource` propagation.** Rejected because it would require:
   - Modifying `Residency` (`streaming/residency.rs:66-120`) — a load-
     bearing core resource the diagnostic warns against touching ("no
     changes to streaming code" was the prior brief's discipline,
     superseded here only as needed; touching `Residency` is more
     invasive than necessary).
   - Adding a new extract path or extending `StreamingExtractRender`
     (`streaming/noise_dispatch.rs:245-328`).
   - Coordinating consumption between the main-world rebase (which needs
     to fire BEFORE `extract_camera_history`) and the render-world
     `prepare_taa` (which would also want to see the shift event).
   The main-world-only rebase eliminates this entire chain.

2. **A Bevy `Events<OriginShifted>` event + `ExtractedEvents` mirror.**
   Rejected because Bevy events have a 2-frame retention (the event is
   readable the same frame it's written + the next frame), which would
   require explicit drain discipline to avoid replaying the same shift
   across frames. `Local<LastOriginSeen>` gives one-shot semantics for
   free.

3. **A render-world-only resource that the render system reads.**
   Rejected because the rebase has to happen on the main-world
   `CameraHistory.positions[..]` BEFORE `extract_camera_history` copies
   them out. A render-world-only diff cannot rewrite values that have
   already been extracted.

### Schedule timing — why the chosen mechanism is sufficient

The `update_camera_history` rebase fires in the SAME frame as the origin
shift because:
- `residency_driver` runs in `PreUpdate` and sets the origin via
  `WindowedSlotMap::set_origin` (`streaming/residency.rs:585`).
- `Residency::origin()` (`streaming/residency.rs:173-175`) returns the
  current origin, including the just-set value.
- `update_camera_history` runs in `Update` AFTER `PreUpdate` finishes, so
  `Res<Residency>::origin()` already shows `new_origin`.
- The `Local<LastOriginSeen>` was populated last frame with the OLD
  origin. `current - last = delta_segments != 0` → rebase fires.

The rebase fires on the SAME tick as the shift; `extract_camera_history`
runs in `ExtractSchedule` AFTER `Update` finishes, so it copies the
already-rebased `positions[..]`. No frame lag.

---

## Design — hash world-absolute correction

The current `taa_data_id_lo13` helper at `taa.wgsl:215-225` derives:

```wgsl
let voxel_pos = vec3<i32>(floor(first_hit_pos + vec3<f32>(cam_pos_int)));
```

Where `cam_pos_int = params.cam_pos_int.xyz` (`taa.wgsl:235`) is the
WINDOW-LOCAL integer camera position. The same world voxel produces a
different `voxel_pos` before vs after an origin shift. The diagnostic
flagged this at item 2 of `## Out-of-scope observations` and item C of
`## Secondary-cause ranking`. Even if the structural rebase fully
eliminates the dist/screen-pos rejects, the hash itself would still
mismatch across shifts and mis-fire the third-line reject.

### Fix

Add an integer-voxel offset `residency_origin_voxels = residency.origin *
SEGMENT_VOXELS` to the hash derivation at both call sites. After the
addition, `voxel_pos` is the WORLD-ABSOLUTE integer voxel coord — the same
world voxel produces the same 13-bit ID regardless of which window-local
frame the renderer is currently in.

### Plumbing — re-use the existing `GpuTaaParams` trailing padding

`GpuTaaParams` (`render/gpu_types.rs:195-231`) has three trailing u32
padding fields `_pad2`, `_pad3`, `_pad4` (lines 226-230). These bring the
struct to its 192-byte aligned size (asserted at
`gpu_types.rs:874`). I will repurpose them as
`residency_origin_voxels: IVec3` (also 12 bytes — fits exactly) plus a
trailing `_pad: u32` to preserve the 16-byte stride.

Concretely:

```rust
// gpu_types.rs — GpuTaaParams (lines 195-231 ish, edit in place):
pub struct GpuTaaParams {
    pub inv_view_proj: Mat4,
    pub view_proj: Mat4,
    pub cam_pos_int: IVec3,
    pub _pad0: u32,
    pub cam_pos_frac: Vec3,
    pub _pad1: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub frame_count: u32,
    pub taa_index: u32,
    pub sample_age: u32,
    // ⇩ was `_pad2/_pad3/_pad4: u32` — repurposed for world-absolute hash.
    pub residency_origin_voxels: IVec3,  // residency.origin * SEGMENT_VOXELS
    pub _pad_tail: u32,                  // preserves the 16-byte stride
}
```

Size invariant: `_pad2 + _pad3 + _pad4 = 12 bytes` is replaced by `IVec3
(12 bytes) + _pad_tail (4 bytes)` — wait, that's 16 bytes, not 12. Recount:

Layout from line 215 to end of struct, byte offsets:
- `screen_width` at 160 (4)
- `screen_height` at 164 (4)
- `frame_count` at 168 (4)
- `taa_index` at 172 (4)
- `sample_age` at 176 (4)
- `_pad2` at 180 (4)
- `_pad3` at 184 (4)
- `_pad4` at 188 (4)
- end at 192.

So the trailing `_pad2/_pad3/_pad4` total 12 bytes, ending at 192. That
fits exactly `IVec3 (12)` if I drop the tail u32 idea. **But** WGSL/std140
requires `vec3<i32>` to be on a 16-byte boundary (offset 180 is NOT 16-
byte aligned — 180 % 16 = 4). I therefore need to shift the layout:

Option A — keep the 192-byte size, shift the IVec3 to start at offset 176
where `sample_age` currently lives, and move `sample_age` into the
trailing slot:

```rust
    pub frame_count: u32,
    pub taa_index: u32,
    pub sample_age: u32,
    pub _pad2: u32,                        // padding to 16-byte boundary
    pub residency_origin_voxels: IVec3,    // offset 176, ends at 188
    pub _pad_tail: u32,                    // offset 188, ends at 192
```

That places `residency_origin_voxels` at offset 176 (176 % 16 == 0, OK),
ends at 188; `_pad_tail` at offset 188 → struct ends at 192. **Size
unchanged.** No bind-group changes. The existing `_pad2` stays at offset
172 (`sample_age` ends at 176), `_pad3` and `_pad4` are absorbed into the
new `residency_origin_voxels` + `_pad_tail`. Net field count change: +1
(IVec3 replacing 2× u32 + new tail = same byte count).

Actually simpler — leave field order alone, just add the layout:

```rust
    pub sample_age: u32,           // offset 176, ends at 180
    pub _pad2: u32,                // offset 180, ends at 184 (kept for alignment)
    pub _pad3: u32,                // offset 184, ends at 188
    pub _pad4: u32,                // offset 188, ends at 192
```

The total trailing pad is 12 bytes from 180..192. An IVec3 placed at 180
would violate WGSL's 16-byte alignment for `vec3<i32>`. **Therefore the
field order MUST change.** Plan:

```rust
    // (everything up through sample_age unchanged)
    pub sample_age: u32,                    // offset 176
    pub _pad2: u32,                         // offset 180 (kept for 16-byte alignment of next field)
    // residency.origin in voxel units (residency.origin × SEGMENT_VOXELS).
    // Zero in non-streaming presets. Composed with `cam_pos_int` at the shader
    // hash call site to give a world-absolute voxel coordinate.
    pub residency_origin_voxels: IVec3,     // offset 184? NO — still misaligned.
```

`184 % 16 = 8`. Not 16-byte aligned.

Correct layout fixing both alignment and total size:

```rust
    pub screen_width: u32,                  // offset 160
    pub screen_height: u32,                 // offset 164
    pub frame_count: u32,                   // offset 168
    pub taa_index: u32,                     // offset 172, ends at 176 ← row boundary
    pub residency_origin_voxels: IVec3,     // offset 176 (16-byte aligned!), ends at 188
    pub sample_age: u32,                    // offset 188, ends at 192
```

Now the struct ends at 192 bytes — **identical size**. WGSL `vec3<i32>` at
offset 176 is 16-byte aligned (176 % 16 == 0). `sample_age` slides from
176 to 188 (still a single u32, just in a different slot). The trailing
`_pad2/_pad3/_pad4` u32s are absorbed/replaced.

The WGSL `GpuTaaParams` struct (`taa.wgsl:114-124`) needs the matching
reordering. Its `sample_age` is the last field (line 123), so the WGSL
declaration becomes:

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,
    cam_pos_frac: vec4<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    residency_origin_voxels: vec4<i32>,   // .xyz are voxels, .w is padding
    sample_age: u32,                      // u32 packs into the last 4 bytes of the 192B struct
    // (no `_pad{2,3,4}` declared in WGSL — they were Rust-side only)
}
```

**Wait — the existing WGSL `GpuTaaParams` has NO `_pad2/_pad3/_pad4`
fields.** The trailing padding was purely Rust-side. The WGSL had only
`sample_age` as the trailing field, with WGSL's implicit struct padding
handling alignment. Re-reading `taa.wgsl:114-124`:

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,
    cam_pos_frac: vec4<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    sample_age: u32,
}
```

That's 64 + 64 + 16 + 16 + 4×4 + 4 = 180 bytes from the WGSL viewpoint,
with std140 padding making it 192. So WGSL `sample_age` is at offset 176,
matching the Rust `sample_age` at 176. Now I declare:

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,
    cam_pos_frac: vec4<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    // residency_origin_voxels.xyz is `residency.origin × SEGMENT_VOXELS`,
    // .w is padding. Composed with `cam_pos_int.xyz` at the hash call site
    // to give a world-absolute voxel coordinate.
    residency_origin_voxels: vec4<i32>,
    sample_age: u32,
}
```

WGSL std140 aligns `vec4<i32>` to 16-byte boundaries → after `taa_index`
at offset 172 (ending at 176), `residency_origin_voxels` lands at 176
(aligned ✓), ends at 192. `sample_age` then lands at 192, with implicit
padding bringing the struct to 208. **That changes the struct size from
192 → 208 bytes.** That breaks the `gpu_types.rs:874` size assertion and
forces a wgpu buffer resize.

To keep 192 bytes, I must use the `vec3<i32>` form so std140 packs
`sample_age` into its trailing word:

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,
    cam_pos_frac: vec4<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,                       // offset 172, ends at 176
    residency_origin_voxels: vec3<i32>,   // offset 176 (16-byte aligned), 12 bytes
    sample_age: u32,                      // offset 188, ends at 192 — packs into vec3 tail
}
```

Per the `taa.wgsl:95-101` file comment, WGSL packs a scalar into a
trailing `vec3<f32>`'s 4-byte tail when no 16-byte-aligned field follows;
the same is true for `vec3<i32>`. So with the field order `vec3<i32>` →
`u32`, the `u32` lands at offset 188 inside the `vec3<i32>`'s 16-byte slot
and the struct stays 192 bytes.

**However** — that's the exact std140 pitfall the existing
`cam_pos_int`/`cam_pos_frac` fields work around (`taa.wgsl:95-101`) by
declaring them as `vec4<i32>`/`vec4<f32>` while the Rust struct has
explicit `_pad` u32s. The clean, drift-resistant declaration is the same:

```wgsl
    residency_origin_voxels: vec4<i32>,   // .xyz are voxels, .w is `sample_age`
```

Where `.w` then holds `sample_age` because the Rust struct packs them as:

```rust
    pub residency_origin_voxels: IVec3,   // offset 176, 12 bytes
    pub sample_age: u32,                  // offset 188, 4 bytes — fills the vec4 tail
```

Then the WGSL access becomes `params.residency_origin_voxels.xyz` for the
hash and `bitcast<u32>(params.residency_origin_voxels.w)` for sample_age
— but that means moving `sample_age` access from
`params.sample_age` to a bitcast. Ugly and a footgun.

**Cleanest answer**: declare both WGSL views differently:

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,
    cam_pos_frac: vec4<f32>,
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    // .xyz = residency.origin × SEGMENT_VOXELS (zero in non-streaming presets);
    // .w   = sample_age (declared as vec4<i32> so std140 keeps the parent struct at 192B —
    //        the same pattern as `cam_pos_int`/`cam_pos_frac` at lines 117-118).
    // Reading sample_age in WGSL: `u32(params.residency_origin_voxels.w)`.
    residency_origin_voxels: vec4<i32>,
}
```

And `sample_age` access in WGSL (`taa.wgsl:349` — the loop bound):
`u32(params.residency_origin_voxels.w)` → store this in a local
`let sample_age = u32(params.residency_origin_voxels.w);` near the top of
each entry point that reads it.

That's invasive at the WGSL call site (one extra local + the same access
in `calc_new_taa_sample` doesn't use sample_age, so only the reproject
pass needs it). Implementer should choose between this approach and the
"two struct fields side-by-side" approach below.

**Final recommendation — Option C: split into two separate `vec4<i32>`
fields**, matching the precedent of `cam_pos_int`/`cam_pos_frac` (each
declared `vec4` with the trailing slot used as `_pad`):

```rust
// gpu_types.rs (final):
pub struct GpuTaaParams {
    pub inv_view_proj: Mat4,
    pub view_proj: Mat4,
    pub cam_pos_int: IVec3,
    pub _pad0: u32,
    pub cam_pos_frac: Vec3,
    pub _pad1: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub frame_count: u32,
    pub taa_index: u32,
    pub residency_origin_voxels: IVec3,  // = residency.origin × SEGMENT_VOXELS
    pub sample_age: u32,                  // u32 packs into vec3 tail per std140
}
```

This is **structurally identical** to the existing
`cam_pos_int (IVec3) + _pad0 (u32)` pair (`gpu_types.rs:207-209`) and
`cam_pos_frac (Vec3) + _pad1 (u32)` pair (`gpu_types.rs:211-213`) —
except the trailing scalar carries a meaningful value (`sample_age`)
instead of padding. Rust offsets:

- `residency_origin_voxels`: offset 176, ends at 188.
- `sample_age`: offset 188, ends at 192.

Struct ends at 192 bytes ✓ (same size as before — assertion at
`gpu_types.rs:874` stays correct).

WGSL declares it the matching way (`taa.wgsl:114-124`):

```wgsl
struct GpuTaaParams {
    inv_view_proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    cam_pos_int: vec4<i32>,                 // .w = padding
    cam_pos_frac: vec4<f32>,                // .w = padding
    screen_width: u32,
    screen_height: u32,
    frame_count: u32,
    taa_index: u32,
    residency_origin_voxels: vec4<i32>,     // .xyz = voxels, .w = sample_age
}
```

And then in the shader:

```wgsl
let cam_pos_int = params.cam_pos_int.xyz;                          // unchanged
let residency_origin_voxels = params.residency_origin_voxels.xyz;  // NEW
let sample_age = u32(params.residency_origin_voxels.w);            // CHANGED — was params.sample_age
```

The only call site of `sample_age` is the reproject pass loop bound at
`taa.wgsl:349`: replace `params.sample_age` with the new local.

Three sites to edit in `taa.wgsl`:
1. The struct declaration (line 114-124).
2. The reproject-pass loop bound (line 349) — switch to the new local.
3. The hash helper `taa_data_id_lo13` (line 215-225) — add the
   `residency_origin_voxels` parameter.

And in both pass entry points, declare the local right after the
`cam_pos_int` declaration (line 235 for reproject, line 464 for
calc-new-taa).

### The hash helper signature change

```wgsl
fn taa_data_id_lo13(
    first_hit_pos: vec3<f32>,
    cam_pos_int: vec3<i32>,
    residency_origin_voxels: vec3<i32>,   // NEW
) -> u32 {
    // Compose to world-absolute integer voxel coord.
    let voxel_pos = vec3<i32>(floor(first_hit_pos + vec3<f32>(cam_pos_int + residency_origin_voxels)));
    var h: u32 = pcg_hash(u32(voxel_pos.x));
    h = pcg_hash(h ^ u32(voxel_pos.y));
    h = pcg_hash(h ^ u32(voxel_pos.z));
    return h & 0x1FFFu;
}
```

Both call sites at `taa.wgsl:317` (read in reproject) and `taa.wgsl:517`
(write in calc-new-taa) pass the same `residency_origin_voxels`. The
existing comment block at `taa.wgsl:175-225` is updated to reflect the
correctly world-absolute derivation; the misleading "world-anchored"
claim at lines 185-192 is rewritten.

The corresponding comment at `taa_common.wgsl:54-57` ("data_id_lo13 is
derived at both call sites via the canonical `taa_data_id_lo13(...)`
helper") is amended to mention that the derivation now composes
residency-origin offset.

### Non-streaming preset

In `prepare_taa` (`render/taa.rs:361-539`), the `residency_origin_voxels`
field is uploaded as:

```rust
let residency_origin_voxels = match extracted_residency_origin {
    Some(origin_segments) => origin_segments * crate::streaming::residency::SEGMENT_VOXELS,
    None => IVec3::ZERO,
};
```

Where `extracted_residency_origin: Option<IVec3>` comes from a new render-
world resource. The cleanest source for this value is the EXISTING
`StreamingExtractRender.window_origin: IVec3`
(`streaming/noise_dispatch.rs:274-277`) which is already mirrored to the
render world by `extract_streaming_state`. `prepare_taa` adds `Option<Res<
StreamingExtractRender>>` to its signature and reads `r.window_origin` if
present:

```rust
fn prepare_taa(
    // ... existing parameters ...
    streaming_extract: Option<Res<crate::streaming::StreamingExtractRender>>,
) {
    // ...
    let residency_origin_voxels = streaming_extract
        .as_deref()
        .map(|s| s.window_origin * crate::streaming::residency::SEGMENT_VOXELS)
        .unwrap_or(IVec3::ZERO);
    // ...
}
```

For the non-streaming presets `StreamingExtractRender` either does not
exist (no `StreamingPlugin` consumer touched it) — actually it always
exists once the plugin is built (`streaming/mod.rs:319-323`), but
`window_origin = IVec3::ZERO` is the default
(`streaming/noise_dispatch.rs:346`). So the value is a clean zero in
non-streaming runs and the hash derivation degenerates to the existing
behaviour.

### Read/write consistency

The reproject pass at `taa.wgsl:317` and the write pass at
`taa.wgsl:517` BOTH go through the same `taa_data_id_lo13(...)` helper,
which now takes `residency_origin_voxels` as a third parameter. The
helper is the single canonical derivation site. Both sites pass the
SAME value (the local read from `params.residency_origin_voxels.xyz` /
`cnts_params.residency_origin_voxels.xyz`), which are bit-identical
within a frame (same `taa_gpu.taa_params` buffer, ruled out at H4 in the
diagnostic).

---

## Design — 8-neighbour hash fallback fix

### The bug

The neighbour-hash precompute at `taa.wgsl:281-283`:

```wgsl
let cur_first_hit_result = get_hit_data_from_planes(
    cur_first_hit, cam_pos_int, cam_pos_frac, ray_dir,
);
```

is called once per `i = 0..9u` of the precompute loop (`taa.wgsl:269-327`).
The local `ray_dir` is computed ONCE before the loop at `taa.wgsl:245-247`
using the **centre pixel's pixel_pos**:

```wgsl
let ray_dir = get_ray_dir(
    params.inv_view_proj, pixel_pos, screen_width, screen_height, vec2<f32>(0.0, 0.0),
);
```

When this same `ray_dir` is fed into `get_hit_data_from_planes` for each
neighbour, the resulting `cur_first_hit_result.pos` is reconstructed as "the
centre's ray extended through the neighbour's hit-plane data". Pre-fix
(before iteration 2's hash-was-pos-dependent change) the hash was derived
only from per-neighbour `cur_first_hit.{y, z, x}` fields — `pos` was
unused for the hash, so the centre-`ray_dir` reuse was benign.

Post-iteration-2 the hash IS derived from `cur_first_hit_result.pos`
(`taa.wgsl:317` calling `taa_data_id_lo13`), so the asymmetric `ray_dir`
makes the 8 neighbour hashes effectively random (the reconstructed `pos`
no longer corresponds to the actual neighbour voxel cells).

### The fix

Inside the precompute loop, compute each neighbour's own `ray_dir` and
use it for `get_hit_data_from_planes`:

```wgsl
for (var i = 0u; i < 9u; i = i + 1u) {
    let off = taa_neighbor_offsets[i];
    let cur_pixel_pos = vec2<u32>(clamp(
        vec2<i32>(pixel_pos) + off,
        vec2<i32>(0, 0),
        vec2<i32>(i32(screen_width) - 1, i32(screen_height) - 1),
    ));
    // Each neighbour pixel needs ITS OWN ray_dir for `get_hit_data_from_planes`
    // to reconstruct its hit voxel correctly. Pre-iteration-2 the hash did not
    // depend on `pos`, so the loop reused the centre `ray_dir`; that asymmetry
    // is now a hash-fallback regression. The centre-`ray_dir` local outside
    // the loop is still used downstream for `pos_virtual = ray_dir *
    // first_hit_dist` (line 346) — centre-only, correct.
    let cur_ray_dir = get_ray_dir(
        params.inv_view_proj, cur_pixel_pos, screen_width, screen_height, vec2<f32>(0.0, 0.0),
    );
    let cur_first_hit =
        first_hit_data[cur_pixel_pos.x + cur_pixel_pos.y * screen_width];
    let cur_first_hit_result = get_hit_data_from_planes(
        cur_first_hit, cam_pos_int, cam_pos_frac, cur_ray_dir,
    );
    // ... rest of the loop body unchanged.
}
```

### Cost

Adding 8 extra `get_ray_dir` calls per output pixel. `get_ray_dir`
(`render_pipeline_common.wgsl`) is a single matrix-vector multiply + a
perspective divide + a normalize — ~10 FLOPs. Per-output-pixel cost
increases from 1 ray_dir to 9 ray_dirs (the centre's is also recomputed
inside the loop on `i == 0`, but the centre's was already needed at line
245 for `pos_virtual`; the new in-loop value replaces it for the per-
iteration-`i == 0` precompute use only). Net: +8 ray_dir computations per
pixel, dwarfed by the loop's existing `get_hit_data_from_planes` + hash
work. Negligible perf cost.

### Existing helper reuse

`get_ray_dir` is already imported at `taa.wgsl:78-83` and called once at
line 245. Reusing it inside the loop adds no new imports.

### Verification confidence

The existing `oasis-edit-visual` gate covers neighbour-hash behaviour
indirectly (its post-edit framebuffer compare includes the 3×3 fallback
window). The new `streaming-taa-shift-noise` gate (below) covers the
post-shift transient where the neighbour-hash matters most.

---

## Design — new e2e gate `streaming-taa-shift-noise`

### Goal

Capture the post-origin-shift TAA transient analytically: shadowed-band
per-pixel temporal variance over frames N..N+3 vs frame N+5 baseline. Pre-
fix the gate FAILS (variance is dominated by the rejected-history-replaced-
by-current-frame noise burst); post-fix the gate PASSES.

### File layout

- New file: `crates/bevy_naadf/src/e2e/streaming_taa_shift_noise.rs`.
- New `Gate::StreamingTaaShiftNoise` variant in
  `crates/bevy_naadf/src/cli.rs` (around line 391-473) with kebab-name
  `streaming-taa-shift-noise`.
- New dispatch arm in `apply_gate_defaults`
  (`crates/bevy_naadf/src/cli.rs:278-364`).
- New `pub mod streaming_taa_shift_noise;` in
  `crates/bevy_naadf/src/e2e/mod.rs:24-40`.
- New mode flag `streaming_taa_shift_noise_mode: bool` on `AppArgs`
  (`crates/bevy_naadf/src/lib.rs:291-517`) — per the project memory
  `feedback-e2e-must-drive-actual-main.md` rule, this is on the SHARED
  `AppArgs`/clap parser, not e2e-only. The gate dispatch sets it.

### Mechanism

Layered on top of `streaming_window_mode` (like `streaming_cold_start`).
The defaults function:

```rust
pub fn apply_streaming_taa_shift_noise_defaults(args: &mut crate::AppArgs) {
    // Layer onto streaming-window's defaults: streaming preset install,
    // residency, the OasisXxx state machine + the camera-pin system.
    super::streaming_window::apply_streaming_window_defaults(args);
    args.streaming_taa_shift_noise_mode = true;
    // We keep `streaming_window_mode = true` so the spawn-pose camera pin
    // and the additive +X walk both fire — the walk is what triggers the
    // origin shifts we want to measure.
    reset_capture_latches();
}
```

### Camera path + shift trigger

Reuse `pin_streaming_window_camera`'s additive +X walk:
- `STREAMING_WALK_TICKS = 256` ticks
- `STREAMING_WALK_VOXELS_PER_TICK = 4.0` voxels
- Total walk = 1024 voxels = 4 segments → ≥ 4 origin shifts
  (`STREAMING_MIN_ORIGIN_SHIFT_SEGMENTS = 4` per
  `streaming_window.rs:113-116`).

The first origin shift fires when the camera crosses the half-window-from-
spawn threshold (segment boundary at world voxel = origin × 256 ± half-
window). With the spawn pose at world segment (8, 1, 8) and the window
centered there at origin (0, 0, 0), the first shift fires after ~64
voxels of +X walk = ~16 ticks.

### Capture timing — the shift-frame detection

The gate captures frames N, N+1, N+2, N+3, N+5 where N = "the first
origin-shift frame". To detect N programmatically, the gate observes
`Residency::origin().x` each tick during the walk; on the tick where
`origin.x` changes from its previous value, that tick is `N`. From then
on, the next 4 ticks (N+1, N+2, N+3, plus one filler N+4) capture
transient frames; the 5th tick after N (= N+5) captures the recovered-
baseline frame.

Implementation: a dedicated `Update` system
`record_shift_transient_frames` that runs `.after(pin_streaming_window_camera)`
and reads `Residency::origin()`. On origin-shift detection, request a
`Screenshot::primary_window()` and stash it; do the same for the next 4
ticks (N+1 .. N+4) and again at tick N+5. Each stashed `Image` is decoded
to a `Framebuffer` at OasisAssert time.

### Shadowed-band selector

A pixel is "shadowed" if its luminance falls below
`SHADOWED_BAND_LUMA_MAX`. The existing `streaming_window` gate's
`centre_non_sky_ratio` (`streaming_window.rs:304-340`) uses a similar
heuristic for sky classification.

Proposed threshold: `SHADOWED_BAND_LUMA_MAX: f32 = 30.0`. Rationale:
- Sky pixels in the streaming gate's measured runs have luminance
  ~60-240 (`streaming_window.rs:325` blue-sky-or-haze branch).
- Lit-terrain pixels are in the 50-200 luminance range.
- Hard-shadowed pixels (the regions where the artefact shows) are
  below ~30. This is consistent with the visual description "shadowed
  regions briefly fill with noisy splotches" — the splotches are in
  pixels normally near-black.

A pixel must be shadowed in **all 5 captured frames** to enter the
metric — otherwise we'd be measuring sample turnover in pixels that
moved across the lit/shadowed boundary (legitimate camera motion).

### Metric

For each pixel `p` that is shadowed in all 5 frames:
- `lum_p(N), lum_p(N+1), lum_p(N+2), lum_p(N+3)` → 4 samples → temporal
  variance `var_transient(p)`.
- `lum_p(N+5)` is the post-recovery baseline (single sample; we use a
  global baseline floor instead — see below).

Aggregated:
- `var_transient = mean over shadowed-band pixels p of var_transient(p)`.
- `var_baseline = mean over shadowed-band pixels p of (lum_p(N+5) -
  mean_baseline)²` where `mean_baseline` is the global mean of the
  baseline frame's shadowed pixels.

Actually that's two different things. The cleaner formulation:

- Define `var_transient` as the **per-pixel variance over frames N..N+3
  of luminance** (a 4-sample per-pixel variance), averaged over
  shadowed-band pixels (the band is taken from frame N+5 to be the post-
  recovery shadow set).
- Define `var_baseline` as the **luminance variance across the shadowed
  band in frame N+5 alone** (a spatial variance — what a single
  recovered shadowed-band frame "looks like" naturally).

For the gate to FAIL pre-fix, the artefact contributes a large temporal
variance burst not present in the spatial-baseline noise.

Concretely (single number test):

```rust
// var_transient: average per-pixel-temporal variance over shadowed pixels.
let mut transient_acc = 0.0_f64;
let mut shadowed_pixel_count = 0_u32;
for p in shadowed_band_pixels {
    let samples = [lum(N, p), lum(N+1, p), lum(N+2, p), lum(N+3, p)];
    let mean = samples.iter().sum::<f32>() / 4.0;
    let var: f32 = samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / 4.0;
    transient_acc += var as f64;
    shadowed_pixel_count += 1;
}
let var_transient = (transient_acc / shadowed_pixel_count as f64) as f32;

// var_baseline: same metric computed at frame N+5 + (N+5 itself only has
// one sample so we use a +/-1 neighbour-pixel spatial proxy or simply
// the spatial variance of N+5 within the band).
// SIMPLER: just use a single-frame spatial variance of N+5 in the band.
let lums_n5: Vec<f32> = shadowed_band_pixels.iter().map(|p| lum(N+5, p)).collect();
let mean_n5 = lums_n5.iter().sum::<f32>() / lums_n5.len() as f32;
let var_baseline_spatial: f32 = lums_n5.iter()
    .map(|l| (l - mean_n5).powi(2))
    .sum::<f32>() / lums_n5.len() as f32;
```

### Assertion

```rust
pub const STREAMING_TAA_SHIFT_NOISE_RATIO_MAX: f32 = 3.0;

// Gate PASSES when temporal variance over the shift transient is not
// much larger than the spatial baseline variance at recovery. Pre-fix
// (artefact present) we measure ratios in the 5-20× range typically.
// Post-fix the temporal variance collapses to ~baseline (1-2× range).
let ratio = var_transient / var_baseline_spatial.max(1.0);  // floor avoids div-by-zero
if ratio > STREAMING_TAA_SHIFT_NOISE_RATIO_MAX {
    return Err(format!("..."));
}
```

The `.max(1.0)` floor guards against pathological all-zero baseline frames
(the rate-limiting failure mode would be "shadowed band has 0 luma in
every captured frame", giving `0/0`). A real shadowed-band frame has a
non-trivial spatial variance from atmosphere / GI bounce.

### Threshold rationale

`STREAMING_TAA_SHIFT_NOISE_RATIO_MAX = 3.0`:
- Calibrated against the project's existing gate floors:
  `STREAMING_MIN_AFTER_LUM_VARIANCE = 800.0` (the streaming-window gate
  measures whole-frame variance, not band-restricted).
- The artefact pattern: a TAA history reset means each post-shift
  shadowed-band pixel drops to `weight = 1` and renders the current
  frame's raw GI noise. GI noise variance for shadowed pixels can
  legitimately spike to 5-20× steady-state during the 3-frame transient.
- 3× ratio is comfortably above any plausible non-artefact temporal
  variation (camera intra-frame motion + atmosphere quarter-stride update
  contribute ≤ 1.5× variation in practice) and well below the artefact's
  measured behaviour.

If the gate is too tight (false positives on a fix that legitimately
leaves a small residual), we widen to 4-5×; if too loose (pre-fix
accidentally passes), we tighten to 2.5×. Pre-fix verification (the gate
MUST FAIL on the current code in the worktree) is what locks in the
choice — if pre-fix fails at 3× by a wide margin (say 10×), the threshold
is sound; if pre-fix barely fails (3.1×), the threshold needs widening.

### CLI flag

No new CLI flag. The dispatch is via `--gate streaming-taa-shift-noise`.
The mode flag `streaming_taa_shift_noise_mode: bool` is on `AppArgs` (per
the `feedback-e2e-must-drive-actual-main.md` memory: shared `AppArgs`/
clap parser, not e2e-only) but does not gain a clap flag — it's set
only by the gate dispatch in `apply_gate_defaults`.

### Pre-fix vs post-fix expected behaviour

- **Pre-fix (current state of the worktree, no rebase applied):** The
  TAA reproject pass rejects every history sample on the shift frame
  (the dist+screen-pos tests fail because `cam_pos_from_cur_int` is off
  by 256). `taa_sample_accum[p].x = (weight=0, …)` for every shadowed
  pixel; the next-pass `calc_new_taa_sample` writes
  `(weight=1, current_light)`. Final blit divides by `max(1, weight)` —
  so the frame shows the current frame's raw GI noise with no temporal
  averaging. Over the 4 transient frames `weight` rises 1 → 2 → 3 → 4
  and the noise burst decays. The per-pixel temporal variance in the
  shadowed band lands in the 5-20× spatial-baseline range. **Gate
  FAILS.**

- **Post-fix (structural rebase landed):** The TAA reproject pass's
  dist+screen-pos tests pass for the just-shifted history slots because
  `cam_pos_from_cur_int` is now correct. Hash test passes (helper now
  composes world-absolute). History accumulates correctly across the
  shift; the per-pixel temporal variance in the shadowed band stays at
  the spatial-baseline level. **Gate PASSES.**

### Driver wiring

The gate routes through the `OasisXxx` state machine like the streaming-
window gate (it inherits `streaming_window_mode = true`), and the OasisAssert
branch dispatches to `assert_streaming_taa_shift_noise_landed` based on
`args.streaming_taa_shift_noise_mode`:

```rust
} else if streaming_taa_shift_noise_mode {
    super::streaming_taa_shift_noise::assert_streaming_taa_shift_noise_landed(
        // pass the 5 captured framebuffers
    )
```

The 5 captured frames are stashed via a static `Mutex<TransientCaptures>`
(mirroring `MID_WALK_IMAGE` at `streaming_window.rs:213`); the assert
reads them at OasisAssert time.

### One-smoke pre-fix verification

Per the `subagent-gpu-app-verification-loop.md` memory, after the new
gate compiles, it is run ONCE on the pre-fix worktree and ONCE on the
post-fix worktree. If pre-fix doesn't FAIL or post-fix doesn't PASS, the
gate is not analytically valid and goes back to the architect for
threshold/metric revision — not back to the implementer for re-runs.

### Wall-clock budget

Re-use the streaming-window wall-clock budget (`STREAMING_GATE_WALL_CLOCK
_MAX_SECS = 120`). The new gate runs the same camera-walk shape; the only
addition is 5 framebuffer captures during the walk. Each capture costs ~5
ms of CPU work at e2e resolution.

### Timeout wrapping

Per `feedback-e2e-gates-must-fail-fast.md`, the gate's invocation in the
verification plan below is wrapped in `timeout 180s` (matching the other
gates).

---

## Decisions & rejected alternatives

### (a) Cross-world wiring mechanism — chose option-zero (no wiring) over the brief's three named options

- **Chose:** No cross-world wiring. The rebase lives in main-world
  `update_camera_history`. `extract_camera_history` then mirrors the
  already-rebased `positions[..]` to the render world via the existing
  byte-for-byte copy at `render/extract.rs:377`.
- **Rejected:**
  - A new `origin_change_this_frame: Option<IVec3>` field on
    `Residency`. Rejected because the rebase doesn't need to cross
    worlds: the data being rebased (`CameraHistory.positions[..]`) is
    main-world-resident, and the consumer is the main-world
    `update_camera_history` itself.
  - A Bevy `Events<OriginShifted>` event. Rejected because Bevy events
    retain for 2 frames; we want one-shot semantics. `Local<LastOriginSeen>`
    gives one-shot for free.
  - A render-world-only resource. Rejected because by the time
    `prepare_taa` runs, `extract_camera_history` has already copied the
    OLD-frame positions; rebasing render-world only would either
    require duplicating the rebase logic OR mirroring the whole
    positions array twice.
- **Why:** Simpler is better. The rebase is a 5-line function call
  inside a system that already takes `Option<Res<Residency>>`. Adding a
  cross-world event channel would multiply surface area for zero gain.
- **What would flip the call:** Only if `update_camera_history` lost its
  `Option<Res<Residency>>` parameter (a regression in the diagnostic
  instrumentation work that lives on lines 192/236 today) or if the
  TAA `positions[..]` ring ever migrated to render-world-only ownership.
  Neither is on any roadmap.

### (b) Hash world-absolute reference source — chose existing `StreamingExtractRender.window_origin` over a new uniform field on a new bind group

- **Chose:** Read `residency.origin` from
  `StreamingExtractRender.window_origin: IVec3`
  (`streaming/noise_dispatch.rs:274-277`), multiply by `SEGMENT_VOXELS`,
  pack into the existing trailing slot of `GpuTaaParams`. Re-uses an
  already-extracted resource.
- **Rejected:**
  - Adding `residency_origin` as a new field on a new bind-group entry.
    Rejected because the diagnostic explicitly forbids new bind-group
    entries for cost reasons (and there's no reason to add one when the
    value fits in trailing struct padding).
  - Adding it as a push constant. Rejected because the codebase has no
    other push-constant precedent; trailing-padding repurposing matches
    the existing `cam_pos_int (IVec3 + _pad0 u32)` precedent at
    `gpu_types.rs:207-209`.
  - Computing `residency_origin_voxels = origin × SEGMENT_VOXELS` GPU-
    side (e.g. passing `origin: IVec3` and multiplying in the shader).
    Rejected because (i) it would force a magic-number `SEGMENT_VOXELS =
    256` in WGSL, violating the SSoT discipline of
    `feedback-ssot-vs-agentic-divergence.md`; (ii) the multiply is once
    per frame on the CPU vs once per pixel on the GPU.
- **Why:** Lowest-risk, smallest diff, no new bindings, no shader-side
  magic numbers. The `StreamingExtractRender.window_origin` field already
  exists and is already kept in sync.
- **What would flip the call:** If `StreamingExtractRender` were ever
  refactored to drop `window_origin` (currently it's used by the
  streaming dispatch loop too — `noise_dispatch.rs:269-277`).

### (c) E2e gate threshold value — chose `ratio = 3.0` over `5.0` or `2.0`

- **Chose:** `STREAMING_TAA_SHIFT_NOISE_RATIO_MAX = 3.0` for the temporal
  / spatial variance ratio over the shadowed band.
- **Rejected:**
  - `5.0` — too loose. Pre-fix might pass at the upper end of normal
    camera-motion noise (~3-4× ratio in atmospheric variability), wiping
    out the gate's analytical power.
  - `2.0` — too tight. The 4-tick transient over `weight 1→2→3→4` can
    legitimately produce ratios in the 2.5× range even with the fix in
    place (the camera is also moving during the walk; some history
    rejection is legitimate).
  - A fixed luminance-Δ threshold (without normalisation by baseline).
    Rejected because per-pixel luminance scales with the actual GI
    brightness, which varies by scene; ratio against baseline normalises
    away the absolute-scale dependency.
- **Why:** 3× is calibrated against `feedback-primitives-then-analytical-
  invariants.md`'s "analytical, not screenshot" requirement and the
  existing project gate floors (the streaming-window `STREAMING_MIN_AFTER_LUM_VARIANCE
  = 800.0` is a whole-frame variance, not directly comparable).
- **What would flip the call:** Empirical pre/post-fix measurements
  during the implementation phase. If the pre-fix variance ratio is
  measured at 4× (only just above the threshold), the threshold goes to
  2.5 or 2.0. If it's at 10×+, the threshold is sound. This is exactly
  the `feedback-primitives-then-analytical-invariants.md` analytical-
  validation step that the structural fix's gate must pass.

### (d) Removing or keeping the instrumentation log at `taa.rs:253`

- **Chose:** REMOVE. The diagnostic instrumentation block at
  `crates/bevy_naadf/src/render/taa.rs:215-287` was added in Phase I to
  empirically confirm the diagnostic; the user's Phase J live run
  confirmed `delta_voxels ≈ −256` for 5/5 shifts. The instrumentation's
  diagnostic purpose is fully discharged. With the structural fix in
  place, every post-shift `delta_voxels` would still equal ±256 by
  construction (window-local re-pinning still happens; the rebase
  corrects the past entries in the same frame). The log would fire on
  every shift in a green system — noise rather than signal.
- **Rejected:** Keeping it. Rejected because:
  - The condition (`|delta| > 64` voxels) is a heuristic that catches
    real origin shifts, but the user already has the
    `streaming-world residency shift:` log line at
    `streaming/residency.rs:676-689` for that. Two log lines per shift is
    noise.
  - "Still useful for rare false rebases" doesn't apply: the rebase
    fires only when `delta_segments != IVec3::ZERO`, which can only
    happen when `Residency::origin` changed since last frame — a
    deterministic property of `residency_driver`, not subject to false-
    positive rebases.
- **Why:** Clean removal. The single Phase I `Local`/instrumentation
  block goes away; only the diagnostic side-files (`06-diagnostic-
  investigation.md`) remain as the audit trail.
- **What would flip the call:** If during implementation a *different*
  origin-shift-trigger source were discovered (e.g. `Residency::origin`
  changing for non-camera-segment-boundary reasons), keeping the
  magnitude heuristic might catch that. But none is plausible — the
  only callers of `WindowedSlotMap::set_origin` are
  `residency_driver`'s segment-boundary detection
  (`residency.rs:585-592`) and a couple of test helpers.

### (e) Faithful-port compliance — flag for reviewer sanity-check

Per `bevy-naadf-faithful-port-rule.md`: the structural rebase introduces a
behaviour that has NO counterpart in C# NAADF, because the streaming
preset itself is a Bevy-side addition (NAADF does not have residency-
window streaming). The user pre-approved this as part of the streaming-
world orchestration; this fix is the missing piece needed for that
streaming preset to work correctly. **Reviewer / orchestrator sanity-
check: confirm the streaming-preset addition has explicit user approval
in the orchestration docs, and this fix is a continuation of that
approved divergence, not a new one.** The continuation is documented
here so the reviewer can stop at the divergence boundary.

---

## Assumptions made

1. **`Residency::origin()` returns the post-shift value within the same
   tick the shift was applied.** Verified by reading
   `streaming/residency.rs:585-592` (`set_origin` is synchronous and
   modifies `WindowedSlotMap.origin` immediately) and
   `streaming/residency.rs:173-175` (`origin()` reads it without
   indirection). True.

2. **`update_camera_history` runs in the same frame's `Update`, after
   `PreUpdate` finishes.** Verified by reading the schedule registration
   at `lib.rs:898-901` (registers in `Update`) and Bevy's standard
   schedule ordering (`PreUpdate` → `Update`). True.

3. **The existing `Option<Res<Residency>>` parameter on
   `update_camera_history` (introduced in Phase I instrumentation) will
   remain present.** The implementer task includes "remove the
   instrumentation log" — that removes the inner `info!` block, NOT the
   system parameter. The parameter is needed for the rebase detection.

4. **`StreamingExtractRender.window_origin` is updated every frame to
   reflect `Residency::origin`.** Verified by reading
   `streaming/noise_dispatch.rs:582-595` (the extract system writes
   `window_indirection: residency.window.indirection_buffer().to_vec()`
   and `window_origin: residency.origin()` each ExtractSchedule tick).
   True.

5. **Adding `Option<Res<StreamingExtractRender>>` to `prepare_taa`'s
   signature is safe.** `prepare_taa` is already on the render world
   (`lib.rs:139-156`, dispatched via render-app schedule registration in
   `render/mod.rs`). `StreamingExtractRender` is a render-world resource
   (`streaming/mod.rs:323`), so the `Option<Res<...>>` access is direct.

6. **`SEGMENT_VOXELS = 256` is the SSoT constant.** Verified at
   `streaming/residency.rs:46`. Already imported by other code via
   `crate::streaming::residency::SEGMENT_VOXELS` and via the
   `crate::streaming::SEGMENT_VOXELS` re-export. No magic numbers.

7. **`Local<LastOriginSeen>` correctly survives across `Update` ticks.**
   Bevy's `Local<T>` is per-system per-app state; this is the standard
   Bevy idiom for "remember last frame's value of X". The streaming
   gate's `record_walk_metrics_and_capture_mid_walk` system uses the
   same shape via `AtomicBool`/`AtomicI32` instead of `Local` (because
   the e2e harness has cross-system state — see
   `streaming_window.rs:344-347`). For the rebase's per-system state, a
   simple `Local` is cleaner.

8. **The repurposed `GpuTaaParams` trailing layout
   (`residency_origin_voxels: IVec3 + sample_age: u32`) packs to 192
   bytes under both Rust `#[repr(C)]` and WGSL std140.** Verified by
   eye against existing patterns (`cam_pos_int: IVec3 + _pad0: u32`
   compiles to 16 bytes in WGSL with `cam_pos_int: vec4<i32>`). The
   implementer MUST verify by checking that the existing
   `assert!(std::mem::size_of::<GpuTaaParams>() == 192)` at
   `gpu_types.rs:874` still passes; if it doesn't, the field order must
   be adjusted (see Option C analysis in §"Plumbing" above).

9. **No other shader reads `cnts_params.sample_age` or
   `params.sample_age`.** Verified by grep: only `taa.wgsl:349`
   references `params.sample_age` (the reproject pass loop bound), and
   `cnts_params.sample_age` is NOT referenced anywhere in
   `calc_new_taa_sample` (it doesn't walk a history loop). Field
   reshuffling has a single call site.

10. **The shadowed-band luminance threshold (30.0) is well-calibrated
    against the renderer's actual shadowed-pixel distribution.**
    Calibrated by analogy against the streaming-window gate's sky-vs-
    terrain heuristic at `streaming_window.rs:304-340`; not measured
    independently. The implementer's pre-fix verification run will reveal
    if the shadowed-band selector picks up the expected ~10-30% of
    framebuffer pixels in a streaming scene — if it picks up too few
    (< 5%) the threshold may need to rise to 50.0; if too many (> 50%)
    the threshold falls to 20.0.

11. **The 5-frame capture window (N..N+5) is sufficient to span the
    transient + baseline.** The diagnostic at item 5 (`taa.wgsl:447-450`)
    says weight = 0 on frame N, then `+1` per frame in
    `calc_new_taa_sample` — so weight progression is 0/1, 1/2, 2/3, 3/4.
    At weight = 4 (frame N+3) the TAA averaging is back to a 4-sample
    window. Frame N+5 = weight 6, well into the recovered regime.

12. **The instrumentation removal does not break any test.** The
    `update_camera_history` test (if any — needs grep verification) does
    not depend on the `info!` block; removing it is a pure code shrink.

13. **The gate's `Gate::StreamingTaaShiftNoise` enum variant addition
    does not break `Gate::as_kebab_str` exhaustiveness.** Rust's `match`
    exhaustiveness will surface any missing arm at compile time; the
    implementer adds the kebab string the same way the other variants
    are listed at `cli.rs:479-507`.

---

## Verification plan

Per `feedback-e2e-gates-must-fail-fast.md` and
`subagent-gpu-app-verification-loop.md`: each command is run ONCE,
wrapped in `timeout 180s`, against the worktree root.

Sequence (in order, each gate ONCE):

1. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo build --workspace 2>&1 | tail -100`
2. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 300s cargo test --workspace --lib 2>&1 | tail -60`
   - expect ≥ 291 passing (post-Phase-2.14.f baseline + the 2 existing
     `taa_hash_world_identity_*` tests + any new unit test for
     `CameraHistory::rebase_for_origin_shift`)
3. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate streaming-cold-start 2>&1 | tail -80`
4. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate streaming-window 2>&1 | tail -80`
5. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate oasis-edit-visual 2>&1 | tail -80`
6. **NEW** `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate streaming-taa-shift-noise 2>&1 | tail -80`

Pre-fix step (BEFORE landing the structural rebase): run only step 6 on
the worktree as-is (with the new gate file added but the rebase NOT
applied). Step 6 MUST FAIL with the variance ratio exceeding the
threshold. This is the analytical validation per
`feedback-primitives-then-analytical-invariants.md`.

Post-fix step (after landing): re-run steps 1-6 in order. Step 6 MUST
PASS; steps 3-5 must continue PASSing.

One smoke per failing gate: if step 6 fails post-fix or any of 3-5
regress, the implementer fixes ONCE and re-runs ONCE. If still failing,
STOP and report (per `subagent-gpu-app-verification-loop.md`).

### Rust unit tests (mandatory per `feedback-primitives-then-analytical-invariants.md`)

Add to `crates/bevy_naadf/src/render/taa.rs::tests` (extending the existing
test module at lines 583-657):

```rust
/// `rebase_for_origin_shift` adds the integer delta × SEGMENT_VOXELS to
/// every entry's `.pos_int`; `.pos_frac` is byte-identical pre/post.
#[test]
fn rebase_for_origin_shift_preserves_frac_and_shifts_int() {
    let mut h = CameraHistory::default();
    // Seed a non-default frac into a few slots so the test detects
    // any accidental writes to .pos_frac.
    h.positions[0] = PositionSplit {
        pos_int: IVec3::new(100, 0, 100),
        pos_frac: Vec3::new(0.25, 0.5, 0.75),
    };
    h.positions[5] = PositionSplit {
        pos_int: IVec3::new(-50, 1, -200),
        pos_frac: Vec3::new(0.1, 0.2, 0.3),
    };
    let frac_before = h.positions.map(|ps| ps.pos_frac);

    h.rebase_for_origin_shift(IVec3::new(1, 0, 0));

    // SEGMENT_VOXELS = 256; delta_seg = (1, 0, 0) ⇒ voxel_delta = (-256, 0, 0).
    assert_eq!(
        h.positions[0].pos_int,
        IVec3::new(100 - 256, 0, 100),
        "slot 0 int rebased",
    );
    assert_eq!(
        h.positions[5].pos_int,
        IVec3::new(-50 - 256, 1, -200),
        "slot 5 int rebased",
    );

    // frac field untouched on every slot, including the default-zero ones.
    for (i, ps) in h.positions.iter().enumerate() {
        assert_eq!(ps.pos_frac, frac_before[i],
                   "slot {i} pos_frac must NOT change under integer rebase");
    }
}

/// Two consecutive shifts compose additively (i.e. the system is correct
/// across multi-shift bursts the cold-start phase can fire).
#[test]
fn rebase_for_origin_shift_composes() {
    let mut h = CameraHistory::default();
    h.positions[0] = PositionSplit {
        pos_int: IVec3::new(0, 0, 0),
        pos_frac: Vec3::ZERO,
    };
    h.rebase_for_origin_shift(IVec3::new(1, 0, 0));
    h.rebase_for_origin_shift(IVec3::new(0, 0, 1));
    assert_eq!(h.positions[0].pos_int, IVec3::new(-256, 0, -256));
}
```

Both tests are pure CPU primitives, no GPU dependency. They run under
`cargo test --workspace --lib`. Baseline goes from 291 to 293 passing
tests post-fix.

---

## File-by-file change list

The implementer's TODO sheet, numbered and anchored to file:line refs in
the current worktree. Each entry is the smallest cohesive diff.

### 1. `crates/bevy_naadf/src/render/taa.rs` — structural rebase + cleanup

- **a.** Add the `rebase_for_origin_shift` method to `impl CameraHistory`
  near lines 96-108 (after the existing `Default` impl, before
  `taa_index_of`). Method body per §"The rebase call" above. ~10 lines.
- **b.** Lines 215-287 (the entire diagnostic instrumentation `let
  prev_slot = …` block + the `info!` calls): **delete**. Removes the
  Phase I instrumentation per decision (d).
- **c.** Just above the existing `let slot = taa_index as usize;` at
  line 214, add a new code block:
  ```rust
  // Origin-shift rebase: when the residency origin advanced this
  // frame, every entry of `positions[..]` written in a previous frame
  // is still in the OLD window-local coordinate frame; re-express
  // them in the NEW window-local frame BEFORE we overwrite this
  // frame's slot. See `docs/orchestrate/taa-hash-world-identity/
  // 02-design.md` §"Design — structural rebase".
  let current_origin = residency.as_deref()
      .map(|r| r.origin())
      .unwrap_or(IVec3::ZERO);
  if let Some(last) = last_origin_seen.0 {
      let delta_segments = current_origin - last;
      if delta_segments != IVec3::ZERO {
          history.rebase_for_origin_shift(delta_segments);
      }
  }
  last_origin_seen.0 = Some(current_origin);
  ```
- **d.** Add a new `Local<LastOriginSeen>` parameter to the system
  signature at line 188-193:
  ```rust
  pub fn update_camera_history(
      camera: Single<(&Camera, &Transform, &PositionSplit), With<PositionSplit>>,
      args: Res<crate::AppArgs>,
      mut history: ResMut<CameraHistory>,
      residency: Option<Res<crate::streaming::residency::Residency>>,
      mut last_origin_seen: Local<LastOriginSeen>,
  )
  ```
- **e.** Add the type `pub struct LastOriginSeen(pub Option<IVec3>);` +
  `#[derive(Default)]` near the top of the file (after the existing
  `pub fn taa_index_of` at line 117).
- **f.** Add the two new unit tests in §"Rust unit tests" above to the
  `#[cfg(test)] mod tests { ... }` block at lines 583-657.

### 2. `crates/bevy_naadf/src/render/gpu_types.rs` — `GpuTaaParams` field reshuffle

- **a.** Lines 220-230 (`taa_index`, `sample_age`, `_pad2`, `_pad3`,
  `_pad4`): reshuffle to:
  ```rust
      pub taa_index: u32,
      pub residency_origin_voxels: IVec3,  // = residency.origin × SEGMENT_VOXELS (zero in non-streaming presets)
      pub sample_age: u32,                  // packs into the trailing 4 bytes of the vec3-then-u32 layout
  ```
  Drops `_pad2`, `_pad3`, `_pad4`. Net field count: +1 (replaces three
  `u32`s with `IVec3 + u32`).
- **b.** The `assert!(std::mem::size_of::<GpuTaaParams>() == 192)` at
  line 874 stays correct (the implementer verifies by `cargo build`).

### 3. `crates/bevy_naadf/src/render/taa.rs` — uniform upload

- **a.** Lines 497-516 in `prepare_taa`: add the
  `residency_origin_voxels` field to the `taa_params_data`
  initialisation:
  ```rust
  let residency_origin_voxels = streaming_extract
      .as_deref()
      .map(|s| s.window_origin * crate::streaming::residency::SEGMENT_VOXELS)
      .unwrap_or(IVec3::ZERO);
  let taa_params_data = GpuTaaParams {
      // ... existing fields ...
      taa_index: extracted_history.taa_index,
      residency_origin_voxels,
      sample_age: ring_depth.clamp(1, ring_depth),
  };
  ```
- **b.** Add the new system parameter
  `streaming_extract: Option<Res<crate::streaming::StreamingExtractRender>>`
  to `prepare_taa`'s signature at lines 361-371.

### 4. `crates/bevy_naadf/src/assets/shaders/taa.wgsl` — uniform struct + hash helper + 8-neighbour fix

- **a.** Lines 114-124 (`struct GpuTaaParams`): replace with the new
  declaration in §"Plumbing" → Option C, adding the
  `residency_origin_voxels: vec4<i32>` field (`.xyz` carries the voxel
  delta, `.w` is unused). Remove the trailing `sample_age: u32` line and
  re-add it as the FINAL field per the Rust ordering above.

  Actually re-checking the Rust layout: with the Rust order
  `residency_origin_voxels: IVec3, sample_age: u32`, the WGSL ordering
  also goes `residency_origin_voxels`, then `sample_age`. Both at the
  same byte offsets. The WGSL declaration uses `vec4<i32>` for the
  IVec3 field so std140 packs the trailing scalar properly — and the
  scalar still needs to be declared. Final WGSL:
  ```wgsl
  struct GpuTaaParams {
      inv_view_proj: mat4x4<f32>,
      view_proj: mat4x4<f32>,
      cam_pos_int: vec4<i32>,
      cam_pos_frac: vec4<f32>,
      screen_width: u32,
      screen_height: u32,
      frame_count: u32,
      taa_index: u32,
      // .xyz = residency.origin × SEGMENT_VOXELS, .w packs `sample_age`.
      // This mirrors the `cam_pos_int`/`cam_pos_frac` Rust(IVec3+u32)
      // → WGSL(vec4<i32>) pattern (`taa.wgsl:95-101`): the trailing
      // scalar lives inside the vec4 slot to preserve 192-byte struct
      // size + 16-byte alignment.
      residency_origin_voxels: vec4<i32>,
      sample_age: u32,
  }
  ```

  WAIT — with `vec4<i32>` declared, std140 pads the WGSL struct to keep
  `sample_age` as a separate field, which means WGSL needs `sample_age`
  declared too (and the Rust layout has it as a separate `u32` after the
  IVec3). That's the same pattern as `cam_pos_int: vec4<i32>` (4 i32s,
  16 bytes) with the Rust struct supplying `IVec3 + _pad0: u32` (also
  16 bytes). For the new field, the Rust struct supplies `IVec3 +
  sample_age: u32` (also 16 bytes); WGSL sees a single `vec4<i32>` of
  which `.xyz` is the voxel delta and `.w` is the u32-bit-pattern of
  `sample_age`. Then WGSL accesses `sample_age` via the new local:
  `let sample_age = u32(params.residency_origin_voxels.w);` near the top
  of the reproject entry point.

  Re-confirming the layout invariant: with the Rust struct ending in
  `(IVec3, u32)` packing to 16 bytes total, the WGSL declaration as a
  single `vec4<i32>` is the right mapping. The `sample_age: u32` should
  be REMOVED from the WGSL struct (it's now `.w` of the vec4), and the
  one call site at `taa.wgsl:349` reads from the new local instead.
- **b.** Line 235 (`let cam_pos_int = params.cam_pos_int.xyz;`): after
  this, add `let residency_origin_voxels =
  params.residency_origin_voxels.xyz;` and `let sample_age =
  u32(params.residency_origin_voxels.w);`.
- **c.** Line 349 (`for (var i = 1u; i < params.sample_age; ...`):
  change to `for (var i = 1u; i < sample_age; ...`.
- **d.** Line 464 (`let cam_pos_int = cnts_params.cam_pos_int.xyz;`):
  after this, add `let residency_origin_voxels =
  cnts_params.residency_origin_voxels.xyz;`. (calc_new_taa_sample does
  NOT use `sample_age`, so the `sample_age` local is not needed here.)
- **e.** Lines 175-225 — the `taa_data_id_lo13` helper block: update the
  comment header to reflect the world-absolute (not window-local) nature
  of the discriminator. Modify the helper signature to take a third
  parameter:
  ```wgsl
  fn taa_data_id_lo13(
      first_hit_pos: vec3<f32>,
      cam_pos_int: vec3<i32>,
      residency_origin_voxels: vec3<i32>,
  ) -> u32 {
      let voxel_pos = vec3<i32>(
          floor(first_hit_pos + vec3<f32>(cam_pos_int + residency_origin_voxels)),
      );
      var h: u32 = pcg_hash(u32(voxel_pos.x));
      h = pcg_hash(h ^ u32(voxel_pos.y));
      h = pcg_hash(h ^ u32(voxel_pos.z));
      return h & 0x1FFFu;
  }
  ```
- **f.** Line 317 (reproject pass call to helper): pass the new arg —
  `let cur_data_id_lo13 = taa_data_id_lo13(cur_first_hit_result.pos,
  cam_pos_int, residency_origin_voxels);`.
- **g.** Line 517 (calc_new_taa_sample call): same — `let data_id_lo13
  = taa_data_id_lo13(first_hit_result.pos, cam_pos_int,
  residency_origin_voxels);`.
- **h.** Lines 269-327 — the 9-iteration precompute loop: inside the
  loop body, compute each neighbour's own `cur_ray_dir` via
  `get_ray_dir(params.inv_view_proj, cur_pixel_pos, screen_width,
  screen_height, vec2<f32>(0.0, 0.0))`. Use that local for the
  `get_hit_data_from_planes` call at lines 281-283 (replacing the
  centre-`ray_dir` reuse). Code:
  ```wgsl
  for (var i = 0u; i < 9u; i = i + 1u) {
      let off = taa_neighbor_offsets[i];
      let cur_pixel_pos = vec2<u32>(clamp(
          vec2<i32>(pixel_pos) + off,
          vec2<i32>(0, 0),
          vec2<i32>(i32(screen_width) - 1, i32(screen_height) - 1),
      ));
      // Each neighbour pixel needs its own ray_dir for the hit-data
      // reconstruction; the centre `ray_dir` local at line 245 stays
      // for `pos_virtual = ray_dir * first_hit_dist` (centre-only).
      let cur_ray_dir = get_ray_dir(
          params.inv_view_proj, cur_pixel_pos,
          screen_width, screen_height, vec2<f32>(0.0, 0.0),
      );
      let cur_first_hit =
          first_hit_data[cur_pixel_pos.x + cur_pixel_pos.y * screen_width];
      let cur_first_hit_result = get_hit_data_from_planes(
          cur_first_hit, cam_pos_int, cam_pos_frac, cur_ray_dir,
      );
      // ... (rest of the loop unchanged)
  }
  ```

### 5. `crates/bevy_naadf/src/assets/shaders/taa_common.wgsl` — docstring update

- **a.** Lines 46-58: update the multi-line comment block to reflect that
  `data_id_lo13` is now genuinely world-absolute (was: "window-local-
  anchored"). One paragraph rewrite, no code change.

### 6. `crates/bevy_naadf/src/cli.rs` — new gate variant

- **a.** Line 391-473 (`pub enum Gate`): add `StreamingTaaShiftNoise,`
  variant with a doc comment.
- **b.** Line 479-507 (`as_kebab_str`): add the match arm `Gate::
  StreamingTaaShiftNoise => "streaming-taa-shift-noise"`.
- **c.** Line 278-364 (`apply_gate_defaults`): add the arm
  ```rust
  Gate::StreamingTaaShiftNoise => {
      crate::e2e::streaming_taa_shift_noise::apply_streaming_taa_shift_noise_defaults(args);
  }
  ```

### 7. `crates/bevy_naadf/src/lib.rs` — new mode flag on `AppArgs`

- **a.** Add `pub streaming_taa_shift_noise_mode: bool` to `AppArgs`
  near line 460 (alongside `streaming_cold_start_mode`).
- **b.** Add `streaming_taa_shift_noise_mode: false` to `impl Default
  for AppArgs` near line 538.

### 8. `crates/bevy_naadf/src/e2e/mod.rs` — new module registration

- **a.** Line 24-40: add `pub mod streaming_taa_shift_noise;`.
- **b.** Line 248-309: add the `record_shift_transient_frames` Update-
  schedule system in the existing `add_systems` tuple (after
  `record_walk_metrics_and_capture_mid_walk`).

### 9. `crates/bevy_naadf/src/e2e/streaming_taa_shift_noise.rs` — new file

The new gate module. Contains:
- `apply_streaming_taa_shift_noise_defaults`
- `reset_capture_latches`
- `record_shift_transient_frames` Update system (detects shift, captures
  frames at N..N+5)
- The static `Mutex<TransientCaptures>` holding the 5 captured
  framebuffers
- `assert_streaming_taa_shift_noise_landed` (the metric + threshold)
- Constants: `SHADOWED_BAND_LUMA_MAX = 30.0`,
  `STREAMING_TAA_SHIFT_NOISE_RATIO_MAX = 3.0`,
  `STREAMING_TAA_SHIFT_NOISE_VARIANCE_BASELINE_FLOOR = 1.0`

Length budget: ~250-300 lines (similar to `streaming_cold_start.rs`).

### 10. `crates/bevy_naadf/src/e2e/driver.rs` — assert dispatch

- **a.** Lines 540-545: extend the route-in flag check to include
  `streaming_taa_shift_noise_mode` so the gate enters `OasisWarmup` on
  tick 0:
  ```rust
  let streaming_taa_shift_noise_mode = app_args
      .as_deref()
      .is_some_and(|a| a.streaming_taa_shift_noise_mode);
  if (oasis_mode || vox_gpu_construction_mode || streaming_window_mode
      || noise_static_mode || streaming_cold_start_mode
      || streaming_taa_shift_noise_mode)
      && state.phase == E2ePhase::Warmup
      && state.phase_ticks == 0
  ```
- **b.** Lines 1015-1101 (`OasisApplyEdit`): add a branch BEFORE the
  `streaming_window_mode` branch for `streaming_taa_shift_noise_mode`:
  the gate calls `promote_camera_to_walk()` (like streaming-window), so
  the walk fires and the shift transient is captured. No special body
  needed — falls through to the streaming-window-mode walk branch.

  Actually since `streaming_taa_shift_noise_mode` layers on top of
  `streaming_window_mode`, the existing `streaming_window_mode` branch
  at line 1050 will fire correctly (it calls `promote_camera_to_walk`).
  The driver dispatch just needs to check
  `streaming_taa_shift_noise_mode` AT or BEFORE the existing branches
  so the OasisAssert routes correctly. So **only** point (c) is needed:
- **c.** Lines 1177-1252 (`OasisAssert`): add a branch BEFORE
  `streaming_window_mode`:
  ```rust
  } else if streaming_taa_shift_noise_mode {
      let _ = (&a, &b);  // We use our own captured frames, not the OasisXxx pair.
      super::streaming_taa_shift_noise::assert_streaming_taa_shift_noise_landed()
          .map(|msg| {
              println!("e2e_render --streaming-taa-shift-noise: {msg}");
          })
  } else if streaming_window_mode {
  ```
- **d.** Capture the residency origin X at gate start for the
  shift-detection state (re-uses `record_origin_x_at_pose_a` indirectly
  via the existing streaming_window plumbing).

### 11. Docs sync

- **a.** Update `crates/bevy_naadf/src/assets/shaders/taa.wgsl` lines
  175-225 comment header — replace "world-anchored" wording to reflect
  the (now actually) world-absolute derivation including the
  `residency_origin_voxels` composition.
- **b.** Update `crates/bevy_naadf/src/assets/shaders/taa_common.wgsl`
  lines 46-58 — same wording cleanup (per item 5).

### Total surface area

- Rust: 5 files touched (`render/taa.rs`, `render/gpu_types.rs`,
  `cli.rs`, `lib.rs`, `e2e/mod.rs`, `e2e/driver.rs`) + 1 new file
  (`e2e/streaming_taa_shift_noise.rs`). ~600 lines net.
- WGSL: 2 files touched (`taa.wgsl`, `taa_common.wgsl`). ~30 lines net.

No new bind-group entries. No new GPU buffers. No new CLI flags. No
struct size changes (Rust + WGSL `GpuTaaParams` both stay 192 bytes).
The streaming-side `Residency` resource is **read-only** to the rebase
mechanism — no changes to `streaming/residency.rs`.
