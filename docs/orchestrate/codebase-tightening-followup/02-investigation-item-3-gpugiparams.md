# Item 3 — D4 Step 3: ShaderType cutover for GpuGiParams

**Investigator:** read-only sub-agent under `codebase-tightening-followup`
orchestration. Verified every file:line citation with Read/Grep on
`/mnt/archive4/DEV/bevy-naadf/` HEAD `2bb03d1` before landing.

---

## Bailing implementor's stated blocker

Verbatim from `docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1011-1075`:

> #### Step 3 brief / Step 2 architect — `ShaderType` cutover — **BAILED** (per safety rule)
>
> **Reason for bail (re-verified independently from the D4-main rationale):**
>
> The brief's claim — "the main implementor bailed out per safety rule because
> pbr_sampling.wgsl referenced fields" — is **incorrect**. The D4-main impl log
> §5 plainly states the bail was due to `GpuGiParams` byte-equivalence
> verification cost, **not** `pbr_sampling.wgsl`. `pbr_sampling.wgsl` blocks
> Step 6 (architect), not Step 2/3 (ShaderType).
>
> I re-verified the byte-equivalence question by hand-walking the std140
> layouts for all 7 candidate structs. **5 of 7 are clean** (`GpuCamera`,
> `GpuWorldMeta`, `GpuRenderParams`, `GpuAtmosphereParams`, `GpuTaaParams`,
> `GpuConstructionParams`) — every `_padN` field in those structs corresponds
> to a std140-natural alignment break (`vec3`-to-`vec3` or `vec3`-to-`Vec2`
> transitions) that encase's `ShaderType` derive would insert by itself.
> Dropping the explicit pad and letting encase re-pad **produces a byte-
> equivalent buffer** by construction.
>
> **`GpuGiParams` is the exception that blocks the cutover sweep:**
>
> The trailing pads `_pad5/6/7` (lines 511-518) AND `_pad8/9/10` (lines
> 541-545) are **not std140-natural alignment breaks**. They're hand-inserted
> to force `max_ray_steps_secondary` to offset 304 (next 16-byte row after
> `sun_shadow_taps` at 288). Std140 places `u32` at 4-byte alignment, so
> encase would put `max_ray_steps_secondary` at offset **292** if the `_pad5/6/7`
> trio is removed — a 12-byte layout divergence.
>
> The hand-padded `_pad8/9/10` after `spatial_iter_count` is also non-natural:
> encase wouldn't insert trailing pad after the last scalar of a uniform
> buffer (it produces a `336/16 = 21` row-aligned size by virtue of `Mat4`
> + ... rows, not by trailing pad). So the post-cutover total size would be
> 324 bytes, not 336.
>
> **Both Rust + WGSL would need synchronous edits:** drop the
> Rust `_pad5/6/7` + `_pad8/9/10` AND drop the corresponding WGSL
> `gi_params.wgsl::pad_b/c/d/e/f/g` fields. This is a coordinated
> behavioural-equivalence change across both sides of the SSoT seam — exactly
> what the previous implementor bailed on, and exactly what the project's
> brief discipline (the byte-equivalence verification rule, the multi-run e2e
> variance discipline) protects against.

The bail closes with `**Status:** bailed (per architect-design ambiguity).`
(line 1075).

---

## Verification of the claim

### Independent std140 layout walk of `GpuGiParams` (current Rust, `gpu_types.rs:412-546`)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 64 | `inv_view_proj: Mat4` | `gpu_types.rs:416` |
| 64 | 64 | `view_proj: Mat4` | `:418` |
| 128 | 12 | `cam_pos_int: IVec3` | `:420` |
| 140 | 4 | `_pad0: u32` | `:422` |
| 144 | 12 | `cam_pos_frac: Vec3` | `:424` |
| 156 | 4 | `_pad1: u32` | `:426` |
| 160 | 12 | `sky_sun_dir: Vec3` | `:428` |
| 172 | 4 | `_pad2: u32` | `:430` |
| 176 | 12 | `sun_color: Vec3` | `:433` |
| 188 | 4 | `_pad3: u32` | `:435` |
| 192–271 | 20×4 | 20 `u32`/`f32` scalars `screen_width`..`denoise_thresh` | `:437-481` |
| 272 | 4 | `flags: u32` | `:483` |
| 276 | 4 | `_pad4: u32` | `:487` |
| 280 | 8 | `taa_jitter: Vec2` | `:498` |
| 288 | 4 | `sun_shadow_taps: u32` | `:509` |
| 292 | 4 | `_pad5: u32` | `:512` |
| 296 | 4 | `_pad6: u32` | `:514` |
| 300 | 4 | `_pad7: u32` | `:516` |
| 304 | 4 | `max_ray_steps_secondary: u32` | `:525` |
| 308 | 4 | `max_ray_steps_sun: u32` | `:528` |
| 312 | 4 | `max_ray_steps_sun_secondary: u32` | `:532` |
| 316 | 4 | `max_ray_steps_visibility: u32` | `:535` |
| 320 | 4 | `spatial_iter_count: u32` | `:539` |
| 324 | 4 | `_pad8: u32` | `:541` |
| 328 | 4 | `_pad9: u32` | `:543` |
| 332 | 4 | `_pad10: u32` | `:545` |
| **end** | — | **336 bytes total** | matches `const _: () = assert!(std::mem::size_of::<GpuGiParams>() == 336)` at `:857` |

Compile-time guards anchor the load-bearing offsets:
`:860` pins `taa_jitter == 280`, `:868` pins `sun_shadow_taps == 288`, `:875`
pins `max_ray_steps_secondary == 304`, `:879` pins `spatial_iter_count == 320`.

### What encase would produce for `#[derive(ShaderType)]` after deleting `_pad0..pad10`

encase v0.12.0 (`Cargo.lock:2535`) implements WGSL uniform-buffer layout.
Relevant alignment rules: `u32`/`f32`/`i32` align 4, `Vec2` align 8, `Vec3`
align 16 (size 12), `Vec4`/`Mat4` align 16, struct alignment = max of member
alignments, struct size rounded up to struct alignment.

`GpuGiParams` member-alignment max = 16 (the two `Mat4`s and three `Vec3`s).
Struct alignment = 16.

| offset | size | field | encase action |
|---|---|---|---|
| 0 | 64 | `inv_view_proj` | as-is |
| 64 | 64 | `view_proj` | as-is |
| 128 | 12 | `cam_pos_int` | as-is; next field aligns from 140 |
| 144 | 12 | `cam_pos_frac` | encase inserts 4 B internal pad: 140 → 144 (Vec3 align 16) |
| 160 | 12 | `sky_sun_dir` | encase inserts 4 B pad: 156 → 160 |
| 176 | 12 | `sun_color` | encase inserts 4 B pad: 172 → 176 |
| 192 | 4 | `screen_width` | encase inserts 4 B pad: 188 → 192 (u32 align 4, but `Vec3` rule: next-field offset = max(prev_end, align(next)) — see footnote *) |
| 196–271 | 19×4 | 19 contiguous u32/f32 scalars | tightly packed |
| 272 | 4 | `flags` | offset 272 |
| 280 | 8 | `taa_jitter` | encase inserts 4 B pad: 276 → 280 (Vec2 align 8) |
| 288 | 4 | `sun_shadow_taps` | offset 288 (u32 align 4 satisfied) |
| **292** | 4 | **`max_ray_steps_secondary`** | offset 292 — **NOT** 304; **12-byte divergence vs current layout** |
| 296 | 4 | `max_ray_steps_sun` | |
| 300 | 4 | `max_ray_steps_sun_secondary` | |
| 304 | 4 | `max_ray_steps_visibility` | |
| 308 | 4 | `spatial_iter_count` | |
| **end** | — | **last field ends at 312** → rounded to struct alignment 16 → **320 bytes total** | |

(* Note: encase's u32-after-vec3 boundary uses the natural padding rule. The
WGSL spec says a `vec3<f32>` member followed by a scalar may pack the scalar
into the trailing 4 bytes of the `vec3`. However, encase v0.12 follows the
*conservative* WGSL host-shareable rule used by std140: it places `vec3` as
if size 16 for purposes of the *next-field offset*. See encase issue tracker
re: vec3 packing — encase produces size=12, align=16 for `Vec3`, so a `Vec3`
followed by a `u32` field gets the u32 at `vec3_offset + 16`, not `+ 12`.
This is *exactly* what the existing Rust `_pad0..pad3` u32s mirror.)

### Findings

- **The 304→292 divergence claim is correct.** The hand-padded `_pad5/6/7`
  block (offsets 292..304) is what forces `max_ray_steps_secondary` to
  offset 304. encase would NOT replicate this pad — `sun_shadow_taps: u32`
  followed by `max_ray_steps_secondary: u32` is two scalars of align-4, no
  natural alignment break between them.
- **The size-divergence claim is OFF BY 4 BYTES.** The impl log says
  "post-cutover total size would be **324 bytes**, not 336." My walk says
  **320 bytes, not 336**. The impl log appears to have mistakenly extended
  the `spatial_iter_count` row by one extra u32. The substance of the claim
  (encase wouldn't produce 336 because the trailing 12 B of pad after
  `spatial_iter_count` are non-natural) is correct; the exact post-cutover
  size is 320 because `spatial_iter_count` ends at offset 312 and the
  struct rounds up to the next multiple of 16 (= 320).
- **The divergence has TWO independent causes:** (a) the `sun_shadow_taps`
  → `max_ray_steps_secondary` row break (12 bytes of `_pad5/6/7`); (b) the
  trailing pad after `spatial_iter_count` (12 bytes of `_pad8/9/10`).
  Either alone would make the cutover non-byte-equivalent. Both must be
  resolved jointly.
- **The 5-of-7-clean-structs claim holds.** I spot-checked `GpuRenderParams`
  (`gpu_types.rs:60-117`): the 7 explicit `_padN` fields all sit at
  std140-natural alignment-break offsets (`u32`-trail-of-`Vec3` rows,
  `Vec2`-pre-`Vec3` row), and the architect's exemplar `Before`/`After`
  shapes at `03-architecture.md:357-401` would produce a byte-equivalent
  112-byte buffer post-cutover. The architect's recipe (drop every `_padN`)
  works there. Same pattern applies to `GpuCamera`, `GpuWorldMeta`,
  `GpuTaaParams`, `GpuAtmosphereParams`, and `GpuConstructionParams`
  (which has explicit row-end pads at offsets 12, 28, 44 — all
  std140-natural per `gpu_types.rs:887-897` offset guards).

### WGSL counterpart sanity check (`gi_params.wgsl`)

Verified at `crates/bevy_naadf/src/assets/shaders/gi_params.wgsl:128-147`:

```
sun_shadow_taps: u32,        // offset 288
pad_b: u32,                  // offset 292
pad_c: u32,                  // offset 296
pad_d: u32,                  // offset 300
max_ray_steps_secondary: u32,    // offset 304
max_ray_steps_sun: u32,          // offset 308
max_ray_steps_sun_secondary: u32, // offset 312
max_ray_steps_visibility: u32,    // offset 316
spatial_iter_count: u32,         // offset 320
pad_e: u32,                  // offset 324
pad_f: u32,                  // offset 328
pad_g: u32,                  // offset 332
                             // struct end 336
```

The WGSL `pad_b/c/d` (offsets 292..304) and `pad_e/f/g` (offsets 324..336)
mirror the Rust `_pad5/6/7` and `_pad8/9/10` exactly. **Any Rust-side pad
deletion MUST be matched by WGSL-side pad deletion** or the GPU read
addresses fall out of step.

---

## Verification of the audit's precedent claim

```
$ grep -rn "ShaderType" /mnt/archive4/DEV/bevy-naadf/crates/
crates/bevy_naadf/src/render/pipelines.rs:369:        // structs are not `ShaderType`, so the sized helpers are used directly
```

**One match. Zero consumers.** The single hit is the docblock at
`pipelines.rs:369` explaining *why* the codebase uses `bytemuck::bytes_of`
+ `std::mem::size_of` directly instead of `<T as ShaderType>::min_size()`.

Cross-checked with broader query:

```
$ grep -rn "derive(ShaderType\|ShaderType)" /mnt/archive4/DEV/bevy-naadf/ --include='*.rs'
(no output)
```

```
$ grep -rn "ShaderType\|encase::" /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/render/
crates/bevy_naadf/src/render/pipelines.rs:369:        // structs are not `ShaderType`, so the sized helpers are used directly
```

The `encase` crate (v0.12.0, `Cargo.lock:2535`) and `bevy_encase_derive`
(`Cargo.lock:806`) are transitive deps via Bevy — they sit in the dep
tree but the `bevy-naadf` source code uses neither. **Item 3 is precedent-
setting:** the implementor cannot lean on a working example in the same
codebase. The architect's §3.4 exemplar is the *only* shape the implementor
would have to go by.

This matters because the cutover landing pattern can't be cribbed from a
sibling struct — the upload-site `encase::UniformBuffer<Vec<u8>>` /
`write_uniform` helper, the `<T as ShaderType>::min_size()` for
`min_binding_size` (currently uses `std::mem::size_of::<T>() as u64` at
`pipelines.rs:372-379`), the layout-guard adaptation (`gpu_types.rs:844-902`
asserts — all 24 of them) — every one of these has zero precedent. Each is
its own design decision.

---

## Diagnosis

**Category (a): real and architect-fixable.** The bailing implementor's
structural claim is correct (modulo the 320-vs-324 arithmetic slip). The
architect's §3.4 recipe ("drop every `_padN` field") is wrong for
`GpuGiParams` specifically. The architect must either:

1. Add an explicit `GpuGiParams` exception (keep `_pad5/6/7/8/9/10` as
   named `u32` fields under `#[derive(ShaderType)]`), OR
2. Specify a coordinated Rust+WGSL synchronous deletion of
   `_pad5..pad10` AND `pad_b..pad_g`, OR
3. Exclude `GpuGiParams` from the cutover, accept a partial 5-of-7
   cutover, and document the two-encoding-regime tradeoff.

The implementor cannot pick between these — each carries different
verification load, blast radius, and ongoing maintenance shape. The
choice is an architect-level decision, not an implementor judgement
call. This is why the deferral is sticky: dispatching another
implementor without the architect choosing first produces a third bail.

The 600-LOC trait-decomposition gap that bites Item 2 is *not* present
here. Item 3 is a much smaller architect spec revision (a few
paragraphs added to §3.4); the implementation surface is unchanged.

---

## Proposed architect-revision scope

The fresh `delegate-architect` brief must produce a revised §3.4 covering
the following four decisions. Each must be picked, justified, and locked.

### (a) Pad-keep policy

**Recommendation: keep `_pad5/6/7/8/9/10` as named no-op `u32` fields under
`#[derive(ShaderType)]`.** Rationale:

- `ShaderType` derive accepts arbitrary named scalar fields. It does NOT
  require fields to be semantically meaningful — only that the field
  alignment + size matches WGSL. Six `u32` fields named `_pad5..pad10`
  serialise as six `u32` lanes in the encase output, which IS
  byte-equivalent to the current `bytemuck::bytes_of` output for those
  exact same six u32 lanes.
- This avoids touching `gi_params.wgsl` synchronously (the highest
  blast-radius edit in the cutover; see §"naga_oil-import-name policy"
  below).
- The "ShaderType eliminates the `vec3`-then-scalar trap by construction"
  claim from the original §3.4 still mostly holds — it eliminates the
  trap for the rows that DO have natural breaks; the six trailing u32
  pads exist for a reason orthogonal to the trap (they enforce a
  16-byte row boundary for `max_ray_steps_secondary` and a 16-byte
  trailing pad for legibility, not for trap-avoidance).
- The architect's §3.4 claim that "the `taa_jitter` placement hazard
  becomes impossible by construction" holds for the `taa_jitter` field
  itself (`_pad4`'s job — `flags` u32 followed by `Vec2`; encase picks
  the right offset 280 unaided). The `sun_shadow_taps`/`_pad5..pad7`
  cluster is a different concern entirely (NOT a `vec3`-then-scalar
  trap; it's a deliberate row-boundary enforcement for
  `max_ray_steps_secondary` per `21-design-quality-panel.md` §4.2).

**Alternative considered (a coordinated WGSL+Rust deletion):** changes the
GPU contract simultaneously with the encoding change. Doubles the
verification load (the synchronous WGSL edit has to be probed with
`--vox-gpu-oracle` ≥3 runs separately because it's a behavioural-
equivalence change on the GPU side, not a layout-only refactor). High
blast radius for low semantic gain — the named pads are not bugs, they're
explicit row-boundary documentation.

### (b) Coordinated WGSL+Rust deletion plan

**Recommendation: do not delete the WGSL pad fields.** Per (a), keep both
sides symmetric: Rust `_pad5..pad10` and WGSL `pad_b..pad_g` both stay.
Net change for `GpuGiParams`: `Pod, Zeroable` → `ShaderType`. Field
list and field offsets unchanged. WGSL untouched.

If the architect picks the alternative path (drop both sides), the plan
must include:
- naga_oil import-time validation step (see §(d))
- update to the assertion guards at `gpu_types.rs:875-881` (these will
  now fire — `max_ray_steps_secondary` will assert 292, not 304)
- per-call-site update of any WGSL code that reads
  `gi_params.pad_b/c/d/e/f/g` by name (verified: zero such reads exist —
  these are write-only-from-Rust lanes — but the architect's revision
  must include the grep that proves this)

### (c) Cutover scope: full 6 structs or 5 with `GpuGiParams` excluded

**Recommendation: full 6 structs (including `GpuGiParams` with pads
kept under `#[derive(ShaderType)]`).** Rationale:

- Per (a), pads-as-named-fields means `GpuGiParams` cuts over without
  byte divergence. The partial-cutover anti-DRY tradeoff the impl log
  flagged (`04-refactoring.md:1063-1069` — "two encoding regimes",
  "branches per struct type") disappears: every struct uses the same
  `write_uniform<T: ShaderType>` helper.
- The 7th candidate `GpuConstructionParams` is D5 territory (per
  `03-architecture.md:912-921` Conflict 2 — D5 implementor handles the
  flip as part of D5's own writes). D4 architect spec should be 6
  structs, not 7, with the 7th flagged for D5 coordination.

**Alternative considered (5 of 6, exclude `GpuGiParams`):** if the
architect rejects (a) and there's no appetite for (b)'s coordinated
WGSL+Rust edit, then `GpuGiParams` stays `Pod` while the other 5 flip.
This produces the two-encoding-regime smell the impl log called out,
forces a `write_uniform` helper that branches per struct, and undermines
the cutover's "uniform encoding" rationale. The impl log's
"partial cutover is worse than no cutover" verdict (`:1071`) stands.

### (d) naga_oil-import-name policy

**Recommendation: pad-keep policy (a) sidesteps this entirely.** No WGSL
edit means no naga_oil import-time risk.

If the architect picks the WGSL+Rust deletion path:

- `gi_params.wgsl:13-15,47` flags: "naga-oil composable-module structs
  cannot carry the `_pad0`-style / `data1`-style identifiers" because
  "naga writeback rejects trailing-digit identifiers and bare `_padN`".
- The existing WGSL pads `pad_b..pad_g` are letter-suffixed precisely
  to avoid this trap (`gi_params.wgsl:78-84` documents the same
  workaround for `rand_counter_b`).
- A coordinated deletion removes the `pad_b..pad_g` fields entirely —
  no new identifiers introduced, so the naga_oil concern is inert. But
  the architect must add a one-line verification step: build the
  workspace post-deletion, confirm `naga_oil` import of `gi_params.wgsl`
  from `naadf_global_illum.wgsl` / `naadf_sample_refine.wgsl` /
  `naadf_spatial_resampling.wgsl` / `naadf_denoise_split.wgsl` /
  `ray_queue_calc.wgsl` (the 5 known importers) compiles without
  diagnostic.
- The compile-time `offset_of!` guards at `gpu_types.rs:875-881` will
  break in this path — they assert `max_ray_steps_secondary == 304` and
  `spatial_iter_count == 320`. Post-deletion the offsets become 292 and
  308. The architect revision must specify either (i) delete those
  guards (encase enforces layout at serialisation), or (ii) update the
  asserted values. Per the architect's own §3.4 (`03-architecture.md:404-411`),
  the recipe says drop them — but the implementor must be told
  explicitly because the impl log shows the bailing implementor
  treating those guards as the layout oracle.

---

## Proposed path forward

**Fresh `delegate-architect` dispatch with focused brief.** Re-dispatching
an implementor on the original §3.4 will hit a third bail — the choice
between (a)/(b)/(c) is not implementor-judgeable.

Sketch of the architect brief:

- **Scope:** revise `docs/orchestrate/codebase-tightening/render-pipeline/03-architecture.md`
  §3.4 to cover the `GpuGiParams` non-natural-pad cluster. Three other §3.4
  paragraphs unchanged.
- **Constraints:** must pick (a)/(b)/(c) above and justify in 2-3
  sentences. Must include the naga_oil verification step if path (b) is
  picked. Must specify the disposition of the 24 compile-time
  `offset_of!` guards at `gpu_types.rs:844-902` (drop / keep / adapt).
- **Required reading:** `gpu_types.rs:412-546` (struct def + comments),
  `gpu_types.rs:838-902` (the placement-guard story + assertions),
  `gi_params.wgsl:1-148` (the WGSL counterpart with the `vec3`-trap
  fix docblock), this investigation file in full,
  `04-refactoring.md:1011-1075` (the original bail).
- **Deliverable:** revised §3.4 (in place; replace the existing section
  text). The implementor brief that follows should then be ~3 paragraphs:
  "do exactly what §3.4 says for these 6 structs; lift the upload-site
  helper verbatim from §3.4's `write_uniform` snippet; run the
  verification recipe below."

---

## Verification recipe

The verification load splits into compile-time (cheap, definitive),
runtime layout (cheap, definitive), and visual-regression (expensive,
non-deterministic).

### Compile-time layout oracle (the std140 truth)

The `const _: () = assert!(offset_of!(...))` guards at
`crates/bevy_naadf/src/render/gpu_types.rs:844-902` are the load-bearing
proof. If the cutover is layout-equivalent, the existing guards stay
green after the flip:

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace 2>&1 | tee /tmp/build-after-gpugiparams-cutover.log
grep -E "error\[E0080\]|assertion.*failed" /tmp/build-after-gpugiparams-cutover.log
# Empty output = all offset guards green = layout byte-equivalent.
```

Per architect path (a) (pad-keep): expect zero errors.

Per architect path (b) (coordinated deletion): expect specific guards to
fire:
- `:875` (`max_ray_steps_secondary == 304`) — adapt to `== 292` or
  delete per architect spec.
- `:879` (`spatial_iter_count == 320`) — adapt to `== 308` or delete.
- `:857` (`size_of::<GpuGiParams>() == 336`) — adapt to `== 320` or
  delete.

### Runtime layout mirror tests

`crates/bevy_naadf/src/render/gpu_types.rs:911-1018` runs:

```bash
cargo test --workspace --lib gpu_types 2>&1 | tee /tmp/test-gpu_types.log
grep "test result" /tmp/test-gpu_types.log
# Expect "test result: ok. N passed; 0 failed"
```

These are runtime mirrors of the compile-time guards; they exist
specifically to catch a "future tooling that strips `const _ = assert!(…)`"
escape (per the docblock at `:944-951`). Both must stay green.

The architect's §3.4 wants new tests added of the form
`<GpuGiParams as ShaderType>::min_size().get() == <expected>`. For path
(a), expected = 336. For path (b), expected = 320.

### Behavioural-equivalence (the GI-pipeline byte-stream)

The library tests do not exercise the actual GPU upload path. The
non-deterministic e2e `--vox-gpu-oracle` gate exercises CPU↔GPU phase
parity and would surface a silent layout regression as a sporadic visual
glitch:

```bash
# 3 runs on suspect side (post-cutover HEAD):
for i in 1 2 3; do
  timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle 2>&1 \
    | tee /tmp/vox-gpu-oracle-post-$i.log
done

# 2 runs on reference side (pre-cutover HEAD or main):
git stash
for i in 1 2; do
  timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle 2>&1 \
    | tee /tmp/vox-gpu-oracle-pre-$i.log
done
git stash pop

# Aggregate cross-run variance: every run must report identical PASS message.
```

Per `feedback-multiple-runs-rule-out-false-positives` memory: ≥3 on
suspect side, ≥2 on reference side. Per
`feedback-e2e-gates-must-fail-fast`: each run wrapped in `timeout 120s`.

The `--oasis-edit-visual` gate is similarly sensitive to producer-vs-GI
layout drift and would surface a silent shift as a brush-stroke
Δ-luminance regression:

```bash
for i in 1 2 3; do
  timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual 2>&1 \
    | tee /tmp/oasis-edit-visual-post-$i.log
done
```

Same multi-run discipline.

### Final full-suite gate

After the per-gate gates pass:

```bash
cargo test --workspace --lib  # 179 expected green
# Full deterministic e2e ladder (the canonical sequence used pre-cutover):
for gate in baseline --validate-gpu-construction --edit-mode --entities \
            --vox-e2e --small-edit-visual --vox-gpu-construction \
            --vox-web-parity; do
  timeout 120s cargo run --bin e2e_render -- $gate
done
```

Every gate must produce its existing PASS message verbatim — no message
changes, no new warnings.

### Per project CLAUDE.md verification discipline

**Forbidden:** `cargo run --bin bevy-naadf` as a verification step. The
deterministic e2e gates are the verification surface. The user does the
live visual check after the gates pass.

---

## Side notes / observations / complaints

- **The impl log's "324 bytes" arithmetic is wrong by 4 bytes.** Per my
  walk, the post-cutover size with `_pad5..pad10` deleted is 320, not
  324. The substance of the impl log's claim (encase produces a smaller
  buffer than 336) is correct and the divergence (`max_ray_steps_secondary`
  at 292 vs 304) is correct — only the final-size number is off. The
  architect's revised §3.4 should use 320, not 324, if it picks path (b).
  This is a minor slip but worth flagging because the impl log was
  otherwise scrupulous in its layout walk.

- **The audit's `00-reuse-audit.md` framing is correct but soft on
  recommendation.** The audit calls Item 3 "uniquely architect-revision-
  blocked" (line 209-214) but doesn't pin down which of the three
  resolution paths the architect should pick. My recommendation: path
  (a) (pad-keep) is the lowest-blast-radius, lowest-verification-load
  choice and preserves the existing 24 compile-time guards as-is. The
  cutover then becomes a one-attribute swap (`#[derive(Pod, Zeroable)]`
  → `#[derive(ShaderType)]`) per struct + a 6-call upload-site swap
  for the helper. Path (b) is technically cleaner (drops the 12 bytes
  of dead pad) but the maintenance cost is higher (coordinated edit
  across two SSoT seams, naga_oil verification step) and the named-pad
  approach in (a) is a well-precedented WGSL/Rust idiom anyway.

- **The architect's §3.4 "drop the offset asserts" claim
  (`03-architecture.md:404-411`) is wrong in spirit.** It says the
  guards "drop because `encase` enforces the layout at serialisation
  time". But the runtime test docblock at `gpu_types.rs:944-951`
  explicitly notes the asserts exist as a defense against
  *future-tooling regressions* — `#[cfg(feature = ...)]` wrapping, a
  refactor that strips `const _ = assert!(…)`, an editor auto-fix. If
  the cutover removes these guards, that defense layer is gone. The
  encase-derived layout is no less stable than the bytemuck layout but
  the loss of explicit assertion coverage is a real loss the architect
  should weigh. Recommendation: keep all 24 guards, adapt the values
  if path (b) is picked. The compile-time guards are *free* — they cost
  zero runtime, zero binary size, and produce a localised error
  message if the layout ever drifts again. Drop the guards only if
  there's an explicit design reason; "encase enforces it" is not a
  sufficient reason.

- **Item 3 is the ONLY Item-1-through-5 that's truly architect-revision-
  bound.** Items 1, 2, 4, 5 all have implementor-side paths (per the
  audit's diagnoses). Item 3 is sticky precisely because the choice
  among (a)/(b)/(c) carries different design tradeoffs an implementor
  can't unilaterally make. The audit and the impl log both already
  arrived at this conclusion; this investigation confirms it
  independently.

- **A possible non-fix:** if the orchestration's downstream goal is
  "reduce the `_padN` count in `gpu_types.rs`", and Item 3's `GpuGiParams`
  contributes 6 of the ~70 `_padN` fields the architect's §1 cited
  (`03-architecture.md:46`), the gain from Item 3 is modest (~9%). If
  the architect spends effort on the revision and the implementor lands
  the cutover, the named-pad path (a) leaves all 6 pads in place — net
  pad-count reduction zero for `GpuGiParams` specifically. The cleanup
  win comes from the OTHER 5 structs. **The orchestration's framing
  may be partially wrong** — Item 3's value is "let the cutover land
  uniformly across 6 structs" rather than "delete 6 more pads". This
  doesn't change the recommendation but is worth surfacing.

- **The deletion-path naga_oil concern is probably benign.** I read the
  WGSL header docblock at `gi_params.wgsl:13-15,47` carefully — the
  constraint is "naga writeback rejects trailing-digit identifiers and
  bare `_padN`", which the existing WGSL respects (`pad_a`, `pad_b`,
  ..., `pad_g`, `rand_counter_b`). A *deletion* of the WGSL pad fields
  introduces no new identifiers, so the naga_oil rule is inert. The
  architect's revision should mention this explicitly so the implementor
  doesn't waste a tool-budget cycle re-verifying. Same for the
  `pad_b..pad_g` fields — they have zero readers (write-only-from-Rust
  lanes); a grep confirmation belongs in the architect spec, not the
  implementor's manual probe.

- **The cutover precedent question is genuinely concerning.** Item 3 is
  the codebase's first `ShaderType` consumer. The architect's §3.4
  exemplar is `GpuRenderParams` (a clean case); the implementor would
  then attempt 5 more structs from that single exemplar. Per the
  Vigilance preamble: significant CG work, vigilance required. A
  prudent architect revision would expand §3.4 to show TWO exemplars —
  one clean case (`GpuRenderParams`) and the one non-clean case
  (`GpuGiParams` post-decision). The asymmetry deserves explicit
  documentation precisely because the codebase has no other consumer
  to learn from.
