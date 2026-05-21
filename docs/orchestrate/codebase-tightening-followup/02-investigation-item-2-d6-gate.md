# Item 2 — D6 Steps 3+4: gate trait + driver decomp

Investigator: read-only sub-agent (codebase-tightening-followup orchestration, 2026-05-21).
Read against working tree at HEAD `2bb03d1` ("D4 final cleanup");
every file:line ref re-verified with Read/Grep before citing.

---

## Bailing implementor's stated blocker

Verbatim from `docs/orchestrate/codebase-tightening/e2e-and-playwright/04-refactoring.md:465-521`:

> **Step 3 — DEFERRED**
>
> **Architect's plan:** per-gate `impl Gate` blocks (3a additive), then
> introduce `ActiveGate` + `pin_active_gate_camera` (3b), then delete
> the old `pin_*_camera` + `save_*_screenshot` per-gate fns (3c).
>
> **Why deferred — analytical, not bandwidth-based:**
>
> After surveying all 8 gate files in detail to scope this work, the
> trait shape in `e2e/gate.rs` (landed by D6 step 2's main implementor)
> **does not actually fit the data each gate needs to write during the
> Apply phase**:
>
> | Gate | `apply_edit` needs | Trait provides |
> |---|---|---|
> | OasisEdit | `world_data` + `OasisEditVisualState.edit_applied` write | `&mut WorldData` only |
> | SmallEditVisual | `world_data` + `SmallEditVisualState.voxel_count_before/after` + `world_size_voxels` writes + `edit_applied` | `&mut WorldData` only |
> | SmallEditRepro | `world_data` + `SmallEditReproState.edit_applied` (plus 2×2×2 pre-edit type sample) | `&mut WorldData` only |
> | VoxGpuConstruction | `OasisEditVisualState.edit_applied` (camera-promote signal) — does NOT need WorldData | `&mut WorldData` (wrong shape) |
>
> The trait shape was designed assuming Step 4 lands first (collapsing
> the per-gate `State` resources into `GateCaptures.aux`). Without
> Step 4, an `impl Gate::apply_edit` for OasisEdit / SmallEditVisual /
> SmallEditRepro **cannot mutate the per-gate state** the driver
> currently uses to drive the OasisApplyEdit / SmallEditApply /
> SmallEditReproApply phases.
>
> […]
>
> Landing ONLY those 7 methods (deferring `apply_edit` until Step 4)
> would still produce ~600 LOC of trait-impl scaffolding that NOTHING
> calls — the driver doesn't yet consume `Res<ActiveGate>`.
>
> **Status:** deferred (analytical reasoning, not bandwidth budget —
> the trait shape needs Step 4 to land coherently).

And Step 4 (`04-refactoring.md:525-547`):

> **Why deferred:**
> - Single biggest edit in the plan — 1240 LOC body replaced.
> - Verification load: ALL 8 gates ≥2× (≥3× for `--vox-gpu-oracle`)
>   + Resize-test on Hyprland + every gate's PNG SSIM-compared
>   against pre-refactor baseline. ~16 e2e runs minimum at 1-2 min each.
> - Architect's recommendation: do this as a single big edit, NOT in
>   pieces (the intermediate states between piece-by-piece edits
>   would not be buildable).
> - Coupling with Step 3: per architect §Step 4 spec, the per-gate
>   `State` resources collapse INTO `GateCaptures.aux` — that
>   migration cannot happen until each gate has an `impl Gate` block
>   declaring its aux shape. Step 4 = Step 3 + driver-rewrite.

---

## Verification of the claim

### The trait signature is exactly as quoted

`crates/bevy_naadf/src/e2e/gate.rs:97`:

```rust
fn apply_edit(&self, _world_data: Option<&mut WorldData>) -> Result<(), String> {
    Ok(())
}
```

Verified: the trait body has no `&mut World`, no per-gate state parameter, no
`Commands`, no `MessageWriter<AppExit>` — only an `Option<&mut WorldData>`.

### The 4 per-gate state writes are real (one nuance)

**OasisEdit** — `crates/bevy_naadf/src/e2e/driver.rs:986, 1009`:
the driver branches on `oasis.edit_applied`, sets `oasis.edit_applied = true`
post-brush. The `apply_erase_brush(&mut wd)` itself (`oasis_edit_visual.rs:253`)
takes only `&mut WorldData` — the `edit_applied` write is **driver-side**, not
inside `apply_erase_brush`. Implementor's claim accurate.

**SmallEditVisual** — `crates/bevy_naadf/src/e2e/driver.rs:1206, 1210`:
the driver calls `apply_small_cube_edit(&mut wd, &mut small_edit)` which is
defined at `crates/bevy_naadf/src/e2e/small_edit_visual.rs:323-377` and writes
ALL of:
- `state.world_size_voxels = Some([...])` at `:328`
- `state.voxel_count_before = Some(...)` at `:331`
- `state.voxel_count_after = Some(...)` at `:352`
- `state.edit_applied = true` at `:376`

So the function signature **today** requires `&mut SmallEditVisualState` as an
explicit parameter. Implementor's claim accurate.

**SmallEditRepro** — `crates/bevy_naadf/src/e2e/driver.rs:1389, 1393`: the
driver calls `apply_small_edit_repro_edit(&mut wd, &mut small_edit_repro)`
defined at `crates/bevy_naadf/src/e2e/small_edit_repro.rs:185-188` taking
`&mut WorldData, &mut SmallEditReproState`. Implementor's claim accurate.

**VoxGpuConstruction** — `crates/bevy_naadf/src/e2e/driver.rs:1003-1009`: in
vox-gpu-construction mode the driver calls
`super::vox_gpu_construction::promote_camera_to_pose_b()` (`:1005`) which
takes **zero** args (`crates/bevy_naadf/src/e2e/vox_gpu_construction.rs:313`)
— it's a pure `println!` stub. The load-bearing side effect is the
**driver-side** `oasis.edit_applied = true` at `:1009`, which the
**separate pin system** at `crates/bevy_naadf/src/e2e/vox_gpu_construction.rs:270-300`
reads via `Option<Res<OasisEditVisualState>>` to flip the camera pose.

This is the cross-gate read the implementor flagged. The pin system already
reads `OasisEditVisualState` declaratively as a system parameter (`:272`); it
does NOT need to be re-encoded into the gate trait. The "wrong shape" framing
is slightly misleading: VoxGpuConstruction's `apply_edit` needs **nothing**
inside the trait body — the brush is a stub, the side-effect is purely the
driver-internal "this phase ran" signal. Once the trait/driver get rewritten
so the driver tracks phase progression natively (architect §Finding 2: "the
one-shot guarantee is enforced by the phase transition, not by a per-gate
`edit_applied: bool`"), the cross-gate read can be replaced by reading
`E2eState.phase` or the `GateCaptures.aux` discriminator.

### The "~600 LOC dead impls" claim — partially overstated

LOC inventory at HEAD:

| file | LOC | per-gate impl footprint estimate |
|---|---|---|
| `oasis_edit_visual.rs` | 453 | ~70 LOC `impl Gate` |
| `small_edit_visual.rs` | 681 | ~85 LOC |
| `small_edit_repro.rs` | 376 | ~60 LOC |
| `vox_gpu_construction.rs` | 493 | ~70 LOC |
| `vox_gpu_oracle.rs` | 696 | ~60 LOC (single-capture) |
| `vox_web_parity.rs` | 428 | ~55 LOC (single-capture) |
| `vox_horizon_parity.rs` | 235 | ~50 LOC (single-capture) |
| `vox_e2e.rs` | 699 | ~40 LOC (standard) |

Estimated **~490 LOC** of new `impl Gate` blocks for 8 gates (each with 7
non-`apply_edit` methods that DO fit the current trait — implementor concedes
this at `04-refactoring.md:492-499`). The implementor's "~600 LOC" is in the
right ballpark — call it 500-600.

But **"dead" is the live word**: at HEAD there is zero consumer of these
impls. Verified via `grep -rn "impl Gate for\|dyn Gate\|<dyn Gate>\|Box<dyn
Gate>\|ActiveGate\|GateCaptures" crates/bevy_naadf/src/` → two hits,
both DOC comments (`bin/e2e_render.rs:333`, `e2e/gate.rs:74`). No code path
calls any `Gate::*` method anywhere. So Step 3a-only WOULD produce dead-code
scaffolding the driver doesn't consume. The implementor's structural reading
of "Step 3 + Step 4 must land together" stands on that front — but for the
WRONG underlying reason. (See next section.)

### Existing pin priority chain is real

`crates/bevy_naadf/src/e2e/mod.rs:249-282`: the seven `pin_*_camera` systems
are registered with an explicit `.after(driver::e2e_driver)` for `pin_oasis_camera`
and `.after(oasis_edit_visual::pin_oasis_camera)` for the other six. Five of
the six (`pin_vox_gpu_construction_camera`, `pin_vox_gpu_oracle_camera`,
`pin_vox_web_parity_camera`, `pin_vox_horizon_camera`, plus `pin_small_edit_*`)
all chain off the Oasis pin to **override** the birdseye write the Oasis pin
unconditionally emits when its mode flag is true. The audit's claim that
this 7-entry chain collapses to ONE registration once `pin_active_gate_camera`
consumes `Res<ActiveGate>` is structurally correct — only ONE gate is active,
so the override chain dissolves.

---

## Verification of the audit's hypothesis

The audit (`00-reuse-audit.md:84, 215-220, 246-254`) proposes:

> Switching `apply_edit` to `&mut World` (or to an extra State-Resource
> trait bound) is a one-parameter change that would resolve all 4
> trait-vs-data mismatches the implementor surfaced.

### Walk gate-by-gate with `apply_edit(&self, world: &mut World)`

Bevy permits exclusive `&mut World` system params (this is the foundational
"exclusive system" pattern); from inside an exclusive system any
`world.resource_mut::<R>()` access works. A `Gate::apply_edit` body could:

```rust
// OasisEdit
fn apply_edit(&self, world: &mut World) -> Result<(), String> {
    let mut wd = world.get_resource_mut::<WorldData>()
        .ok_or("WorldData missing at OasisApplyEdit")?;
    apply_erase_brush(&mut wd);
    drop(wd); // release borrow before re-borrowing state
    let mut oasis = world.resource_mut::<OasisEditVisualState>();
    oasis.edit_applied = true;
    Ok(())
}

// SmallEditVisual
fn apply_edit(&self, world: &mut World) -> Result<(), String> {
    // Two-step borrow split — drop WorldData borrow before SmallEditVisualState.
    // Alternative: pull both via .resource_scope.
    world.resource_scope::<SmallEditVisualState, _>(|world, mut state| {
        let mut wd = world.get_resource_mut::<WorldData>()
            .ok_or("WorldData missing at SmallEditApply")?;
        apply_small_cube_edit(&mut wd, &mut state);
        Ok(())
    })
}

// SmallEditRepro — same shape as SmallEditVisual

// VoxGpuConstruction
fn apply_edit(&self, world: &mut World) -> Result<(), String> {
    promote_camera_to_pose_b();
    // The "edit_applied = true on OasisState" hack disappears in the
    // post-decomp world — but if we land Step 3 BEFORE Step 4, the
    // current pin system still reads OasisEditVisualState.edit_applied.
    // So bridge: keep the write.
    world.resource_mut::<OasisEditVisualState>().edit_applied = true;
    Ok(())
}
```

All 4 gates compile against the `&mut World` signature. The cross-gate
`OasisEditVisualState.edit_applied` write VoxGpuConstruction needs is
accessible from `world.resource_mut::<OasisEditVisualState>()` — no new
state plumbing.

### `&mut World` precedent in the codebase

Verified via `grep -rn "fn.*&mut World" crates/bevy_naadf/src/`:

- `crates/bevy_naadf/src/render/pipelines.rs:351` — `FromWorld::from_world(world: &mut World)`.
  This is the `FromWorld` trait method, **not** a registered exclusive system.

No exclusive-system precedent in the actual e2e/ or render/ system surface.
This is mildly mitigating against the audit's "well-precedented in Bevy
idioms" framing — it IS a Bevy idiom (the engine supports it via the
`ExclusiveSystemParam` machinery), but this codebase doesn't currently
exercise it as a system signature. The trait method here would be CALLED FROM
a regular system (the driver), so we wouldn't be registering the trait method
as an exclusive system; we'd be calling it from within the driver's body.
The driver currently takes 14 system parameters (verified
`crates/bevy_naadf/src/e2e/driver.rs:452-468`) and would need to convert to
an exclusive `&mut World` signature itself to forward a `&mut World` to
`Gate::apply_edit`. That's a 14-param-to-1-param conversion, with every
existing `ResMut<X>` access becoming `world.resource_mut::<X>()` — a
substantial mechanical rewrite of the driver body even before Step 4
proper.

### Edge cases

1. **Borrow-checker discipline.** `world.resource_mut::<WorldData>()` borrows
   the world mutably; you cannot also hold `world.resource_mut::<State>()`
   simultaneously. Each gate's `apply_edit` must use sequential borrows or
   `resource_scope`. Not a blocker, just a discipline requirement.

2. **Tracing context.** The driver currently does `outcome.gate_result =
   Some(Err(err)); exit.write(AppExit::error())` on the WorldData-missing
   error path (`driver.rs:1018-1020`). Moving that logic into `Gate::apply_edit`
   means the trait must also expose `MessageWriter<AppExit>` access (via
   `world.send_event(AppExit::error())`-equivalent) or the error must
   propagate via the `Result<_, String>` return type, with the driver
   wrapping in the AppExit write. The current trait returns `Result<(),
   String>` — already structured for this; the driver wraps the error.
   No issue.

3. **`vox_gpu_construction`'s "OasisEditVisualState as cross-gate signal" hack.**
   In post-Step-4 world (`GateAuxState::VoxGpuConstruction { camera_promoted }`)
   this dissolves cleanly. In Step-3-only world (no GateCaptures yet, current
   pin systems still in place), the impl must keep writing to
   `OasisEditVisualState.edit_applied` to keep the existing
   `pin_vox_gpu_construction_camera` happy. Two LOC. Fine.

4. **The `Gate::camera_pose(world_data: Option<&WorldData>)` signature.**
   Verified at `gate.rs:92`: it takes `Option<&WorldData>` (immutable Res-shape).
   If we widen `apply_edit` to `&mut World`, consistency might suggest
   `camera_pose(&self, world: &World)`. But the current shape (immutable
   `&WorldData`) works fine — `pin_active_gate_camera` would read
   `Res<WorldData>` and pass `Some(&*wd)`. No need to widen unless we want
   the gate's camera pose to depend on other resources.

5. **`Resource` requirement.** All 5 cited per-gate State types (`oasis_edit_visual.rs:165`,
   `small_edit_visual.rs:188`, `small_edit_repro.rs:114`, `vox_gpu_oracle.rs:669`,
   `vox_web_parity.rs:140`) are `#[derive(Resource, Default)]`. Universally
   accessible from `world.resource_mut::<T>()`. No edge case.

**Conclusion:** the audit's hypothesis is correct on the structural facts —
`&mut World` cleanly accommodates all 4 gate mismatches. But it understates
the secondary work: converting the driver to forward `&mut World` to
`Gate::apply_edit` requires the driver itself to be either (a) an exclusive
system OR (b) refactored to call `Gate::apply_edit` via a `Commands`-style
deferred action (which is awkward — the brush mutation needs to be observable
in the next-frame capture). Path (a) IS the Step-4 driver decomposition, just
with `&mut World` as the API instead of `&mut Resource<X>` per gate. So the
audit's "one-parameter fix dissolves Step 3+4-must-land-together" framing
is **half-right**: it dissolves the trait-shape mismatch but does NOT
dissolve the driver-rewrite dependency. The driver still needs decomposing
either way.

---

## Diagnosis

**Category: (a) Real and architect-fixable.**

The bailing implementor's structural facts are accurate (verified
above). Their framing — "Step 3 + Step 4 must land together" — is **also
accurate**, but they identified the wrong dependency:

- **Implementor's stated dependency:** trait `apply_edit` signature is too
  narrow to carry per-gate state writes; needs Step 4's `GateCaptures.aux`
  redesign first.
- **Real dependency:** there is no driver call site to consume any `impl Gate`
  block at all. Step 3a alone produces dead-code scaffolding, regardless of
  whether the trait signature is `Option<&mut WorldData>` or `&mut World`.
  The dead-code problem isn't about parameter shape; it's about the absence
  of a consumer.

The audit is correct that **widening to `&mut World` resolves the
trait-shape problem** — but solving only that problem still leaves
~500 LOC of un-called trait impls. To make Step 3 land non-dead, the
driver must AT MINIMUM be modified to dispatch the active gate's
`Gate::apply_edit` in the `OasisApplyEdit`/`SmallEditApply`/`SmallEditReproApply`
arms. That's a partial Step 4 (not the full enum collapse).

**Architect-fixable shape:**

1. Revise the `gate.rs:97` `apply_edit` signature to `&mut World`. (Audit
   recommendation — correct.)
2. Document an INCREMENTAL landing path: Step 3 lands `impl Gate` for all
   8 gates AND the minimum driver wiring needed to dispatch them — i.e.
   create an `ActiveGate` resource, replace the per-arm `apply_erase_brush
   /apply_small_cube_edit/apply_small_edit_repro_edit/promote_camera_to_pose_b`
   calls in the driver with `active_gate.apply_edit(world)`. This is a
   strictly smaller change than the full Step 4 (the 49→8 enum collapse,
   the per-gate State→GateCaptures.aux migration, the route-in block
   removals all stay deferred). LOC delta: replace ~30 LOC of direct
   function calls in `OasisApplyEdit`/`SmallEditApply`/`SmallEditReproApply`/
   the vox-gpu-construction branch with ~15 LOC of `active_gate.apply_edit(...)`
   dispatches. The dead-code problem evaporates.
3. The pin-system collapse (`pin_active_gate_camera`) can land in the same
   step or split off — the trait's `camera_pose` method already fits cleanly.

So Step 3 becomes "trait impls + minimal driver wiring," Step 4 becomes
"full enum collapse + GateCaptures + route-in block removal." Each is
landable atomically with verifiable e2e gates.

The implementor was correct that the architect-as-spec'd Step 3 doesn't
work; they were wrong about the fix being "land it all together." The fix
is "expand Step 3 to include the dispatch wiring." Architect-side
intervention required.

---

## Proposed path forward

**(a) Fresh `delegate-architect` dispatch with focused brief on the
gate-trait + minimum driver-wiring redesign.**

The architect must produce a revised Step 3 spec that:

1. Widens `gate.rs:97` to `fn apply_edit(&self, world: &mut World) -> Result<(), String>`.
2. Adds an `ActiveGate` resource (a thin `pub struct ActiveGate(pub Box<dyn Gate>)`)
   to `gate.rs` and registers it in `e2e/mod.rs` based on `AppArgs.<flag>`.
3. Specifies the **minimum** driver edits to make Step 3 non-dead:
   convert `e2e_driver` to an exclusive system (`world: &mut World`),
   replace `super::oasis_edit_visual::apply_erase_brush(&mut wd)` etc.
   with `world.resource::<ActiveGate>().0.apply_edit(world)` (via a
   one-shot `resource_scope` to avoid borrow conflict).
4. Explicitly defers: the 49→8 `E2ePhase` collapse, the `GateCaptures.aux`
   tagged-enum, the route-in block removal, the `Standard`/`Resize`
   fast-path consolidation. Those remain Step 4.

If the architect produces that revised spec, a follow-up implementor
dispatch can land Step-3-revised atomically. Step 4 stays deferred until
a separate dedicated dispatch.

**Why architect-first (not just re-dispatch implementor):** the bailing
implementor's analytical reasoning was sound on the architect-spec
mismatch but their proposed remedy ("dispatch Steps 3+4 combined") still
inherits the architect's flawed sequencing assumption (that
`GateCaptures.aux` requires the full driver decomp). The architect needs
to validate that an INCREMENTAL trait dispatch (`Gate::apply_edit` with
`&mut World`, no `GateCaptures` yet) is sound; that's a decision only the
architect can make. Re-dispatching the same implementor without that
revision risks a third bail with the same framing.

**Briefing key correction (2-3 sentences):** "Step 3 as originally
spec'd produces unreachable scaffolding because no driver call site
consumes the trait. Revise the trait to take `&mut World` (per audit),
AND expand Step 3 to include the minimum driver wiring — convert
`e2e_driver` to exclusive (`world: &mut World`), introduce `ActiveGate`
resource, replace the 4 direct function calls in the 4 Apply arms with
`active_gate.apply_edit(world)`. Defer everything else in original Step 4
(enum collapse, GateCaptures, route-in block removal) to a separate
later dispatch — the goal of revised Step 3 is to land trait+minimum
driver consumption atomically with all 8 e2e gates green."

---

## Verification recipe

The architect's own stated verification load (per `04-refactoring.md:536`):
ALL 8 gates ≥2× plus `--ssim-compare`. Anchored here.

**Pre-revision sanity check (cheap, run first to confirm baseline):**

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace                                          # baseline compile
cargo test --workspace --lib                                     # 179 passing expected
timeout 120s cargo run --bin e2e_render -- --ssim-compare        # pure PNG diff, fastest
```

**Post-revision full verification (every e2e gate, multi-run for non-determ):**

```bash
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace
cargo test --workspace --lib

# Standard / deterministic gates — 1× each:
timeout 120s cargo run --bin e2e_render                                            # baseline
timeout 120s cargo run --bin e2e_render -- --vox-e2e
timeout 120s cargo run --bin e2e_render -- --entities
timeout 120s cargo run --bin e2e_render -- --edit-mode
timeout 120s cargo run --bin e2e_render -- --runtime-edit-mode
timeout 120s cargo run --bin e2e_render -- --validate-gpu-construction
timeout 120s cargo run --bin e2e_render -- --vox-gpu-construction
timeout 120s cargo run --bin e2e_render -- --small-edit-visual
timeout 120s cargo run --bin e2e_render -- --small-edit-repro
timeout 120s cargo run --bin e2e_render -- --vox-web-parity
timeout 120s cargo run --bin e2e_render -- --vox-horizon-native

# Non-deterministic gates — ≥3× per `feedback-multiple-runs-rule-out-false-positives`:
for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --oasis-edit-visual; done
for i in 1 2 3; do timeout 120s cargo run --bin e2e_render -- --vox-gpu-oracle; done

# SSIM short-circuit (post-everything):
timeout 60s cargo run --bin e2e_render -- --ssim-compare
```

**Resize gate (Hyprland only — skip if not on Hyprland):**

```bash
[ -n "$HYPRLAND_INSTANCE_SIGNATURE" ] && \
  timeout 120s cargo run --bin e2e_render -- --resize-test
```

Total: ~16 e2e runs at 1-2 min each + 2 fast runs + build/test. Architect's
~30-minute estimate is accurate.

**Pass criteria:**
- `cargo build --workspace` clean.
- `cargo test --workspace --lib` 179 passed, 1 ignored (same as HEAD).
- Every e2e gate prints the same PASS message it does at HEAD (audit cites
  `04-refactoring.md` Step-5-summary table for the per-gate PASS strings).
- `--ssim-compare` reports zero diff between pre-refactor and post-refactor
  PNGs (if pre-refactor PNGs are stashed under
  `target/e2e-screenshots-baseline/` first — recipe omits the stash step;
  implementor should produce a "pre" set, refactor, produce a "post" set,
  then diff. Standard architect-spec'd practice).

---

## Side notes / observations / complaints

- **The implementor was right structurally; the audit is right tactically.**
  The implementor correctly identified that the current trait shape doesn't
  work; the audit correctly identifies that `&mut World` resolves the shape
  problem. Both are missing the same thing: Step 3a-alone produces
  unreachable code REGARDLESS of trait signature, because there's no
  driver consumer. The deadlock isn't "trait shape vs state writes" — it's
  "no consumer for the trait." The right unblock is to revise Step 3 to
  include the minimum driver wiring (4 call-site swaps + driver→exclusive
  conversion), not to dispatch Step 3 and Step 4 as one mega-edit.

- **The architect's "Step 4 = Step 3 + driver-rewrite" coupling claim
  is partially load-bearing.** Architect spec at
  `03-architecture.md:486-498` says: "the one-shot guarantee is enforced
  by the phase transition, not by a per-gate `edit_applied: bool`. The
  fields on the per-gate `State` structs go away
  (`OasisEditVisualState.edit_applied`, etc.). **Exception**:
  `vox_gpu_construction` reads `oasis.edit_applied` to promote camera
  pose A→B; that signal moves to a dedicated `VoxGpuConstructionState.
  camera_promoted: bool` set by the gate's `apply_edit`." That migration
  IS coupled to Step 4. But it's NOT coupled to Step 3 — Step 3 can
  land the trait impls + minimum dispatch, leaving the
  `edit_applied/voxel_count_before/world_size_voxels/camera_promoted`
  fields in place; Step 4 then removes them. Two atomic landings, both
  green. The "must land together" framing was the architect's failure
  to factor the coupling correctly, not a hard structural constraint.

- **No exclusive `&mut World` system signature exists in this codebase
  today.** Verified via grep. The closest precedent is
  `FromWorld::from_world` (`render/pipelines.rs:351`) — a different
  trait, not a registered system. Converting `e2e_driver` from a 14-param
  function-system to an exclusive `world: &mut World` system is a
  precedent-setting change. Not blocking — Bevy fully supports it — but
  worth the architect knowing.

- **`bin/e2e_render.rs:336` already carries `let _ = gate;`** — the
  `GateKind` is being threaded through `BootCommand::NamedGate { gate,
  run }` and then dropped at the run-site. Step 5's structural shape is
  ALREADY anticipating Step 3+4. Once `ActiveGate` lands, that `let _ =
  gate;` becomes `commands.insert_resource(ActiveGate(make_gate(gate)));`
  or similar. The post-Step-5 binary shape made Step 3 cheaper, not
  harder.

- **The audit's `00-reuse-audit.md` is high-quality on this item.** Its
  flip-conditions on lines 84-85 are well-scoped and actionable: "if
  widening signature unblocks Step 3 atomically without Step 4, the
  deferred-twice deadlock breaks" — that's exactly the question I had to
  verify. I confirm the widening unblocks the trait-shape mismatch but
  NOT the dead-code problem; the audit slightly overstates "one
  parameter change resolves all 4 mismatches" because the dead-code
  problem isn't a parameter-shape mismatch. The audit's tactical signal
  is right; the cause-and-effect framing is one layer too shallow.

- **Sub-agent honey-trap risk for the architect dispatch.** The
  architect doc (`03-architecture.md`) is 1683 lines and elaborately
  specifies the post-refactor target state. The revised Step 3 brief
  must explicitly say "do NOT re-architect the full Step 4; the only
  question is what minimum subset of driver wiring makes Step 3 non-dead
  atomically." Without that scope-cap, the architect may produce another
  full-rewrite spec that re-creates the same Step-3+4-must-land-together
  deadlock.

- **Faithful-port compliance: no concern.** This is pure refactor — no
  C#-divergent behavior introduced. CPU oracle untouched (Step 3+4 are
  e2e-harness-internal). The Bevy idiom (`&mut World` exclusive system)
  doesn't correspond to anything on the C# side; the C# port has no e2e
  trait equivalent. The architect picked a reasonable Bevy-native shape;
  the question is purely whether it lands incrementally or atomically.

- **The 4-gate mismatch table in `04-refactoring.md:480-483` is
  technically accurate but rhetorically misleading.** It says
  "VoxGpuConstruction: `OasisEditVisualState.edit_applied` (camera-promote
  signal) — does NOT need WorldData | trait provides `&mut WorldData` (wrong
  shape)". The "wrong shape" framing implies a type-mismatch problem; the
  actual issue is cross-gate state coupling that the post-Step-4 design
  resolves cleanly (via `GateAuxState::VoxGpuConstruction { camera_promoted
  }`). With `&mut World` the cross-gate coupling becomes one line
  (`world.resource_mut::<OasisEditVisualState>().edit_applied = true`)
  in the bridge period. Trivial.
