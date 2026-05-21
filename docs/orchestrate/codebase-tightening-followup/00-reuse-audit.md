# codebase-tightening-followup — reuse audit

## delegate-auditor findings (2026-05-21)

Scope: five deferred items from the multi-session `codebase-tightening`
orchestration. The audit looks for verification helpers / probes / tests / patterns
the investigators can lean on, plus reuse-targets the items could leverage if
they need follow-through. Faithful-port rule respected (no PBR resurrection,
CPU oracle `aadf/edit.rs` sacred).

Verified against the working tree at HEAD `2bb03d1` ("D4 final cleanup");
every file:line citation re-checked with Read/Grep before landing here.

---

### Item 1 — D5 Step 4: `prepare_construction` split (5 cross-workstream couplings)

**Blocker claim** (from `docs/orchestrate/codebase-tightening/gpu-construction/04-refactoring.md:1020-1109`):
two implementors deferred with five specific gaps the architect's §2.1 did not
spell out — `want_gpu_producer` is computed once at `mod.rs:541-542` and consumed
across W1/W3/W4/W5; W2 defensively allocates W1 placeholders inline
(`mod.rs:1632-1676`); first-frame `bounds_initialized` seed has tiered `return;`
bails; `construction_world` bind group depends on buffers from W1+W3+W5; W4
rebuilds `world_gpu.bind_group` (D4-owned mutable state) at `mod.rs:1697-1726`.

#### Verification infrastructure

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `--validate-gpu-construction` byte-for-byte gate | `crates/bevy_naadf/src/render/construction/validation.rs:4890` (anchor) + dispatched from `crates/bevy_naadf/src/bin/e2e_render.rs` | byte-equal GPU↔CPU oracle assertion against `aadf::construct` | reuse | the canonical signal that any per-workstream split preserved producer correctness — "388 bytes byte-equal" is the pass message from the prior follow-up |
| `--validate-gpu-construction-scaled` + `…-production-scale` | dispatched from `bin/e2e_render.rs` via `validate_gpu_construction_scaled` / `_production_scale` | scaled-volume byte-diff with reportable semantic-mismatch count | reuse | "total semantic mismatches: 0" — exactly what investigators need to prove no allocation race introduced after split |
| `--oasis-edit-visual` non-deterministic gate | `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs` (full module) | brush stroke before/after Δ-luminance over rect | reuse | the only end-to-end visual gate sensitive to producer-vs-placeholder ordering races; **must run ≥2× per `feedback-multiple-runs-rule-out-false-positives`** |
| Existing `.run_if(resource_exists::<_>)` patterns in construction | `render/construction/mod.rs:1904,1907` | declarative scheduler-build-time precondition check | extend | already used for `populate_cpu_mirror_from_gpu_producer`; the split can adopt the same idiom verbatim per architect §2.5 |
| Embedded `mod tests`, `mod tests_w1`, `mod tests_w4` | `render/construction/mod.rs` (locations cited in architect §2.3) | per-workstream CPU↔GPU oracle unit tests | reuse | direct verification of `prepare_chunk_calc`/`prepare_bounds_calc`/etc. allocation correctness post-split, independent of e2e |

#### Reuse-targets if the split lands

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `ConstructionConfig` resource | `render/construction/config.rs:36-` (struct), `:252-` (`From<AppArgs>`), `:136-` (Default) | shared knobs (`gpu_construction_enabled`, `initial_hash_map_size`, `entities_enabled`) | extend | architect's coupling #1 (the `want_gpu_producer` shared-derivation) wants exactly this: a `pub fn want_gpu_producer(&self, world_data_meta: Option<&WorldDataMeta>, model_data: Option<&ModelDataRender>) -> bool` method here. Single canonical computation, all five systems re-derive. |
| `rebuild_world_bind_group_with_entities` named helper | `render/prepare/world.rs:590` (def), called at `render/construction/mod.rs:1714` | D4-owned constructor for the W0-seam cross-write | reuse | coupling #5 (W4 mutates D4's `world_gpu.bind_group`) is *already* resolved — the named helper is grep-able; per-workstream split keeps it intact verbatim |
| Per-workstream submodules (each owns layout + dispatch) | `chunk_calc.rs`, `bounds_calc.rs`, `world_change.rs`, `entity_update.rs`, `generator_model.rs`, `producer.rs`, `extract.rs`, `validation.rs` | architect §2.1 cites their layout-descriptor + dispatch helper line:cols | extend | new `prepare_<workstream>` system per file would land naturally next to existing `queue_*_pipeline` / `dispatch_*` peers |
| `RenderSystems::PrepareResources` set + `.after(prepare_world_gpu)` edge | `render/construction/mod.rs:1887-1888` | the single ordering edge each split prepare needs | reuse | transfers verbatim per architect §2.1; idiomatic Bevy 0.19 RenderSystems pattern already in use |

#### Borderline calls

- **`bounds_calc/tests.rs`** (`render/construction/bounds_calc/tests.rs`) — a Bevy-app-builder GPU oracle test fixture exists; *almost* fits as a single-workstream prepare-system test harness post-split, but the fixture builds the entire `construction_pipelines` + `world_layout` chain. Would need a "single-workstream" mode added to act as a prepare-split smoke. **Flip condition:** if Item 1 investigator wants per-workstream prepare unit tests rather than end-to-end gates, extend this fixture.
- **`construction_config.want_gpu_producer(...)` helper** — does NOT yet exist; investigator may want to call this borderline "extend" because the obvious home (`config.rs`) is already a single-purpose module. The 1-liner method addition is the cheap fix the impl logs called for (`04-refactoring.md:1093`).

---

### Item 2 — D6 Steps 3+4: gate trait migration + driver decomposition

**Blocker claim** (`docs/orchestrate/codebase-tightening/e2e-and-playwright/04-refactoring.md:465-547`):
the trait `Gate::apply_edit(&self, _world_data: Option<&mut WorldData>)` signature
at `e2e/gate.rs:97` is missing per-gate State resources — OasisEdit needs to
write `OasisEditVisualState.edit_applied`, SmallEditVisual needs three writes
(`voxel_count_before/after` + `world_size_voxels` + `edit_applied`),
VoxGpuConstruction needs `OasisEditVisualState.edit_applied` only (no
`WorldData`). Landing Step 3 alone produces ~600 LOC of dead trait impls because
the driver doesn't yet consume `Res<ActiveGate>`.

#### Verification infrastructure

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| All 8 e2e gates (incl. `--oasis-edit-visual`, `--small-edit-visual`, `--vox-gpu-construction`, `--vox-gpu-oracle`, `--vox-web-parity`) | `crates/bevy_naadf/src/bin/e2e_render.rs` post-Step-5 (`523` LOC) | full Warmup→Shoot→Drain→Apply→PostEditWait→Assert behavioural gates | reuse | every gate must still produce identical PASS message after trait migration; ~16 e2e runs minimum is the architect's stated verification load |
| `gate.rs` scaffold already landed | `crates/bevy_naadf/src/e2e/gate.rs:1-138` | `Gate` trait, `GateKind` enum, `FrameBudget` struct, `set_camera_pose` helper | extend | the trait shape is *the* artefact under question — `apply_edit` signature needs revision to either widen the parameter set or carry per-gate aux via `GateCaptures.aux` (per architect §"Finding 6") |
| `--ssim-compare` pure-PNG-diff path | `bin/e2e_render.rs` short-circuit | no-boot byte comparison of capture pairs | reuse | smallest signal for "capture shape unchanged after trait migration" — completes in seconds |

#### Reuse-targets / state-passing patterns the investigator can adopt

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| Existing per-gate `State` resources (Bevy `Resource` newtype pattern) | `e2e/oasis_edit_visual.rs:165` (`OasisEditVisualState`), `small_edit_visual.rs:188`, `small_edit_repro.rs:114`, `vox_gpu_oracle.rs:669`, `vox_web_parity.rs:140` | per-gate mutable state, registered in `e2e/mod.rs:230-234` | extend | architect's `GateCaptures.aux: GateAuxState` enum (§"Finding 6") is the proposed reuse-target — collapse 5 resources into one tagged-enum; only the aux shape changes, not the fields |
| `Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>` query | `e2e/driver.rs:465` and similar | unique-camera mutation pattern | reuse | `pin_active_gate_camera` already drafted in `gate.rs:134-137` — proven Bevy idiom for gate-routed camera writes |
| `World` parameter on Bevy systems (alternative State carrier) | n/a in current e2e/ | exclusive `&mut World` system signature lets a trait impl reach any resource | extend | a cleaner alternative to widening `apply_edit`'s parameter list — `fn apply_edit(&self, world: &mut World) -> Result<(), String>` lets each gate fetch its own state resource. Removes the parameter-shape mismatch the implementor flagged. |
| `MessageWriter<AppExit>` for verdict | `driver.rs:467` (existing Assert arms) | uniform driver-owned exit write | reuse | architect's `run_assert_verdict` (§"Finding 8") wraps this — no new infra needed |
| `Framebuffer::save_in_screenshots_dir(filename, gate_tag)` | `crates/bevy_naadf/src/e2e/framebuffer.rs` (added by D6 Step 2 per impl log) | one-liner consolidating 7 per-gate `save_*_screenshot` wrappers | reuse | the only Step-3-blocker that *was* resolved cleanly in Step 2 — proves the pattern works for Step 3a |
| `.after(driver::e2e_driver)` ordering chain | `e2e/mod.rs:249-282` | seven `pin_*_camera` systems' explicit `.after(pin_oasis_camera)` priority chain | extend | collapses to ONE registration once `pin_active_gate_camera` consumes `Res<ActiveGate>` (per architect §"Finding 3") |

#### Borderline calls

- **`World` vs `&mut WorldData`-only** trait signature: investigator could either (a) widen `apply_edit` to take `world: &mut World` (architect's `Gate` trait + Bevy idiom; one parameter change resolves all 4 gate mismatches) OR (b) follow the architect's original "land Step 4 first then Step 3" sequencing. **Flip condition:** if widening signature unblocks Step 3 atomically without Step 4, the deferred-twice deadlock breaks; investigator should verify the architect's stated coupling actually requires the deferral.
- **`vox_gpu_construction` reads `OasisEditVisualState.edit_applied`** (per impl log line 480-483) is a cross-gate state read — almost-but-not-quite fits the `GateAuxState` enum. **Flip condition:** if the camera-promote signal can live on `GateCaptures.aux` (as architect §"Finding 6" sketched: `VoxGpuConstruction { camera_promoted: bool }`), the cross-gate read dissolves.

---

### Item 3 — D4 Step 3: `ShaderType` cutover for `GpuGiParams` (non-natural std140 pads)

**Blocker claim** (`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1011-1075`):
the architect's §3.4 recipe ("drop every `_padN` field") is wrong for
`GpuGiParams` because trailing `_pad5/6/7` (at `gpu_types.rs:511-516`) and
`_pad8/9/10` (at `:541-545`) are NOT std140-natural alignment breaks — they
force `max_ray_steps_secondary` to offset 304 and the struct to 336 bytes. The
WGSL counterpart at `gi_params.wgsl:128-147` mirrors them as `pad_b/c/d/e/f/g`.
Dropping the Rust pads would put `max_ray_steps_secondary` at offset 292 (12-byte
divergence). Requires synchronous Rust+WGSL edits.

#### Verification infrastructure

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| Compile-time `size_of` + `offset_of` assertions | `crates/bevy_naadf/src/render/gpu_types.rs:844-902` | 24 `const _: () = assert!(...)` guards on every uniform struct's std140 placement | reuse | the existing layout oracle — drop pads in a probe branch, build, watch which `const _` assertions fire. **This IS the std140 validation tool** investigator needs. |
| Runtime mirror tests for layout | `gpu_types.rs:911-1018` (`#[cfg(test)] mod tests`: `construction_params_layout`, `hash_value_slot_layout`) | runtime `#[test]` mirrors of the compile-time `offset_of!` guards | reuse | architect's §3.4 recommendation to "adapt via `encase::ShaderType::SHADER_SIZE` and field-position queries" — extend these tests to assert `<GpuGiParams as ShaderType>::min_size().get() == 336` and field-position equivalence |
| WGSL counterpart layout documentation | `assets/shaders/gi_params.wgsl:1-148` | the WGSL `struct GpuGiParams` mirror with explicit pad fields + the 4-vec4-rows fix docblock at `:13-46` | reuse | the side-by-side comparison oracle — investigator can verify any `encase` output byte-equal against the documented WGSL layout |
| `--vox-gpu-oracle` non-deterministic gate | `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` | CPU↔GPU phase parity gate, sensitive to silent layout regressions | reuse | the gate that would surface a silent layout shift as a sporadic visual glitch; **≥3 runs per non-determinism rule** |
| `taa_jitter` placement guard story (`gpu_types.rs:838-867` comments) | inline | documents the `vec3`-then-scalar trap that bit the port 3× and the exact pattern `GpuGiParams._pad5/6/7` defends against | reuse | the "why" doc the investigator needs to internalise before touching pads — `_pad5/6/7` literally prevents the `sun_shadow_taps` row from sliding into `taa_jitter`'s trailing 8 bytes |

#### Reuse-targets if the cutover lands

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `bevy::render::render_resource::ShaderType` derive | available via existing `bevy_render` import; `encase` in `Cargo.lock:2535` | layout-correct uniform serialisation | reuse | architect's chosen post-cutover encoding; transitive dep already present, no Cargo edit needed |
| `bevy_encase_derive` in dep tree | `Cargo.lock:806,1156,1205,1406,1426,2535` | the derive macro infrastructure | reuse | no third-party dep work for the cutover itself |
| Existing TAA path's `ShaderType` precedent | search returns **none** — `grep ShaderType` in src returns only a docblock at `render/pipelines.rs:369` ("structs are not `ShaderType`, so the sized helpers are used directly") | n/a | not applicable | **no existing `ShaderType` consumer in the codebase** — Item 3 is the precedent-setter, not a follow-the-leader case. Investigators cannot lean on a working example. |
| 5 byte-equivalent struct cutover targets | `gpu_types.rs:60-…` (`GpuRenderParams`), `:…` (`GpuCamera`, `GpuWorldMeta`, `GpuTaaParams`, `GpuAtmosphereParams`, `GpuConstructionParams`) | std140-natural padding per follow-up §1 (`04-refactoring.md:1022-1028`) | reuse | clean cutover targets if `GpuGiParams` is excluded and the partial-cutover anti-DRY tradeoff is accepted (impl log §1.0 lists this as worse-than-no-cutover) |

#### Borderline calls

- **Compile-time `const _: () = assert!(offset_of! == ...)` guards** — they detect divergence *if* the test runs, but architect §3.4 explicitly says they "drop because `encase` enforces the layout at serialisation time". **Flip condition:** if investigator keeps the asserts and just adapts the expected values, they remain the std140 oracle even post-cutover (contradicts architect; but more conservative and verifiable).
- **WGSL `pad_b/c/d/e/f/g` fields** at `gi_params.wgsl:129-147` — almost reusable as the "what the GPU expects" side of a layout test. **Flip condition:** if the investigator writes a probe that uploads via `encase` then byte-grep'd compares against the WGSL-documented sizes, they get a closed-loop verification independent of the e2e gates.
- **`naga_oil` shader-import status** — `gi_params.wgsl:13-15,47` claims "naga-oil composable-module structs cannot carry the `_pad0`-style identifiers" and the file is consumed via `#import`. Any cutover that re-emits Rust fields with different names risks WGSL import-time failures. **Flip condition:** if WGSL is hand-touched too (architect's brief said it must be), confirm `naga_oil` accepts the new field names before committing.

---

### Item 4 — D4 Step 5: plugin-per-subsystem (dispatch budget; no analytical blocker)

**Blocker claim** (`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1585-1619`):
both implementors hit the 100-tool-use budget — the architect's §3.3 + Step 4
spec requires 6 new files (`first_hit.rs`, `ray_queue.rs`, `sample_refine.rs`,
`spatial_resampling.rs`, `denoise.rs`, `final_blit.rs`), dissolving
`graph.rs` (309 LOC) + `graph_b.rs` (574 LOC), splitting `pipelines.rs` into
per-subsystem `*Pipelines` resources, converting the 17-element `.chain()` into
a `SystemSet`-edge web with 9 plugins. No analytical blocker reported — the
deferrals are budget bailouts.

#### Verification infrastructure

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| Full e2e suite | `bin/e2e_render.rs` (all 8 gates + `--ssim-compare`) | end-to-end visual + behavioural assertions | reuse | architect §3.3 verification line: "compare resolved schedule order against old 17-element `.chain()` order"; only e2e gates surface a per-frame ordering slip |
| 17-element `.chain()` source-of-truth | `crates/bevy_naadf/src/render/mod.rs:298-331` | the canonical render-graph node order + the `Core3dSystems::PostProcess`-and-`.before(tonemapping)` envelope | reuse | the pre-refactor truth investigator compares against; the docblock comments at `:194-297` explain WHY each node lands at its slot — preserve the WHY when carving into plugins |
| `cargo test --workspace --lib` (179 passing) | n/a | unit tests | reuse | catches resource-existence / plugin-registration smoke; not directional but a green floor |

#### Reuse-targets for the migration

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| Existing `Plugin` impl pattern in `ConstructionPlugin` | `render/construction/mod.rs:1827-1913` | live demo of `RenderApp` plugin shell, `insert_resource`, `init_resource`, `init_gpu_resource`, `add_systems(Render, ..in_set(RenderSystems::PrepareResources))`, `.after(...)` | reuse | architect's §3.3 plugin template literally mirrors this shape — `ConstructionPlugin` IS the precedent |
| `SystemSet` declaration idiom (Bevy 0.19) | `bevy::prelude::SystemSet` derive used implicitly in `RenderSystems::PrepareResources`/`PrepareBindGroups` and `Core3dSystems::PostProcess` at `render/mod.rs:188,192,329` | already-used pattern for set-based ordering | reuse | each new subsystem's `FooSet` is one derive macro |
| `tonemapping` + `Core3dSystems::PostProcess` envelope | `render/mod.rs:329-330` | the existing `.in_set(...).before(...)` chain anchor | reuse | every new plugin needs these two edges; lift verbatim from the current registration |
| `NaadfPipelines` (post-Resolution-D-merge) | `crates/bevy_naadf/src/render/pipelines.rs:286-` (post-merge fields) | now 57-field unified resource per impl log `:1115-1119` | extend | architect's per-subsystem `*Pipelines` decomposition was *the alternate path*; impl log notes the merge "implicitly supersedes" the split — investigator can choose either, but starting from the merge is no longer the spec'd path |
| `cell_shader_defs()` helper at `pipelines.rs:76-81` | inline | a 1-call-site shared shader-def injection helper | reuse | per-plugin pipeline build calls this verbatim regardless of plugin shape |

#### Borderline calls

- **Resolution D merged everything into `NaadfPipelines`** (`pipelines.rs:286-` Phase-C absorption, see search hits at `:286,922`). Architect's §3.3 assumed the per-workstream split path. The architect-design ambiguity (Conflict 1 in `03-architecture.md:892-911`) was resolved AWAY from architect intent. **Flip condition:** investigator must either (a) accept Conflict 1's partial-landing fallback ("plugin-per-subsystem but reading from existing `NaadfPipelines`") or (b) re-split the now-merged 57-field resource. Architect spec needs revision either way.
- **The deferral is "budget" — not "analytical"**: both impl logs (`:609-616` D4 follow-up, `:1601-1607` D4 final cleanup) say it cleanly. Investigator should test the claim by scoping ONE subsystem (`first_hit.rs`) end-to-end as a proof-of-concept; if that fits in <30 tool calls, the rest is 8× repetition. **Flip condition:** if the proof-of-concept subsystem exceeds 30 tool calls, the architect spec genuinely is over-budget and needs decomposition.

---

### Item 5 — `window_config.rs` → e2e dep-arrow inversion

**Blocker claim** (`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1460-1495,1738`):
production `crates/bevy_naadf/src/window_config.rs` reads `crate::e2e::{E2E_WIDTH,
E2E_HEIGHT, E2E_RESIZE_BOOT_WIDTH, ...}` at lines 47, 48, 69, 70, 99, 100, 122,
123 (verified). The dep arrow runs backwards (production → e2e); analogous to
the `demo_origin_v` inversion that commit `2bb03d1` resolved by relocating the
definition into `voxel/grid.rs` and leaving a `pub use` re-export in
`e2e/gates.rs`.

#### Verification infrastructure

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `grep -rn "crate::e2e\|use crate::e2e" crates/bevy_naadf/src/ \| grep -v "^.*/e2e/"` | per impl log `:1460-1467` | one-shot tree-walk that surfaces every production→e2e import | reuse | the pre-existing audit verifies dep-arrow correctness; investigator runs it before+after and confirms `window_config.rs` lines drop out |
| `cargo build --workspace` + full e2e suite | per impl log `:1452-1458` | confirms the move + re-export resolves verbatim | reuse | the demo_origin_v precedent's verification pattern; identical here |
| `--entities` gate | per impl log `:1455-1458` | confirms relocated constant still produces identical entity placement | reuse | analogue: any e2e gate that consumes the moved constant (e.g. `--small-edit-repro` for `SMALL_EDIT_REPRO_WIDTH`, `--vox-horizon-native` for `HORIZON_WIDTH`) should green post-move |

#### Reuse-targets — the canonical inversion template

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `demo_origin_v` move from `e2e/gates.rs` → `voxel/grid.rs` | git commit `2bb03d1`; see `voxel/grid.rs:66-89` (new home), `e2e/gates.rs:23-30` (re-export shell) | exact inversion-pattern template: function/constant moves out of `e2e/`, `pub use` shim stays where callers expected it | reuse | **THIS is the template** investigator copies verbatim — function-level move, doc-update, `pub use` shim, dep-arrow audit grep, e2e verification |
| `test_fixture.rs` doc + call-site update | `render/construction/test_fixture.rs:11-22,61` (post-2bb03d1) | the production caller updates its docstring + the call expression | reuse | shows the production-side edit shape — module docstring narrates the inversion resolution; call expression changes path |
| `e2e/gates.rs` re-export rationale (`Notes` block in impl log `:1479-1488`) | `04-refactoring.md:1479-1488` | the "why keep the e2e/ re-export?" decision (back-compat + semantic ownership) | reuse | investigator may want this rationale for the window_config.rs case (which has 4 distinct constants — `E2E_*` vs `HORIZON_*` vs `E2E_RESIZE_BOOT_*` vs `SMALL_EDIT_REPRO_*`; each could go to a different non-e2e home) |
| `window_for_e2e_args` mode-→-config switch | `window_config.rs:137-147` | already-co-located mode dispatch | reuse | unaffected by move; the *constants* go elsewhere, the *function* stays |

#### Borderline calls

- **`HORIZON_WIDTH` / `HORIZON_HEIGHT`** live in `e2e/vox_horizon_parity.rs` (per `window_config.rs:69-70`) — these are *genuinely* e2e-gate-specific (set to match a Playwright viewport, per the docblock at `window_config.rs:60-65`). Moving them out of `e2e/` is semantically wrong — they ARE e2e dimensions, just consumed by production window setup. **Flip condition:** investigator may need a *different* resolution than the `demo_origin_v` template here — perhaps push the `e2e_horizon()` constructor INTO `vox_horizon_parity.rs` so `window_config.rs` doesn't have to import. Conversely, `E2E_WIDTH` / `E2E_HEIGHT` / `E2E_RESIZE_BOOT_WIDTH/HEIGHT` / `SMALL_EDIT_REPRO_WIDTH/HEIGHT` look more like raw screen dimensions that could go to a `dimensions.rs` module.
- **Three constants, three sources** (`e2e/mod.rs::E2E_*`, `e2e/vox_horizon_parity::HORIZON_*`, `e2e/small_edit_repro::SMALL_EDIT_REPRO_*`) — the demo_origin_v inversion moved ONE function. Here the investigator must decide either (a) one bulk move to a shared non-e2e module, or (b) per-gate moves keeping locality. **Flip condition:** if locality matters (a quick read of `vox_horizon_parity.rs` shows the constants are intrinsic to that gate's logic), per-gate move + per-gate `pub use` shim in `window_config.rs` may invert (b) becomes "leave window_config.rs to import from each gate" — which is the *current* arrow direction. The dep-arrow finding would then re-cast as "window_config.rs IS by-design a consumer of e2e knobs, accept the import." **Investigator should validate the brief's premise that this is actually an inversion to fix, not a legitimate consumer relationship.**

---

## Cross-item observations

1. **The e2e gate suite is the shared verification spine for items 1, 3, 4, 5.**
   Every item's primary verification reduces to "run `cargo run --bin e2e_render
   -- <gate>` ≥2× (or ≥3× for non-deterministic gates) and compare against
   pre-change baseline." The investigator workflow is uniform; only the *gate
   selection* per item changes.
2. **`ConstructionPlugin::build` (`render/construction/mod.rs:1827-1913`) is
   the reuse template for both Item 1 (per-workstream prepare splitting) AND
   Item 4 (plugin-per-subsystem extraction).** Both architects' designs
   literally copy this shape. Investigators landing either item should keep
   the other in mind — overlap is high.
3. **Item 3 (`ShaderType` cutover) is uniquely architect-revision-blocked.**
   Items 1, 2, 4, 5 have implementor-side paths forward (with caveats). Item 3
   *explicitly* requires the architect to update the §3.4 recipe to cover
   `GpuGiParams.{_pad5..pad7,_pad8..pad10}` as the non-natural-alignment
   exception. **Without architect revision, Item 3 should not be re-dispatched
   to implementors** — a third bail is the expected outcome.
4. **Item 2 (gate trait) is the only item with a Bevy-native unblock at hand.**
   Switching `apply_edit` to `&mut World` (or to an extra State-Resource trait
   bound) is a one-parameter change that would resolve all 4 trait-vs-data
   mismatches the implementor surfaced. The "Step 4 first, then Step 3"
   sequencing assumption is the architect's; the trait redesign is well-
   precedented in Bevy idioms.
5. **No item is foundation-rotten.** All five sit on a clean substrate: D4's
   `prepare/{mod,frame,world}.rs` split landed clean (commit `2bb03d1`), the
   W0 seam was retired via Resolution D (`pipelines.rs:286-`), CPU oracle
   intact, e2e gate harness fully operational. The deferrals are all
   architect-spec ambiguities or budget bailouts — not load-bearing rot.

---

## Side notes / observations / complaints

- **Item 3 (`GpuGiParams`) brief premise is half-wrong, half-right.** The
  brief says "implementor bailed citing 'architect §3.4 recipe wrong on
  trailing `_pad5/6/7/_pad8/9/10` non-natural std140 alignment'." That IS the
  blocker. But the brief frames it as "to verify" — there is nothing to
  verify. The follow-up impl log §1 (`04-refactoring.md:1011-1075`) walked
  the std140 layout by hand, found 5 of 7 structs clean and `GpuGiParams`
  the lone exception, and named the exact offset divergence (12 bytes, 304
  vs 292). The investigator who re-verifies will reproduce that exact result.
  **What the investigator should actually do is propose the architect-side
  spec revision** (either keep the trailing pad, or land a coordinated
  Rust+WGSL `pad_b/c/d/e/f/g` deletion as one atomic step) — not "verify the
  bail." Item 3 is the only one that genuinely cannot be unblocked by an
  implementor.

- **Item 2's "trait signature missing per-gate State" is fixable in-flight.**
  The implementor's bail rationale (`04-refactoring.md:478-488`) lists the
  4-gate trait-vs-data mismatch as if it requires the entire Step 4 driver
  rewrite. **It doesn't.** Replacing `Option<&mut WorldData>` with `&mut
  World` (Bevy's universal data access) lets every gate's `apply_edit`
  fetch its own `Res<XxxState>` directly. The 600-LOC scaffolding claim
  evaporates — each `impl Gate::apply_edit` body uses the existing State
  resources verbatim. The architect could have specified this in `gate.rs`
  on day one. **Investigator should challenge the implementor's framing
  before accepting Step 3+4-must-land-together.**

- **Item 1 is genuinely architecturally expensive (impl log §5.3 nailed
  this).** The 5 specific cross-workstream couplings the implementor
  surfaced (`04-refactoring.md:1027-1080`) are real. The cheapest fix —
  adding a `pub fn want_gpu_producer(...)` method on `ConstructionConfig`
  — closes coupling #1 in 5 LOC. Couplings #2, #3, #4 are harder: they're
  about *defensive allocation fallbacks* in the W2 block that disappear
  when the architect spells out "W1's `prepare_chunk_calc` allocates
  unconditionally, regardless of `want_gpu_producer`." That's a 1-line
  architectural decision the architect punted on. **Investigator's
  proposal for an architect revision should focus on those 4 ambiguities
  specifically; everything else lifts directly per the cited file:line
  refs.**

- **Item 4's "dispatch budget" claim is testable.** Both impl logs say
  the budget ran out, neither says *which step* ran them out. A cheap
  probe: investigator pick ONE subsystem (smallest — likely
  `final_blit.rs` or `ray_queue.rs`), implement the full per-subsystem
  extraction end-to-end, count tool calls. If <30, the architect spec
  scales as 9 × 30 = 270 tool calls — confirming 100-tool-budget
  inadequacy. If >50, the spec needs decomposition into per-subsystem
  sub-steps. Either outcome is actionable.

- **Item 5's brief premise is partially flawed.** The impl log
  (`:1489-1495`) suggests `window_config.rs`'s imports might be a
  **legitimate** consumer relationship, not an inversion. `HORIZON_WIDTH`
  *is* an e2e-gate-specific dimension (Playwright viewport-pinned). The
  `demo_origin_v` template doesn't fit cleanly: that was a function whose
  body referenced production constants (`WORLD_SIZE_IN_CHUNKS`); the
  e2e dimensions here are *intrinsically* e2e. **Investigator may
  surface that the brief's framing is wrong — the right resolution may
  be "leave it, document the legitimate consumer relationship" rather
  than force a `demo_origin_v`-shaped move.** This is the most likely
  item to come back as "we shouldn't have asked."

- **Sub-agent compliance risk for the investigator phase.** Per
  `feedback-subagent-research-only-compliance` memory: `general-purpose`
  agents may disobey research-only briefs and start editing.
  Investigation of Items 1-5 is read-only by design (verify the bails,
  propose architect revisions); make sure the next-phase brief routes
  through `Explore` or a no-edit-tool agent variant. The 5 deferred
  items all have implementor-bailout patterns that look like "I should
  just go fix it" honey-traps.

- **The `bevy-naadf` master is genuinely well-architected (D4 architect
  side note 14 echoed this; I concur after this audit).** The deferred
  items are residual edges of structural rot in `mod.rs` god-files (D5)
  and a 1956-LOC driver (D6). The foundations — W0 seam retired, CPU
  oracle sacred, e2e harness operational, `prepare/` split landed,
  `rebuild_world_bind_group_with_entities` named — are sound. No item
  requires foundation work; all are local refactor questions.
