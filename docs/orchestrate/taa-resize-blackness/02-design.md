# 02 — Design: taa-resize-blackness

## Goal recap

Fix the transient "shadows go pitch black for a fraction of a second to ~1–2 s
after a window resize" symptom. Per `01-context.md` the scope is locked to the
TAA + GI ring zero-clears in `prepare_taa` (`crates/bevy_naadf/src/render/taa.rs:286-464`)
and `prepare_gi` (`crates/bevy_naadf/src/render/gi.rs:224-266`). Delivery is
strict TDD: Impl-A lands a failing reproduction test in the existing
`e2e_render` binary; Impl-B applies the fix; in between, the orchestrator
gates on the test failing against `main`.

---

## Test design (Deliverable A)

### A.1 Phase placement — between MOTION and SETTLE (rename SETTLE → POST_RESIZE_SETTLE)

The new state is `E2ePhase::Resize`, slotted **after MOTION and before SETTLE**.
Concretely the state machine becomes:

```text
WARMUP → MOTION → Resize → Settle → Shoot → Drain → Assert → Done
```

Justification: the TAA `taa_samples` ring (32 deep) and GI `sample_counts`
(128-frame accumulator) need to have **meaningfully filled** before the
resize, or there's nothing to lose when the buffers are recreated. The
existing harness already fills them: 96 WARMUP frames at the start pose
(`crates/bevy_naadf/src/e2e/mod.rs:77` — `E2E_WARMUP_FRAMES = 96`, deliberately
"past the 64-frame `computeValidHistory` ring-capacity window") and 48 MOTION
frames (`crates/bevy_naadf/src/e2e/mod.rs:93` — `E2E_MOTION_FRAMES = 48`)
through the open camera path. Triggering the resize at the *end of MOTION /
start of SETTLE* is the highest-value moment to disrupt the rings: the
camera has just landed at the readback pose, every ring is fully populated
with samples that map (post-motion) to the new pose, and any post-resize
zero-clear of those rings will manifest as a `solid_block_rect` luminance
collapse — exactly the symptom we are reproducing.

Rejected: between WARMUP and MOTION — the GI bounce would have time to
*re-converge* through 48 MOTION frames before SETTLE/SHOOT, masking the bug.
Rejected: between SETTLE and SHOOT — SETTLE is one frame
(`E2E_SETTLE_FRAMES = 1` at `mod.rs:108`) and we need the post-resize frame
count to be *small but at least a few*, so it must be its own phase.

### A.2 Resize trigger mechanism — mutate `Window::resolution` via `Query<&mut Window, With<PrimaryWindow>>`

The `Resize` phase pulls the primary window mutably and calls
`window.resolution.set_physical_resolution(new_w, new_h)`
(`bevy_window-0.19.0-rc.1/src/window.rs:1013`). The Bevy 0.19 `bevy_winit`
runtime sees `Changed<Window>` in its `changed_windows` system
(`bevy_winit-0.19.0-rc.1/src/system.rs:305-391`) and forwards the resize to
the underlying winit window the same frame; the OS delivers a
`WindowResized` event, `Camera::physical_viewport_size()` flips to the new
size, `ExtractedCameraData.viewport_size` follows, and the next
`prepare_taa` / `prepare_gi` see `pixel_count != old_pixel_count` and hit
the zero-clear codepath. The pattern follows the existing `panel.rs`
`Query<&Window, With<PrimaryWindow>>` usage (`crates/bevy_naadf/src/panel.rs:71,851`).

`PrimaryWindow` comes from `bevy::window::PrimaryWindow`. The driver gains
one new system parameter: `mut window: Single<&mut Window, With<PrimaryWindow>>`.

Rejected: sending a `WindowResized` event directly — that's an *output*
event from bevy_winit, not an input. Mutating `Window.resolution` is the
documented input path.

Rejected: `WindowResolution::set(f32, f32)` (logical) — it multiplies by
scale_factor (`bevy_window-0.19.0-rc.1/src/window.rs:1001-1006`); on
fractional-DPI displays the resulting physical size is non-deterministic
across CI machines. `set_physical_resolution` is exact and deterministic.

Note: `WindowConfig::e2e()` sets `resizable: false`
(`crates/bevy_naadf/src/lib.rs:271-281`), but `resizable` controls
*user-driven* resize-by-drag; programmatic resolution mutation is
unaffected. We do NOT need to flip `resizable: true`.

### A.3 Resize delta — 256×256 → 384×288 (aspect change, +50% width / +12.5% height)

Target is **384×288**, an aspect-ratio change (1:1 → 4:3). Rationale:

1. The buffer-recreation path triggers on `taa.pixel_count != pixel_count`
   (`crates/bevy_naadf/src/render/taa.rs:323-344`); ANY pixel-count delta
   trips it. `257×257` would technically trip the gate but is a
   non-realistic resize and gives the camera-history reprojection (which is
   *out of scope* — `01-context.md` line 41) almost no aspect-ratio change
   to expose stale matrices, which is what we want.

2. `512×512` doubles every dimension. That has two downsides: (a) it
   compounds the bug with a 4× memory-bandwidth jump and a quadrupled
   first-hit ray budget (slower frame, potentially crossing the
   `E2E_DRAIN_FRAMES = 8` capture bound), and (b) aspect-ratio stays 1:1,
   so the camera-history stale-projection issue stays masked — *which is
   what scope says*, but a square→square resize gives the auditor's
   `screenPosDistanceSqr > 16.0` reject (`taa.wgsl:349`) less opportunity
   to misbehave.

3. `384×288` (an aspect change, +50% width / +12.5% height) is a realistic
   user resize, modest enough that the GPU work stays within the
   `E2E_DRAIN_FRAMES` window, and it changes the aspect ratio so the
   reproduction does NOT accidentally hide the (out-of-scope)
   camera-history aspect issue — if Impl-B's fix is insufficient and the
   camera-history matrices ARE load-bearing, the test will fail anyway and
   we surface that fact in `## Assumptions made` rather than silently
   passing.

The gate region (`solid_block_rect` —
`crates/bevy_naadf/src/e2e/gates.rs:188-190`) is defined fractionally
(`Rect::from_fractional(fb, 0.42, 0.52, 0.58, 0.66)`), so it transparently
follows the new resolution — no rect recalibration needed.

### A.4 Frame budget after resize — 8 frames

The new `Resize` phase counts **`E2E_RESIZE_FRAMES = 8`** ticks after the
resize is issued, then transitions to SETTLE → SHOOT. The Bevy resize
typically propagates to `ExtractedCameraData` on the next frame, so:

- Frame 0 of Resize: write `Window.resolution`. winit's `changed_windows`
  runs in `Last`; the actual surface rescale happens at end-of-frame /
  start-of-next-frame.
- Frame 1: `Camera::physical_viewport_size()` reports the new size;
  `extract_camera` writes it; `prepare_taa` / `prepare_gi` rebuild +
  zero-clear.
- Frames 2-7: the bug window — the TAA ring is 32 deep (`taa_common.wgsl:20`)
  and the GI sample_counts is 128-frame, so 8 frames is well inside the
  drain window for both. Auditor measured collapse to ~4 luminance (vs
  healthy ~242) during this window per `01-context.md` line 56.

8 frames is also identical to `E2E_DRAIN_FRAMES = 8` (`mod.rs:131`) — the
existing async-readback budget — so the harness's frame-count tuning is
internally consistent.

Rejected: 32 frames — far enough for the TAA ring to fully refill
(`TAA_SAMPLE_RING_DEPTH = 32`), so the bug masks. The whole point of TDD
here is to fail in the drain window, not after recovery.

Rejected: 2-3 frames — risks racing the resolution-propagation latency
(some platforms take 1-2 frames for the surface to actually rescale).
8 frames gives 1-2 frames of slack for propagation + 4-7 frames inside the
bug window.

### A.5 Assertion shape — reuse `assert_batch_6`'s `MIN_GI_BOUNCE_AFTER_MOTION` ≥ 150.0 threshold

The Assert phase reuses the EXISTING `batch_gate(CURRENT_BATCH, &state)`
call in `crates/bevy_naadf/src/e2e/driver.rs:322` unchanged. We are at
Batch 6 (`assert_batch_6` — `gates.rs:537-600`), and its third sub-check is
*exactly* the discriminator we need:

```rust
// gates.rs:584-597
if solid_lum < MIN_GI_BOUNCE_AFTER_MOTION {
    return Err(format!(
        "Batch 6: TAA camera-motion reprojection decay — \
         the GI-lit diffuse geometry measured luminance {solid_lum:.1} \
         at the post-camera-motion readback pose (expected >= {MIN_GI_BOUNCE_AFTER_MOTION}, …"
    ));
}
```

`MIN_GI_BOUNCE_AFTER_MOTION = 150.0` (`gates.rs:643`). Measured healthy
value at the readback pose is ~235–242; a TAA/GI ring drain collapses it to
~4-6 (audit measurement, `01-context.md` line 56, `00-reuse-audit.md` line
29). 150 is comfortably above the broken regime and well below healthy.

No new gate function is needed. The existing `solid_block_rect` already
discriminates the symptom; the only thing changing is *when* we reach
ASSERT — after the resize phase, instead of after a no-resize SETTLE.

If we wanted resize-test-specific error wording, we'd write a new
`assert_resize_recovery(state)` — but that's gold-plating; `assert_batch_6`'s
existing error message already names the failure mode honestly ("the GI-lit
diffuse geometry measured luminance X.X at the post-camera-motion readback
pose"), and the test mode itself will be obvious from the `--resize-test`
CLI flag printed alongside.

### A.6 CLI / mode switch — `--resize-test` flag in the existing `e2e_render` binary, plumbed via `AppArgs.resize_test: bool`

A new `pub resize_test: bool` field on `AppArgs`
(`crates/bevy_naadf/src/lib.rs:188-228`), defaulting `false`. The
`e2e_render` binary's `main` (`crates/bevy_naadf/src/bin/e2e_render.rs:71-93`)
parses `--resize-test`, sets `app_args.resize_test = true`, and dispatches
through the existing `run_e2e_render_with_args` path (lib.rs:555-558).

The driver reads `app_args.resize_test` and only enters `E2ePhase::Resize`
when set. When `resize_test == false`, the state machine is the existing
WARMUP → MOTION → SETTLE → SHOOT (unchanged), preserving full bit-exact
behaviour for every other run mode.

Rejected: new `[[bin]]` target (`e2e_resize_render`) — duplicates
`build_app` / `run_with_app` plumbing; new binary means new pipeline
compile cost; the existing harness already handles all the e2e
infrastructure. `--resize-test` reuses 100% of the existing wiring.

Rejected: a separate `AppConfig::e2e_resize()` — same reason, plus
`AppConfig` is the wiring-shape struct and `resize_test` is an option to
the wiring, not a re-shaping. `AppArgs` is the right home (it already
carries `spawn_test_entity`, identical pattern — `lib.rs:227`).

### A.7 Test output on failure — `assert_batch_6` panic with measured luminance + PNG saved to `target/e2e-screenshots/e2e_latest.png`

The failing path leverages `run_assertions` (`driver.rs:280-358`) entirely
unchanged: it (a) unconditionally writes
`target/e2e-screenshots/e2e_latest.png` *before* the gate runs
(`driver.rs:292-302`), (b) prints `region_luminance_report` showing
emissive/solid/sky luminances (`driver.rs:316`), and (c) on the gate
failure prints the `assert_batch_6` error message including measured
`solid_lum` vs threshold (`gates.rs:584-597`).

On a failing test against `main`, the user / orchestrator will see:

```text
e2e_render: screenshot saved to target/e2e-screenshots/e2e_latest.png
e2e_render: region luminance — emissive 234.5, solid(GI-lit diffuse) 4.3, sky 132.6  (…)
e2e_render: FAIL —
1 check(s) failed:

region gate:
  Batch 6: TAA camera-motion reprojection decay — the GI-lit diffuse geometry measured
  luminance 4.3 at the post-camera-motion readback pose (expected >= 150, mean rgba …).
  …
```

Unambiguous: measured-vs-threshold luminance is printed, the PNG is on disk
for visual confirmation, and the gate names the failure mode. No new
plumbing required for output.

---

## Fix design (Deliverable B)

### B.1 TAA fix — strategy (a) for `taa_samples` + `taa_sample_accum`, with `taa_dist_min_max` still zero-cleared

**For `prepare_taa` at `crates/bevy_naadf/src/render/taa.rs:286-464`:**
chosen strategy is **(a) — skip the zero-clear of `taa_samples` and
`taa_sample_accum` on resize**. The buffers are still *recreated* at the
new pixel count (they have to be — they're sized by `pixel_count *
ring_depth`), but the freshly-allocated buffer is left uninitialised; the
reproject pass's existing distance/screen-position/hash rejects in
`assets/shaders/taa.wgsl:325-369` discard the garbage.

Why this works without producing garbage frames:

- **`screenPosDistanceSqr > 16.0` reject (taa.wgsl:349)** — for a random
  uninitialised `vec2<u32>`, the unpacked sample distance + decompressed
  normal hash + extra_data are arbitrary, so when reprojected into the new
  screen the resulting `screen_pos_dif` is almost always > 4 px in any
  direction → rejected.
- **Hash reject (taa.wgsl:362-369)** — fresh `taa_samples[i].y` bits
  decode to a random hash; the centre + 8-neighbour hash test
  (`s.hash == valid_hash_center` || any of 8 neighbours) collides with
  probability ~9/65536 per sample — negligible.
- **Distance reject (taa.wgsl:330-333)** — the unpacked `s.dist` from
  garbage bits is uniformly distributed over the full f32 range; the
  `[dist_min × 1022/1024, dist_max × 1026/1024]` band is a tiny fraction
  of f32 space → rejected.

In short, on a wgpu backend the freshly-allocated buffer is unmapped device
memory (typically zero on Vulkan/D3D12/Metal by driver convention — but we
DON'T rely on that). Whatever the bits decode to, the three rejects discard
~all of them; the rejected-sample count `color_sum.a` stays at 0; the
reproject path falls through to the single-current-frame estimate; TAA
recovers on the *next* frame as new samples are written into the ring
slot-by-slot.

`taa_dist_min_max` (the `base/renderTaaSampleReverse.fx` extra output,
`taa.rs:244-253`) is **still zero-cleared** — it's a single-frame output
(written by the reproject pass, read by the sample-refine pass the same
frame), not a ring. Zero-clearing it is harmless because the writers
overwrite it before any reader sees it. We keep the clear for safety
without cost.

**Code shape — diff sketch for `prepare_taa`:**

```rust
// taa.rs:373-387 — current behaviour
//
// Old:
//   if needs_new_storage {
//       let mut encoder = …;
//       encoder.clear_buffer(&taa_samples, 0, None);          // <-- DROP this
//       encoder.clear_buffer(&taa_sample_accum, 0, None);     // <-- DROP this
//       encoder.clear_buffer(&taa_dist_min_max, 0, None);     // keep
//       render_queue.submit([encoder.finish()]);
//   }
//
// New: split the clears between the FIRST-build path and the RESIZE path.
//   The first-build path (existing.is_none()) still zeroes everything (no
//   in-flight content to lose). The resize path (existing.is_some() and
//   pixel_count differs) zeroes only `taa_dist_min_max`.
```

Concretely, thread a second bool — `was_resize: bool` — through the match,
or split `needs_new_storage` into `(needs_new_storage, was_resize)`:

```rust
let (…, needs_new_storage, was_resize) = match &existing {
    Some(taa) if taa.pixel_count == pixel_count => (…, false, false),
    Some(_) => (…, true, true),   // <— resize
    None    => (…, true, false),  // <— first build
};
…
if needs_new_storage {
    let mut encoder = …;
    if !was_resize {
        // First build only — zero the ring contents.
        encoder.clear_buffer(&taa_samples, 0, None);
        encoder.clear_buffer(&taa_sample_accum, 0, None);
    }
    encoder.clear_buffer(&taa_dist_min_max, 0, None);  // always
    render_queue.submit([encoder.finish()]);
}
```

Rejected alternative (b) "warm resize / preserve+resample the ring": would
require a CPU-side blit / GPU staging copy at a different resolution. The
ring is slot-major (pixel_count × ring_depth); resampling it to a new
pixel_count is either a) per-slot bilinear, which is far more complex than
the reject-everything path achieves on its own, or b) preserve-and-pad,
which leaks the OLD aspect ratio into the new buffer with all the
camera-history aspect issues compounded. Not load-bearing — strategy (a) is
sufficient.

Rejected alternative (c) "preserve zero-clear, force explicit
single-cycle invalidation of the reprojector": equivalent in effect to (a)
on the first post-resize frame (single fall-through-to-current-frame
estimate), but trades a single frame of zero TAA history (visible as
extra noise) for the multi-frame drain. Net: identical behavior frames
2..N, worse on frame 1. (a) wins.

### B.2 GI fix — strategy (a) for `sample_counts`, also conditional on resize

**For `prepare_gi` at `crates/bevy_naadf/src/render/gi.rs:224-266`:** chosen
strategy is the GI analogue of (a) — **stop calling `create_gi_buffers` on
resize for the fixed-size `sample_counts` buffer** specifically. Keep
recreating the pixel_count-sized buffers (they have to grow), but preserve
the existing `sample_counts` `Buffer` across the resize.

Why this works: `sample_counts` is sized by `SAMPLE_COUNTS_LEN`
(`crates/bevy_naadf/src/render/gi.rs:497-501`) — **128 + 3 entries, NOT
pixel_count-sized** — so it is dimensionally invariant under resize. The
comment at gi.rs:243-247 explicitly says this:

```rust
// `sample_counts` + the three indirect buffers are fixed-size,
// created once and kept. `sample_counts` MUST NOT be re-zeroed on a resize
// it survives — it carries the 128-frame ring; but on a viewport change the
// screen-space sample buffers are discarded, so the ring's contents become
// stale anyway — re-zero it then too (the next 128 frames rebuild it).
```

The comment authors *knew* this was a hack — they re-zeroed `sample_counts`
on resize specifically because the screen-space sample buffers around it
were being discarded. With Impl-B's TAA fix (B.1), the screen-space buffers
in TAA are no longer being functionally discarded — the new buffer is
left to be rejected/refilled. The same reasoning extends to GI: the
128-frame `sample_counts` ring carries information that *is* still partly
valid after a resize (it's an N-frame accumulation, not a per-pixel
projection), and the `refineBuckets` `< 12` gate
(`sample_refine.wgsl:706-708`) will close any bucket that has truly stale
data via its own logic.

**Code shape — diff sketch for `prepare_gi`:**

The current match arm at gi.rs:248-265 has only two branches: "same
pixel_count → clone everything" and "anything else → `create_gi_buffers`
wholesale". We split the second branch into "resize" (rebuild screen-space
buffers, *keep* `sample_counts`) and "first build" (build everything,
zero `sample_counts`).

```rust
let resources = match &existing {
    Some(gpu) if gpu.pixel_count == pixel_count => GiBuffers { …, fresh: false },
    Some(gpu) => {
        // RESIZE: rebuild the pixel_count- and bucket_count-sized buffers,
        // but PRESERVE `sample_counts` (it's fixed-size and carries the ring).
        let mut new_buffers = create_gi_buffers_screen_only(&render_device, &render_queue, pixel_count, bucket_count);
        new_buffers.sample_counts = gpu.sample_counts.clone();
        new_buffers
    }
    None => create_gi_buffers(&render_device, &render_queue, pixel_count, bucket_count),
};
```

`create_gi_buffers_screen_only` is a new helper that mirrors
`create_gi_buffers` (gi.rs:443-579) but skips the `sample_counts`
allocation + its zero-clear. It is the minimum new code; alternative is
to inline the buffer creation in the Resize arm with a `gpu.sample_counts.clone()`.

Rejected: keep `create_gi_buffers` as-is and just clone `sample_counts`
into the new struct after the call. That would still ALLOCATE a new
`sample_counts` buffer and immediately throw it away — wasteful but
functionally identical. The split-helper approach is cleaner.

Rejected: preserve all of `valid_samples` / `invalid_samples` /
`valid_samples_refined` / etc. across resize — they ARE pixel_count- or
bucket_count-sized so they have to be re-allocated, and they're written
fresh per frame by the GI pipeline anyway (no temporal accumulation).
Keeping them would mean wgpu blit copies at the wrong sizes — out of scope.

### B.3 Cross-system ordering — no coordination needed

`prepare_taa` and `prepare_gi` both run in `RenderSystems::PrepareResources`
(`crates/bevy_naadf/src/render/taa.rs:268`, `gi.rs:215-223`). Both read the
same `ExtractedCameraData.viewport_size` and apply the same fix
independently. There is no shared resource between them that the resize
fix needs to coordinate. The interaction with `prepare_frame_gpu`'s
`first_hit_data` / `final_color` rebuild (`prepare.rs:600-668`) is
unchanged — those buffers are written-before-read per frame, so their
zero-clear on resize is harmless and stays as-is.

### B.4 Stale-coord risk — single-frame fall-through-to-current-frame estimate

If we leave `taa_samples` uninitialised on the resize frame:

- **Frame 1 (the resize-trigger frame, before extract sees new size):**
  unchanged — old buffers, old pixel_count, old behaviour.
- **Frame 2 (extract sees new size, `prepare_taa` re-allocates):** the
  reproject pass walks the new ring; ALL slots have arbitrary bits;
  rejects fire for ~all samples; `color_sum.a` is near zero; the per-pixel
  fold falls through to "blend current-frame first-hit estimate with empty
  history" — exactly like the FIRST frame of a fresh app boot.
- **Frame 3+:** new samples written into `taa_samples[taa_index]` slots
  start passing the rejects (their hashes match, their distances are
  inside the band); the ring refills slot-by-slot at the standard rate; by
  frame 32 + 1 = 33 it is fully populated with new-resolution samples.

The worst-case symptom is **one frame of slightly noisier output than
steady-state** (the current-frame estimate without temporal smoothing).
That is dramatically better than 32 frames of pitch-black, and is
indistinguishable from the first frame after a TAA settings change — a
class of frame the renderer already handles correctly (see the same
single-frame "rebuild" behaviour around `extract.rs:144-152`'s
last-known-good guard).

The `sample_counts` ring carries old-resolution sample counters across
resize. The `refineBuckets` `< 12` gate
(`sample_refine.wgsl:706-708`) reads `new_valid_count +
new_invalid_count`, both derived from `sample_counts` entries written by
the SAME bucket-IDs in past frames. After a resize the bucket grid
(`bucket_grid_of(viewport)` — gi.rs:238) at the new viewport produces a
*different* set of bucket-IDs (the bucket layout is `bucket_count =
ceil(w/8) * ceil(h/8)`), so each new bucket inherits old counters from
*some* bucket at the old layout. Worst case: a bucket inherits inflated
counts → gate opens prematurely → emits compressed entries before the
spatial-resampling reservoirs are populated → one frame of slightly
miscalibrated GI. This is far less catastrophic than the current
behaviour (gate closed for 128 frames → black), and the spatial-resampling
chain self-corrects within 2-3 frames as fresh samples land.

### B.5 Bit-exact preservation on the no-resize path

The change is **strictly conditional** on the resize codepath:

- `prepare_taa`: when `taa.pixel_count == pixel_count` (steady state),
  `needs_new_storage = false` (taa.rs:323-330 unchanged); no buffer
  recreation, no clear submission, every clone identical. Bit-exact
  with today.
- `prepare_taa` FIRST build (`None` arm at taa.rs:345-370): with the new
  split, `was_resize = false`, so the zero-clear of `taa_samples` /
  `taa_sample_accum` still runs identically. Bit-exact with today.
- `prepare_taa` RESIZE (`Some(_)` mismatched-count arm): `was_resize =
  true`, the two `clear_buffer` calls for `taa_samples` /
  `taa_sample_accum` are *skipped*. This is the ONLY behavioural change.

Same for `prepare_gi`: steady state and first-build paths are unchanged;
only the resize arm skips `sample_counts` recreation + zero-clear.

Predicate stated precisely: **when `(existing.is_some() && taa.pixel_count
!= pixel_count)` for TAA, OR `(existing.is_some() && gpu.pixel_count !=
pixel_count)` for GI, the new behaviour is "keep ring contents
uninitialised / preserved"; otherwise, identical to today.**

### B.6 Interaction with fix #4 (extract_camera last-known-good)

`extract_camera` (`crates/bevy_naadf/src/render/extract.rs:121-165`) is
NOT touched. Its job is to prevent `viewport_size` from collapsing to a
degenerate `(0, *)` / `(*, 0)` / `(1, 1)` mid-resize. That guarantee is
upstream of our fix: we operate downstream on `prepare_taa` /
`prepare_gi` once `viewport_size` is already either (a) unchanged (no
resize trigger) or (b) a real new size (legitimate resize, our new path
kicks in).

The fix #4 invariant is preserved: `prepare_taa` / `prepare_gi` continue
to see `extracted_camera.viewport_size` as either "unchanged from last
frame" or "real new size", never bogus. Our change only refines what
happens in the "real new size" case.

---

## Implementer outline (Deliverable C)

### Impl-A — the failing test

**File: `crates/bevy_naadf/src/lib.rs`**
- Add `pub resize_test: bool` field to `AppArgs` (after line 227,
  alongside `spawn_test_entity`). Default `false` in `impl Default`
  (after line 238).
- No other changes here; existing `run_e2e_render_with_args(args:
  AppArgs)` (lib.rs:555-558) already plumbs `AppArgs` through.

**File: `crates/bevy_naadf/src/e2e/mod.rs`**
- Add `pub const E2E_RESIZE_FRAMES: u32 = 8;` alongside the other
  constants (after line 131).
- Add `pub const E2E_RESIZE_WIDTH: u32 = 384;` and `pub const
  E2E_RESIZE_HEIGHT: u32 = 288;` for the post-resize physical resolution.
- No driver-system reordering — the new `Resize` phase is internal to
  `e2e_driver`.

**File: `crates/bevy_naadf/src/e2e/driver.rs`**
- Add new variant `Resize` to `E2ePhase` (driver.rs:57-76), positioned
  between `Motion` and `Settle`. Doc comment: "Programmatic
  `Window::resolution` change; counts `E2E_RESIZE_FRAMES` ticks
  post-resize to land inside the TAA/GI ring drain window."
- Add `mut window: Single<&mut Window, With<PrimaryWindow>>` parameter to
  `e2e_driver` (driver.rs:108-118). Also import
  `bevy::window::PrimaryWindow`.
- In the `E2ePhase::Motion` end-of-phase transition (driver.rs:155-165),
  conditionally branch on `app_args.resize_test`:
  - If true: transition to `E2ePhase::Resize`, `phase_ticks = 0`.
  - Else: transition to `E2ePhase::Settle` as today.
- Add new match arm `E2ePhase::Resize`:
  - On `phase_ticks == 0`: mutate
    `window.resolution.set_physical_resolution(E2E_RESIZE_WIDTH,
    E2E_RESIZE_HEIGHT)`. Pin the camera at the readback pose (same as
    SETTLE — `e2e_orbit_camera_transform(1.0)`) so the camera-pose-coupled
    region rects stay valid post-resize.
  - On `phase_ticks > 0`: keep the camera pinned at the readback pose.
  - On `phase_ticks >= E2E_RESIZE_FRAMES`: transition to
    `E2ePhase::Settle`, `phase_ticks = 0`.
- Update the docstring in driver.rs:11-22 to show the optional Resize
  phase.

**File: `crates/bevy_naadf/src/bin/e2e_render.rs`**
- After line 77 (`let edit_mode = …`), parse `let resize_test = args.iter().any(|a| a == "--resize-test");`.
- Add a third branch to the `if entities_mode { … } else { … }` (line 86):
  if `resize_test`, build `AppArgs::default()` with `app_args.resize_test
  = true` and dispatch through `bevy_naadf::run_e2e_render_with_args`.
  Else dispatch through `run_e2e_render` as today.
- Update the module docstring to document the `--resize-test` flag.

**No changes to `gates.rs`, `framebuffer.rs`, `readback.rs`, `checks.rs`,
`taa.rs`, `gi.rs`.** The assertion is the existing `assert_batch_6` →
`MIN_GI_BOUNCE_AFTER_MOTION = 150.0` gate (gates.rs:584-597).

**Smoke-run gate:** Impl-A's smoke run is `cargo run --bin e2e_render --
--resize-test`. Expected exit code: non-zero. Expected output: PNG saved
to `target/e2e-screenshots/e2e_latest.png`, `region_luminance_report`
showing `solid` ~ 4-6, `region gate` failure citing measured solid_lum
below MIN_GI_BOUNCE_AFTER_MOTION.

### Impl-B — the fix

**File: `crates/bevy_naadf/src/render/taa.rs`**
- In `prepare_taa` (taa.rs:286-464), extend the match at lines 315-371 to
  produce `(buffers..., needs_new_storage, was_resize)`. The arms
  become:
  - Same pixel_count: `(…, false, false)` (no change).
  - `Some(_)` with mismatch: `(…, true, true)`. (NEW: was_resize flag.)
  - `None`: `(…, true, false)`.
- Modify the zero-clear block at lines 379-387 to:

  ```rust
  if needs_new_storage {
      let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
          label: Some("naadf_clear_taa_buffers"),
      });
      // Resize-aware: the screen-space sample ring is NOT zero-cleared on a
      // resize, only on first build. The reproject pass's hash + distance +
      // screen-position rejects discard the new-buffer's arbitrary bits in
      // ~all cases; this lets the TAA ring refill from frame-1 instead of
      // taking 32 frames to drain through zero history → fixes the
      // "shadows-go-black after resize" bug (`docs/orchestrate/taa-resize-blackness/02-design.md`).
      if !was_resize {
          encoder.clear_buffer(&taa_samples, 0, None);
          encoder.clear_buffer(&taa_sample_accum, 0, None);
      }
      // `taa_dist_min_max` is a per-frame single-shot output, not a ring; the
      // zero-clear is always safe + the reproject pass overwrites it before
      // any reader sees it.
      encoder.clear_buffer(&taa_dist_min_max, 0, None);
      render_queue.submit([encoder.finish()]);
  }
  ```

**File: `crates/bevy_naadf/src/render/gi.rs`**
- Split `create_gi_buffers` into two functions: a private
  `create_gi_buffers_inner(…, include_sample_counts: bool, include_indirects: bool)`
  containing the shared body, and the existing public `create_gi_buffers`
  delegating to it with both flags `true`. Add a new helper
  `create_gi_buffers_screen_only` delegating with both flags `false` —
  it allocates the screen-space-only buffers, skipping `sample_counts` +
  the three indirect buffers + their clears/seeds.
- Modify the match at gi.rs:248-265 to handle the resize arm:
  ```rust
  let resources = match &existing {
      Some(gpu) if gpu.pixel_count == pixel_count => GiBuffers { …existing clone…, fresh: false },
      Some(gpu) => {
          // RESIZE: rebuild the pixel_count / bucket_count buffers, but
          // PRESERVE `sample_counts` (fixed-size, carries the 128-frame ring
          // — zero-clearing it caused the post-resize black-shadow drain
          // bug; see `02-design.md`).
          let mut new_buffers = create_gi_buffers_screen_only(&render_device, &render_queue, pixel_count, bucket_count);
          new_buffers.sample_counts = gpu.sample_counts.clone();
          new_buffers.valid_dispatch = gpu.valid_dispatch.clone();
          new_buffers.invalid_dispatch = gpu.invalid_dispatch.clone();
          new_buffers.ray_queue_indirect = gpu.ray_queue_indirect.clone();
          new_buffers.gi_params = gpu.gi_params.clone();
          new_buffers
      }
      None => create_gi_buffers(&render_device, &render_queue, pixel_count, bucket_count),
  };
  ```
- Update the stale comment at gi.rs:243-247 ("but on a viewport change the
  screen-space sample buffers are discarded, so the ring's contents become
  stale anyway — re-zero it then too") to the new policy ("preserve
  `sample_counts` across resize: the 128-frame ring is fixed-size and
  carries information that out-lives a viewport change; cf. design B.2").

**No changes to `extract.rs`, the WGSL shaders, the render graph nodes, or
the e2e harness during Impl-B.**

**Smoke-run gate:** Impl-B's smoke run is `cargo run --bin e2e_render --
--resize-test`. Expected exit code: zero. Expected output: PNG saved,
`region_luminance_report` showing `solid` ~150+ (typically much higher,
near steady-state ~235), all gates pass.

---

## Decisions & rejected alternatives

- **Decision: Reuse `e2e_render` binary + `--resize-test` CLI flag.**
  Rejected: separate `[[bin]]` binary (`e2e_resize_render`), separate
  `AppConfig::e2e_resize()`. Why: zero-marginal-cost reuse of all existing
  e2e plumbing (`run_e2e_render_with_args`, `build_app`, `add_e2e_systems`,
  `Framebuffer`, `Screenshot::primary_window`); same pattern as
  `--entities` (lib.rs:227). What would flip the call: if mutating
  `Window::resolution` mid-run is incompatible with the bounded winit
  runner — verified not the case, `changed_windows` handles programmatic
  resolution writes (bevy_winit:387).

- **Decision: New phase `E2ePhase::Resize` between MOTION and SETTLE
  (counts 8 ticks).** Rejected: between WARMUP and MOTION (motion-frames
  would re-converge the GI, masking the bug). Rejected: between SETTLE and
  SHOOT (SETTLE is 1 frame; can't fit the resize-propagation latency + bug
  window in 1 frame). Why: rings have meaningfully filled by end of MOTION
  (~144 total accumulation frames), and 8 post-resize frames lands inside
  both the TAA-32 and GI-128 drain windows. What would flip the call:
  measuring that post-resize propagation takes >8 frames on this hardware,
  pushing E2E_RESIZE_FRAMES higher.

- **Decision: Resize delta = 256×256 → 384×288 (aspect change).** Rejected:
  512×512 (4× memory cost, no aspect change). Rejected: 257×257 (trivial
  delta, doesn't exercise reprojection). Why: realistic resize, modest GPU
  cost, aspect change exposes whether the (out-of-scope) camera-history
  stale-projection issue is actually load-bearing for the test — surfaces
  scope creep early. What would flip the call: if 384×288 causes the
  camera-history stale-projection issue to dominate (test fails after
  Impl-B), we'd need to either drop to 384×256 (no aspect change) or
  expand scope to the camera-history fix.

- **Decision: TAA fix is strategy (a) — skip zero-clear, trust rejects.**
  Rejected: strategy (b) — warm resize with GPU blit/resample (too
  complex; rejected by the audit's reasoning). Rejected: strategy (c) —
  preserve zero-clear, force explicit one-cycle invalidation (equivalent
  effect for frames 2+, worse for frame 1). Why: the WGSL rejects in
  `taa.wgsl:325-369` already discard arbitrary samples by construction;
  letting them do their job means zero new code paths. What would flip the
  call: if the rejects fail to discard the new-buffer bits often enough to
  cause visible artefacts during the recovery window — would be visible as
  per-pixel sparkle in the smoke screenshot (the user inspects PNG output).

- **Decision: GI fix is the analogue — preserve `sample_counts` across
  resize.** Rejected: preserve every GI buffer (most ARE pixel_count-sized
  and must be re-allocated). Rejected: keep `create_gi_buffers` whole and
  swap `sample_counts` post-hoc (wastes an allocation + a clear). Why:
  `sample_counts` is `SAMPLE_COUNTS_LEN = 128+3` entries — fixed-size
  by NAADF design (`gi.rs:497-501`), so its contents are dimensionally
  invariant under resize. What would flip the call: if `bucket_grid_of`
  changes radically and pre-resize `sample_counts` entries map to
  post-resize buckets in such a way that the `< 12` gate produces *worse*
  output than zero-clear — empirically untested, but the spatial-
  resampling self-correction within 2-3 frames mitigates.

- **Decision: Reuse `assert_batch_6` / `MIN_GI_BOUNCE_AFTER_MOTION = 150.0`
  unchanged.** Rejected: new `assert_resize_recovery` helper with custom
  error wording. Why: the existing gate already discriminates exactly the
  shadow-blackness symptom (`solid_block_rect` collapse from ~242 to ~4),
  and its error message already names the failure mode. Adding a parallel
  helper duplicates the threshold logic. What would flip the call: if the
  resize regime needs a DIFFERENT threshold than the camera-motion regime
  (e.g. resize legitimately produces ~120 luminance for one frame and
  that's acceptable). Empirically the post-fix value should match
  steady-state ~235, so 150 is correct for both.

- **Decision: Conditional behaviour — split `needs_new_storage` into
  `(needs_new_storage, was_resize)`.** Rejected: a single bool flipping
  meaning (no, more confusing). Rejected: making `was_resize` available via
  a separate path (e.g. `existing.is_some() && taa.pixel_count !=
  pixel_count` re-derived later). Why: derive the bool once at the
  match site where the information naturally lives; explicit. What would
  flip the call: if the implementer prefers `bool resize: bool` carried on
  `TaaGpu` itself — fine, equivalent.

- **Decision: Do not touch `extract_camera`.** Rejected: extending the
  last-known-good guard. Why: it's already correct for the bogus 1×1 case
  (fix #4), and the user's described symptom is downstream (the multi-frame
  drain on a real-resize, not a 1×1-buffer collapse). What would flip the
  call: if the smoke run shows a fully-black frame (not solid-region-black-
  on-otherwise-correct-frame) — that would indicate the bogus-1×1 path
  is still firing and `extract_camera` needs additional work.

---

## Assumptions made

- **Assumption: The bug the user is reporting is the TAA-ring + GI
  `sample_counts` drain — NOT a residual `extract_camera` bogus-1×1
  collapse.** Confidence: high (audit explicitly distinguishes the two:
  `00-reuse-audit.md` borderline call #1). If wrong, this fails because:
  Impl-B fixes the wrong thing; the e2e test still fails post-Impl-B; the
  fix doesn't ship until the orchestrator runs a follow-up that escalates
  the `extract_camera` path. The smoke-run will tell us — full-black frame
  = wrong; solid-region-black-on-otherwise-correct = right (and fix lands).

- **Assumption: `TaaGpu.camera_history` stale matrices are NOT load-
  bearing for the failing test.** Confidence: medium. The audit's "Stale-
  coord / stale-dimension risk sites" §1 (`00-reuse-audit.md` lines 161-
  168) flags this as a known issue causing `screenPosDistanceSqr` rejects
  for ~128 frames post-resize, which means the camera-history is ACTUALLY
  helping our fix (it discards old-aspect entries). However, if the
  aspect-changing 256×256 → 384×288 resize causes those rejects to be
  *insufficient* and stale-history samples survive into the new screen,
  the test could fail even after Impl-B. If wrong, this fails because: the
  reject is too permissive, the user sees brief geometric distortion (not
  blackness) for one frame post-resize and the test passes via the
  luminance gate (which doesn't see geometric mis-projection) but the
  *user's actual problem* (the blackness) doesn't resurface — net: fix
  partially works, scope-deferred follow-up needed. The orchestrator
  should hold this assumption explicit for re-confirmation if smoke-run
  shows residual artifacts.

- **Assumption: The new-buffer bits from
  `Buffer::create_buffer(…COPY_DST | STORAGE…)` are not pathologically
  patterned in a way that systematically passes the WGSL rejects.**
  Confidence: high. wgpu does not guarantee initial contents (per spec)
  but on Vulkan / D3D12 / Metal the underlying API zero-fills new
  allocations by convention. Even if it did NOT zero-fill, the rejects
  (hash 16-bit space, distance band ~1024:1, screen-pos 4-px radius)
  would discard random bits at ~100% rate. If wrong, this fails because:
  resize produces visible geometric ghosting for 1-2 frames (the user's
  visual report says "shadows pitch-black", NOT "garbled image") — the
  empirical test would still pass the luminance gate; the user's complaint
  would not regress; but the smoke screenshot may show ghosts. We document
  the risk and rely on the user's visual check (per the GPU verification-
  loop rule).

- **Assumption: 8 post-resize frames is enough for `extract_camera` to see
  the new viewport size and `prepare_taa` / `prepare_gi` to rebuild.**
  Confidence: high. Bevy's `changed_windows` system runs in winit's `Last`
  schedule; the OS resize event fires synchronously or within 1 frame; the
  next frame's `extract_camera` polls `camera.physical_viewport_size()`
  which now returns the new size. Empirically Bevy handles this in 1-2
  frames. If wrong, the test would consistently see the *pre-resize*
  buffer at SHOOT — the test would PASS post-Impl-A even on the broken
  code (false negative). Mitigation: if the smoke run on `main` against
  Impl-A passes, that's the signal to bump `E2E_RESIZE_FRAMES`. The user
  can verify by inspecting the readback PNG resolution (should be 384×288,
  not 256×256).

- **Assumption: Out-of-scope per user — do NOT rebuild
  `TaaGpu.camera_history` matrices on resize.** Confidence: certain (user
  decision in `01-context.md` lines 18-19). If wrong: Impl-B's smoke run
  reveals residual artefacts that aren't blackness but ARE wrong, the
  orchestrator escalates back to user for scope expansion.

- **Assumption: The aspect-changing 384×288 target stays within wgpu /
  Bevy's surface configuration window-resize tolerances on this hardware.**
  Confidence: high (the target is a small, conventional resolution
  matched to a 4:3 aspect, with both dimensions well within typical
  texture / surface limits). If wrong: the resize would silently fail
  (winit logs a warning, surface stays at 256×256) → test would pass on
  broken code (false negative). Same mitigation as above.

- **Assumption: The Impl-B fix needs no shader-side changes.** Confidence:
  high — the WGSL rejects in `taa.wgsl` and the `< 12` gate in
  `sample_refine.wgsl` are exactly the mechanisms that make strategy (a)
  work; no new shader logic is needed. If wrong: shader-level fixes
  expand scope considerably; orchestrator pauses for user.

- **Assumption: The Impl-A test addition does not perturb any of the
  existing batch gates' values when `--resize-test` is NOT set.**
  Confidence: high — the new phase is gated by `app_args.resize_test`,
  defaulting `false`, so the default e2e harness state machine path is
  unchanged. The only risk is the `Single<&mut Window, With<PrimaryWindow>>`
  system parameter — its presence should not affect anything (Bevy's
  `Single<&mut Window…>` is read-only-until-modified at the dependency
  level; the system itself doesn't run extra logic when `resize_test ==
  false`). If wrong: the existing CI/e2e baselines drift. Mitigation: the
  smoke run for Impl-A without `--resize-test` should still pass; if it
  doesn't, that's a separate Impl-A bug to fix before the failing test
  lands.

---

## Risks & open questions for the orchestrator

- **Risk: 8 post-resize frames may not span the full drain window in the
  CI / test environment.** If the smoke run on Impl-A passes against
  `main`, that's the signal to bump `E2E_RESIZE_FRAMES`. The user should
  visually inspect the PNG: if the PNG shows the (broken) black-shadow
  symptom and the test passed, the threshold is wrong; if the PNG looks
  clean and the test passed, the resize hasn't propagated in time.

- **Risk: The post-resize camera-history aspect issue may surface
  visually as geometric distortion even after Impl-B fixes the blackness.**
  This is out-of-scope per user, but the smoke screenshot is the
  diagnostic. If the orchestrator sees ghosting, that's the trigger to
  re-confirm scope with the user.

- **Open question: Should Impl-A be considered green if the test fails
  with the expected error message (region gate, `solid_block_rect <
  150`), OR should it also assert the resize actually propagated (e.g.
  framebuffer dimensions == 384×288)?** Recommendation: the latter would
  be gold-plating — the existing PNG-on-disk output lets the user verify
  resolution visually, and adding a framebuffer-dimensions gate to
  `assert_batch_6` widens scope beyond the brief. Defer.

- **Open question: Does the user want a follow-up impl pass for the
  camera-history aspect-stale issue, or is this fix sufficient?** That
  decision belongs at the synthesis pause AFTER Impl-A confirms the bug
  manifests as expected and Impl-B confirms strategy (a) recovers.

- **Confirm: Impl-B should NOT additionally clear the
  `valid_dispatch` / `invalid_dispatch` / `ray_queue_indirect` /
  `gi_params` buffers in the resize arm.** The first three are
  indirect-arg buffers (5 × u32) reset in-shader / re-seeded per frame
  (`gi.rs:543-561`); `gi_params` is fully overwritten per frame
  (`gi.rs:357+`). Cloning them across resize is safe and avoids needless
  allocation. Re-confirm with user before Impl-B if there's any doubt.
