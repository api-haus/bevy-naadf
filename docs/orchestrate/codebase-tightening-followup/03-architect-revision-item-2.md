# Item 2 architect revision — D6 revised Step 3 spec

Architect: read-only sub-agent (codebase-tightening-followup orchestration, 2026-05-21).
Read against working tree at HEAD `2bb03d1` ("D4 final cleanup");
every file:line ref re-verified with Read/Grep before citing.

This file revises the original Step 3 spec at
`docs/orchestrate/codebase-tightening/e2e-and-playwright/03-architecture.md:1193-1265`.
Step 4 (originally `03-architecture.md:1267-1330`) stays deferred verbatim;
see `## Step 4 explicit deferral` below.

---

## Original Step 3+4 context

The original D6 Step 3 (`03-architecture.md:1193`) spec'd a substep ladder —
3a per-gate `impl Gate` blocks (additive), 3b `ActiveGate` resource +
`pin_active_gate_camera`, 3c delete the per-gate `pin_*_camera` +
`save_*_screenshot` fns. The bailing implementor
(`04-refactoring.md:465-521`) correctly surfaced that
`gate.rs:97`'s `apply_edit(&self, _world_data: Option<&mut WorldData>)`
signature cannot carry the per-gate State writes the driver currently
needs at OasisApplyEdit (`OasisEditVisualState.edit_applied`),
SmallEditApply (`SmallEditVisualState.{voxel_count_before,
voxel_count_after, world_size_voxels, edit_applied}`),
SmallEditReproApply (`SmallEditReproState.edit_applied`), and the
vox-gpu-construction sub-branch inside OasisApplyEdit
(`OasisEditVisualState.edit_applied` as cross-gate camera-promote
signal). The investigator
(`02-investigation-item-2-d6-gate.md`) verified this **and** identified
the deeper issue: **Step 3a-alone produces dead-code regardless of
trait signature, because the driver has no consumer for any `impl Gate`
value at any call site at HEAD**. Grep confirms: `impl Gate for|dyn
Gate|Box<dyn Gate>|ActiveGate` matches only two doc comments
(`bin/e2e_render.rs:333`, `e2e/gate.rs:74`). The right fix expands Step 3
to include the minimum driver wiring that makes the trait reachable —
widen the trait AND give it a consumer. The full Step 4 (49→8 enum
collapse, `GateCaptures.aux`, route-in block removal, fast-path
consolidation, pin-system collapse) stays deferred.

---

## Revised Step 3 spec (the deliverable)

**Goal:** land an atomic non-dead Step 3 that consists of the trait
widening, per-gate `impl Gate` blocks, an `ActiveGate` resource, the
minimum driver-side dispatch wiring needed to make the trait reachable,
and the 4 call-site swaps that exercise it.

**Atomicity contract:** the 5 items below land as ONE commit. No
intermediate state is buildable; the trait signature change cascades
through all 8 gates and the driver in lockstep.

**Pass criteria:** ALL 8 e2e gates green after the single landing,
non-deterministic gates (`--oasis-edit-visual`, `--vox-gpu-oracle`)
green ≥3× each.

### Item 1 — Widen trait signature on `gate.rs:97` to `&mut World`

**Rule.** Replace
```rust
fn apply_edit(&self, _world_data: Option<&mut WorldData>) -> Result<(), String> {
    Ok(())
}
```
at `crates/bevy_naadf/src/e2e/gate.rs:94-99` with
```rust
fn apply_edit(&self, _world: &mut World) -> Result<(), String> {
    Ok(())
}
```
The `_world: &mut World` parameter lets each gate's body fetch its own
per-gate State resource via `world.resource_mut::<XxxState>()` (and
`WorldData` via `world.get_resource_mut::<WorldData>()` for gates that
need it). Default `Ok(())` no-op stays as today's signature — only the
parameter shape changes. The doc-comment block at `gate.rs:94-96`
updates accordingly ("each gate's body fetches its own state
resources via `world.resource_mut`"). The doc reference at `gate.rs:84-91`
on `camera_pose` stays unchanged — `camera_pose` keeps
`Option<&WorldData>` because `pin_active_gate_camera` only needs
immutable read access.

**Lift-into-code.** Edit `gate.rs:97` directly. Drop the
`#![allow(dead_code)]` line at `gate.rs:18` ONLY after item 4 lands a
driver consumer (otherwise the trait is still unused mid-step in the
single-commit landing — keep the allow until items 2-4 are all wired,
then drop in the same commit).

**Why `&mut World` and not enriched parameter list:** see
`## Decisions & rejected alternatives` D-A below.

### Item 2 — Introduce `ActiveGate` resource in `gate.rs`

**Rule.** Add to `gate.rs` (after the existing `set_camera_pose`
helper at `:134-137`):

```rust
/// The active gate for this e2e run. Inserted by `e2e/mod.rs`'s
/// `add_e2e_systems` based on the `AppArgs.*_mode` flag that boot
/// time set. Read by the driver's Apply-arm dispatches and (in Step 4)
/// by `pin_active_gate_camera`.
#[derive(Resource)]
pub struct ActiveGate(pub Box<dyn Gate>);
```

In `e2e/mod.rs`, between the `init_resource` calls at lines 226-235
and the `add_systems` block at line 249, insert a per-gate match that
boxes the correct `impl Gate` value into `ActiveGate` and inserts it
as a resource. The match keys on the `AppArgs.*_mode` boolean fields
(verified `app_args.rs:80-122`+):

- `args.resize_test == true` → no `ActiveGate` insertion (Resize keeps
  its inline state machine for revised Step 3; see Step 4 deferral).
- `args.oasis_edit_visual_mode` → `ActiveGate(Box::new(OasisEditVisualGate))`.
- `args.vox_gpu_construction_mode` → `ActiveGate(Box::new(VoxGpuConstructionGate))`.
- `args.small_edit_visual_mode` → `ActiveGate(Box::new(SmallEditVisualGate))`.
- `args.small_edit_repro_mode` → `ActiveGate(Box::new(SmallEditReproGate))`.
- `args.vox_e2e_mode` → `ActiveGate(Box::new(VoxE2eGate))` (kind = `Standard`).
- (vox-gpu-oracle / vox-web-parity / vox-horizon-parity → see
  Assumption A-2).
- else → `ActiveGate(Box::new(StandardGate))`.

Each gate's `Gate`-impl unit struct (`OasisEditVisualGate`,
`SmallEditVisualGate`, etc.) is `#[derive(Default)]` and zero-sized;
its `impl Gate` block lives in the gate's own module (per item 3).

**Lift-into-code.** New `pub struct ActiveGate(pub Box<dyn Gate>);` at
end of `gate.rs`. New registration block in `e2e/mod.rs` between
`:235` and `:249`. The registration reads `app_args` via a `Startup`
system or via reading the `AppArgs` resource directly inside
`add_e2e_systems` (verify: `AppArgs` is inserted as a `Resource` —
search confirms `crate::AppArgs` is `Option<Res<...>>` accessible from
the driver; same path works at startup-system time inside `add_e2e_systems`).

**Why `ActiveGate` resource and not enum-based dispatch:** see
D-B below.

### Item 3 — Per-gate `impl Gate` blocks land at the same commit

**Rule.** Each of the 8 gates implements `Gate` for its zero-sized
unit struct. The 7 methods that already fit the current trait shape
(`kind`, `frame_budget`, `camera_pose`, `assert`, `verdict_log`,
`capture_filenames`, `log_tag` — enumerated at `04-refactoring.md:492-499`)
are mechanical wrappers around existing per-gate consts and fns. The
8th method, `apply_edit`, is the load-bearing change:

| Gate impl (module) | `apply_edit` body |
|---|---|
| `OasisEditVisualGate` (`oasis_edit_visual.rs`) | Fetch `world.get_resource_mut::<WorldData>()` → call `apply_erase_brush(&mut wd)` (verbatim `oasis_edit_visual.rs:253`); drop `wd` borrow; `world.resource_mut::<OasisEditVisualState>().edit_applied = true`. |
| `SmallEditVisualGate` (`small_edit_visual.rs`) | `world.resource_scope::<SmallEditVisualState, _>(|world, mut state| { let mut wd = world.get_resource_mut::<WorldData>().ok_or(...)?; apply_small_cube_edit(&mut wd, &mut state); Ok(()) })`. |
| `SmallEditReproGate` (`small_edit_repro.rs`) | Same shape as `SmallEditVisualGate`; calls `apply_small_edit_repro_edit(&mut wd, &mut state)` (verbatim `small_edit_repro.rs:185-188`). |
| `VoxGpuConstructionGate` (`vox_gpu_construction.rs`) | `promote_camera_to_pose_b()` (verbatim `vox_gpu_construction.rs:313`); then `world.resource_mut::<OasisEditVisualState>().edit_applied = true` (bridges the cross-gate signal; see Side note 5). |
| `VoxGpuOracleGate` (`vox_gpu_oracle.rs`) | Default `Ok(())` no-op (single-capture gate, no edit). |
| `VoxWebParityGate` (`vox_web_parity.rs`) | Default `Ok(())` no-op. |
| `VoxHorizonParityGate` (`vox_horizon_parity.rs`) | Default `Ok(())` no-op. |
| `VoxE2eGate` (`vox_e2e.rs`) | Default `Ok(())` no-op. |
| `StandardGate` (`oasis_edit_visual.rs` adjacent — or new submodule) | Default `Ok(())` no-op. |
| `ResizeGate` | NOT impl'd in revised Step 3; resize keeps inline state machine. See Step 4 deferral. |

**Lift-into-code.** New `impl Gate for <GateStruct> { … }` blocks at
the bottom of each cited gate module. The 7 non-`apply_edit` methods
are mechanical lifts of existing consts/fns — each gate already exports
the consts (`*_WARMUP_FRAMES`, `*_DRAIN_FRAMES`, `*_PNG`, `*_LOG_TAG`,
etc.) and the per-gate `assert_*_landed` fn. Wrap them; no logic
change. Estimated ~490 LOC of new `impl` blocks total (per
investigator at `02-investigation-item-2-d6-gate.md:127-144`).

**The 7 methods fit verbatim today** (no Step-4 dependency for them);
only `apply_edit` was blocked by the parameter shape, now resolved by
item 1.

### Item 4 — Convert `e2e_driver` to an exclusive `&mut World` system

**Rule.** Change `crates/bevy_naadf/src/e2e/driver.rs:452-469`
(the 14-param function-system signature) to:

```rust
pub fn e2e_driver(world: &mut World) {
    // ... body fetches each resource individually via
    //     world.resource_mut::<R>() / world.get_resource::<R>()
}
```

Every existing `ResMut<X>` / `Res<X>` / `Single<...>` / `Commands` /
`MessageWriter<AppExit>` parameter becomes a fetch inside the body.
The `Single<(&mut Transform, &mut PositionSplit), With<Camera3d>>`
pattern at `:465` becomes a query: `world.query_filtered::<(&mut
Transform, &mut PositionSplit), With<Camera3d>>().single_mut(world)`
(verify imported API surface — see Assumption A-3). `MessageWriter<AppExit>`
becomes `world.send_event(AppExit::error())` or
`world.resource_mut::<Events<AppExit>>().send(AppExit::error())` —
again, verify Bevy 0.19 API surface (Assumption A-3).

**Borrow-checker discipline.** The 14-param shape gives independent
`ResMut`s with non-overlapping borrows enforced by the system param
collector. The exclusive shape gives a single `&mut World` that can
only hold one borrow at a time. Each existing `ResMut<X>` access
becomes a sequenced `world.resource_mut::<X>()` borrow at the use
site; long-lived borrows must be released (via scope) before the next
borrow. `world.resource_scope::<R, _>(|world, mut r| { ... })` is the
right tool for cases where two resources need to mutate in a
single arm (per `02-investigation-item-2-d6-gate.md:197-206`). The vast
majority of arms touch one resource at a time and become straight-line
`world.resource_mut::<X>()` accesses.

**Lift-into-code.** This is the largest mechanical edit in revised
Step 3 — ~1956 LOC body, every `ResMut/Res/Single/Commands/MessageWriter`
parameter access translated. Touch only the system signature and the
in-body accesses; do NOT touch the arm logic, do NOT collapse arms, do
NOT remove route-in blocks (those are Step 4). The 49-variant
`E2ePhase` enum stays. The 6 route-in fast-path blocks at
`driver.rs:475-577` stay. Only the resource-access mechanism changes.

The `add_systems` registration at `e2e/mod.rs:252` must add the
exclusive-system marker if Bevy 0.19 requires it (verify via existing
Bevy docs / source — exclusive systems are auto-detected from the
`fn(world: &mut World)` shape on registration in modern Bevy; no
explicit `.exclusive_system()` call needed; Assumption A-3).

**Why convert driver to exclusive vs. partition `apply_edit` into
per-gate trait methods:** see D-C below.

### Item 5 — Replace the 4 direct function calls in the Apply arms with `ActiveGate.apply_edit(world)`

**Rule.** In the three driver Apply arms — `OasisApplyEdit`
(`driver.rs:981-1022`), `SmallEditApply` (`driver.rs:1205-1223`),
`SmallEditReproApply` (`driver.rs:1388-1409`) — replace the direct
function calls with one `ActiveGate.apply_edit(world)` dispatch via
`resource_scope`:

```rust
// Pattern (Bevy 0.19 idiom):
let result = world.resource_scope::<ActiveGate, _>(|world, active_gate| {
    active_gate.0.apply_edit(world)
});
match result {
    Ok(()) => {
        // Advance to *WaitPostEdit phase as today
        world.resource_mut::<E2eState>().phase = E2ePhase::OasisWaitPostEdit;
        world.resource_mut::<E2eState>().phase_ticks = 0;
    }
    Err(msg) => {
        let err = format!("<gate>: {msg}");
        eprintln!("e2e_render: FAIL — {err}");
        world.resource_mut::<E2eOutcome>().gate_result = Some(Err(err));
        world.send_event(AppExit::error()); // or Events<AppExit> path
        world.resource_mut::<E2eState>().phase = E2ePhase::Done;
    }
}
```

`resource_scope` temporarily removes `ActiveGate` from the world for
the duration of the closure; this avoids the borrow conflict between
"`world.resource::<ActiveGate>()` borrows" and the closure body's
internal `world.resource_mut::<XxxState>()` borrows. Inside the closure,
`active_gate` is owned (`Mut<ActiveGate>`-equivalent) and the body
freely re-borrows the world. Standard Bevy idiom (verify imports —
Assumption A-3).

**The 4 call sites to replace** (per investigator
`02-investigation-item-2-d6-gate.md:282-298`):

1. `driver.rs:1007` (`apply_erase_brush(&mut wd)` for OasisEdit). Replaced
   by `OasisEditVisualGate::apply_edit` body (item 3).
2. `driver.rs:1005` (`promote_camera_to_pose_b()` for vox-gpu-construction
   sub-branch inside `OasisApplyEdit`). Replaced by
   `VoxGpuConstructionGate::apply_edit` body (item 3).
3. `driver.rs:1210` (`apply_small_cube_edit(&mut wd, &mut small_edit)`).
   Replaced by `SmallEditVisualGate::apply_edit` body.
4. `driver.rs:1393-1396` (`apply_small_edit_repro_edit(&mut wd, &mut
   small_edit_repro)`). Replaced by `SmallEditReproGate::apply_edit`
   body.

**Note:** sites 1 and 2 live inside the SAME driver arm
(`OasisApplyEdit`, `driver.rs:981-1022`). After the swap, both branches
collapse into ONE `active_gate.apply_edit(world)` dispatch — the gate's
own `apply_edit` body knows whether to call `apply_erase_brush` (Oasis
case) or `promote_camera_to_pose_b` (vox-gpu-construction case). The
`if vox_gpu_construction_mode { … } else { … }` branch at
`driver.rs:1003-1008` evaporates. The `oasis.edit_applied = true` write
at `:1009` moves INTO each gate's `apply_edit` body (it remains the
load-bearing cross-gate signal — see Side note 5).

**Behavioural delta = zero.** Each gate's `apply_edit` body does
EXACTLY what the inline driver code did — same brush call, same state
writes, same cross-gate `edit_applied` write. Only the dispatch
mechanism moves from "driver calls the per-gate fn directly" to
"driver dispatches through the trait."

**Lift-into-code.** Three Apply-arm rewrites in `driver.rs`. Each
arm shrinks from ~30 LOC of direct calls + WorldData-missing branches
to ~15 LOC of `resource_scope` dispatch + result-match. The
WorldData-missing failure path migrates INTO each gate's `apply_edit`
body (the gate returns `Err("WorldData missing at <gate>".to_string())`).
The per-arm error formatting (`"oasis-edit-visual: WorldData missing
at OasisApplyEdit ..."`) is preserved as the `Err` payload.

**Why include the 4 call-site swaps in Step 3 (not Step 4):** see D-D
below.

### Ordering between items

The 5 items are atomic (one commit), but the implementor must
sequence the edits to keep `cargo build --workspace` green at
intermediate workspace-state during the impl session:

1. **Item 1 first** (widen trait signature). Compile-clean alone; no
   consumer of `apply_edit` exists.
2. **Item 3 next** (per-gate `impl Gate` blocks). Each impl block
   uses the new `&mut World` signature. Compiles clean against the
   widened trait. Still no driver consumer.
3. **Item 2 third** (`ActiveGate` resource definition + e2e/mod.rs
   registration). Inserts the resource; nothing reads it yet. Compiles.
4. **Item 4 fourth** (driver conversion to exclusive). The driver
   compiles against `&mut World`; the existing 4 Apply arms still
   call the original direct functions. Compiles green; gates pass
   unchanged.
5. **Item 5 last** (Apply-arm swap to `active_gate.apply_edit(world)`).
   Final wiring. Compiles, gates green, dead-code problem dissolved.

After item 5, `#![allow(dead_code)]` at `gate.rs:18` may be dropped
(the trait is now consumed). The implementor's single commit covers
all 5 items.

---

## Step 4 explicit deferral

Step 4 stays deferred verbatim per
`docs/orchestrate/codebase-tightening/e2e-and-playwright/03-architecture.md:1267-1330`
and `04-refactoring.md:525-547`. Specifically: the **49→8 `E2ePhase`
enum collapse** (driver.rs:58-248), the **`GateCaptures` struct +
`GateAuxState` tagged enum** introduction, the **per-gate `State`
resource migration into `GateAuxState`** (`OasisEditVisualState`,
`SmallEditVisualState`, `SmallEditReproState`, `VoxGpuOracleState`,
`VoxWebParityState` — collapse 5 resources to 1), the **6 route-in
fast-path block removals** (`driver.rs:475-577`), the **`Standard` /
`Resize` fast-path consolidation** (the two named-exception arms),
and the **pin-system collapse to single `pin_active_gate_camera`** (the
7-system `.after(driver::e2e_driver)` chain at `e2e/mod.rs:249-282`
collapsing to one registration consuming `Res<ActiveGate>`).

**What triggers a Step 4 dispatch:** revised Step 3 lands green AND
the orchestrator decides the next 16-run verification load (per
`04-refactoring.md:534-537`) is worth the dispatch. The Step 4 design
is unchanged by revised Step 3 — the `ActiveGate` resource Step 3
introduces is the same resource Step 4's `GateCaptures` consumes; the
trait shape is stable; the per-gate `impl Gate` bodies move their
`State` writes from `world.resource_mut::<XxxState>()` to
`captures.aux: GateAuxState`. Step 4 becomes a pure
"collapse-and-migrate" pass, not a "design-and-collapse" pass. The
coupling Step 4 had with Step 3 (per architect at `04-refactoring.md:541-544`,
"per-gate `State` collapse INTO `GateCaptures.aux` cannot happen until
each gate has an `impl Gate` block declaring its aux shape") is now
satisfied — every gate has an `impl Gate` block.

---

## Decisions & rejected alternatives

### D-A — `&mut World` vs. extending the existing `Option<&mut WorldData>` with extra state-resource trait bounds

**Chose:** `&mut World`.

**Rejected:** widening `apply_edit` to take an explicit per-gate state
tuple (e.g.
`apply_edit(&self, world_data: Option<&mut WorldData>, state: &mut dyn Any)`)
or one parameter per state resource the architect enumerates upfront.

**Why:** the parameter-tuple approach requires the trait to enumerate
every per-gate State resource type upfront — `OasisEditVisualState`,
`SmallEditVisualState`, `SmallEditReproState`, `VoxGpuOracleState`,
`VoxWebParityState` — which (a) leaks gate identity into the trait
shape (defeats the trait abstraction), (b) bloats the parameter list
for the no-op gates (5 of 9 gates don't write any per-gate State in
`apply_edit`), (c) requires the trait to make per-gate decisions about
which State resources to expose — i.e. the trait knows about every
gate's internals. `&mut World` gives every gate uniform access to
ANY resource it needs (current AND future) with one type parameter.
The trait stays gate-agnostic. The borrow-checker discipline (sequenced
`world.resource_mut` borrows or `resource_scope`) is per-gate-body
local, not a trait-shape concern.

**What would flip the call:** if Bevy 0.19's borrow-checker / exclusive
system machinery had hidden costs the investigator missed — e.g. if
`resource_scope` is deprecated in the imported API surface, or if the
exclusive-system shape conflicts with another scheduling constraint
the architect spec assumed. Verify before landing (Assumption A-3).

### D-B — `ActiveGate` resource (`pub struct ActiveGate(pub Box<dyn Gate>)`) vs. enum-based gate dispatch

**Chose:** `ActiveGate(Box<dyn Gate>)` resource.

**Rejected:** `enum ActiveGate { OasisEdit(OasisEditVisualGate),
SmallEditVisual(SmallEditVisualGate), … }` with `match active_gate {
… }` at each call site.

**Why:** the architect's original D1 decision
(`03-architecture.md:1441-1456`) was already `Box<dyn Gate>` over
`enum Gate`. Quoting (`:1448-1454`): "`Box<dyn Gate>` is the
canonical Bevy idiom for plugin-like extension; `enum Gate` would
require every new gate to extend the enum (closed-set), defeating the
trait abstraction's purpose of letting gates be added module-locally."
The investigator's `02-investigation-item-2-d6-gate.md` does not
challenge this decision; the audit's reuse-target
(`00-reuse-audit.md:77`) cites `pub struct ActiveGate(Box<dyn Gate>)`
explicitly. Box-dyn is consistent with the original architect intent;
the cost (vtable indirection) is negligible for the once-per-frame
dispatch the driver does. Enum dispatch would force every new gate to
modify a shared file, contradicting the trait-isolation goal.

**What would flip the call:** measurable dispatch overhead at the
once-per-frame call site. Negligible — the dispatch happens once per
e2e run during the Apply phase. Not a concern.

### D-C — Convert driver to exclusive `&mut World` system vs. partition `apply_edit` into smaller per-gate-trait methods

**Chose:** convert driver to exclusive `&mut World` system.

**Rejected:** partition the trait into N smaller methods —
`apply_brush(&mut WorldData)`, `apply_count_writes(&mut SmallEditVisualState)`,
`apply_camera_promote(&mut OasisEditVisualState)` — so each method
takes only the resources it needs, keeping the driver as a 14-param
function-system that fans out to the per-method calls.

**Why:** partitioning the trait reintroduces the original architect
problem in a worse shape. Each `apply_*` method adds a per-gate
specialisation to the trait surface; gates that don't need a method
either default-no-op (clutter) or omit (breaks the uniform call
mechanism the driver wants). The driver's Apply-arm dispatch becomes
`if gate.kind() == OasisEdit { gate.apply_brush(world_data);
gate.apply_camera_promote(oasis); } else if gate.kind() == SmallEditVisual { … }`
— which is exactly the per-gate dispatch the trait was supposed to
abstract over. `&mut World` collapses N methods into 1 with uniform
access. The driver becomes one `active_gate.apply_edit(world)` call
that works for every gate uniformly. The cost is the precedent-setting
exclusive-system signature (no current `&mut World` system in the
codebase per investigator `:493-499`) — but this IS a Bevy idiom and
the path Step 4 was always going to require regardless. Setting the
precedent in revised Step 3 ratchets us forward; the precedent has to
be set somewhere.

**What would flip the call:** if Bevy 0.19's exclusive system has
scheduling constraints we've missed — e.g. exclusive systems can't run
in parallel with anything else in the same schedule label, which could
break the `.before(crate::camera::sync_position_split)` ordering edge
at `e2e/mod.rs:281`. The pin systems all run `.after(driver::e2e_driver)`
and `.before(sync_position_split)`; the chain is fragile. Verify
exclusive-system scheduling semantics in Bevy 0.19 before landing.

### D-D — Include the 4 call-site swaps in Step 3 (not Step 4)

**Chose:** include the swaps in revised Step 3.

**Rejected:** keep the swaps in Step 4 ("trait infrastructure first,
swap consumers later"). I.e. land items 1-3 in Step 3a, items 4-5 in
Step 4.

**Why:** this is exactly the bailing implementor's framing
(`04-refactoring.md:502-509`: "Landing ONLY those 7 methods … would
still produce ~600 LOC of trait-impl scaffolding that NOTHING calls
— the driver doesn't yet consume `Res<ActiveGate>`"). The investigator
(`02-investigation-item-2-d6-gate.md:310-339`) verified this is the
real dead-code problem and the fix is "expand Step 3 to include the
minimum driver wiring." If the swaps land in Step 4, Step 3 produces
unreachable code, the build emits dead-code warnings (or relies on
the `#![allow(dead_code)]` at `gate.rs:18`), and the architect-spec
constraint "every step lands buildable + green" becomes "every step
lands buildable + green except Step 3 which is intentionally
unreachable until Step 4." That's the deadlock the previous
implementor bailed on. Including the swaps in Step 3 dissolves the
dead-code problem at landing time. The atomicity contract is
unchanged from the architect's original intent (one commit), only the
contents of the commit expand.

**What would flip the call:** if the verification load of "Step 3
+ Step 4 in one commit" exceeds the implementor's bandwidth so badly
that splitting them by some other axis (e.g. trait + ActiveGate +
4 gate impls in one commit, the 4 no-op gate impls in another) is
cheaper than the current shape. Doesn't apply — revised Step 3 lands
~500 LOC of trait impls + ~30 LOC of driver edits + 1 ActiveGate
resource + 1 exclusive-system conversion; the bandwidth is moderate,
not extreme. Step 4 remains the bigger edit (1240 LOC body replaced;
49→20 variant enum collapse; 5→1 resource consolidation).

---

## Assumptions made

### A-1 — The 4 cited Apply-arm sites are the ONLY 4 sites that route to per-gate state writes

**Assumed:** `driver.rs:981` (`OasisApplyEdit` — handles both OasisEdit
and vox-gpu-construction), `:1205` (`SmallEditApply`), `:1388`
(`SmallEditReproApply`) are the only Apply-arm sites; the
`promote_camera_to_pose_b` call at `:1005` lives inside `OasisApplyEdit`
(so 3 arms = 4 call sites). The other 5 gates (vox-gpu-oracle,
vox-web-parity, vox-horizon-parity, vox-e2e, standard) have no Apply
phase and no `apply_edit` dispatch is needed for them; their
`Gate::apply_edit` is the default `Ok(())` no-op.

**Verify:** the implementor must confirm by grepping
`grep -n "ApplyEdit\b\|Apply\b" crates/bevy_naadf/src/e2e/driver.rs`
that no other Apply-style arm exists (the investigator's grep
confirms this — `:981`, `:1205`, `:1388` are the only three). If a
gate I missed has a similar pattern hidden under a different name, its
`impl Gate::apply_edit` body and the driver call-site swap must be
added.

### A-2 — Single-capture gates (vox-gpu-oracle, vox-web-parity, vox-horizon-parity) need an `ActiveGate` registration but never trigger `apply_edit`

**Assumed:** these gates run the single-capture flow (Warmup→Shoot→
Drain→Assert) and never enter an Apply phase, so their `impl Gate::apply_edit`
is the default `Ok(())` no-op. Their `ActiveGate` registration is
needed only for `pin_active_gate_camera` consumption in Step 4 — in
revised Step 3, the pin systems still read `OasisEditVisualState` /
their per-gate state directly. The `ActiveGate(Box::new(VoxGpuOracleGate))`
insertion is essentially "for future Step 4" — but landing it in Step 3
keeps the ActiveGate registration uniform across all 8 gates.

**Verify:** the implementor must confirm the single-capture gates'
flow at their respective module-bottoms (`vox_gpu_oracle.rs`,
`vox_web_parity.rs`, `vox_horizon_parity.rs`). If any of them has a
hidden pre-edit hook the revised Step 3 misses, its `Gate::apply_edit`
must be implemented non-default.

### A-3 — Bevy 0.19's API surface (`resource_scope`, `world.send_event`, exclusive-system shape, `query_filtered`)

**Assumed:** Bevy 0.19 (the bevy_naadf workspace's pinned version)
exposes:
- `World::resource_scope::<R, _>(|world, mut r| { ... })` — temporarily removes
  resource R from the world for the closure body.
- `World::send_event<E>(event: E)` or `World::resource_mut::<Events<E>>().send(event)`
  for `AppExit` writes from inside an exclusive system.
- `fn(world: &mut World)` as a registrable exclusive-system shape —
  auto-detected by `add_systems(Update, e2e_driver)` (no explicit
  `.exclusive_system()` qualifier needed in modern Bevy).
- `world.query_filtered::<(&mut Transform, &mut PositionSplit), With<Camera3d>>().single_mut(world)`
  as the in-body replacement for the `Single<>` system param.

**Verify:** the implementor must check the imported Bevy API surface
before applying. If any of these is wrong (e.g. `send_event` was
removed in 0.19 in favour of `EventWriter`, or `resource_scope` was
deprecated), the gate body / driver body adjusts to the actually-imported
API. The mechanism (temporarily-detached resource for borrow-conflict
avoidance; exclusive-world resource access; exit event write) is
universal Bevy ECS; only the spelling may shift.

### A-4 — Atomic single-commit landing is feasible inside one implementor session

**Assumed:** the 5 items can land as one commit inside the
implementor's tool-call budget. The work is:
- 1 trait-signature edit (`gate.rs:97`).
- 1 resource type definition (`gate.rs`).
- 1 registration block edit (`e2e/mod.rs`, ~30 LOC).
- 8 `impl Gate` blocks (~500 LOC total, mechanical).
- 1 driver signature conversion (`driver.rs:452-469`, ~10 LOC of header).
- 1956 LOC of driver-body resource-access translation (mechanical;
  `ResMut<X>` → `world.resource_mut::<X>()`; `Single<...>` →
  `world.query_filtered::<...>().single_mut(world)`).
- 3 Apply-arm rewrites in driver (~45 LOC of diff per arm).

Total: ~700 LOC additive + ~2000 LOC translation. Generous estimate:
~80-100 file edits, ~30-50 tool calls. Within budget for a focused
session.

**Verify:** if the implementor's bandwidth budget exhausts before
landing all 5 items atomically, they should bail and surface the
size estimate so the architect can decompose differently. The
fallback would be "land items 1-3 as a `#![allow(dead_code)]`-shielded
build-green non-functional precursor, then land items 4-5 as the
consumer-wiring follow-up" — explicitly NOT recommended (re-creates
the dead-code problem), but available as a controlled escape hatch.

### A-5 — The `oasis.edit_applied = true` write inside `VoxGpuConstructionGate::apply_edit` keeps `pin_vox_gpu_construction_camera` working

**Assumed:** the cross-gate signal at
`vox_gpu_construction.rs:270-300` (the pin system reads
`OasisEditVisualState.edit_applied` to flip the camera pose A→B)
continues to work after revised Step 3 because the
`VoxGpuConstructionGate::apply_edit` body still writes
`world.resource_mut::<OasisEditVisualState>().edit_applied = true`.
The pin system itself is unchanged. The signal stays driver-side
visible.

**Verify:** the implementor must run `--vox-gpu-construction` to
confirm the camera promotes A→B as today. If the timing of the
`oasis.edit_applied = true` write shifts by a frame (the Apply-arm
swap may execute it inside `resource_scope` before `state.phase`
advances), the pin system on the SAME frame should still see it
because the pin system runs `.after(driver::e2e_driver)`
(`e2e/mod.rs:260`). No frame-shift expected.

---

## Side notes / observations / complaints

1. **The investigator's read was excellent and gave the architect a
   well-scoped problem.** I had to add no analytical content the
   investigator hadn't already verified at file:line precision. The
   "expand Step 3 to include the minimum driver wiring" framing is
   exactly right; the 5-item breakdown the brief sketches matches the
   actual edit shape. This is what good multi-agent handoff looks like
   when the upstream agent does the homework.

2. **The `&mut World` precedent-setting concern is real but
   non-blocking.** Investigator's grep
   (`02-investigation-item-2-d6-gate.md:493-499`) confirms only
   `FromWorld::from_world` at `render/pipelines.rs:351` uses `&mut World`
   today, and that's a trait method, not a registered exclusive
   system. Revised Step 3 sets the precedent for registered
   exclusive systems in this codebase. The architectural shift is
   real — exclusive systems serialize against the rest of the
   schedule — but the driver is already effectively serial (single
   `Single<>` query plus a 14-param ResMut soup that pessimistically
   serializes through Bevy's scheduler anyway). Net cost: zero
   measurable runtime impact; net benefit: every-resource access from
   one parameter. Worth setting the precedent here.

3. **Revised Step 3's verification load is heavier than the original
   Step 3.** Original Step 3a was "additive — old fns still present,
   gates not yet exercised through trait" → only `cargo build` +
   `cargo test --lib` verification. Revised Step 3 wires the trait into
   the driver atomically → ALL 8 e2e gates need to re-pass + `≥3×` for
   non-deterministic gates. This matches the original Step 4
   verification load (per `04-refactoring.md:534-537`). The trade is
   correct: pay the verification cost once, with a smaller code edit,
   to dissolve the dead-code problem.

4. **The 7-system pin-chain at `e2e/mod.rs:249-282` stays untouched
   in revised Step 3.** This is deliberate — collapsing the chain
   to `pin_active_gate_camera` is the cleanest part of Step 4 (every
   gate has a `camera_pose` method that already fits; no per-gate
   state migration needed), but landing it in revised Step 3 expands
   scope into Step-4 territory. The implementor must NOT touch the
   pin chain; it stays as-is until a dedicated Step 4 dispatch.

5. **The `oasis.edit_applied` cross-gate hack persists into revised
   Step 3.** This is the load-bearing signal between
   `VoxGpuConstructionGate::apply_edit` (write) and
   `pin_vox_gpu_construction_camera` (read). It's an architecturally
   ugly cross-gate coupling that the architect's Step-4 design
   resolves cleanly via `GateAuxState::VoxGpuConstruction { camera_promoted }`.
   Revised Step 3 keeps the hack alive (writes `edit_applied` from
   inside `VoxGpuConstructionGate::apply_edit`) because eliminating
   it requires the Step-4 `GateAuxState` migration. This is the
   single most-likely-fragile point of revised Step 3 — the
   implementor should verify with `--vox-gpu-construction` that the
   camera A→B promotion still fires correctly after the swap.

6. **Risk of the implementor expanding scope into Step 4 territory.**
   Per the investigator's side-note at
   `02-investigation-item-2-d6-gate.md:519-526` ("sub-agent honey-trap
   risk … 1683-line architect doc … may produce another full-rewrite
   spec"): the implementor reading the original architect doc will
   see a beautiful 250-LOC driver loop spec (`03-architecture.md:388-474`)
   and be tempted to lift it as the new driver body. They MUST resist.
   Revised Step 3 keeps the 49-variant enum + 1956-LOC driver body
   verbatim; only the resource-access mechanism changes. The implementor
   brief should call this out explicitly with "Step 4's 49→20
   collapse is OUT OF SCOPE; touch only the driver signature and
   in-body resource accesses." If the implementor lands Step 3 +
   parts of Step 4, the verification load explodes and a partial-Step-4
   landing creates a worse state than no Step 4 at all (intermediate
   shapes that need un-landing).

7. **Could revised Step 3 be even smaller?** A weaker variant is
   "land only items 1 + 2 + the trait impls for the 4 edit-gates
   (`OasisEditVisualGate`, `VoxGpuConstructionGate`,
   `SmallEditVisualGate`, `SmallEditReproGate`), plus items 4 + 5".
   Skips the 4 no-op single-capture gates' `impl Gate` blocks until
   Step 4. Saves ~200 LOC of mechanical wrapper code, but creates
   asymmetry — only some gates have `impl Gate` blocks; the architect
   has to remember which when landing Step 4. Not worth the
   asymmetry; the 200 LOC is mechanical. Recommended shape stays "all
   8 gates impl `Gate` in revised Step 3, the 4 no-op ones default
   to `Ok(())`."

8. **Foundation health.** No rot, no shoehorned abstractions, no
   accidental global state in the gate/driver path. The trait shape
   the original architect spec'd was off by one parameter; the fix
   is one parameter widening. The driver god-function is mechanically
   convertible to exclusive shape. The per-gate State resources are
   well-isolated and resource-scope-friendly. The cross-gate
   `OasisEditVisualState.edit_applied` hack is the one architectural
   ugly, and Step 4 has a clean migration plan for it. Revised Step 3
   is healthy; the original architect doc was almost-right and the
   implementor's bail was a valuable correction signal. The whole
   D6 deferred chunk is high-leverage refactor territory — once
   revised Step 3 lands, Step 4 becomes mostly mechanical
   collapse-and-migrate.

9. **No faithful-port-rule concern.** This is pure e2e-harness
   plumbing; no C# behaviour is touched. The CPU oracle
   (`aadf/edit.rs`) is untouched. The brush call sites
   (`apply_erase_brush`, `apply_small_cube_edit`,
   `apply_small_edit_repro_edit`, `promote_camera_to_pose_b`) move
   from "called directly from driver" to "called from inside
   `Gate::apply_edit`" with byte-equal behaviour. Memory
   `bevy-naadf-faithful-port-rule` is honoured.

10. **The `let _ = gate;` at `bin/e2e_render.rs:336` is the cleanest
    payoff signal of revised Step 3.** Post-Step-5 (`523` LOC of
    `bin/e2e_render.rs` post-D6-Step-5), the `BootCommand::NamedGate {
    gate, run }` arm already threads the `GateKind` through but drops
    it. After revised Step 3, that `let _ = gate;` becomes a
    `commands.insert_resource(ActiveGate::from_kind(gate))` or
    equivalent — making `bin/e2e_render.rs:333-336`'s docblock
    ("`gate` is carried through for potential diagnostic use + as
    the anchor for the D6 step 3/4 driver decomposition") finally
    load-bearing. The infrastructure was right; only the consumer
    was missing.
