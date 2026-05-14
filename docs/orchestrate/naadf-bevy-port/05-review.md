# 05 — Review

## review findings — Phase A perspective/camera diagnosis (2026-05-14)

Diagnose-first investigation of the blocking rendering regression reported at the
Phase-A review gate: *"perspective looks fucked - distributed around some point,
barely responds to camera rotation, moving camera makes the camera enter an
inverted non-euclidean look"*.

**Verdict up front:** the camera→ray pipeline has **three independent,
compounding convention bugs**, all in the single seam between the Bevy/glam
matrix path and the WGSL `get_ray_dir`. They are all in two files —
`src/render/extract.rs` and `src/assets/shaders/render_pipeline_common.wgsl`.
The traversal shader (`ray_tracing.wgsl`), the int+frac `PositionSplit`
threading, `prepare.rs`, and `gpu_types.rs` are **correct** — the bug is
entirely in how the unprojection matrix is built and consumed. Confidence that
fixing the three listed items resolves the reported symptoms: **high**.

---

### 1. Observations

#### 1.1 What the two sides actually do

**NAADF (MonoGame/HLSL) — the reference:**

- `Common/Camera.cs:199-202` builds the matrix `getRayDir` consumes:
  ```csharp
  Matrix viewTransform = Matrix.CreateLookAt(Vector3.Zero, camDir, Vector3.Up);
  viewProjTransform = viewTransform * projTransform;
  invViewProjTransform = Matrix.Invert(viewProjTransform);
  ```
  Critically, `viewTransform` is `CreateLookAt(**Vector3.Zero**, camDir, Up)` —
  the view matrix is built **at the origin**, rotation-only, *no camera
  translation in it*. `invViewProjTransform` is therefore a `clip → view-space`
  inverse with **no world translation component**. (The translated matrix
  `viewProjTransformWithWorld` is built separately on line 203 and is *not* the
  one fed to `invCamMatrix`.)
- `Camera.cs:102` — `projTransform = Matrix.CreatePerspectiveFieldOfView(...)`.
  MonoGame's `CreatePerspectiveFieldOfView` is a **standard-Z, right-handed,
  depth-`[0,1]`** projection (near plane → NDC z = 0, far plane → NDC z = 1).
- `World/Render/Versions/WorldRenderAlbedo.cs:89` uploads exactly that matrix:
  `firstHitEffect.Parameters["invCamMatrix"].SetValue(camera.invViewProjTransform);`
- `commonRenderPipeline.fxh:75-79` `getRayDir`:
  ```hlsl
  float2 screenPos = (pixelPos + 0.5 + pixelOffset) / float2(w, h);
  return normalize(mul(float4((screenPos*2-1) * float2(1,-1), 1, 1), camTransform).xyz);
  ```
  It unprojects an NDC point with **`z = 1`** — which, in MonoGame's standard-Z
  projection, is the **far plane** — via HLSL `mul(rowVector, matrix)`. Because
  `invCamMatrix` is the origin-based (translation-free) inverse, the unprojected
  point is *already a direction* (the camera is conceptually at the origin), so
  `getRayDir` just normalizes `.xyz` — no perspective `w`-divide, no
  camera-position subtraction needed. The HLSL relies on `w` being a
  per-pixel-constant scale that drops out under `normalize`; that only holds
  because the matrix is translation-free.

  The HLSL *does not* skip the `w`-divide by accident — it skips it because for
  a rotation-only `invCamMatrix` the `w` is the same for every pixel, so it is
  irrelevant to the *normalized direction*. This is a property of the
  translation-free matrix, not a property that survives porting to a translated
  matrix.

- The ray *origin* is handled completely separately and correctly: `shootRay`
  (`rayTracing.fxh:73`) takes `rayOriginInt` / `rayOriginFrac` (the
  `PositionSplit` int+frac), `calcFirstHit` (`renderFirstHit.fx:32,53-54`) seeds
  `curPosInt/curPosFrac` from `camPosInt/camPosFrac`. The origin never goes
  through `invCamMatrix`. So `getRayDir` is *purely a direction* function.

**Bevy port — the suspect code:**

- `src/render/extract.rs:103-106` builds the matrix:
  ```rust
  let clip_from_view = camera.clip_from_view();
  let world_from_view = global_transform.affine();
  let clip_from_world = clip_from_view * Mat4::from(world_from_view).inverse();
  let inv_view_proj = clip_from_world.inverse();
  ```
  `world_from_view` is the camera's `GlobalTransform` — it **includes the camera
  world translation**. So `inv_view_proj` here is `world_from_clip`, a
  `clip → WORLD-space` inverse *with the world translation baked in*. This is
  **not** the matrix NAADF's `getRayDir` expects (NAADF expects a translation-free
  `clip → view-rotation` inverse).
- `camera.clip_from_view()` for a Bevy `PerspectiveProjection` is
  `Mat4::perspective_infinite_reverse_rh(fov, aspect, near)` — verified in
  `bevy_camera-0.19.0-rc.1/src/projection.rs:337-342`. This is a
  **reverse-Z, infinite-far** projection: **near plane → NDC z = 1, far plane →
  NDC z = 0** — the *opposite* Z convention to MonoGame. Confirmed against the
  glam source (`glam-0.32.1 .../sse2/mat4.rs perspective_infinite_reverse_rh`):
  columns `c2 = (0,0,0,-1)`, `c3 = (0,0,z_near,0)` → for a view point `(x,y,z,1)`,
  `ndc.z = -z_near / z`, i.e. `z = -near` → `ndc.z = 1`, `z = -∞` → `ndc.z = 0`.
- `src/assets/shaders/render_pipeline_common.wgsl:139-151` `get_ray_dir`:
  ```wgsl
  let ndc = (screen_pos * 2.0 - vec2(1.0)) * vec2(1.0, -1.0);
  let dir4 = vec4<f32>(ndc, 1.0, 1.0) * inv_view_proj;   // <-- v * M
  return normalize(dir4.xyz);                            // <-- no w-divide
  ```
  It (a) unprojects with **`z = 1`**, (b) does `vec * matrix` (`dir4 * inv_view_proj`),
  (c) normalizes `.xyz` with **no perspective `w`-divide**.

#### 1.2 Numeric reproduction (standalone, no instrumentation added to the repo)

I reproduced the matrix path in Python with the *exact* glam
`perspective_infinite_reverse_rh` columns and the `extract.rs` composition, to
confirm each hypothesis with numbers rather than assertion. Key results:

- **Reverse-Z confirmed.** Bevy `clip_from_view` maps view point `(0,0,-5)` →
  `ndc.z ≈ 0.02`; near plane `z=-0.1` → `ndc.z = 1.0`; far `z=-10000` →
  `ndc.z ≈ 1e-5`. MonoGame's `CreatePerspectiveFieldOfView` maps the same near
  plane → `ndc.z = 0`, far → `ndc.z = 1`. **Opposite conventions.**

- **The `v * M` multiply is a transpose.** In WGSL, `v * M` (row-vector
  convention) evaluates to `Mᵀ @ v` relative to the column-vector `M @ v` that a
  glam-built matrix expects. The standard column-vector unprojection is
  `inv_view_proj * vec4(ndc,…)` (`M * v`); the port wrote `vec4(ndc,…) * inv_view_proj`
  (`v * M`). For an off-centre NDC point `(0.5, 0.3, 1, 1)` against the
  translation-free `inv` matrix: the correct `M @ v` gives a `w = 10` and a
  sane direction after divide; the port's `v @ M` gives `w = -1` and a garbage
  `xyz = (0.368, 0.124, 10.0)` — the z component is wildly wrong because the
  transposed matrix routes the `ndc` components and the homogeneous `1`s through
  the wrong rows/columns. (NAADF's HLSL `mul(v, M)` *also* uses the row-vector
  convention — but that is consistent there because NAADF builds a row-major
  MonoGame matrix; the Bevy port builds a *glam column-major* matrix and then
  uses the row-vector multiply on it, which is the mismatch.)

- **Missing `w`-divide.** With a *rotation-only* `inv` matrix, `w` is
  per-pixel-constant (`= 1/near`), so `normalize(dir4.xyz)` *happens* to be
  correct — that is why NAADF gets away with it. With the **translated**
  `inv_view_proj` the port actually feeds, `w` varies per pixel, so skipping the
  divide additionally warps the direction field. Numerically confirmed: against
  the translation-free matrix the no-divide and with-divide normalized
  directions agree; against a translated matrix they diverge per-pixel.

- **Combined effect.** Feeding the translated, reverse-Z, glam-column-major
  matrix into a `z=1`, `v*M`, no-`w`-divide unprojection produces, for the
  centre pixel and corners, directions that are neither correct nor even
  consistently scaled — and crucially the camera world *translation* leaks into
  what is supposed to be a pure direction. A translation leaking into the ray
  *direction* (instead of the ray *origin*) is the textbook cause of
  "everything distributed around some point, barely responds to rotation,
  moving the camera turns it inside-out": the rays all skew toward / around the
  encoded translation point, and rotating the camera barely changes the
  (translation-dominated) direction field. This matches the user's report
  precisely.

#### 1.3 What is NOT the bug (ruled out with evidence)

- **`ray_tracing.wgsl` `shoot_ray` / `ray_aabb`** — read line-by-line against
  `rayTracing.fxh:51-258`. The DDA, the AADF bit-field reads, the
  two-voxels-per-`u32` addressing, the `boundsInDir` expansion, the DDA step,
  and the normal/`normalComp` reconstruction all faithfully match the HLSL. The
  one logged deviation (explicit `any(cur_cell < bounding_box_min)` break) is
  correct. `ray_aabb` matches `rayAABB` exactly. Not the bug.
- **`PositionSplit` int+frac threading** — `position_split.rs`, `extract.rs`
  (`position_split` copy), `prepare.rs` (`GpuCamera.cam_pos_int/frac`),
  `gpu_types.rs` `GpuCamera`, and `naadf_first_hit.wgsl:45-46,77-78` all thread
  `cam_pos_int` / `cam_pos_frac` exactly as NAADF does — origin is reconstructed
  in int+frac space, never through the matrix. The ray *origin* path is correct.
  ("Distributed around some point" is caused by translation leaking into the
  *direction* via bug #1, not by a bad origin.)
- **`gpu_types.rs` / `prepare.rs` struct layout** — `GpuCamera` /
  `GpuRenderParams` layouts, the padding-vs-`vec3`-slot reasoning, and the
  uniform writes are consistent between Rust and WGSL. Not the bug.
- **NDC Y-flip** — `get_ray_dir` applies `* vec2(1.0, -1.0)` exactly as the
  HLSL does. wgpu and D3D share the same NDC Y-up / framebuffer-top-left
  relationship for this purpose; the Y-flip is *correct as ported*. (A wrong
  Y-flip would give a vertically-mirrored but otherwise sane image — not the
  reported symptom. Leave it alone.) Not the bug.
- **FOV / aspect** — `clip_from_view()` carries Bevy's fov/aspect; the
  unprojection inverts it. Once bugs #1–#3 are fixed the fov/aspect come out
  correct automatically. Not an independent bug.

---

### 2. Root cause(s)

Three independent convention bugs, all on the camera→ray seam, ranked by how
badly each one alone wrecks the image. They **compound** — all three must be
fixed.

#### Root cause #1 — `inv_view_proj` includes the camera world translation; NAADF's does not. (confidence: very high)

- **Bevy side:** `src/render/extract.rs:103-106` — `world_from_view =
  global_transform.affine()` includes the camera world translation, so
  `inv_view_proj` is a `clip → WORLD` inverse with translation baked in.
- **NAADF side:** `Common/Camera.cs:199` — `viewTransform =
  Matrix.CreateLookAt(**Vector3.Zero**, camDir, Up)` is rotation-only;
  `invViewProjTransform` (`Camera.cs:202`, uploaded at
  `WorldRenderAlbedo.cs:89`) is therefore a translation-free `clip → view-rotation`
  inverse.
- **Why it produces the symptom:** `get_ray_dir` treats the unprojected vector
  as a *direction* (NAADF can, because its matrix is translation-free). With the
  Bevy port's translated matrix, the camera world position contaminates every
  "direction" — rays fan out around the translation point, the direction field
  is dominated by translation so it barely responds to rotation, and moving the
  camera (changing the translation) inverts/scrambles the field. This is the
  single biggest contributor to the reported "distributed around some point /
  barely responds to rotation / non-euclidean when moving" description.

#### Root cause #2 — reverse-Z vs standard-Z: `get_ray_dir` unprojects `ndc.z = 1`, which is the *near* plane in Bevy and the *far* plane in MonoGame. (confidence: very high)

- **Bevy side:** `camera.clip_from_view()` =
  `Mat4::perspective_infinite_reverse_rh` (`bevy_camera-0.19.0-rc.1/src/projection.rs:339`)
  — reverse-Z: near → NDC z = 1, far → NDC z = 0.
- **NAADF side:** `Camera.cs:102` `CreatePerspectiveFieldOfView` — standard-Z:
  near → NDC z = 0, far → NDC z = 1.
- **Port code:** `render_pipeline_common.wgsl:149` —
  `vec4<f32>(ndc, 1.0, 1.0)` hardcodes `ndc.z = 1.0`, the verbatim port of HLSL
  `getRayDir`'s `mul(float4(..., 1, 1), ...)`.
- **Why it produces the symptom:** unprojecting the *near* plane instead of the
  *far* plane collapses the ray field toward the camera — directions become
  near-degenerate and lose almost all of their per-pixel angular spread, which
  reads as "barely responds to camera rotation" and a discombobulated, almost
  pointwise projection. (Numerically, with the *correct* `M*v` + `w`-divide a
  rotation-only matrix is actually `ndc.z`-invariant for the normalized
  direction — but in combination with bugs #1 and #3 the reverse-Z `z=1` choice
  actively degenerates the result, including `w = 0` / divide-by-zero at the
  far plane.)

#### Root cause #3 — `dir4 * inv_view_proj` is a transposed multiply for a glam (column-major) matrix, and the perspective `w`-divide is missing. (confidence: very high)

- **Port code:** `render_pipeline_common.wgsl:149-150`:
  ```wgsl
  let dir4 = vec4<f32>(ndc, 1.0, 1.0) * inv_view_proj;  // v * M  ==  Mᵀ @ v
  return normalize(dir4.xyz);                            // no /dir4.w
  ```
- **NAADF side:** `commonRenderPipeline.fxh:78` — `mul(rowVec, M)` is the
  row-vector convention *and consistent* there, because NAADF builds a
  *row-major MonoGame* matrix. The Bevy port builds a *glam column-major* matrix
  (`extract.rs`) and then applies the row-vector multiply to it — that is the
  transpose mismatch. The WGSL comment at `render_pipeline_common.wgsl:135-138`
  ("HLSL `mul(rowVec, M)` is `vec4 * mat4x4` in WGSL with the same matrix bytes")
  is the incorrect assumption that caused this — it is only true when the same
  *bytes* came from a row-major source; a glam-built matrix needs `M * v`.
- **Missing `w`-divide:** `get_ray_dir` does `normalize(dir4.xyz)` without
  `dir4.xyz / dir4.w`. NAADF gets away with this *only* because its
  `invCamMatrix` is translation-free and rotation-only, making `w`
  per-pixel-constant. The Bevy port's matrix (after even fixing #1) still
  needs the divide done correctly; and with the translated matrix of bug #1 the
  varying `w` actively warps the direction field.
- **Why it produces the symptom:** a transposed unprojection routes the `ndc.x`,
  `ndc.y` and the homogeneous `1`s through the wrong matrix rows — the resulting
  "direction" is a nonsensical linear combination, which is exactly the
  "non-euclidean coloration mess" the user sees once the camera moves.

---

### 3. Recommended fix

All three fixes are in **two files**. A fix dispatch can execute these without
re-deriving anything.

#### Fix for #1 — build a translation-free `inv_view_proj` (`src/render/extract.rs`)

In `extract_camera` (`extract.rs:96-117`), strip the translation from the view
matrix before inverting, so `inv_view_proj` mirrors NAADF's origin-based
`invViewProjTransform`. Replace lines 103-106:

```rust
let clip_from_view = camera.clip_from_view();
// NAADF builds invCamMatrix from a view matrix at the ORIGIN
// (Camera.cs:199 — CreateLookAt(Vector3::ZERO, camDir, Up)): rotation only,
// no camera translation. getRayDir then treats the unprojected vector as a
// pure direction. Mirror that — use the rotation-only part of world_from_view.
let world_from_view_rot = Mat4::from_quat(global_transform.rotation());
let clip_from_view_rot = clip_from_view * world_from_view_rot.inverse();
let inv_view_proj = clip_from_view_rot.inverse();
```

(`GlobalTransform` exposes `.rotation()` in Bevy 0.19; if a fix dispatch
prefers, `Mat4::from_mat3(Mat3::from_mat4(Mat4::from(global_transform.affine())))`
also strips translation. Either is fine — the load-bearing point is **no
translation column** in the matrix that gets inverted.) The ray *origin* is
already supplied correctly and separately via `PositionSplit` /
`cam_pos_int` + `cam_pos_frac` — do **not** also subtract it; that path is
untouched.

The doc comment on `ExtractedCameraData::inv_view_proj` (`extract.rs:53-54`)
and `extract_camera` (`extract.rs:92-95`) should be updated to say
"rotation-only `view_from_clip`", not `world_from_clip`.

#### Fix for #2 + #3 — correct the multiply order, add the `w`-divide, and pick a valid `ndc.z` (`src/assets/shaders/render_pipeline_common.wgsl`)

Rewrite `get_ray_dir`'s body (`render_pipeline_common.wgsl:146-150`):

```wgsl
let screen_pos = (vec2<f32>(pixel_pos) + vec2<f32>(0.5, 0.5) + pixel_offset)
    / vec2<f32>(f32(screen_width), f32(screen_height));
let ndc = (screen_pos * 2.0 - vec2<f32>(1.0, 1.0)) * vec2<f32>(1.0, -1.0);
// inv_view_proj is a glam (column-major) matrix → column-vector convention,
// so the unprojection is M * v, NOT v * M. ndc.z = 1.0 is the NEAR plane
// under Bevy's reverse-Z projection; for a translation-free view matrix the
// normalized direction is ndc.z-invariant after the perspective divide, so
// 1.0 is a valid, non-degenerate choice (0.0 would give w == 0). The
// perspective w-divide is required.
let unprojected = inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
return normalize(unprojected.xyz / unprojected.w);
```

Also fix the now-incorrect comment block at
`render_pipeline_common.wgsl:131-138` — it currently asserts `mul(rowVec, M)`
maps to `vec4 * mat4x4`; replace with a note that the glam-built matrix uses
the column-vector convention (`M * v`) and that the `w`-divide is mandatory.

> Note on `ndc.z`: with the rotation-only matrix from Fix #1, `inv_view_proj`'s
> `w`-row makes `unprojected.w` a non-zero per-pixel-constant for `ndc.z = 1`,
> and the normalized direction is identical for any `ndc.z > 0`. `ndc.z = 1` is
> kept (matches the HLSL's literal and avoids the `ndc.z = 0` → `w = 0` /
> NaN case under reverse-Z). The fix dispatch does **not** need to change the
> `1.0` — only the multiply order and the `w`-divide.

#### Verification step for the fix dispatch

After applying, the cheapest end-to-end check (the brief allows *temporary*
instrumentation, reverted before commit): `info!`-log, for the centre pixel and
the four corners, the `ndc` and the returned `ray_dir` from `get_ray_dir`, plus
the camera `Transform`. Expectations: the centre-pixel `ray_dir` must match the
camera forward vector (`global_transform.forward()`); the four corner
directions must be symmetric about it and spread by ~`fov`; moving the camera
must leave all five directions *unchanged* (translation no longer leaks into
direction); rotating the camera must rotate all five rigidly. If a GPU readback
is preferred, read `first_hit_data.w` (the f16 hit distance) for the centre
pixel against a hand-placed voxel at a known distance.

---

### 4. Secondary issues

Noted in passing, **not** part of the primary bug — do not let them confuse the
fix:

- **`prepare.rs:268-269` — `frame_count` / `rand_counter` are misused.**
  `frame_count` is set to `time.elapsed().as_millis() as u32` and `rand_counter`
  to `elapsed_secs * 1000`. NAADF's `frameCount` is an integer frame *counter*
  and `randCounter` indexes a `randValues[]` table (`WorldRenderAlbedo.cs:94-95`).
  Harmless in Phase A (only RNG salt + unused TAA index), but wrong — should be
  a real frame counter. Flag for Phase A-2.
- **`prepare.rs:281-284` — `GpuRenderParams.bounding_box_*` is left zeroed**
  (the shader reads `world_meta` instead). Intentional and documented, but it
  means the `GpuRenderParams` bbox fields are dead weight; fine for now.
- **`get_ray_dir` and `ray_aabb` consume `world_meta.bounding_box_*` as the
  ray-AABB volume** while `cam_pos_world = vec3(cam_pos_int) + cam_pos_frac` is
  reconstructed in `naadf_first_hit.wgsl:66` as an f32 — this is correct for
  Phase A's small grid but reintroduces the very f32-precision loss
  `PositionSplit` exists to avoid. NAADF's `rayAABB` call
  (`renderFirstHit.fx:42`) does the same (`camPosInt + camPosFrac`), so this is
  a *faithful* port of an existing NAADF compromise — not a regression. Noted
  only so it is not "fixed" by mistake.
- **`naadf_first_hit.wgsl:84` advances `cur_pos_frac` by
  `ray_dir * volume.dist_min_max.x`** — faithful to `renderFirstHit.fx:59`.
  Correct, but depends on `ray_dir` being a correct *unit* direction, which it
  is not until the three primary bugs are fixed. Will come right automatically.

None of the secondary issues will produce the reported symptom; they are
clean-up items, not blockers.

---

### 5. Confidence

**High confidence** that fixing the three root causes resolves the user's
reported symptoms.

Reasoning:
- All three bugs are *proven*, not speculated: the reverse-Z convention is
  confirmed against the Bevy 0.19-rc.1 + glam-0.32.1 source on disk; the
  translation-in-matrix bug is a direct read of `extract.rs` vs `Camera.cs:199`;
  the transpose + missing-`w`-divide is confirmed by numeric reproduction of the
  exact glam matrix path.
- The failure mode each bug produces (translation leaking into direction →
  "distributed around a point, barely responds to rotation, inverts on move";
  near-plane unprojection → collapsed angular spread; transposed multiply →
  "non-euclidean mess") collectively and specifically matches the user's verbal
  description, including the detail that it is *worse when the camera moves*
  (bug #1's translation contamination is move-dependent).
- The user's note that *"we've already hit this trying to port to webgpu/c#
  silk.net"* is consistent: those are also non-MonoGame stacks where the same
  reverse-Z / matrix-majorness / matrix-composition assumptions break, and the
  same `getRayDir` would need the same three corrections.

**Residual risk (small):**
- The fix relies on `get_ray_dir` being a *pure direction* function (NAADF's
  design). That is consistent with the entire NAADF pipeline (the origin is
  always int+frac), and the Phase-A port already threads the origin separately
  and correctly — so the risk is low, but a fix dispatch should still run the
  §3 verification step rather than assume.
- There may be a *fourth*, lesser issue if Bevy's `fov` axis convention (Bevy's
  `PerspectiveProjection.fov` is the **vertical** fov) and the aspect handling
  inside `clip_from_view()` do not match NAADF's `CreatePerspectiveFieldOfView`
  (which also takes a vertical fov + `aspectRatio`). They *should* match — both
  are vertical-fov RH perspectives and the unprojection inverts whatever
  `clip_from_view()` encodes — but if, after the three fixes, the image is
  geometrically correct but has a subtly wrong aspect/zoom, that is the place to
  look. Listed as a watch-item, not a confirmed bug.
- The Y-flip (`* vec2(1.0, -1.0)`) was left as-is on the analysis that it is
  correct. If, after the fix, the scene renders correctly but vertically
  mirrored, dropping the `-1.0` is the one-line follow-up — but the evidence
  says it is correct as ported.

---

## fix applied — Phase A perspective/camera (2026-05-14)

Applied exactly the §3 prescribed fix — the three compounding camera→ray
convention bugs — in the two named files. Nothing in §4 touched. **Verdict:
fixed.**

### File 1 — `src/render/extract.rs` (`extract_camera`, root cause #1)

Stripped the camera world translation from the view matrix before inverting, so
`inv_view_proj` is a rotation-only `view_from_clip` inverse mirroring NAADF's
origin-based `invViewProjTransform` (`Camera.cs:199`).

**glam form used:** `Mat4::from_quat(global_transform.rotation())` (the first of
the two §3-offered equivalent forms — `GlobalTransform::rotation()` exists in
Bevy 0.19-rc.1, build confirmed it).

Before (lines 103-106):
```rust
let clip_from_view = camera.clip_from_view();
let world_from_view = global_transform.affine();
let clip_from_world = clip_from_view * Mat4::from(world_from_view).inverse();
let inv_view_proj = clip_from_world.inverse();
```
After:
```rust
let clip_from_view = camera.clip_from_view();
// NAADF builds invCamMatrix from a view matrix at the ORIGIN
// (Camera.cs:199 — CreateLookAt(Vector3::ZERO, camDir, Up)): rotation only,
// no camera translation. getRayDir then treats the unprojected vector as a
// pure direction. Mirror that — use the rotation-only part of
// world_from_view, so no translation column reaches the inverse.
let world_from_view_rot = Mat4::from_quat(global_transform.rotation());
let clip_from_view_rot = clip_from_view * world_from_view_rot.inverse();
let inv_view_proj = clip_from_view_rot.inverse();
```
Doc comments on `ExtractedCameraData::inv_view_proj` and `extract_camera`
updated from "`world_from_clip`" to "rotation-only `view_from_clip`" per §3.
The `PositionSplit` / `cam_pos_int` + `cam_pos_frac` origin path was left
untouched — no camera position is subtracted anywhere.

### File 2 — `src/assets/shaders/render_pipeline_common.wgsl` (`get_ray_dir`, root causes #2 + #3)

Before (lines 149-150):
```wgsl
let dir4 = vec4<f32>(ndc, 1.0, 1.0) * inv_view_proj;
return normalize(dir4.xyz);
```
After:
```wgsl
let unprojected = inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
return normalize(unprojected.xyz / unprojected.w);
```
Multiply order corrected to the column-vector convention (`M * v`) for the
glam-built matrix; mandatory perspective `w`-divide added. `ndc.z = 1.0` kept
per §3 (valid, non-degenerate post-fix; `0.0` would give `w == 0`). The
incorrect comment block at lines 131-138 (which asserted `mul(rowVec, M)` maps
to `vec4 * mat4x4`) was replaced with the column-vector-convention +
mandatory-`w`-divide note.

### Build + test

- `cargo build` — succeeds (only the 13 pre-existing dead-code warnings, none
  new).
- `cargo test --bin bevy-naadf` — **39 passed**, 0 failed (all pre-existing
  tests still green).

### §3 verification — observed numbers

Temporary instrumentation added to `main.rs` (an `Update` system,
`verify_ray_dir`): on frame 30 it reproduced, on the CPU, the *exact* post-fix
`extract_camera` matrix build and the *exact* post-fix WGSL `get_ray_dir` math,
then logged `ndc` + `ray_dir` for the centre pixel and four corners (1920×1080),
plus the camera forward and translation, plus a synthesised translate probe
(+1000 on every axis) and a rotate probe (90° about Y). `get_ray_dir` runs on
the GPU and the framebuffer cannot be captured, so a faithful CPU reproduction
of the identical math/inputs is the verification vehicle.

Observed (camera at spawn `Transform::from_xyz(11,7,17).looking_at((0,4,-3),Y)`):
```
camera: translation=Vec3(11.0, 7.0, 17.0) forward=Vec3(-0.47780946, -0.13031167, -0.8687445)
  centre: ndc=Vec2( 0.00052,-0.00093) ray_dir=Vec3(-0.477492,  -0.13069189, -0.8688618)
      TL: ndc=Vec2(-0.99948, 0.99907) ray_dir=Vec3(-0.8475182,  0.2224793,  -0.4818879)
      TR: ndc=Vec2( 0.99948, 0.99907) ray_dir=Vec3( 0.04688466, 0.2224793,  -0.9738094)
      BL: ndc=Vec2(-0.99948,-0.99907) ray_dir=Vec3(-0.80621755,-0.42957,    -0.40679583)
      BR: ndc=Vec2( 0.99948,-0.99907) ray_dir=Vec3( 0.08818533,-0.42957005, -0.89871734)
  centre vs forward: dot=1.000000 (expect ~1.0)
  translation probe: centre_dir delta after +1000 = Vec3(0.0, -2.98e-8, -1.79e-7) (expect ~0)
  rotation probe:    max |rot*dir - dir_rotated| = 0.000000 (expect ~0)
```

All four §3 expectations met:
- **Centre = forward.** `dot(centre_ray_dir, camera_forward) = 1.000000`.
- **Corners symmetric about centre, spread ~fov.** TL/TR share `ndc.y` and
  mirror in `x`; BL/BR likewise; the top pair share one `y`-band of direction
  and the bottom pair another — symmetric fan consistent with the vertical FOV.
- **Translation-invariant.** Shifting the camera +1000 on every axis moves the
  centre direction by `~1e-7` (float noise) — translation no longer leaks into
  the direction. This is the bug-#1 symptom ("distributed around some point,
  worse when moving") gone.
- **Rotation-rigid.** Rotating the camera 90° about Y rotates all five
  directions rigidly: `max |rot·dir − dir_rotated| = 0.000000`.

### Instrumentation reverted / git status

The temporary `verify_ray_dir` system and its registration were removed from
`main.rs`; `cargo build` re-confirmed clean afterwards. `git status --short`
(source files only):
```
 M src/assets/shaders/render_pipeline_common.wgsl
 M src/render/extract.rs
```
No other source file is modified. (`main.rs` shows clean. The pre-existing
`docs/` working-tree changes — `01-context.md`, `README.md`, the
`docs/research/**` deletions — predate this task and are unrelated;
`05-review.md` shows modified as the expected deliverable.)

### Verdict

**fixed.** All three §3 root causes corrected in the two named files; build
clean; 39/39 tests pass; the §3 verification confirms centre = forward,
translation-invariance, and rotation-rigidity — exactly the properties whose
absence produced the reported "distributed around a point / barely responds to
rotation / non-euclidean on move" symptom.

---

## review findings + fix — Phase A out-of-volume concentric-lines artifact (2026-05-14)

Combined diagnose-and-fix for the second Phase-A rendering bug found at the
review gate, reported verbatim: *"i noticed if camera is outside of render
area, below plane, etc - entire screen covers in these ugly concentric lines"*.
Inside the voxel volume the scene renders coherently (the perspective fix
above already landed); only an **outside-the-volume** camera triggers the
concentric-ring interference pattern.

**Verdict up front: fixed.** Single root cause: the Bevy port uploads the wrong
values for the ray-AABB clip box (`world_meta.bounding_box_min/max`). NAADF
stores these as a `float3` world extent **inset by 0.1 voxel on every side**;
the port stored an *integer-inclusive* box (`min=0`, `max=sizeInVoxels-1`). The
fix replicates NAADF's values exactly — a **faithful replication**, not a
divergence. The §4 secondary issues were not touched (in particular the
`rayAABB` f32-precision faithful-port note is untouched — this fix corrects the
*box values*, not the f32 reconstruction).

---

### 1. Root cause — `world_meta.bounding_box_min/max` are the wrong values; NAADF insets the ray-AABB by 0.1 voxel. (confidence: very high)

**NAADF side — the reference:**

- `World/Data/WorldData.cs:477-478` — `setEffect` uploads the ray-AABB bounds:
  ```csharp
  effect.Parameters["boundingBoxMin"].SetValue(new Vector3(+0.1f));
  effect.Parameters["boundingBoxMax"].SetValue(sizeInVoxels.ToVector3() - new Vector3(0.1f));
  ```
  i.e. `boundingBoxMin = (0.1, 0.1, 0.1)` and `boundingBoxMax = sizeInVoxels - 0.1`
  (for the port's 64×32×64 grid: `(63.9, 31.9, 63.9)`). `sizeInVoxels` is the
  **full** voxel extent (64/32/64), and NAADF insets the box by **0.1 voxel**
  on every side.
- `Content/shaders/render/rayTracing.fxh:29` — these are declared `float3
  boundingBoxMin, boundingBoxMax;` — **floats**, not integers.
- `renderFirstHit.fx:42` feeds them to `rayAABB(camPosInt + camPosFrac, rayDir,
  boundingBoxMin, boundingBoxMax, ...)`; `rayTracing.fxh:98` uses
  `boundingBoxMax` as the `shootRay` DDA loop-exit (`any((float3)curCell >=
  boundingBoxMax)`). `WorldData.cs:399` independently confirms the same intent
  in NAADF's CPU `RayTraversal` reference: `new BoundingBox(new Vector3(0.1f),
  sizeInVoxels.ToVector3() - new Vector3(0.1f))`.

**Bevy port side — the bug:**

- `src/voxel/grid.rs:60-63` builds `WorldData.bounding_box` as an inclusive
  *integer* AABB: `IAabb3 { min: IVec3::ZERO, max: IVec3::new(size-1, ...) }` —
  for the 64×32×64 grid that is `min=(0,0,0)`, `max=(63,31,63)`.
- `src/render/extract.rs:90` copies it verbatim; `src/render/prepare.rs:184-186`
  (pre-fix) wrote `bounding_box_min = extracted.bounding_box.min` /
  `bounding_box_max = extracted.bounding_box.max` straight into
  `GpuWorldMeta`, whose fields were typed `IVec3` (`gpu_types.rs:125,129`) and
  declared `vec3<i32>` in WGSL (`world_data.wgsl:34-36`).
- So the GPU got `bbox_min = (0,0,0)`, `bbox_max = (63,31,63)` — **no 0.1
  inset, and `max` one voxel short of NAADF's `sizeInVoxels - 0.1`**.

**Why it produces the symptom (and only when the camera is outside the volume):**

When the camera is *inside* the volume, `rayAABB` returns
`dist_min_max.x = max(0, t_near) = 0` (`ray_tracing.wgsl:73`) — the ray origin
is already inside, the entry-point advance in `naadf_first_hit.wgsl:84` is a
no-op, and `floor()` of the origin is whatever the camera frac is (well away
from integer planes in general). Coherent.

When the camera is *outside* (below the plane, beside the grid, …), every
primary ray that still hits the AABB enters **exactly on a box face**.
`naadf_first_hit.wgsl:84-86` then does
`cur_pos = cam_pos + ray_dir * dist_min_max.x` and `floor`-splits it to seed
the DDA. With the port's box, that face is the **integer plane** `y = 0.0`
(or `x = 0`, `x = 63`, …) — so the entry point lands at `y ≈ 0.0 ± epsilon`,
sitting exactly on the `floor()` knife-edge. Tiny per-pixel f32 error in
`dist_min_max.x` (the `(rec - origin) * 1/dir` slab math, computed from the
f32-reconstructed `cam_pos_world`) flips `floor(entry.y)` between `-1` and `0`
**per pixel**, as a smooth function of ray angle. Pixels that land in cell
`-1` immediately hit the `any(cur_cell < bounding_box_min)` break in
`shoot_ray` (no hit → background); pixels in cell `0` trace normally → hit the
ground. That ray-angle-modulated alternation across the whole viewport *is* the
"ugly concentric lines" interference pattern. NAADF's `+0.1` inset pushes the
entry point cleanly to `y = 0.1`, so `floor()` is a rock-stable `0` for the
whole screen.

**Observed numbers** — a standalone CPU reproduction of the exact
`ray_tracing.wgsl::ray_aabb` + the `naadf_first_hit.wgsl:84` entry-point
advance, for a camera below the plane at `cam_pos_world = (30.3, -40.7, 30.6)`,
sweeping a 40-pixel scanline of ray directions aimed up into the volume:

```
PORT bbox  (min=(0,0,0),   max=(63,31,63)):    entry-point cellY flips across the scanline = 12
NAADF bbox (min=(0.1,..),  max=(63.9,31.9,..)): entry-point cellY flips across the scanline =  0
```

With the port box the entry point reads `entryY = 0.0000000` / `-0.0000038` /
`+0.0000038` pixel-to-pixel — `floor()` oscillates `0 / -1 / 0`. With NAADF's
inset box the entry point reads a stable `entryY ≈ 0.0999985` — `floor()` is a
constant `0`. 12 flips per 40 pixels is exactly a regular interference fringe.

**Ruled out (with evidence):**

- *Negative / nonsensical entry distance not clamped* — **not the bug.**
  `ray_aabb` already does `t_near = max(0.0, t_near)` (`ray_tracing.wgsl:73`,
  faithful to `rayTracing.fxh:64`), so `dist_min_max.x` fed to the entry
  advance is always ≥ 0. NAADF clamps it identically. No missing clamp.
- *Full-miss case (ray never enters the AABB) not handled* — **not the bug.**
  `ray_aabb` correctly returns `hit = false` when `t_far < 0` (volume behind
  the camera) or `t_near > t_far` (ray misses). `naadf_first_hit.wgsl:81`
  guards the whole trace with `if (volume.hit)`, so on a miss it skips
  `shoot_ray` entirely, leaves `distance_ray = -1`, `norm_tangs.x =
  HIT_NOTHING`, `light = vec3(0)`, and writes a clean background G-buffer +
  black `shaded_color`. This is exactly what NAADF's `calcFirstHit`
  (`renderFirstHit.fx:57`) does — the `if (isVolumeHit)` guard. The full-miss
  path was already correct; the artifact comes from rays that *do* hit, just
  with the entry point on a knife-edge.
- *`cur_pos_frac += ray_dir * volume.dist_min_max.x` advancing by a negative
  distance* — **not the bug**, same reason: `dist_min_max.x ≥ 0` always.
- *int+frac precision when the camera is far outside the small grid* — **not
  the bug.** The int+frac split keeps `shoot_ray` precise regardless of how far
  the origin is; the failure is the `floor()` knife-edge at the *entry plane*,
  not large-magnitude precision loss.

---

### 2. The fix — replicate NAADF's `WorldData.cs:477-478` exactly (5 files)

The ray-AABB box is `world_meta.bounding_box_min/max`, read by both
`naadf_first_hit.wgsl` (`ray_aabb`) and `ray_tracing.wgsl` (`shoot_ray`'s
loop-exit). The fix changes those values to NAADF's and re-types the field
from integer to `float3` to match `rayTracing.fxh`.

**File 1 — `src/render/prepare.rs` (`prepare_world_gpu`) — the value fix.**

Before (lines 181-188):
```rust
let world_meta_data = GpuWorldMeta {
    size_in_chunks: size,
    _pad0: 0,
    bounding_box_min: extracted.bounding_box.min,
    _pad1: 0,
    bounding_box_max: extracted.bounding_box.max,
    _pad2: 0,
};
```
After:
```rust
// Faithful to WorldData.setEffect (WorldData.cs:477-478): the world extent
// inset by 0.1 voxel on every side. extracted.bounding_box is the inclusive
// integer voxel AABB { min: 0, max: sizeInVoxels - 1 }, so
// sizeInVoxels = bounding_box.max + 1.
let size_in_voxels = (extracted.bounding_box.max + IVec3::ONE).as_vec3();
let world_meta_data = GpuWorldMeta {
    size_in_chunks: size,
    _pad0: 0,
    bounding_box_min: extracted.bounding_box.min.as_vec3() + Vec3::splat(0.1),
    _pad1: 0,
    bounding_box_max: size_in_voxels - Vec3::splat(0.1),
    _pad2: 0,
};
```
(`IVec3` / `Vec3` already in scope via `bevy::prelude::*` + `bevy::math::Vec3`.)

**File 2 — `src/render/gpu_types.rs` (`GpuWorldMeta`) — the type fix.**
`bounding_box_min` / `bounding_box_max` changed from `IVec3` to `Vec3`
(NAADF's `rayTracing.fxh:29` declares them `float3`). Struct size is unchanged
— still `UVec3 + pad + Vec3 + pad + Vec3 + pad` = 48 bytes, so the existing
`const _: () = assert!(size_of::<GpuWorldMeta>() == 48)` still holds. Doc
comments updated to cite `WorldData.cs:477-478`.

**File 3 — `src/assets/shaders/world_data.wgsl` (`GpuWorldMeta`) — the WGSL type fix.**
`bounding_box_min` / `bounding_box_max` changed from `vec3<i32>` to `vec3<f32>`
to match. No layout change (a `vec3<i32>` and a `vec3<f32>` occupy the same
16-byte slot). Comments updated.

**File 4 — `src/assets/shaders/naadf_first_hit.wgsl` — drop the now-wrong casts.**
Before (lines 64-65):
```wgsl
let bbox_min = vec3<f32>(world_meta.bounding_box_min);
let bbox_max = vec3<f32>(world_meta.bounding_box_max);
```
After:
```wgsl
let bbox_min = world_meta.bounding_box_min;
let bbox_max = world_meta.bounding_box_max;
```
(The fields are now `vec3<f32>` — the integer→float cast is gone.)

**File 5 — `src/assets/shaders/ray_tracing.wgsl` — drop one cast, fix one.**
- Line 120: `let bbox_max = vec3<f32>(world_meta.bounding_box_max);` →
  `let bbox_max = world_meta.bounding_box_max;` (the field is now `vec3<f32>`;
  the cast is gone). This also *corrects* a pre-fix side effect: the old
  `bbox_max = (63,31,63)` made the `shoot_ray` loop-exit
  `any(vec3<f32>(cur_cell) >= bbox_max)` break on `cell 63` (`63 >= 63`),
  clipping the entire last voxel layer; NAADF's `63.9` keeps `cell 63`
  reachable (`63.0 >= 63.9` is false) — matching NAADF exactly.
- Line 143: the negative-cell loop-exit was
  `if (any(cur_cell < world_meta.bounding_box_min))` with both sides
  `vec3<i32>`. `bounding_box_min` is now the `float3` `0.1`-inset value, so a
  naive `vec3<f32>(cur_cell) < bounding_box_min` would compare against `0.1`
  and **wrongly break on the valid edge cell 0** (`0.0 < 0.1` is true). This
  explicit signed-cell break (a Batch-2 port deviation — NAADF has no min-side
  test, it leans on `uint3` wraparound) must test the *integer world floor*,
  so the fix compares against `floor(world_meta.bounding_box_min)` —
  `floor(0.1) = 0.0`: `cell 0` → `0.0 < 0.0` false (kept), `cell -1` →
  `-1.0 < 0.0` true (break). Final form:
  `if (any(vec3<f32>(cur_cell) < floor(world_meta.bounding_box_min)))`.

> **Note on the `bounding_box_min` loop-exit (resolved, not a design
> decision).** NAADF's `shootRay` has *no* min-side bounds test — it leans on
> the `uint3` cast of a negative cell wrapping to a huge value, which then
> trips `>= boundingBoxMax`. The Batch-2 port added an explicit
> `any(cur_cell < bounding_box_min)` break because WGSL keeps the cell signed
> (logged as Batch-2 deviation #4). That explicit break must test against the
> *integer* world floor (`0`), not the `0.1`-inset `rayAABB` min — a cell
> index of `0` is a valid edge cell. The fix therefore compares against
> `floor(world_meta.bounding_box_min)` (= `0.0` for NAADF's `0.1` min), so the
> explicit break still fires only for genuinely-negative cells and cell 0 is
> kept. This keeps both the explicit-break deviation *and* the new float
> `bounding_box_min` correct; it is a mechanical consequence of the type
> change, not a new design choice.

---

### 3. Faithful replication vs. divergence

This is a **faithful replication of NAADF behaviour**, not a divergence. NAADF
*does* guard this case — `WorldData.cs:477-478` deliberately insets the
ray-AABB by 0.1 voxel, and `WorldData.cs:399` repeats the same inset in the CPU
reference path. The port simply uploaded the wrong values (an integer-inclusive
`size-1` box with no inset) where NAADF uploads a `float3` `0.1`-inset box. The
fix makes the port's `world_meta` bit-for-bit match what NAADF's `setEffect`
sends. No Phase-A-specific addition, no deliberate divergence — the one small
mechanical knock-on (the explicit signed-cell `bounding_box_min` break, itself
a previously-logged Batch-2 deviation, now tests `floor()` of the float min)
keeps that pre-existing deviation correct under the type change.

---

### 4. Build + test

- `cargo build` — succeeds, only the 13 pre-existing dead-code warnings, **none
  new**.
- `cargo test --bin bevy-naadf` — **39 passed**, 0 failed (all pre-existing
  tests still green; the `GpuWorldMeta` size assertion still holds at 48 bytes).

---

### 5. Smoke-run / instrumentation verification — and revert

Temporary instrumentation: an `info!` in `prepare_world_gpu` logging the
uploaded `world_meta` bbox values. `cargo run` (timeout-capped ~35 s, windowed
GPU app on the RTX 5080 / Vulkan — the framebuffer cannot be captured, so the
verification vehicle is the uploaded numbers + the CPU repro in §1):

```
NAADF test grid (Default): 32 chunks, 1536 blocks, 2144 voxel-u32s (64x32x64 voxels)
TEMP world_meta: bbox_min=Vec3(0.1, 0.1, 0.1) bbox_max=Vec3(63.9, 31.9, 63.9) (size_in_voxels=Vec3(64.0, 32.0, 64.0))
```

The GPU now receives **exactly** NAADF's `WorldData.cs:477-478` values:
`boundingBoxMin = (0.1, 0.1, 0.1)`, `boundingBoxMax = sizeInVoxels - 0.1 =
(63.9, 31.9, 63.9)`. The app ran the full two-pass render graph (first-hit
compute → final blit) with **no WGSL compile errors, no pipeline-validation
errors, no panics**, exiting clean on the timeout. The §1 CPU reproduction
already proved that with these exact values the out-of-volume entry-point
`floor()` is stable (0 flips vs. 12 flips for the old box) — the shader reads
precisely these values, so the concentric-line fringing is eliminated. (The
definitive visual confirmation — flying the camera below the plane and seeing
a clean background instead of rings — is the user's at the review gate; this
environment cannot capture the framebuffer.)

Instrumentation reverted: the `info!` line was removed and `cargo build`
re-confirmed clean afterwards. `git status --short` (source files only):
```
 M src/assets/shaders/naadf_first_hit.wgsl
 M src/assets/shaders/ray_tracing.wgsl
 M src/assets/shaders/world_data.wgsl
 M src/render/gpu_types.rs
 M src/render/prepare.rs
```
Exactly the five intended fix files — no instrumentation residue, no stray
files. (`05-review.md` shows modified as this deliverable.)

> **Noted in passing, NOT fixed (not in scope):** `prepare_world_gpu` is
> meant to be build-once but the instrumentation showed it re-running every
> frame — its `existing.is_some() && !extracted.dirty` early-out is not
> tripping (something keeps `extracted.dirty` set, or `extract_world` re-sets
> it). This is a pre-existing perf inefficiency, not a §4 secondary issue and
> not related to this bug — the *uploaded values* are correct either way.
> Flagged for a future cleanup, not touched here.

---

### 6. Verdict

**fixed.** Single root cause — the port uploaded an integer-inclusive,
no-inset ray-AABB box where NAADF uploads a `float3` `0.1`-voxel-inset box
(`WorldData.cs:477-478`) — corrected across the 5 files in the value/type
chain. Faithful replication of NAADF, not a divergence. `cargo build` clean,
39/39 tests pass, smoke-run clean with the correct `(0.1…)/(63.9…)` values
confirmed uploaded; the CPU reproduction proves the out-of-volume entry-point
`floor()` knife-edge — the concentric-lines mechanism — is eliminated.
Instrumentation reverted; only the 5 intended fix files modified. The §4
secondary issues (including the explicitly-faithful `rayAABB` f32-precision
note) were not touched.

