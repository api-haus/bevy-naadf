# D6 — e2e-and-playwright exploration

**Domain**: e2e harness + Playwright tests.
**Scope** (read-only investigation):
- `crates/bevy_naadf/src/e2e/**` (10 292 LOC Rust, 19 files)
- `crates/bevy_naadf/src/bin/e2e_render.rs` (481 LOC)
- `crates/bevy_naadf/src/bin/diag_compare.rs` (314 LOC)
- `e2e/playwright.config.ts`
- `e2e/tests/**` (1 638 LOC TS)

**Architectural anchor** — `CLAUDE.md` (lines 1-44): the named e2e gates are
the **verification surface** of the project. `cargo run --bin bevy-naadf`
"verifications" are explicitly forbidden. Everything proposed below must
preserve gate behaviour for the gates `bin/e2e_render.rs` still dispatches
(`baseline`, `--validate-gpu-construction*`, `--edit-mode`,
`--runtime-edit-mode`, `--entities`, `--resize-test`, `--vox-e2e`,
`--oasis-edit-visual`, `--small-edit-visual`, `--small-edit-repro`,
`--vox-gpu-construction`, `--vox-gpu-oracle{,-cpu,-gpu}`,
`--vox-web-parity{,-skybox,-loaded}`, `--vox-horizon-native`,
`--device-snapshot-native`, `--ssim-compare`). Architect: take the
existing gate set as fixed unless the user explicitly removes one.

---

## Findings

### Summary table

| # | severity | location | category | one-line description |
|---|----------|----------|----------|----------------------|
| 1 | **high** | `e2e/pbr_debug_modes.rs:1-218`, `pbr_hard_edge.rs:1-1023`, `pbr_visual.rs:1-747` | dead-orphan-module | three full PBR gate modules (1 988 LOC) reference `AppArgs.pbr_*_mode` fields that no longer exist; not listed in `e2e/mod.rs`, not dispatched from `bin/e2e_render.rs` — they cannot even compile if re-included |
| 2 | **high** | `e2e/driver.rs:58-248`, `e2e/driver.rs:439-1679` | god-function + flat enum state-machine | one 1 956-LOC `e2e_driver` system holds a 49-variant `E2ePhase` enum across **eight** disjoint sub-flows (Standard, Resize, Oasis, SmallEdit, SmallEditRepro, VoxGpuOracle, VoxWebParity, plus VoxGpuConstruction reusing Oasis path) — each sub-flow's Warmup→Shoot→Drain→Save→Assert is structurally identical but inlined separately |
| 3 | **high** | seven `pin_*_camera` systems + 2 `pin_resize_test_camera` + 4 driver-internal sites | DUP-6 (camera-write boilerplate) | nine identical 3-line camera-pose writes (`**transform = pose; **position_split = PositionSplit::from_world(pose.translation)`); every gate also re-implements the "args-gate + Option-resolve + camera Single mutate" preamble |
| 4 | **medium** | `e2e/oasis_edit_visual.rs:442`, `small_edit_visual.rs:save_*_screenshot`, `small_edit_repro.rs`, `vox_gpu_oracle.rs:675`, `vox_web_parity.rs`, `vox_horizon_parity.rs:235`, `vox_gpu_construction.rs` | DUP — `save_*_screenshot` | seven identical `save_*_screenshot(fb, filename)` functions, each just `Path::new(E2E_SCREENSHOT_DIR).join(filename) → fb.save_png(path)` with a stringly-typed log prefix |
| 5 | **medium** | `bin/e2e_render.rs:71-208` | CLI dispatch ladder | 18 separate `args.iter().any(\|a\| a == "--flag")` lines + a 250-line if/else-if ladder routing them; each new gate forces editing the central ladder (the very anti-pattern the e2e_render module's `e2e-render-test.md` §9 warned about) |
| 6 | **medium** | `e2e/mod.rs:204-296` + 9 per-gate `*State` resources in `add_e2e_systems` | god-init + flat resource registration | `add_e2e_systems` inserts every gate's `State` resource and registers every `pin_*_camera` system regardless of which gate runs; gates are not first-class — they are special-cased every place the harness loops over them |
| 7 | **medium** | `bin/diag_compare.rs:1-314` + `e2e/tests/device-snapshot.spec.ts` + `--device-snapshot-native` mode in `bin/e2e_render.rs:143-144,364-375` + `e2e/mod.rs` device-snapshot register | dead-on-D7-deletion | `diag_compare` binary + `device-snapshot.spec.ts` + the `--device-snapshot-native` route all consume `diagnostics::device_snapshot` which D7's user directive (`01-context.md` Q2) DELETES; this entire D6 surface becomes dead the moment D7 lands |
| 8 | **medium** | `e2e/driver.rs:1709-1816` + per-gate `assert_*_landed`/`assert_*_visible` fns scattered | scattered assertion shapes | each gate's "assertion + PNG save + verdict log + AppExit write" lives partly in `driver.rs` and partly in the per-gate module, with no shared shape — same content is structured differently per gate |
| 9 | **low** | `e2e/mod.rs:55-193` + per-gate `*_FRAMES` constants | proliferating frame-budget constants | 14 frame-budget constants in `e2e/mod.rs` + 6 more per-gate (`OASIS_WARMUP_FRAMES`, `SMALL_EDIT_WARMUP_FRAMES`, `ORACLE_WARMUP_FRAMES`, `PARITY_WARMUP_FRAMES`, etc.) — each gate re-declares its own with no shared "warmup / wait / drain" type |
| 10 | **low** | `e2e/driver.rs:447-468`, fast-path routing block at `driver.rs:475-577` | flag-soup mode detection | 6 separate `app_args.as_deref().is_some_and(\|a\| a.<flag>)` lookups + 6 `state.phase == E2ePhase::Warmup && state.phase_ticks == 0` route-in branches — `AppArgs` has become the gate-discriminator and gate-config bag both |

---

### Finding 1 — three orphan PBR gate modules (1 988 LOC) (severity: high)

**Location:** `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs:1-218`,
`pbr_hard_edge.rs:1-1023`, `pbr_visual.rs:1-747`.

**Current state:** Per `01-context.md` Q2's verbatim user directive
("everything else can go") **AND** independent verification I ran:

- `e2e/mod.rs:24-38` lists 12 `pub mod` entries — **none of them are
  `pbr_debug_modes`, `pbr_hard_edge`, or `pbr_visual`**.
- `grep -n "pbr" crates/bevy_naadf/src/bin/e2e_render.rs` → 0 matches. The
  CLI does NOT dispatch them.
- Each PBR module references `args.pbr_<x>_mode` fields:
  - `pbr_debug_modes.rs:82,111` — `args.pbr_debug_modes_mode`
  - `pbr_hard_edge.rs:294,366` — `args.pbr_hard_edge_mode`
  - `pbr_visual.rs:219,247` — `args.pbr_visual_mode`
  - `grep -rn "pbr_visual_mode\|pbr_hard_edge_mode\|pbr_debug_modes_mode"
    crates/bevy_naadf/src/` → **only the PBR files themselves match**.
    The `AppArgs` fields they read don't exist. These files **cannot
    compile** if you re-add them to `e2e/mod.rs`.
- `git log --all --since=14.days.ago -- e2e/pbr_*.rs` →
  - `725fcdf` checkpoint commit
  - `3643d6d`, `2b5fa80`, `22ff1f5` (mid-May PBR work)

  The user's `01-context.md` Q2 carve-out says "EXCEPT any PBR gate the
  user is actively iterating on — architect: confirm via `git log -- e2e/pbr_*`
  whether commits in the last 14 days touch them; if so flag for user
  confirmation, else delete." Commits exist within the 14-day window —
  **escalate to user before deleting.** But the modules being orphaned
  from `mod.rs` strongly suggests the user already started the deletion
  themselves and these are the dangling carcass.

**Why it's a problem:** 1 988 LOC of dead code shadowing the e2e/
directory and inviting confusion ("is there a PBR gate? where is it
wired?"). The audit's §1.3 top-files list shows `pbr_hard_edge.rs:1023`
as the **9th-largest file in the entire Rust crate** — visible LOC bloat
in every navigation/grep, but contributing zero verification surface.

**Suggested direction (NOT a design):** Architect: escalate to user with
the evidence above (orphaned from mod.rs, references-non-existent-AppArgs,
checkpoint commit). If user confirms: delete the three files plus their
support assets if any (`grep -rn pbr_visual\|pbr_hard_edge\|pbr_debug_modes`
across `assets/`, `e2e/`, `docs/`). The `bin/e2e_render.rs` dispatch
ladder mention in the brief — "remove their CLI dispatch entries" — is
**already done** (no CLI flag exists for any of them).

**Out-of-scope ripple:** If `e2e/pbr_hard_edge.rs:166-169` is the only
consumer of `PbrVisualState` (Resource shared with `pbr_visual.rs`), the
shared resource also goes. No D6-external ripple.

---

### Finding 2 — `driver.rs` god-function with 49-variant flat enum state-machine (severity: high)

**Location:** `crates/bevy_naadf/src/e2e/driver.rs:58-248` (enum),
`driver.rs:439-1679` (system body).

**Current state:** A single `pub fn e2e_driver(...)` (15 system args,
double `#[allow(clippy::too_many_arguments)]`) drives an `E2ePhase` enum
with **49 distinct variants** (counted by reading the enum at lines
58-248: Warmup, Motion, Settle, Shoot, Drain, Assert, then 11 resize-test
phases, 8 oasis phases, 8 smalleditvisual phases, 8 smalleditrepro phases,
3 voxgpuoracle phases, 3 voxwebparity phases, Done). The body is a single
1 240-line `match state.phase { ... }` arm-block. Each gate's flow has
the same skeleton:

```
Warmup     → tick++; if hit budget → Shoot
Shoot      → shoot_primary_window(commands); → Drain
Drain      → tick++; if screenshot ready → decode → stash → Assert
            else if past drain budget → fail + AppExit::error + Done
ApplyEdit  → (edit gates only) call into per-gate function
WaitPostEdit → tick++; if budget reached → reset screenshot → ShootAfter
Assert     → run per-gate assert fn → format verdict → AppExit + Done
```

This pattern appears **8 times** in the body — once per top-level gate.
Counts of structural duplication I verified by grep:
- `shoot_primary_window(&mut commands);` — 11 sites in `driver.rs`
- `Framebuffer::from_image(&image)` decode arm — 9 sites
- `screenshot.0 = None;` reset before shoot — 8 sites
- `if let Some(image) = screenshot.0.take() {` drain pattern — 8 sites
- `state.phase = E2ePhase::Done;` after writing AppExit::error — 16+ sites

The fast-path routing block at `driver.rs:475-577` is six separate
"`if <flag> && state.phase == E2ePhase::Warmup && state.phase_ticks == 0`"
branches that re-route the state machine into per-gate sub-flows — i.e.
the state-machine is hardcoded to know about every gate by name.

**Why it's a problem:** This is the architectural anchor of D6. The
project's verification discipline (CLAUDE.md) says "If a gate is missing
for a behavior the agent needs to verify, the right move is to add a gate
to `e2e_render`, not to launch the binary and stare at it." But the
current shape of `driver.rs` makes adding a new gate require: (a) adding
6-12 new variants to the global `E2ePhase` enum, (b) adding a new
fast-path branch at `driver.rs:475-577`, (c) inlining a new copy of the
Warmup→Shoot→Drain→Save→Assert pattern in the giant `match` body. Every
addition fights every other one in the same file. The "shared `GateRunner<G:
Gate>` trait" the audit brief mentions IS the missing abstraction.

**Suggested direction (NOT a design):** Architect should consider a
two-level state machine: an outer `enum DriverPhase { Warmup, Motion,
Settle, Capture(CaptureSlot), Apply(ApplyHook), Wait(u32), Assert(Box<dyn
GateAssert>), Done }` where each gate is a value implementing a `Gate`
trait (`warmup_frames(), wait_frames(), apply(world, state), assert(before,
after)`) and the driver is parameterised over the gate. Independently, the
resize-test sub-machine looks like a genuinely separate flow (Wayland
resize is unique) and might stay carved out.

**Out-of-scope ripple:** Gate-specific `State` resources currently in
`e2e/mod.rs:228-234` move to live with the trait impls. No production-crate
ripple.

---

### Finding 3 — DUP-6 camera-pose-pin boilerplate × 9 sites (severity: high)

**Location:** seven `pin_*_camera` systems plus three driver-internal
camera writes:

- `e2e/oasis_edit_visual.rs:306-327` (`pin_oasis_camera`)
- `e2e/small_edit_visual.rs:255-273` (`pin_small_edit_camera`)
- `e2e/small_edit_repro.rs:163-180` (`pin_small_edit_repro_camera`)
- `e2e/vox_gpu_construction.rs:270` (`pin_vox_gpu_construction_camera`)
- `e2e/vox_gpu_oracle.rs:642-659` (`pin_vox_gpu_oracle_camera`)
- `e2e/vox_web_parity.rs:387` (`pin_vox_web_parity_camera`)
- `e2e/vox_horizon_parity.rs:183-200` (`pin_vox_horizon_camera`)
- `e2e/driver.rs:286-297` (`pin_resize_test_camera` — driver-internal)
- `e2e/driver.rs:586-589 / 612-614 / 637-639` (three inline writes in
  Warmup / Motion / Settle)

**Current state:** every site is:

```rust
let pose = <per-gate compute_pose()>;
let (transform, position_split) = &mut *camera;
**transform = pose;
**position_split = PositionSplit::from_world(pose.translation);
```

(Verified by `grep -n "PositionSplit::from_world\|\*\*transform = "
crates/bevy_naadf/src/e2e/*.rs` — 29 matches across 12 files.) The 9 pin
systems also share a verbatim preamble:

```rust
let Some(args) = args else { return; };
if !args.<gate>_mode { return; }
let Some(world_data) = world_data else { return; }; // some gates
let size_v = world_data.size_in_chunks * (CELL_DIM as u32 * CELL_DIM as u32);
if size_v.x == 0 || size_v.y == 0 || size_v.z == 0 { return; }
```

Plus the noise-suppressing `let _ = WinitSettings::game;` /
`let _ = (Hdr, Tonemapping::default());` lines at the end of several pin
systems (lines `oasis_edit_visual.rs:325-326`, `vox_gpu_oracle.rs:657-658`,
`vox_horizon_parity.rs:199`) — these read like leftover debug placebos
silencing an unused-import warning rather than load-bearing config.

**Why it's a problem:** Anchor: the audit's `00-reuse-audit.md §3.2` lists
DUP-6 explicitly ("every `pin_*_camera` system across 7+ e2e gates writes
camera `Transform` + `PositionSplit::from_world` + `camera-history`"). The
~30 LOC × 9 sites is ~270 LOC of pure boilerplate that the
"fight-Bevy-idiom" entries `00-reuse-audit.md §3.3` BEV-5 (no `Added<T>` /
`Changed<T>`) is the cousin of — these pin systems poll EVERY Update tick
to re-write a constant pose.

**Suggested direction (NOT a design):**
- A small `set_camera_pose(camera: &mut (Transform, PositionSplit), pose:
  Transform)` helper collapses the 3-line write to one line everywhere.
- The "args-gate + Option<Res<WorldData>>-resolve + world-size compute +
  pose" preamble becomes a `fn pose_for_gate(args, world_data) ->
  Option<Transform>` per gate, and a single shared system iterates over
  all enabled gates' poses with priority resolution (vox_gpu_construction
  > vox_gpu_oracle > vox_web_parity > vox_horizon_parity > oasis >
  small_edit > small_edit_repro). Today's "runs `.after(pin_oasis_camera)`"
  ordering chain (`e2e/mod.rs:259-279`) is what the priority resolution
  would replace.
- Pin only on `Added<>` / `Changed<>` of the active-gate flag, or with a
  `.run_if(...)` system condition — current behaviour rewrites the pose
  every frame even after it has stabilised.

**Out-of-scope ripple:** None — D6-internal.

---

### Finding 4 — `save_*_screenshot` duplicated 7× across gates (severity: medium)

**Location:** seven near-identical functions:

- `e2e/oasis_edit_visual.rs:442` (`save_oasis_screenshot`)
- `e2e/small_edit_visual.rs::save_small_edit_screenshot` (used at
  `driver.rs:1173-1176, 1249-1252`)
- `e2e/small_edit_repro.rs::save_small_edit_repro_screenshot` (used at
  `driver.rs:1357-1360, 1435-1438`)
- `e2e/vox_gpu_oracle.rs:675` (`save_oracle_screenshot`)
- `e2e/vox_web_parity.rs::save_parity_screenshot`
- `e2e/vox_horizon_parity.rs:235` (`save_horizon_screenshot`)
- `e2e/vox_gpu_construction.rs::save_vox_gpu_construction_screenshot`
- (the standard flow at `driver.rs:1862-1872` does its own inline save)

**Current state:** Each is verbatim:

```rust
pub fn save_<gate>_screenshot(fb: &Framebuffer, filename: &str) {
    let path = Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join(filename);
    match fb.save_png(&path) {
        Ok(()) => println!("e2e_render --<gate>: screenshot saved to {}", path.display()),
        Err(e) => eprintln!("e2e_render --<gate>: {filename} save failed: {e}"),
    }
}
```

Only the log prefix differs.

**Why it's a problem:** Lowest-effort obvious DRY violation. Anchor:
CLAUDE.md frames the e2e harness as the verification surface — readable,
not bloated. The `Framebuffer::save_png` API at
`e2e/framebuffer.rs:374-405` already returns `Result<(), String>` with
the path baked into errors; a single `save_to(fb, filename, label) ->
Result<(), String>` helper makes every per-gate copy redundant.

**Suggested direction (NOT a design):** Single
`framebuffer::save_to_dir(fb, filename, log_prefix) -> Result<PathBuf,
String>` helper; delete the seven wrappers and update the ~12 call sites
in `driver.rs`. Even simpler: bake the "save + log" pattern into the
`GateRunner` trait of Finding 2 so per-gate modules don't carry the
function at all.

**Out-of-scope ripple:** None.

---

### Finding 5 — `bin/e2e_render.rs` CLI dispatch is an 18-flag ladder (severity: medium)

**Location:** `crates/bevy_naadf/src/bin/e2e_render.rs:71-208,241-412`.

**Current state:** Eighteen `let <name>_mode = args.iter().any(|a| a ==
"--<flag>");` lines (one per gate flag), then a 250-line if/else-if ladder
selecting which gate function to call. Each entry follows the same shape:

```rust
} else if <gate>_mode {
    let mut app_args = bevy_naadf::AppArgs::default();
    app_args.<flag_field> = true;
    bevy_naadf::run_e2e_render_with_args(app_args)
}
```

Some entries do more (resize-test installs a Hyprland windowrule; vox-e2e
synthesises a fixture; vox-gpu-oracle/vox-web-parity short-circuit to
out-of-process compare; ssim-compare short-circuits with no Bevy boot).

Three diagnostic short-circuits at lines 213-239 each do
`validate_gpu_construction*()` and `return ExitCode::from(N)` — these
duplicate dispatch structure (parse the flag, call the fn, format the
exit code) within the same file.

After the gate match, lines 414-478 have four **separate** post-app
validation tails (`if validate_gpu_construction`, `if entities_mode`,
`if edit_mode`, `if runtime_edit_mode`) that each call
`bevy_naadf::render::construction::validate_*()`. These tails are
gate-orthogonal (they can compose with any e2e mode), so they shouldn't
go into a flat dispatch alongside the mutually-exclusive gate flags.

**Why it's a problem:** Each new gate addition forces a 6-line edit at
the central dispatch — exact opposite of the W-seam contract that lets
workstreams land features in isolation. The flag layout doesn't reflect
the actual structure (mutually-exclusive boot modes vs additive
post-app validation tails vs no-boot short-circuit flags). The audit
side-note #8 in `00-reuse-audit.md` flags this explicitly.

**Suggested direction (NOT a design):** Architect: consider `bin/
e2e_render.rs` becoming `match parse_command(args) { Command::Boot(gate)
=> ..., Command::SsimCompare(args) => ..., Command::ValidateOffline(kind)
=> ..., }` with `Gate` an enum derived from CLI args via the same source
of truth that defines the modes (one entry per gate, not three: flag
name, AppArgs field, dispatcher). The post-app tails (`if
validate_gpu_construction { ... }` × 4) are a separate concern — they
should compose orthogonally on top of any gate, not race the gate
dispatch.

**Out-of-scope ripple:** `bevy_naadf::AppArgs` field mapping crosses
into D7's territory (`lib.rs` owns the type). If the architect proposes a
struct-of-flags → `enum Gate` migration, that's a coordinated D6+D7
change. Flag in side-notes.

---

### Finding 6 — `e2e/mod.rs::add_e2e_systems` registers every gate's systems unconditionally (severity: medium)

**Location:** `crates/bevy_naadf/src/e2e/mod.rs:204-296`.

**Current state:** `add_e2e_systems` inserts **every** gate's `State`
resource (`OasisEditVisualState`, `SmallEditVisualState`,
`SmallEditReproState`, `VoxGpuOracleState`, `VoxWebParityState`,
`TracingErrorCounter`, the resize one is at line 228) regardless of which
gate runs, and registers all seven `pin_*_camera` systems on `Update`
with hand-managed `.after(...)` ordering between them (lines 248-281).
The resize-test, oasis-edit-visual, small-edit-visual, small-edit-repro,
vox-gpu-oracle, vox-web-parity, vox-horizon-parity gates each get a
separate `Resource` carved off, but they're all initialised whether or
not the gate is the active one — wasted memory + a non-zero startup
cost + a place where adding a new gate forces another central edit.

Anchor reference: `00-reuse-audit.md §3.3 BEV-5` flags this as a Bevy-
idiom misfit: `Added<>`/`Changed<>` filters or `.run_if(...)` system
conditions would replace the unconditional registration.

**Why it's a problem:** Same shape as Finding 5 — the gate set is a
flat constant the harness knows by name. Adding a gate is a multi-site
edit; removing one (e.g. PBR — Finding 1) leaves dead `State` registration
behind because the registry is one big call.

**Suggested direction (NOT a design):** `Plugin`-per-gate. Each gate
becomes its own `Plugin` that adds its `State`, its pin-system, and its
driver hook; `add_e2e_systems` becomes a thin coordinator that
conditionally adds the active gate's plugin (driven off the same `Gate`
enum from Finding 5). Anchor: Bevy idiom (`PluginGroup` / `add_plugins`).

**Out-of-scope ripple:** `lib.rs::build_app` (D7) calls
`add_e2e_systems(app)` — D6+D7 coordinated change to swap the call shape.

---

### Finding 7 — `diag_compare.rs` + `device-snapshot.spec.ts` + `--device-snapshot-native` become dead the moment D7 deletes `diagnostics::device_snapshot` (severity: medium)

**Location:**
- `crates/bevy_naadf/src/bin/diag_compare.rs:1-314` (binary).
- `crates/bevy_naadf/Cargo.toml:40-41` (binary declaration).
- `e2e/tests/device-snapshot.spec.ts:1-122` (Playwright spec).
- `bin/e2e_render.rs:143-144` (flag parse), `bin/e2e_render.rs:364-375`
  (dispatch arm), the `--device-snapshot-native` mode.
- `e2e/mod.rs` — no direct register, but the `DeviceSnapshotPlugin` is
  registered in `lib.rs` (D7) — confirmed by reading
  `bin/e2e_render.rs:374-375` ("`bevy_naadf::run_e2e_render` boots the
  standard e2e harness to capture device snapshot").
- `justfile:194-204` — two recipes (`just diag-snapshot-native` /
  `just diag-snapshot-web`) consume this surface.

**Current state:** `diag_compare.rs:1-21` (doc-comment) is explicit:
"Inputs: workspace-relative paths to two JSON files produced by
`src/diagnostics.rs::device_snapshot`". The `01-context.md` Q2 directive
deletes `diagnostics::device_snapshot` outright (D7 owns that).

Verified consumer chain by grep:
- `grep -rn "diag_compare"` → 8 hits, **all internal** to the surface
  itself (its own source + doc + Cargo.toml entry + the
  `device-snapshot.spec.ts` JSDoc that references it for human readers).
- `grep -rn "device_snapshot\|DeviceSnapshot"` → consumers are exactly:
  `diagnostics.rs` (D7), `bin/diag_compare.rs` (D6), `bin/e2e_render.rs`
  (D6 — `--device-snapshot-native` arm), `device-snapshot.spec.ts` (D6),
  `vox-horizon-parity.spec.ts:122,147,158,187` (consumes the
  `[device-snapshot]` *console sentinel* for diagnostic output only —
  not load-bearing).

Git history: `bin/diag_compare.rs` has just 2 commits ever (`grep
--diff-filter` shows zero, full log shows `6cf4746` initial + the
`725fcdf` PBR checkpoint touch).

**Why it's a problem:** D7's deletion of `diagnostics::device_snapshot`
is binding (`01-context.md` Q2). When it lands, this entire D6 surface
becomes dead code — D7 cannot delete it without crossing D6's domain, so
D6 must do the coordinated deletion.

**Suggested direction (NOT a design):** Architect: sequence the
deletion **after** D7's `device_snapshot` removal lands. Then D6's
implementor deletes (in one commit) `bin/diag_compare.rs`, the
`[[bin]] diag_compare` entry in `Cargo.toml:40-41`, the
`--device-snapshot-native` flag parse + dispatch arm at
`bin/e2e_render.rs:143-144,364-375`, `e2e/tests/device-snapshot.spec.ts`,
and the `justfile:194-204` recipes. `vox-horizon-parity.spec.ts`'s
sentinel-grepping at lines 122-187 is **diagnostic noise** (it just
forwards the line into its report) and stays — though it can stop looking
for a sentinel that no longer fires.

**Out-of-scope ripple:** `lib.rs::build_app` calls
`DeviceSnapshotPlugin` registration (D7's owner). D7's architect needs to
know D6 is the coordinated deleter for this group.

---

### Finding 8 — Per-gate assertion + verdict-log scattered between driver.rs and gate modules (severity: medium)

**Location:** `e2e/driver.rs:1087-1147` (Oasis verdict), `1281-1329`
(SmallEdit verdict), `1466-1493` (SmallEditRepro verdict), `1538-1573`
(VoxGpuOracle verdict), `1591-1672` (VoxWebParity / horizon verdict),
`888-913` (Resize verdict); plus the per-gate `assert_*` fns that live in
the per-gate module:
- `oasis_edit_visual.rs::assert_visual_edit_landed`
- `vox_gpu_construction.rs::assert_vox_gpu_construction_landed`
- `small_edit_visual.rs::assert_small_edit_landed`
- `small_edit_repro.rs::assert_no_pitch_black_pixels`

**Current state:** Each gate's `Assert` arm in the driver
- pulls before/after Framebuffer out of the per-gate State,
- calls the per-gate `assert_*_landed` (returns `Result<_, String>`),
- on `Ok(_)`: emits a 6-12 line `println!` summary referencing per-gate
  constants (`OASIS_WARMUP_FRAMES`, `OASIS_POST_EDIT_WAIT_FRAMES`,
  `OASIS_ERASE_RADIUS`, `OASIS_EDIT_DIFF_FLOOR`),
- on `Err(msg)`: `eprintln!` + `AppExit::error()`,
- stashes the result into `outcome.gate_result`,
- transitions to `Done`.

Same shape five times in the driver body, each time with a slightly
different gate-config-constant set hand-written into the `println!`.

**Why it's a problem:** The verdict-log content is gate-specific but the
verdict-emit + result-stash + AppExit shape isn't. The verdict format
drift between gates (some include `floor`, some include `radius`, some
include `world_size`) makes machine consumption of the e2e harness's
output harder than it needs to be — and the verdict log was added
explicitly so the orchestrator can read a one-line PASS/FAIL summary
(CLAUDE.md verification discipline).

**Suggested direction (NOT a design):** Combine with Finding 2's
`GateRunner<G: Gate>`: a gate trait method `fn verdict_log(&self,
config: &Config, outcome: Result<(), String>) -> String` lets each gate
own its verdict format, while the driver owns the AppExit write +
`outcome.gate_result` stash uniformly. (Or have the trait method just
return a `Display`-able outcome and the driver does the
PASS/FAIL/print sandwich.)

**Out-of-scope ripple:** None.

---

### Finding 9 — proliferating per-gate frame-budget constants (severity: low)

**Location:** `e2e/mod.rs:55-193` (14 top-level constants), plus
per-gate:
- `e2e/oasis_edit_visual.rs:113,121,125` (`OASIS_WARMUP_FRAMES = 120`,
  `OASIS_POST_EDIT_WAIT_FRAMES = 300`, `OASIS_DRAIN_FRAMES = 16`).
- `e2e/small_edit_visual.rs::SMALL_EDIT_*_FRAMES`,
  `e2e/small_edit_repro.rs::SMALL_EDIT_REPRO_*_FRAMES`,
- `e2e/vox_gpu_oracle.rs::ORACLE_WARMUP_FRAMES / ORACLE_DRAIN_FRAMES`,
- `e2e/vox_web_parity.rs::PARITY_WARMUP_FRAMES / PARITY_DRAIN_FRAMES`,
- `e2e/vox_horizon_parity.rs:110,113` — explicitly aliases
  `super::vox_web_parity::PARITY_WARMUP_FRAMES`. So one gate already
  noticed.

**Current state:** Every gate re-declares its own warmup/wait/drain
budgets. Some agree (`vox_horizon_parity` aliases vox_web_parity's).
Most don't. There's no shared `struct FrameBudget { warmup, motion?,
wait_post_edit?, drain }` and there's no documented "why does this gate
need a different value than that one" rationale at the constant
declarations.

**Why it's a problem:** Every value is an opaque magic number; their
relationships (oasis 120 vs vox_gpu_oracle 60 vs vox_web_parity 60 — are
these intentional different convergence requirements or accidental
drift?) are invisible. Anchor: same "documentation discipline" smell
that `00-reuse-audit.md §3.1 SSoT-3` flags for `CELL_DIM` — values are
hardcoded everywhere, with no SSoT making the relationships explicit.

**Suggested direction (NOT a design):** A `GateBudget` struct owned per
gate; default impl that documents reasonable values + a per-gate override
where the gate needs different timing. The Architect's job to decide
whether the convergence requirements actually justify N different
budgets or whether they collapse to ~3 budget classes (single-shot,
edit-with-W2-propagation, edit-with-W2+W3-bgaadf).

**Out-of-scope ripple:** None.

---

### Finding 10 — `AppArgs` is both gate-discriminator and gate-config bag (severity: low)

**Location:** `e2e/driver.rs:447-468` (system parameter list),
`driver.rs:475-577` (fast-path routing block).

**Current state:** The driver opens with six `app_args.as_deref().is_some_and(|a|
a.<flag>)` lookups to identify which gate is active:

```rust
let resize_test_mode = app_args.as_deref().is_some_and(|a| a.resize_test);
let oasis_mode = app_args.as_deref().is_some_and(|a| a.oasis_edit_visual_mode);
let vox_gpu_construction_mode = app_args.as_deref().is_some_and(|a| a.vox_gpu_construction_mode);
let small_edit_mode = app_args.as_deref().is_some_and(|a| a.small_edit_visual_mode);
let small_edit_repro_mode = app_args.as_deref().is_some_and(|a| a.small_edit_repro_mode);
let vox_gpu_oracle_mode = app_args.as_deref().is_some_and(|a| a.vox_gpu_oracle_cpu_phase || a.vox_gpu_oracle_gpu_phase);
let vox_web_parity_mode = app_args.as_deref().is_some_and(|a| a.vox_web_parity_skybox_phase || a.vox_web_parity_loaded_phase || a.vox_horizon_native_phase);
```

That's seven mutually-exclusive boolean fields in `AppArgs`, plus the
non-mutually-exclusive `vox_e2e_mode` / `spawn_test_entity` / `entities_enabled`
flags. The gate identity is encoded as a tuple of bools — invalid
states (`oasis_mode == true && small_edit_mode == true`) are
representable.

**Why it's a problem:** Anchor: `00-reuse-audit.md §3.5 UA` (weak-type
section) — the `AppArgs` flag-bag is the canonical example for "should be
an enum". Today's encoding lets bugs through silently (two flags set →
one wins, ordering-dependent).

**Suggested direction (NOT a design):** Replace the seven booleans with
`AppArgs.e2e_gate: Option<E2eGate>` where `E2eGate` is the same enum that
drives `bin/e2e_render.rs`'s gate dispatch (Finding 5). Mutually-exclusive
by construction. Bonus: makes the fast-path routing block a single match.

**Out-of-scope ripple:** Crosses into D7 (`AppArgs` lives in `lib.rs`).
Architect coordination required.

---

## Confirmed / refuted audit suspicions

**Audit suspicion 1 — `e2e/driver.rs:1956 LOC` single state-machine.**
Confirmed. See Finding 2. The driver is now actually 1 956 LOC (verified
by `wc -l`), holds a 49-variant enum (counted by reading the enum
definition), is a single system with 15 args (`driver.rs:452-468`). The
brief's enumeration of phases ("`WARMUP`/`MOTION`/`SETTLE`/`SHOOT`/`DRAIN`/
`ASSERT` plus the bolted-on `ResizeTestState`") undersells it — there are
**six** more sub-state-machines bolted on (Oasis × 8 variants,
SmallEdit × 8, SmallEditRepro × 8, VoxGpuOracle × 3, VoxWebParity × 3,
Resize × 11). Resize is the most-distinct one (Wayland resize is
genuinely unique); the rest converge to the same shape.

**Audit suspicion 2 — 9+ gate-specific files, shared `GateRunner<G: Gate>`
trait could absorb boilerplate.** Confirmed for the gates that **exist**
in the dispatch surface. The PBR files (`pbr_debug_modes.rs:218`,
`pbr_hard_edge.rs:1023`, `pbr_visual.rs:747`) listed in the brief are
**orphaned** (see Finding 1) — they shouldn't be part of the shared-trait
absorption, they should be deleted. The remaining gates that the trait
would absorb: `oasis_edit_visual`, `small_edit_visual`, `small_edit_repro`,
`vox_e2e`, `vox_gpu_construction`, `vox_gpu_oracle`, `vox_web_parity`,
`vox_horizon_parity` = 8 gates with the same Warmup → Shoot → Drain →
(Apply/Wait → Shoot → Drain →) Assert pattern.

**Audit suspicion 3 — image-diff utilities overlap with crate
functionality.** Refuted (partially). Verified `Cargo.toml:78-93`:
`image_compare = "0.5"` and `image = "=0.25.10"` are direct deps.
`e2e/ssim.rs:29-60` uses `image_compare::rgb_similarity_structure(MSSIMSimple, ...)`
— **already wraps the third-party crate**, not a custom SSIM impl. The
file is ~230 LOC of glue (PNG→Framebuffer→RgbImage conversion + CLI
arg parsing) — not duplication of the algorithm. The `e2e/framebuffer.rs`
helpers (`region_mean`, `region_luminance`, `mean_pixel_delta`,
`check_not_degenerate`, `check_luminance_alive`, `stability_hash`) are
NOT image-diff utilities in the third-party sense — they're domain-
specific gate predicates that the per-batch gates call. The audit's "audit
`Cargo.toml` for already-pulled deps" suspicion is moot; the deps were
already pulled and the harness is using them.

**Audit suspicion 4 — PBR e2e gates should be DELETED outright.**
Confirmed with stronger evidence than the brief had. Per Finding 1: the
files are orphaned from `e2e/mod.rs`, the CLI doesn't dispatch them, and
they reference non-existent `AppArgs.pbr_*_mode` fields. They would not
even compile if re-included. The brief says "user directive: DELETE outright,
except gates the user is actively iterating on." Recent commits exist in
the 14-day window (`725fcdf` checkpoint, `3643d6d`, `2b5fa80`,
`22ff1f5`) — escalate to user per Q2's carve-out before deleting, but
the orphan state is strong evidence the user has already moved on.

**Audit suspicion 5 — `bin/diag_compare.rs` likely dead.** Refuted on
"dead today", confirmed on "dead post-D7". See Finding 7. Today it is
load-bearing for the `device-snapshot.spec.ts` + `--device-snapshot-native`
+ `diagnostics::device_snapshot` chain. The moment D7's user directive to
delete `diagnostics::device_snapshot` lands, this entire surface drops
dead. The D6 implementor needs to coordinate the deletion sequence with
D7.

---

## Deletion candidates with caller-audit evidence

| candidate | path(s) | lines | callers / consumers (verified by grep) | when to delete |
|---|---|---|---|---|
| **PBR e2e gates (orphan)** | `crates/bevy_naadf/src/e2e/pbr_debug_modes.rs`, `pbr_hard_edge.rs`, `pbr_visual.rs` | 218 + 1 023 + 747 = **1 988** | **Zero compilable callers.** Not in `e2e/mod.rs`. Not in `bin/e2e_render.rs` (grep "pbr" → 0 matches). The files reference `AppArgs.pbr_*_mode` fields that grep can find ONLY inside these files themselves. Recent commits (within 14d window): `725fcdf`, `3643d6d`, `2b5fa80`, `22ff1f5`. | After user confirmation (the 14-day window triggers `01-context.md` Q2's carve-out). High confidence the user is done with them. |
| **`diag_compare` binary** | `crates/bevy_naadf/src/bin/diag_compare.rs`, plus `[[bin]] diag_compare` entry in `Cargo.toml:40-41`, plus `justfile:194-204` recipes | 314 + ~5 + ~10 = **~329** | Consumes JSON written by `diagnostics::device_snapshot` (D7). `justfile` has two recipes referencing it. Referenced in `device-snapshot.spec.ts:20` JSDoc only (human-readable, not runtime). | **After** D7 deletes `diagnostics::device_snapshot`. Coordinated D7+D6 deletion. |
| **`--device-snapshot-native` mode** | `bin/e2e_render.rs:143-144` (flag parse), `bin/e2e_render.rs:364-375` (dispatch arm) | ~15 | Triggers the `DeviceSnapshotPlugin` (D7) on a standard e2e harness boot. | **After** D7 deletes `DeviceSnapshotPlugin`. Same coordinated deletion. |
| **`device-snapshot.spec.ts` Playwright spec** | `e2e/tests/device-snapshot.spec.ts` | 122 | Captures the WASM-side `[device-snapshot]` console sentinel from `diagnostics::device_snapshot` (D7). No re-use as a test fixture. | **After** D7 deletes `device_snapshot`. |
| **vox-horizon-parity sentinel-grep cruft** (partial cleanup, not full delete) | `vox-horizon-parity.spec.ts:122,147,158,187` | ~30 (within a 582-LOC file) | Forwards the `[device-snapshot]` sentinel into the test's report annotations. Diagnostic noise — not load-bearing for the SSIM gate. | After D7's deletion lands and the sentinel no longer fires. Optional cleanup; the grep just won't match anything. |

**Refuted as deletion candidates (KEEP):** `e2e/oasis_edit_visual.rs`,
`e2e/small_edit_visual.rs`, `e2e/small_edit_repro.rs`, `e2e/vox_e2e.rs`,
`e2e/vox_gpu_construction.rs`, `e2e/vox_gpu_oracle.rs`,
`e2e/vox_web_parity.rs`, `e2e/vox_horizon_parity.rs` (minus the
device-snapshot sentinel cleanup above) — all are live gates the CLI
dispatches and the project's verification discipline depends on. The
`e2e/checks.rs`, `e2e/framebuffer.rs`, `e2e/gates.rs`, `e2e/readback.rs`,
`e2e/ssim.rs`, `e2e/tracing_error_counter.rs` infrastructure is also
all live and load-bearing.

---

## Side notes / observations / complaints

1. **The brief's audit suspicion list is mostly right but two items
   diverge from current reality.** (a) The PBR gate files are already
   orphaned from the dispatch surface (Finding 1) — the user appears to
   have started the deletion themselves. The brief's framing "DELETE
   outright … remove their `bin/e2e_render.rs` CLI dispatch entries"
   over-states what remains to delete: the CLI dispatch entries are
   already gone. (b) `diag_compare`'s deletion-readiness is **gated on
   D7's deletion order**, not standalone — the brief's framing "if
   dead, propose deletion" misses that it's load-bearing today and only
   becomes dead the moment D7 deletes `device_snapshot`.

2. **The audit-section §3.2 DUP-6 description is incomplete.** It lists
   "every `pin_*_camera` system across 7+ e2e gates writes camera
   `Transform` + `PositionSplit::from_world` + `camera-history`" but the
   `camera-history` part is **wrong** — I grep'd for it and the e2e pin
   systems do NOT write camera-history (that's a D7 concern via the
   `diagnostics::press-P` dump). The DUP-6 collision the audit was
   thinking of is real but smaller-scoped (pose write + PositionSplit
   write, ~2 lines × 9 sites = ~20 LOC plus the args-gate boilerplate;
   not the ~30 LOC × 9 = ~270 LOC the audit implied).

3. **`e2e/mod.rs::add_e2e_systems`'s 7-system ordering chain
   (lines 248-281) is brittle and load-bearing.** Every new gate that
   needs to override the standard driver pose write has to land **after**
   `pin_oasis_camera` (verbatim string from `e2e/mod.rs:259-279`: "runs
   `.after(pin_oasis_camera)` so the C# `(500, 200, 40)` pose overrides
   the birdseye…"). The architect should respect this ordering when
   redesigning — a priority resolver in a unified pose-pin system is the
   natural shape but the priority order has to be derivable from
   "more-specific-gate beats less-specific-gate", not encoded as system
   ordering.

4. **CLAUDE.md verification discipline is binding on this domain's
   refactor.** Every change in D6 must result in the same set of gates
   passing on the same fixtures with the same assertions. The
   `image_compare` / SSIM / region-luminance helpers are **part of the
   verification surface** — they're not free to rewrite even if a
   "cleaner" implementation exists. Architect: don't propose subtle
   changes to `Framebuffer` predicates (`check_not_degenerate`,
   `check_luminance_alive`, `mean_pixel_delta`); they're calibrated.
   Cosmetic / signature changes are fine; behaviour changes need user
   sign-off (the threshold values were tuned by the user).

5. **`bin/e2e_render.rs:213-239` is a separate dispatch concern from the
   main gate dispatch — the diagnostic short-circuits
   (`validate_gpu_construction_scaled`,
   `validate_gpu_construction_production`) call into D5's
   `render::construction::validate_*` functions WITHOUT booting the
   e2e harness. Refactoring D6's CLI surface needs to know D5's
   architect may move/rename these functions in their D5 split (the
   audit §1.5 expressly flags `validate_gpu_construction*` for
   structural extraction). Architect: coordinate the CLI flag → fn
   contract with D5's architect before D5 reshapes the construction
   surface.

6. **Subjective**: the `--vox-horizon-parity` chain is the most-recent
   addition and the cleanest of the gates I read — `vox_horizon_parity.rs`
   is 246 LOC, has one camera-pin, one PNG save helper, one config
   block. It also re-uses `vox_web_parity` constants instead of
   re-declaring them (Finding 9 acknowledges this). If the architect
   wants a model for what a "minimal gate" looks like, that's the file
   to study. Conversely `vox_gpu_oracle.rs` (696 LOC) is the most-grown
   gate: subprocess fan-out, three CLI sub-modes, two render-phase entry
   points, oracle PNG path helpers — it's effectively three smaller gates
   wearing the same name.

7. **The Playwright TS side is in better shape than the Rust side.**
   1 638 LOC across 5 spec files + 1 helper. `console-collector.ts`
   already factors out the shared "watch the console for panic
   markers" pattern. `vox-horizon-parity.spec.ts` is large (582 LOC)
   but most of it is the cross-target capture + funnel-PNG writing
   that doesn't appear elsewhere. The Playwright tests do **not**
   re-implement Rust-side gates — they drive the WASM build to the
   point where the WASM canvas is screenshotted, then shell out to
   `cargo run --bin e2e_render -- --ssim-compare` (verified in
   `vox-horizon-parity.spec.ts`). The "verify Playwright TS doesn't
   re-implement Rust-side gates" audit suspicion is cleanly refuted.

8. **Equal-footing complaint**: the audit brief's "verify the file:line
   refs with Read/Grep" preamble was load-bearing here. The PBR-gate
   LOC numbers (`pbr_debug_modes.rs:218`, `pbr_hard_edge.rs:1023`,
   `pbr_visual.rs:747`) in the brief match `wc -l` today, but the brief
   describes them as "their `bin/e2e_render.rs` CLI dispatch entries
   (`--pbr-debug-modes`, `--pbr-hard-edge`, `--pbr-visual`)" — those
   flag dispatches don't exist in the current `bin/e2e_render.rs`.
   Either the brief's audit was older than the current tree, or the
   user removed them already. The deliverable above takes the **current
   tree** as the source of truth (verified `grep -n "pbr"
   bin/e2e_render.rs` → 0 matches).
