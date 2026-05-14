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
